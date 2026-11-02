//! What models cost, as their makers list them.
//!
//! From the owner of the live room: the Usage tab said "cost unknown" for
//! every bot, because a price existed only where a room's settings named
//! one, and none did. These are the makers' published API list prices in US
//! dollars per million tokens, taken from their pricing pages on 1 November
//! 2026 (Ollama's cloud models from ollama.com/pricing), so a turn is priced
//! the moment it ends. A room's own rates still
//! win where it sets them. A bot on a subscription pays its plan, not these;
//! for it they say what the same tokens would cost on the API.

use crate::agent::ProviderRef;
use crate::spend::TokenUsage;
use crate::time::Timestamp;

/// Millionths of a dollar per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rates {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Rates {
    /// What `usage` costs at these rates, in millionths of a dollar.
    pub fn cost(&self, usage: &TokenUsage) -> u64 {
        let units = u128::from(usage.input_tokens) * u128::from(self.input)
            + u128::from(usage.output_tokens) * u128::from(self.output)
            + u128::from(usage.cache_read_tokens) * u128::from(self.cache_read)
            + u128::from(usage.cache_write_tokens) * u128::from(self.cache_write);
        units.div_ceil(1_000_000).min(u128::from(u64::MAX)) as u64
    }

    const fn halved(self) -> Self {
        Self {
            input: self.input / 2,
            output: self.output / 2,
            cache_read: self.cache_read / 2,
            cache_write: self.cache_write / 2,
        }
    }
}

/// A model's price for a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListPrice {
    /// Charged at these rates.
    Paid(Rates),
    /// Runs on this computer, so the tokens cost nothing.
    Free,
}

/// One model's list price, and a dearer one for prompts past a size.
struct Listing {
    model: &'static str,
    rates: Rates,
    /// Tokens per call past which the long-context rates apply.
    long: Option<(u32, Rates)>,
    /// Half price off-peak, on the schedule of whoever serves it.
    off_peak: bool,
}

const fn usd(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Rates {
    Rates { input, output, cache_read, cache_write }
}

// Anthropic: cache writes at 1.25x input for the 5-minute cache Claude Code
// uses. OpenAI: cached input as listed, 30-minute cache writes as listed (1.25x
// input). xAI and DeepSeek list no write premium, so writes cost input.
const LISTINGS: &[Listing] = &[
    Listing { model: "claude-fable-5-1", rates: usd(10_000_000, 50_000_000, 250_000, 12_500_000), long: None, off_peak: false },
    Listing { model: "claude-opus-5-5", rates: usd(4_000_000, 20_000_000, 200_000, 5_000_000), long: None, off_peak: false },
    Listing { model: "claude-sonnet-5-5", rates: usd(2_000_000, 10_000_000, 200_000, 2_500_000), long: None, off_peak: false },
    Listing {
        model: "claude-haiku-5-5",
        rates: usd(100_000, 500_000, 10_000, 125_000),
        long: Some((100_000, usd(500_000, 2_500_000, 50_000, 625_000))),
        off_peak: false,
    },
    Listing {
        model: "gpt-6-astra",
        rates: usd(10_000_000, 50_000_000, 1_000_000, 12_500_000),
        long: Some((272_000, usd(20_000_000, 75_000_000, 2_000_000, 25_000_000))),
        off_peak: false,
    },
    Listing {
        model: "gpt-6.1-sol",
        rates: usd(2_000_000, 10_000_000, 100_000, 2_500_000),
        long: Some((272_000, usd(4_000_000, 15_000_000, 200_000, 5_000_000))),
        off_peak: false,
    },
    Listing { model: "gpt-6-sol", rates: usd(2_000_000, 10_000_000, 200_000, 2_500_000), long: None, off_peak: false },
    Listing { model: "gpt-6.1-terra", rates: usd(1_000_000, 5_000_000, 50_000, 1_250_000), long: None, off_peak: false },
    Listing {
        model: "gpt-6-luna",
        rates: usd(100_000, 500_000, 10_000, 125_000),
        long: Some((272_000, usd(200_000, 750_000, 20_000, 250_000))),
        off_peak: false,
    },
    Listing {
        model: "grok-4.7",
        rates: usd(2_000_000, 6_000_000, 500_000, 2_000_000),
        long: Some((200_000, usd(4_000_000, 12_000_000, 1_000_000, 4_000_000))),
        off_peak: false,
    },
    Listing {
        model: "grok-build-0.1",
        rates: usd(1_000_000, 2_000_000, 200_000, 1_000_000),
        long: Some((200_000, usd(2_000_000, 4_000_000, 400_000, 2_000_000))),
        off_peak: false,
    },
    // DeepSeek's peak rates; the rest of the week is half price.
    Listing { model: "deepseek-flash", rates: usd(300_000, 1_200_000, 6_000, 300_000), long: None, off_peak: true },
    Listing { model: "deepseek-v4-flash", rates: usd(300_000, 1_200_000, 6_000, 300_000), long: None, off_peak: true },
    Listing { model: "deepseek-v4-pro", rates: usd(1_320_000, 3_960_000, 44_000, 1_320_000), long: None, off_peak: true },
    // Ollama's cloud models: no cache-write price is published, so writes
    // cost input.
    Listing { model: "deepseek-v4.1-flash", rates: usd(300_000, 1_200_000, 6_000, 300_000), long: None, off_peak: true },
    Listing { model: "glm-5.3-flash", rates: usd(150_000, 500_000, 30_000, 150_000), long: None, off_peak: false },
    // ollama.com/pricing on 2026-11-02, the same as MiniMax's own list price.
    Listing { model: "minimax-m3.1-flash", rates: usd(150_000, 600_000, 15_000, 150_000), long: None, off_peak: false },
    Listing { model: "glm-5.3", rates: usd(1_400_000, 4_400_000, 260_000, 1_400_000), long: None, off_peak: false },
    Listing { model: "glm-5.2", rates: usd(1_400_000, 4_400_000, 260_000, 1_400_000), long: None, off_peak: false },
];

/// Whether a bot runs on a model server on this computer.
///
/// Not Ollama: OrgBots reaches Ollama's cloud at ollama.com, and from the
/// owner of the live room, its two Ollama bots had used $6.41 of the
/// account's credits while the Usage tab called them free.
pub fn is_local(provider: &ProviderRef) -> bool {
    match provider {
        ProviderRef::OpenAiCompatible { profile, .. } => {
            let profile = profile.to_ascii_lowercase();
            ["local", "lmstudio", "llamacpp", "llama.cpp", "vllm"]
                .iter()
                .any(|name| profile.contains(name))
        }
        _ => false,
    }
}

/// Whether a bot is served by Ollama's cloud.
fn is_ollama(provider: &ProviderRef) -> bool {
    matches!(provider, ProviderRef::OpenAiCompatible { profile, .. } if profile.eq_ignore_ascii_case("ollama"))
}

/// The list price of one turn of `model` for the bot behind `provider`.
///
/// `usage` picks the long-context rates when its calls averaged past a
/// model's threshold, and `at` picks DeepSeek's off-peak half price. `model`
/// is what the backend reported; the bot's configured model stands in when
/// that is not listed. `None` when there is no published price.
pub fn list_price(provider: &ProviderRef, model: &str, usage: &TokenUsage, at: Timestamp) -> Option<ListPrice> {
    if is_local(provider) {
        return Some(ListPrice::Free);
    }
    let listing = find(model).or_else(|| find(provider.model()))?;
    let prompt_per_call = (u64::from(usage.input_tokens)
        + u64::from(usage.cache_read_tokens)
        + u64::from(usage.cache_write_tokens))
        / u64::from(usage.calls.max(1));
    let mut rates = match listing.long {
        Some((threshold, long)) if counts_requests(provider) && prompt_per_call > u64::from(threshold) => long,
        _ => listing.rates,
    };
    let off_peak = if is_ollama(provider) { ollama_off_peak(at) } else { deepseek_off_peak(at) };
    if listing.off_peak && off_peak {
        rates = rates.halved();
    }
    Some(ListPrice::Paid(rates))
}

/// What `usage` costs at list price, or `None` when unpriced. Free is zero.
pub fn list_cost(provider: &ProviderRef, model: &str, usage: &TokenUsage, at: Timestamp) -> Option<u64> {
    match list_price(provider, model, usage, at)? {
        ListPrice::Paid(rates) => Some(rates.cost(usage)),
        ListPrice::Free => Some(0),
    }
}

/// Whether `usage.calls` counts the model requests behind the tokens.
///
/// From the live room: Codex, Claude Code, Grok and Antigravity report one
/// total for a turn of many requests, so a turn counted as one call of a
/// million tokens and every turn was billed at the long-context rate,
/// roughly doubling @astra's cost. Their requests' sizes are unknown, so
/// they get the standard rate; a request past the threshold inside such a
/// turn is priced low. OpenCode reports each request.
fn counts_requests(provider: &ProviderRef) -> bool {
    match provider {
        ProviderRef::CliAgent { cli, .. } => *cli == crate::agent::CliKind::OpenCode,
        _ => true,
    }
}

fn find(model: &str) -> Option<&'static Listing> {
    let name = model.trim().to_ascii_lowercase();
    let name = name.rsplit('/').next().unwrap_or(&name);
    let name = name.split(':').next().unwrap_or(name);
    LISTINGS.iter().find(|listing| listing.model == name).or_else(|| {
        // A dated or suffixed release of a listed model, such as
        // `claude-opus-5-5-20261001`.
        LISTINGS
            .iter()
            .filter(|listing| name.strip_prefix(listing.model).is_some_and(|rest| rest.starts_with('-')))
            .max_by_key(|listing| listing.model.len())
    })
}

/// DeepSeek charges half outside 01:00–04:00 and 06:00–10:00 UTC on weekdays.
/// Chinese public holidays are off-peak too; they are not known here, so a
/// turn on one is priced at the peak rate.
fn deepseek_off_peak(at: Timestamp) -> bool {
    let millis = at.0.max(0);
    let days = millis / 86_400_000;
    // 1 January 1970 was a Thursday.
    let weekday = (days + 4) % 7; // 0 = Sunday
    let hour = (millis / 3_600_000) % 24;
    let weekend = weekday == 0 || weekday == 6;
    let peak = !weekend && ((1..4).contains(&hour) || (6..10).contains(&hour));
    !peak
}

/// Ollama charges its off-peak rate outside 12:00–18:00 UTC on weekdays.
fn ollama_off_peak(at: Timestamp) -> bool {
    let millis = at.0.max(0);
    let weekday = (millis / 86_400_000 + 4) % 7; // 0 = Sunday
    let hour = (millis / 3_600_000) % 24;
    weekday == 0 || weekday == 6 || !(12..18).contains(&hour)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentPolicy, CliKind};

    fn cli(cli: CliKind, model: &str) -> ProviderRef {
        ProviderRef::CliAgent {
            cli,
            model: model.into(),
            workdir: String::new(),
            policy: AgentPolicy::ReadOnly,
            mcp: false,
            effort: String::new(),
        }
    }

    fn usage(input: u32, output: u32, read: u32, write: u32) -> TokenUsage {
        TokenUsage {
            reported: true,
            complete: true,
            calls: 1,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: read,
            cache_write_tokens: write,
            ..TokenUsage::default()
        }
    }

    // A Saturday, so DeepSeek is off-peak, and a Monday at 07:00 UTC, peak.
    const SATURDAY: Timestamp = Timestamp(1_793_448_000_000);
    const MONDAY_PEAK: Timestamp = Timestamp(1_793_602_800_000);
    // Monday at 13:00 UTC, Ollama's peak.
    const MONDAY_AFTERNOON: Timestamp = Timestamp(1_793_624_400_000);

    #[test]
    fn a_turn_is_priced_at_its_models_list_price() {
        let opus = cli(CliKind::Claude, "claude-opus-5-5");
        // 1M in, 100k out, 2M cache read, 200k cache write at $4 / $20 / $0.20 / $5.
        let cost = list_cost(&opus, "claude-opus-5-5", &usage(1_000_000, 100_000, 2_000_000, 200_000), SATURDAY);
        assert_eq!(cost, Some(4_000_000 + 2_000_000 + 400_000 + 1_000_000));
        let astra = ProviderRef::OpenAiCompatible { model: "gpt-6-astra".into(), profile: "openai".into() };
        let mut ten_calls = usage(1_000_000, 0, 0, 0);
        ten_calls.calls = 10;
        assert_eq!(list_cost(&astra, "gpt-6-astra", &ten_calls, SATURDAY), Some(10_000_000));
        assert_eq!(
            list_cost(&astra, "gpt-6-astra", &usage(1_000_000, 0, 0, 0), SATURDAY),
            Some(20_000_000),
            "one call past 272k is billed at the long-context rate"
        );
        // An agent CLI's turn is many requests reported as one.
        let codex = cli(CliKind::Codex, "gpt-6-astra");
        assert_eq!(list_cost(&codex, "gpt-6-astra", &usage(1_000_000, 0, 0, 0), SATURDAY), Some(10_000_000));
        let haiku = cli(CliKind::Claude, "claude-haiku-5-5");
        assert_eq!(list_cost(&haiku, "claude-haiku-5-5", &usage(0, 0, 1_000_000, 0), SATURDAY), Some(10_000));
        let opencode = cli(CliKind::OpenCode, "gpt-6-astra");
        assert_eq!(list_cost(&opencode, "gpt-6-astra", &usage(1_000_000, 0, 0, 0), SATURDAY), Some(20_000_000), "OpenCode reports each request");
        let grok = cli(CliKind::Grok, "grok-4.7");
        assert_eq!(list_cost(&grok, "", &usage(0, 1_000_000, 0, 0), SATURDAY), Some(6_000_000), "the configured model stands in");
    }

    #[test]
    fn long_prompts_local_models_and_deepseek_hours_are_priced_as_listed() {
        let grok = ProviderRef::OpenAiCompatible { model: "grok-4.7".into(), profile: "xai".into() };
        assert_eq!(list_cost(&grok, "grok-4.7", &usage(300_000, 0, 0, 0), SATURDAY), Some(1_200_000), "past 200k, $4 per million");
        let local = ProviderRef::OpenAiCompatible { model: "glm-5.3-flash".into(), profile: "lmstudio".into() };
        assert_eq!(list_price(&local, "glm-5.3-flash", &usage(1, 1, 0, 0), SATURDAY), Some(ListPrice::Free));
        // Ollama's cloud bills its credits per token, with its own off-peak hours.
        let glm = ProviderRef::OpenAiCompatible { model: "glm-5.3-flash".into(), profile: "ollama".into() };
        assert!(!is_local(&glm));
        assert_eq!(list_cost(&glm, "glm-5.3-flash", &usage(1_000_000, 1_000_000, 1_000_000, 0), SATURDAY), Some(680_000));
        // From benchmark 2: @iris's spend showed as unknown.
        let minimax = ProviderRef::OpenAiCompatible { model: "minimax-m3.1-flash:cloud".into(), profile: "ollama".into() };
        assert_eq!(list_cost(&minimax, "minimax-m3.1-flash:cloud", &usage(1_000_000, 1_000_000, 1_000_000, 0), MONDAY_AFTERNOON), Some(765_000));
        let flash = ProviderRef::OpenAiCompatible { model: "deepseek-v4.1-flash:1017".into(), profile: "ollama".into() };
        let out = usage(0, 1_000_000, 0, 0);
        assert_eq!(list_cost(&flash, "deepseek-v4.1-flash:1017", &out, MONDAY_PEAK), Some(600_000), "07:00 UTC is off-peak on Ollama");
        assert_eq!(list_cost(&flash, "deepseek-v4.1-flash:1017", &out, MONDAY_AFTERNOON), Some(1_200_000));
        let deepseek = ProviderRef::DeepSeek { model: "deepseek-flash".into() };
        let million_out = usage(0, 1_000_000, 0, 0);
        assert_eq!(list_cost(&deepseek, "deepseek-flash", &million_out, MONDAY_PEAK), Some(1_200_000));
        assert_eq!(list_cost(&deepseek, "deepseek-flash", &million_out, SATURDAY), Some(600_000));
        assert_eq!(list_cost(&deepseek, "some-unlisted-model", &million_out, SATURDAY), Some(600_000), "the configured model stands in");
        let unknown = ProviderRef::OpenAiCompatible { model: "mystery-1".into(), profile: "acme".into() };
        assert_eq!(list_price(&unknown, "mystery-1", &million_out, SATURDAY), None);
        assert!(find("claude-opus-5-5-20261001").is_some_and(|l| l.model == "claude-opus-5-5"));
    }
}
