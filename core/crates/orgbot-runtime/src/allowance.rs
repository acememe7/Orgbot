//! Local provider allowances shared by every desktop room process.

use async_trait::async_trait;
use orgbot_proto::agent::ProviderRef;
use orgbot_providers::{
    CompletionRequest, CompletionResponse, ContentBlock, Provider, ProviderError, StreamEvent,
    Usage,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub window_secs: u64,
    pub max_requests: u64,
    pub max_tokens: u64,
    pub max_cost_microusd: u64,
    pub input_microusd_per_million: u64,
    pub output_microusd_per_million: u64,
    pub cache_read_microusd_per_million: Option<u64>,
    pub cache_write_microusd_per_million: Option<u64>,
    pub model_rates: BTreeMap<String, TokenRates>,
    pub max_output_tokens: u32,
    pub enabled: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            window_secs: 604800,
            max_requests: 0,
            max_tokens: 0,
            max_cost_microusd: 0,
            input_microusd_per_million: 0,
            output_microusd_per_million: 0,
            cache_read_microusd_per_million: None,
            cache_write_microusd_per_million: None,
            model_rates: BTreeMap::new(),
            max_output_tokens: 16000,
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TokenRates {
    pub input_microusd_per_million: u64,
    pub output_microusd_per_million: u64,
    pub cache_read_microusd_per_million: u64,
    pub cache_write_microusd_per_million: u64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub providers: BTreeMap<String, Policy>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    pub window_start: u64,
    pub requests: u64,
    pub tokens: u64,
    pub cost_microusd: u64,
    pub cooldown_until: u64,
    pub consecutive_limits: u32,
    pub unreported_requests: u64,
    pub message: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Ledger {
    providers: BTreeMap<String, Record>,
}

#[derive(Debug, Clone)]
pub struct Allowances {
    config: PathBuf,
    ledger: PathBuf,
}

#[derive(Debug)]
struct Reservation {
    start: u64,
    tokens: u64,
    cost: u64,
    policy: Policy,
    metered: bool,
}

/// Recorded when a capped request used more than it reserved.
const RESERVATION_OVERRUN: &str = "Provider exceeded its reservation; further requests paused";

impl Policy {
    /// Whether a token or money cap bounds this provider. Request counts and
    /// on/off do not need a per-request reservation to be honest.
    fn capped(&self) -> bool {
        self.max_tokens > 0 || self.max_cost_microusd > 0
    }
}

/// Lift a pause that an uncapped provider should never have been given.
///
/// Earlier builds paused any provider whose reply used more than its
/// reservation, until the end of the window. Agent CLIs reserve nothing, so
/// their first reply always "overran" and every Claude, Codex and Grok bot sat
/// out the rest of the week.
fn clear_spurious_overrun(record: &mut Record, policy: &Policy) {
    if !policy.capped() && record.message == RESERVATION_OVERRUN {
        record.cooldown_until = 0;
        record.message.clear();
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn waiting(backend: &str, message: impl Into<String>, seconds: u64) -> ProviderError {
    ProviderError::Allowance {
        provider: backend.into(),
        message: message.into(),
        retry_after_secs: seconds.max(1),
    }
}

fn read_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(error) => return Err(format!("Cannot open allowance data: {error}")),
    };
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err("Allowance data is not a bounded regular file".into());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes)
        .map_err(|_| "Allowance data is invalid; repair it in Connections & tools".into())
}

impl Allowances {
    pub fn from_environment(
        environment: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<Option<Self>, String> {
        let read = |key: &str| {
            environment
                .and_then(|values| values.get(key).cloned())
                .or_else(|| std::env::var(key).ok())
        };
        match (
            read("ORGBOT_ALLOWANCE_CONFIG"),
            read("ORGBOT_ALLOWANCE_LEDGER"),
        ) {
            (None, None) => Ok(None),
            (Some(config), Some(ledger))
                if Path::new(&config).is_absolute() && Path::new(&ledger).is_absolute() =>
            {
                Ok(Some(Self {
                    config: config.into(),
                    ledger: ledger.into(),
                }))
            }
            _ => Err("Spending controls require absolute configuration and ledger paths".into()),
        }
    }

    fn config(&self) -> Result<Config, String> {
        let config: Config = read_json(&self.config)?;
        for policy in config.providers.values() {
            if !(60..=31_536_000).contains(&policy.window_secs)
                || policy.max_output_tokens == 0
                || policy.max_output_tokens > 128_000
            {
                return Err("Allowance window or output ceiling is invalid".into());
            }
            if policy.max_cost_microusd > 0
                && ((policy.input_microusd_per_million == 0
                    || policy.output_microusd_per_million == 0)
                    && policy.model_rates.is_empty())
            {
                return Err("A money limit requires both input and output prices".into());
            }
            if policy
                .model_rates
                .keys()
                .any(|model| model.trim().is_empty())
            {
                return Err("Model rate names cannot be empty".into());
            }
        }
        Ok(config)
    }

    fn transaction<T>(
        &self,
        action: impl FnOnce(&mut Ledger) -> Result<T, String>,
    ) -> Result<T, String> {
        let parent = self
            .ledger
            .parent()
            .ok_or("Allowance ledger needs a directory")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            if fs::symlink_metadata(parent)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
            {
                return Err("Allowance directory cannot be a symbolic link".into());
            }
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(parent.join("ledger.lock"))
                .map_err(|e| e.to_string())?;
            fs2::FileExt::lock_exclusive(&lock).map_err(|e| e.to_string())?;
            self.locked_transaction(action)
        }
        #[cfg(not(unix))]
        {
            let _ = action;
            Err("Desktop allowances require a supported file-locking platform".into())
        }
    }

    fn locked_transaction<T>(
        &self,
        action: impl FnOnce(&mut Ledger) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut ledger: Ledger = read_json(&self.ledger)?;
        let result = action(&mut ledger)?;
        let temporary = self
            .ledger
            .with_extension(format!("{}.tmp", orgbot_proto::RoomId::new()));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let write = || -> Result<(), String> {
            let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
            serde_json::to_writer(&mut file, &ledger).map_err(|e| e.to_string())?;
            file.flush()
                .and_then(|_| file.sync_all())
                .map_err(|e| e.to_string())?;
            fs::rename(&temporary, &self.ledger).map_err(|e| e.to_string())?;
            File::open(self.ledger.parent().unwrap())
                .and_then(|dir| dir.sync_all())
                .map_err(|e| e.to_string())
        };
        if let Err(error) = write() {
            let _ = fs::remove_file(temporary);
            return Err(error);
        }
        Ok(result)
    }

    fn reserve(
        &self,
        backend: &str,
        request: &mut CompletionRequest,
        metered: bool,
        time: u64,
    ) -> Result<Reservation, ProviderError> {
        let policy = self
            .config()
            .map_err(|e| waiting(backend, e, 60))?
            .providers
            .get(backend)
            .cloned()
            .unwrap_or_default();
        request.max_tokens = request.max_tokens.min(policy.max_output_tokens);
        let strict = policy.max_tokens > 0 || policy.max_cost_microusd > 0;
        if strict && !metered {
            return Err(waiting(
                backend,
                "This backend does not expose a bounded API request. Use a request/episode limit instead of a token or money cap.",
                60,
            ));
        }
        if policy.max_cost_microusd > 0 && token_rates(&policy, &request.model).is_none() {
            return Err(waiting(
                backend,
                "No token rates configured for this model",
                60,
            ));
        }
        if strict
            && request
                .messages
                .iter()
                .flat_map(|t| &t.content)
                .any(|b| matches!(b, ContentBlock::Image { .. }))
        {
            return Err(waiting(
                backend,
                "Image token costs cannot be bounded here. Use a request limit for image work.",
                60,
            ));
        }
        // Reserve text bytes plus framing, not a guessed four-characters-per-token ratio.
        let input = serde_json::to_vec(request)
            .map_err(|e| waiting(backend, e.to_string(), 60))?
            .len() as u64
            + 4096;
        let tokens = if metered {
            input.saturating_add(request.max_tokens as u64)
        } else {
            0
        };
        let rates = token_rates(&policy, &request.model).unwrap_or_default();
        let input_reservation_rate = rates
            .input_microusd_per_million
            .max(rates.cache_read_microusd_per_million)
            .max(rates.cache_write_microusd_per_million);
        let cost = if metered {
            price(
                input,
                request.max_tokens as u64,
                input_reservation_rate,
                rates.output_microusd_per_million,
            )
        } else {
            0
        };
        let result = self
            .transaction(|ledger| {
                let record = ledger.providers.entry(backend.into()).or_default();
                rollover(record, &policy, time);
                clear_spurious_overrun(record, &policy);
                let delay = record
                    .window_start
                    .saturating_add(policy.window_secs)
                    .saturating_sub(time)
                    .max(1);
                let reason = if !policy.enabled {
                    Some(("Provider disabled in spending controls", 60))
                } else if record.cooldown_until > time {
                    Some(("Provider cooldown", record.cooldown_until - time))
                } else if exceeds(record.requests, 1, policy.max_requests) {
                    Some(("Request allowance reached", delay))
                } else if exceeds(record.tokens, tokens, policy.max_tokens) {
                    Some(("Not enough token allowance to reserve this request", delay))
                } else if exceeds(record.cost_microusd, cost, policy.max_cost_microusd) {
                    Some(("Not enough money allowance to reserve this request", delay))
                } else {
                    None
                };
                if let Some((reason, seconds)) = reason {
                    record.message = reason.into();
                    return Ok(Err(waiting(backend, reason, seconds)));
                }
                record.requests = record.requests.saturating_add(1);
                record.tokens = record.tokens.saturating_add(tokens);
                record.cost_microusd = record.cost_microusd.saturating_add(cost);
                record.message.clear();
                Ok(Ok(Reservation {
                    start: record.window_start,
                    tokens,
                    cost,
                    policy,
                    metered,
                }))
            })
            .map_err(|e| waiting(backend, e, 60))?;
        result
    }

    fn settle(
        &self,
        backend: &str,
        reservation: Reservation,
        result: &Result<CompletionResponse, ProviderError>,
        time: u64,
    ) -> Result<(), String> {
        self.transaction(|ledger| {
            let record = ledger.providers.entry(backend.into()).or_default();
            match result {
                Ok(response) => {
                    if record.cooldown_until <= time {
                        record.consecutive_limits = 0;
                        record.cooldown_until = 0;
                    }
                    if response.usage.is_empty() {
                        record.unreported_requests = record.unreported_requests.saturating_add(1);
                        record.message = "Usage not reported; reservation retained".into();
                    } else if record.window_start == reservation.start {
                        let tokens = usage_total(response.usage);
                        let cost = response
                            .usage
                            .actual_api_cost_microusd
                            .unwrap_or(reservation.cost);
                        record.tokens = record
                            .tokens
                            .saturating_sub(reservation.tokens)
                            .saturating_add(tokens);
                        record.cost_microusd = record
                            .cost_microusd
                            .saturating_sub(reservation.cost)
                            .saturating_add(cost);
                        // Overrunning a reservation only matters when a cap
                        // depends on it. Uncapped providers, including every
                        // agent CLI, reserve nothing and always "overrun".
                        if reservation.policy.capped()
                            && (tokens > reservation.tokens || cost > reservation.cost)
                        {
                            record.cooldown_until = record
                                .window_start
                                .saturating_add(reservation.policy.window_secs);
                            record.message = RESERVATION_OVERRUN.into();
                        }
                    }
                }
                Err(error) => {
                    // Interrupted and failed calls may still be billed. Never refund unknown usage.
                    record.unreported_requests = record.unreported_requests.saturating_add(1);
                    if error.is_retryable() {
                        record.consecutive_limits = record.consecutive_limits.saturating_add(1);
                        let delay = error
                            .retry_after_secs()
                            .unwrap_or(15 * (1u64 << record.consecutive_limits.min(8)));
                        record.cooldown_until = time.saturating_add(delay.clamp(1, 604800));
                        record.message = format!(
                            "Provider unavailable; retry in {} seconds",
                            record.cooldown_until - time
                        );
                    }
                }
            }
            Ok(())
        })
    }

    /// Scheduling uses only known local allowance consumption, never invented provider quotas.
    pub fn routing(&self, time: u64) -> Result<BTreeMap<String, Option<u64>>, String> {
        let config = self.config()?;
        let ledger: Ledger = read_json(&self.ledger)?;
        let mut scores = BTreeMap::new();
        for backend in config.providers.keys().chain(ledger.providers.keys()) {
            let policy = config.providers.get(backend).cloned().unwrap_or_default();
            let mut record = ledger.providers.get(backend).cloned().unwrap_or_default();
            rollover(&mut record, &policy, time);
            clear_spurious_overrun(&mut record, &policy);
            let unavailable = !policy.enabled
                || record.cooldown_until > time
                || exceeds(record.requests, 1, policy.max_requests)
                || exceeds(record.tokens, 1, policy.max_tokens)
                || exceeds(record.cost_microusd, 1, policy.max_cost_microusd);
            let ratio = [
                (record.requests, policy.max_requests),
                (record.tokens, policy.max_tokens),
                (record.cost_microusd, policy.max_cost_microusd),
            ]
            .into_iter()
            .filter(|(_, cap)| *cap > 0)
            .map(|(used, cap)| ((used as u128 * 10000) / cap as u128).min(u64::MAX as u128) as u64)
            .max()
            .unwrap_or(0);
            scores.insert(
                backend.clone(),
                if unavailable { None } else { Some(ratio) },
            );
        }
        Ok(scores)
    }
}

fn rollover(record: &mut Record, policy: &Policy, time: u64) {
    if record.window_start == 0 || time >= record.window_start.saturating_add(policy.window_secs) {
        let cooldown_until = record.cooldown_until;
        *record = Record {
            window_start: time / policy.window_secs * policy.window_secs,
            cooldown_until,
            ..Record::default()
        };
    }
}
fn exceeds(used: u64, extra: u64, cap: u64) -> bool {
    cap > 0 && (used >= cap || extra > cap.saturating_sub(used))
}
fn usage_input(usage: Usage) -> u64 {
    usage.input_tokens as u64 + usage.cache_read_tokens as u64 + usage.cache_write_tokens as u64
}
fn usage_total(usage: Usage) -> u64 {
    usage_input(usage) + usage.output_tokens as u64
}
fn price(input: u64, output: u64, input_rate: u64, output_rate: u64) -> u64 {
    let units = input as u128 * input_rate as u128 + output as u128 * output_rate as u128;
    units.div_ceil(1_000_000).min(u64::MAX as u128) as u64
}

fn price_usage(usage: Usage, rates: TokenRates) -> u64 {
    let units = usage.input_tokens as u128 * rates.input_microusd_per_million as u128
        + usage.output_tokens as u128 * rates.output_microusd_per_million as u128
        + usage.cache_read_tokens as u128 * rates.cache_read_microusd_per_million as u128
        + usage.cache_write_tokens as u128 * rates.cache_write_microusd_per_million as u128;
    units.div_ceil(1_000_000).min(u64::MAX as u128) as u64
}

fn token_rates(policy: &Policy, model: &str) -> Option<TokenRates> {
    policy.model_rates.get(model).copied().or_else(|| {
        (policy.input_microusd_per_million > 0 || policy.output_microusd_per_million > 0).then_some(
            TokenRates {
                input_microusd_per_million: policy.input_microusd_per_million,
                output_microusd_per_million: policy.output_microusd_per_million,
                cache_read_microusd_per_million: policy
                    .cache_read_microusd_per_million
                    .unwrap_or(policy.input_microusd_per_million),
                cache_write_microusd_per_million: policy
                    .cache_write_microusd_per_million
                    .unwrap_or(policy.input_microusd_per_million),
            },
        )
    })
}

#[derive(Debug)]
struct GuardedProvider {
    inner: Arc<dyn Provider>,
    allowances: Allowances,
    backend: String,
    metered: bool,
}

#[async_trait]
impl Provider for GuardedProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn last_turn_activity(&self) -> Option<usize> {
        self.inner.last_turn_activity()
    }
    fn start_fresh_session(&self) {
        self.inner.start_fresh_session()
    }
    async fn complete(
        &self,
        mut request: CompletionRequest,
        sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> Result<CompletionResponse, ProviderError> {
        let manager = self.allowances.clone();
        let backend = self.backend.clone();
        let metered = self.metered;
        let (reservation, request) = tokio::task::spawn_blocking(move || {
            let reservation = manager.reserve(&backend, &mut request, metered, now())?;
            Ok::<_, ProviderError>((reservation, request))
        })
        .await
        .map_err(|_| waiting(&self.backend, "Allowance accounting task failed", 60))??;
        // A cancelled process/future leaves its reservation charged, preventing restart overspend.
        let mut result = self.inner.complete(request, sink).await;
        if let Ok(response) = &mut result {
            if response.usage.reported {
                if let Some(rates) = token_rates(&reservation.policy, &response.model) {
                    let cost = price_usage(response.usage, rates);
                    if reservation.metered {
                        response.usage.actual_api_cost_microusd = Some(cost);
                    } else {
                        response.usage.api_equivalent_cost_microusd = Some(cost);
                    }
                }
            }
        }
        let manager = self.allowances.clone();
        let backend = self.backend.clone();
        let (result, settlement) = tokio::task::spawn_blocking(move || {
            let settlement = manager.settle(&backend, reservation, &result, now());
            (result, settlement)
        })
        .await
        .map_err(|_| waiting(&self.backend, "Allowance settlement task failed", 60))?;
        settlement.map_err(|e| waiting(&self.backend, e, 60))?;
        result
    }
}

pub fn wrap(
    provider: Arc<dyn Provider>,
    reference: &ProviderRef,
    allowances: Allowances,
) -> Arc<dyn Provider> {
    let backend = reference.backend();
    if backend.is_empty() {
        return provider;
    }
    let metered = matches!(
        reference,
        ProviderRef::DeepSeek { .. } | ProviderRef::Anthropic { .. }
    ) || matches!(reference, ProviderRef::OpenAiCompatible { profile, .. } if profile != "codex" && profile != "chatgpt" && profile != "ollama");
    Arc::new(GuardedProvider {
        inner: provider,
        allowances,
        backend,
        metered,
    })
}

pub fn routing(manager: Option<&Allowances>) -> BTreeMap<String, Option<u64>> {
    manager
        .and_then(|manager| manager.routing(now()).ok())
        .unwrap_or_default()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn model_rates_price_uncached_input_output_and_both_cache_paths() {
        let usage = Usage {
            reported: true,
            complete: true,
            calls: 1,
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 300,
            cache_write_tokens: 40,
            ..Usage::default()
        };
        let cost = price_usage(
            usage,
            TokenRates {
                input_microusd_per_million: 1_000_000,
                output_microusd_per_million: 2_000_000,
                cache_read_microusd_per_million: 100_000,
                cache_write_microusd_per_million: 500_000,
            },
        );
        assert_eq!(cost, 190);
        let policy = Policy {
            model_rates: BTreeMap::from([(
                "m".into(),
                TokenRates {
                    input_microusd_per_million: 1,
                    ..TokenRates::default()
                },
            )]),
            ..Policy::default()
        };
        assert!(token_rates(&policy, "unknown").is_none());
        assert!(token_rates(&policy, "m").is_some());
    }
    use orgbot_providers::{StopReason, Turn};
    struct Fixture {
        root: PathBuf,
        manager: Allowances,
    }
    impl Fixture {
        fn new(policy: Policy) -> Self {
            let root = std::env::temp_dir()
                .join(format!("orgbot-allowance-{}", orgbot_proto::RoomId::new()));
            fs::create_dir(&root).unwrap();
            let manager = Allowances {
                config: root.join("policies.json"),
                ledger: root.join("ledger.json"),
            };
            fs::write(
                &manager.config,
                serde_json::to_vec(&Config {
                    providers: BTreeMap::from([("deepseek".into(), policy)]),
                })
                .unwrap(),
            )
            .unwrap();
            Self { root, manager }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
    fn request() -> CompletionRequest {
        CompletionRequest::new("test", "system", vec![Turn::user("Check the fixture")])
    }
    fn response(input: u32, output: u32) -> Result<CompletionResponse, ProviderError> {
        Ok(CompletionResponse {
            content: vec![ContentBlock::text("checked")],
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                reported: true,
                complete: true,
                calls: 1,
                input_tokens: input,
                output_tokens: output,
                ..Default::default()
            },
            model: "test".into(),
        })
    }

    #[test]
    fn concurrent_rooms_reserve_before_requests_and_restart_cannot_reset_usage() {
        let fixture = Fixture::new(Policy {
            max_requests: 2,
            ..Default::default()
        });
        let handles = (0..8)
            .map(|_| {
                let manager = fixture.manager.clone();
                std::thread::spawn(move || {
                    manager
                        .reserve("deepseek", &mut request(), true, 700000)
                        .is_ok()
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .filter(|ok| *ok)
                .count(),
            2
        );
        assert!(fixture
            .manager
            .clone()
            .reserve("deepseek", &mut request(), true, 700001)
            .is_err());
    }

    #[test]
    fn reservations_survive_restart_and_unknown_usage_is_not_free() {
        let fixture = Fixture::new(Policy {
            max_requests: 1,
            ..Default::default()
        });
        let hold = fixture
            .manager
            .reserve("deepseek", &mut request(), true, 700000)
            .unwrap();
        let mut unknown = response(0, 0).unwrap();
        unknown.usage = Usage::missing();
        fixture
            .manager
            .settle("deepseek", hold, &Ok(unknown), 700001)
            .unwrap();
        assert!(fixture
            .manager
            .clone()
            .reserve("deepseek", &mut request(), true, 700002)
            .is_err());
        let ledger: Ledger = read_json(&fixture.manager.ledger).unwrap();
        assert_eq!(ledger.providers["deepseek"].unreported_requests, 1);
        assert!(ledger.providers["deepseek"].tokens > 0);
        assert_eq!(fixture.manager.routing(700002).unwrap()["deepseek"], None);
    }

    #[test]
    fn an_uncapped_agent_cli_is_not_paused_by_its_first_reply() {
        let fixture = Fixture::new(Policy::default());
        let hold = fixture
            .manager
            .reserve("cli:claude", &mut request(), false, 700000)
            .unwrap();
        fixture
            .manager
            .settle("cli:claude", hold, &response(170_000, 2_000), 700001)
            .unwrap();
        assert_eq!(
            fixture.manager.routing(700002).unwrap()["cli:claude"],
            Some(0)
        );
        assert!(fixture
            .manager
            .reserve("cli:claude", &mut request(), false, 700002)
            .is_ok());
    }

    #[test]
    fn a_week_long_pause_from_the_old_overrun_rule_is_lifted() {
        let fixture = Fixture::new(Policy::default());
        let mut ledger = Ledger::default();
        ledger.providers.insert(
            "cli:grok".into(),
            Record {
                window_start: 604800,
                requests: 1,
                tokens: 31_113,
                cooldown_until: 604800 * 2,
                message: RESERVATION_OVERRUN.into(),
                ..Default::default()
            },
        );
        fs::write(
            &fixture.manager.ledger,
            serde_json::to_vec(&ledger).unwrap(),
        )
        .unwrap();
        assert_eq!(
            fixture.manager.routing(700000).unwrap()["cli:grok"],
            Some(0)
        );
        assert!(fixture
            .manager
            .reserve("cli:grok", &mut request(), false, 700000)
            .is_ok());
        let ledger: Ledger = read_json(&fixture.manager.ledger).unwrap();
        assert_eq!(ledger.providers["cli:grok"].cooldown_until, 0);
    }

    #[test]
    fn reported_usage_refunds_only_the_unused_reservation() {
        let fixture = Fixture::new(Policy {
            max_tokens: 20_000,
            max_output_tokens: 100,
            input_microusd_per_million: 1_000_000,
            output_microusd_per_million: 2_000_000,
            ..Default::default()
        });
        let mut req = request();
        let hold = fixture
            .manager
            .reserve("deepseek", &mut req, true, 700000)
            .unwrap();
        assert_eq!(req.max_tokens, 100);
        let mut reported = response(30, 10);
        reported.as_mut().unwrap().usage.actual_api_cost_microusd = Some(50);
        fixture
            .manager
            .settle("deepseek", hold, &reported, 700001)
            .unwrap();
        let ledger: Ledger = read_json(&fixture.manager.ledger).unwrap();
        assert_eq!(ledger.providers["deepseek"].tokens, 40);
        assert_eq!(ledger.providers["deepseek"].cost_microusd, 50);
    }

    #[test]
    fn unbounded_backends_and_images_do_not_bypass_hard_caps() {
        let fixture = Fixture::new(Policy {
            max_tokens: 100_000,
            ..Default::default()
        });
        assert!(fixture
            .manager
            .reserve("deepseek", &mut request(), false, 700000)
            .is_err());
        let mut req = request();
        req.messages[0].content.push(ContentBlock::Image {
            media_type: "image/png".into(),
            source: orgbot_providers::types::ImageSource::Base64 {
                data: "AAAA".into(),
            },
        });
        assert!(fixture
            .manager
            .reserve("deepseek", &mut req, true, 700000)
            .is_err());
    }

    #[test]
    fn failed_requests_keep_reservations_and_cooldowns_honor_retry_after() {
        let fixture = Fixture::new(Policy::default());
        let hold = fixture
            .manager
            .reserve("deepseek", &mut request(), true, 700000)
            .unwrap();
        fixture
            .manager
            .settle(
                "deepseek",
                hold,
                &Err(ProviderError::RateLimited {
                    provider: "deepseek".into(),
                    message: "wait".into(),
                    retry_after_secs: Some(90),
                }),
                700001,
            )
            .unwrap();
        assert!(fixture
            .manager
            .reserve("deepseek", &mut request(), true, 700090)
            .is_err());
        assert!(fixture
            .manager
            .reserve("deepseek", &mut request(), true, 700091)
            .is_ok());
    }

    #[test]
    fn parallel_success_does_not_clear_another_requests_cooldown() {
        let fixture = Fixture::new(Policy::default());
        let earlier = fixture
            .manager
            .reserve("deepseek", &mut request(), true, 700000)
            .unwrap();
        let later = fixture
            .manager
            .reserve("deepseek", &mut request(), true, 700001)
            .unwrap();
        fixture
            .manager
            .settle(
                "deepseek",
                later,
                &Err(ProviderError::RateLimited {
                    provider: "deepseek".into(),
                    message: "wait".into(),
                    retry_after_secs: Some(90),
                }),
                700002,
            )
            .unwrap();
        fixture
            .manager
            .settle("deepseek", earlier, &response(30, 10), 700003)
            .unwrap();
        assert!(fixture
            .manager
            .reserve("deepseek", &mut request(), true, 700091)
            .is_err());
        assert!(fixture
            .manager
            .reserve("deepseek", &mut request(), true, 700092)
            .is_ok());
    }

    #[tokio::test]
    async fn explicit_environment_caps_actual_provider_calls() {
        let fixture = Fixture::new(Policy {
            max_requests: 1,
            ..Default::default()
        });
        let environment = std::collections::HashMap::from([
            (
                "ORGBOT_ALLOWANCE_CONFIG".into(),
                fixture.manager.config.to_string_lossy().into_owned(),
            ),
            (
                "ORGBOT_ALLOWANCE_LEDGER".into(),
                fixture.manager.ledger.to_string_lossy().into_owned(),
            ),
        ]);
        let manager = Allowances::from_environment(Some(&environment))
            .unwrap()
            .unwrap();
        let provider = wrap(
            Arc::new(orgbot_providers::ScriptedProvider::always_says("done")),
            &ProviderRef::DeepSeek {
                model: "test".into(),
            },
            manager,
        );
        assert!(provider.complete(request(), None).await.is_ok());
        assert!(matches!(
            provider.complete(request(), None).await,
            Err(ProviderError::Allowance { .. })
        ));
        let ledger: Ledger = read_json(&fixture.manager.ledger).unwrap();
        assert_eq!(ledger.providers["deepseek"].requests, 1);
    }

    #[test]
    fn money_limit_rejects_before_provider_request() {
        let fixture = Fixture::new(Policy {
            max_cost_microusd: 1,
            input_microusd_per_million: 1_000_000,
            output_microusd_per_million: 1_000_000,
            ..Default::default()
        });
        assert!(fixture
            .manager
            .reserve("deepseek", &mut request(), true, 700000)
            .is_err());
        let ledger: Ledger = read_json(&fixture.manager.ledger).unwrap();
        assert_eq!(ledger.providers["deepseek"].requests, 0);
    }

    #[test]
    fn window_rollover_recovers_but_clock_rollback_does_not_reset() {
        let fixture = Fixture::new(Policy {
            max_requests: 1,
            window_secs: 3600,
            ..Default::default()
        });
        fixture
            .manager
            .reserve("deepseek", &mut request(), true, 7200)
            .unwrap();
        assert!(fixture
            .manager
            .reserve("deepseek", &mut request(), true, 7199)
            .is_err());
        assert!(fixture
            .manager
            .reserve("deepseek", &mut request(), true, 10799)
            .is_err());
        assert!(fixture
            .manager
            .reserve("deepseek", &mut request(), true, 10800)
            .is_ok());
    }

    #[test]
    fn corrupt_or_symlinked_accounting_fails_closed() {
        let fixture = Fixture::new(Policy::default());
        fs::write(&fixture.manager.ledger, "broken").unwrap();
        assert!(fixture
            .manager
            .reserve("deepseek", &mut request(), true, 700000)
            .is_err());
        fs::remove_file(&fixture.manager.ledger).unwrap();
        std::os::unix::fs::symlink(&fixture.manager.config, &fixture.manager.ledger).unwrap();
        assert!(fixture
            .manager
            .reserve("deepseek", &mut request(), true, 700000)
            .is_err());
    }
}
