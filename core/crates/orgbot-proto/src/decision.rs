//! Operator decisions, and the authority they leave behind.
//!
//! A real run kept asking the same person the same question. The room had
//! approved "same VPS, confined Docker" once; every rerun, every rebuilt
//! container, every regenerated manifest inside that boundary came back for
//! approval again, because the approval lived in a chat message and nothing
//! could check an action against it.
//!
//! So an approval is stored as a capability rather than remembered as a
//! conversation. A [`Decision`] records what was asked and what was answered;
//! the [`Envelope`] it carries says what that answer permits. A bot can then
//! determine mechanically whether what it wants to do is already authorised,
//! and only come back when it genuinely steps outside.
//!
//! Two properties matter more than the shapes:
//!
//! - **The same question is only asked once.** A request carries a
//!   [`Decision::fingerprint`]; if an answered decision has the same one and
//!   the scope has not widened, the answer applies without anybody being
//!   disturbed.
//! - **A later policy wins cleanly.** Superseding a decision makes the old one
//!   permanently non-authoritative, so an obsolete approval cannot resurface
//!   and re-authorise something the operator has since revoked.

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, DecisionId, RoomId, TaskId};
use crate::time::Timestamp;

/// The action class a server-access decision grants.
///
/// From the live room: a bot installed a server release over SSH while the
/// rest of the room had that release on hold, because every bot sandbox
/// carried the bot server key on every turn. The key is now mounted only for
/// a bot the owner has allowed it, for one task, by a decision whose envelope
/// grants this action in that task's environment ([`server_access_scope`]).
pub const SERVER_ACCESS: &str = "server-access";

/// The answer that grants a server-access request.
pub const SERVER_ACCESS_ALLOW: &str = "Allow server access for this task";

/// The answer that refuses a server-access request.
pub const SERVER_ACCESS_DENY: &str = "Deny";

/// The action class a decision lending the owner's sign-in `name` grants.
pub fn lend_action(name: &str) -> String {
    format!("access:{name}")
}

/// The answer that grants a request including one of the owner's sign-ins.
pub const ACCESS_ALLOW: &str = "Allow for this task";

/// The answer that lets a bot land the commits it asked to land.
pub const LAND_COMMITS_ALLOW: &str = "Land these commits";

/// The answer that keeps them out.
pub const LAND_COMMITS_DENY: &str = "Don't land them";

/// The action a landing decision grants: exactly these commits onto `branch`,
/// from where it stood when the bot asked. Anything else needs asking again.
pub fn land_action(branch: &str, base: &str, head: &str) -> String {
    format!("land-commits:{branch}:{base}..{head}")
}

/// The name a bot asks for the bot server key by, beside the owner's sign-ins.
pub const SERVER_ITEM: &str = "server";

/// What a bot says it will do with what it asks to use.
///
/// The bot's word only ever raises how carefully a request is treated: a
/// request is rated the higher of this and the risk the owner gave each thing
/// asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessAction {
    /// Read or check something.
    Inspect,
    /// Build, test or make changes that are easy to undo.
    Change,
    /// Release to production or change a live service.
    Release,
    /// Delete data or change credentials.
    Destroy,
    /// Spend money.
    Spend,
}

impl AccessAction {
    pub const fn risk(self) -> crate::room::Risk {
        use crate::room::Risk;
        match self {
            Self::Inspect | Self::Change => Risk::Regular,
            Self::Release => Risk::Risky,
            Self::Destroy | Self::Spend => Risk::Critical,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Change => "change",
            Self::Release => "release",
            Self::Destroy => "destroy",
            Self::Spend => "spend",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw.trim() {
            "inspect" => Self::Inspect,
            "change" => Self::Change,
            "release" => Self::Release,
            "destroy" => Self::Destroy,
            "spend" => Self::Spend,
            _ => return None,
        })
    }
}

/// The environment a decision about one task is scoped to.
pub fn task_scope(task: TaskId) -> String {
    format!("task:{task}")
}

/// The environment a server-access grant is scoped to: one task.
pub fn server_access_scope(task: TaskId) -> String {
    task_scope(task)
}

/// The action class a retire-task decision grants.
///
/// From the live room: an obsolete task stayed open, woke idle bots every turn
/// for nothing, and the bots asked the owner to archive it in an app that had
/// no way to. A coordinator now proposes retiring it, with the reason, and the
/// owner's answer retires it.
pub const RETIRE_TASK: &str = "retire-task";

/// The answer that retires a task proposed for retirement.
pub const RETIRE_TASK_ALLOW: &str = "Retire this task";

/// The answer that keeps a task proposed for retirement.
pub const RETIRE_TASK_KEEP: &str = "Keep it";

/// What an approval covers.
///
/// Deliberately coarse. A scope that has to enumerate every command is one
/// nobody can reason about, and the point is that a bot can answer "am I
/// inside this?" without asking a model to interpret prose.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Envelope {
    /// Classes of action this permits, e.g. `rebuild`, `run-tests`.
    /// Empty means the decision authorises nothing by itself.
    #[serde(default)]
    pub allowed: Vec<String>,
    /// Classes explicitly refused, checked before `allowed`, so a broad grant
    /// can still carve out something dangerous.
    #[serde(default)]
    pub forbidden: Vec<String>,
    /// Where it applies — a machine, a container boundary, an environment
    /// name. Empty means anywhere in this room.
    #[serde(default)]
    pub environment: String,
    /// When the authority lapses, if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
}

/// Something a bot wants to do, checked against an [`Envelope`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The class of action, matching an entry in [`Envelope::allowed`].
    pub action: String,
    /// Where it would happen.
    pub environment: String,
}

impl Request {
    /// A request to perform `action` in `environment`.
    pub fn new(action: impl Into<String>, environment: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            environment: environment.into(),
        }
    }
}

/// Why an envelope does not cover a request.
///
/// Named rather than boolean because the bot's next move differs: an expired
/// approval should be renewed, a forbidden action should never be retried, and
/// stepping outside the environment is the one case worth asking about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum OutsideEnvelope {
    /// The action is explicitly refused. Asking again will not help.
    Forbidden {
        /// The refused action class.
        action: String,
    },
    /// The action was never granted.
    NotGranted {
        /// The action class that has no grant.
        action: String,
    },
    /// Granted, but somewhere else.
    WrongEnvironment {
        /// Where the grant applies.
        granted: String,
        /// Where the action would happen.
        requested: String,
    },
    /// The authority has lapsed.
    Expired {
        /// When it lapsed.
        at: Timestamp,
    },
}

impl OutsideEnvelope {
    /// Whether asking the operator could plausibly change the answer.
    ///
    /// A forbidden action is a standing refusal: coming back with the same
    /// request is exactly the pestering this module exists to stop.
    pub const fn worth_asking(&self) -> bool {
        !matches!(self, Self::Forbidden { .. })
    }

    /// One line for a bot, saying what to do about it.
    pub fn advice(&self) -> String {
        match self {
            Self::Forbidden { action } => {
                format!("`{action}` is explicitly forbidden here. Do not ask again; find another way.")
            }
            Self::NotGranted { action } => {
                format!("`{action}` is outside what was approved. Ask for it explicitly.")
            }
            Self::WrongEnvironment { granted, requested } => format!(
                "the approval covers `{granted}`, not `{requested}`. Ask before acting there."
            ),
            Self::Expired { at } => {
                format!("that approval lapsed at {at}. Ask for it again.")
            }
        }
    }
}

impl Envelope {
    /// Whether `request` is already authorised, or why it is not.
    ///
    /// Order matters: a forbidden action loses even inside an otherwise valid
    /// grant, and an expired envelope authorises nothing at all.
    pub fn covers(&self, request: &Request, now: Timestamp) -> Result<(), OutsideEnvelope> {
        if let Some(expiry) = self.expires_at {
            if now >= expiry {
                return Err(OutsideEnvelope::Expired { at: expiry });
            }
        }
        if self.forbidden.iter().any(|a| matches(a, &request.action)) {
            return Err(OutsideEnvelope::Forbidden {
                action: request.action.clone(),
            });
        }
        if !self.environment.is_empty() && self.environment != request.environment {
            return Err(OutsideEnvelope::WrongEnvironment {
                granted: self.environment.clone(),
                requested: request.environment.clone(),
            });
        }
        if !self.allowed.iter().any(|a| matches(a, &request.action)) {
            return Err(OutsideEnvelope::NotGranted {
                action: request.action.clone(),
            });
        }
        Ok(())
    }
}

/// Whether a grant covers an action, allowing a single trailing `*`.
///
/// `container:*` covering `container:rebuild` is the difference between an
/// approval a person gives once and one they give twenty times.
fn matches(grant: &str, action: &str) -> bool {
    match grant.strip_suffix('*') {
        Some(prefix) => action.starts_with(prefix),
        None => grant == action,
    }
}

/// How a person answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    /// The option chosen, matching one offered where one was.
    pub chosen: String,
    /// Anything they added.
    #[serde(default)]
    pub note: String,
    /// When they answered.
    pub at: Timestamp,
}

/// A question only a person can answer, and what came of it.
///
/// Kept as its own record rather than as a task, because a decision is not
/// work: nobody does it, it has no assignee, and turning it into a card is how
/// an operator ends up hunting through a board for yellow items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// Stable id.
    pub id: DecisionId,
    /// The room that asked.
    pub room: RoomId,
    /// Completes "Operator decision required because: ___".
    pub because: String,
    /// Informational owner question, rather than a request for an authority grant.
    #[serde(default)]
    pub owner_question: bool,
    /// The answers offered, for a one-tap reply.
    #[serde(default)]
    pub options: Vec<String>,
    /// What an approval would authorise.
    #[serde(default)]
    pub envelope: Envelope,
    /// The task that prompted it, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskId>,
    /// Bots waiting for the operator's answer, including questions asked
    /// before a task was claimed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asked_by: Vec<AgentId>,
    /// The answer, once given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Answer>,
    /// When it was asked.
    pub asked_at: Timestamp,
    /// A later decision that replaces this one's authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<DecisionId>,
    /// Why an unanswered question was taken back, such as the owner parking
    /// the task it was about. A withdrawn question no longer waits on anyone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawn: Option<String>,
    /// Which standing rules were in force when this was asked.
    ///
    /// So a decision can be read against the rules that governed it rather
    /// than the ones adopted since.
    #[serde(default)]
    pub governed_by: String,
}

impl Decision {
    /// A fresh, unanswered question.
    pub fn ask(room: RoomId, because: impl Into<String>) -> Self {
        Self {
            id: DecisionId::new(),
            room,
            because: because.into(),
            owner_question: false,
            options: Vec::new(),
            envelope: Envelope::default(),
            task: None,
            asked_by: Vec::new(),
            answer: None,
            asked_at: Timestamp::now(),
            superseded_by: None,
            withdrawn: None,
            governed_by: String::new(),
        }
    }

    /// Whether this decision still carries authority.
    ///
    /// A superseded decision is inert even though it was answered, so a
    /// revoked approval cannot come back and authorise something.
    pub const fn is_authoritative(&self) -> bool {
        self.answer.is_some() && self.superseded_by.is_none()
    }

    /// Whether this is still waiting on a person.
    pub const fn is_pending(&self) -> bool {
        self.answer.is_none() && self.superseded_by.is_none() && self.withdrawn.is_none()
    }

    /// An identity for "the same question".
    ///
    /// Two requests match when they ask the same thing about the same
    /// boundary. Wording is normalised — case, surrounding space and trailing
    /// punctuation — because a model rarely phrases a question identically
    /// twice, and a fingerprint that changes with the phrasing would let the
    /// same question through again and again.
    ///
    /// The envelope is part of the identity on purpose: asking to rebuild a
    /// container is not the same question as asking to rebuild it somewhere
    /// new, and only the second deserves to reach anyone.
    pub fn fingerprint(&self) -> String {
        let mut parts = vec![
            if self.owner_question { "question" } else { "authorization" }.to_string(),
            normalise(&self.because),
            self.envelope.environment.clone(),
        ];
        if self.owner_question {
            let mut options: Vec<String> =
                self.options.iter().map(|option| normalise(option)).collect();
            options.sort();
            parts.extend(options);
        }
        let mut allowed = self.envelope.allowed.clone();
        allowed.sort();
        parts.extend(allowed);
        let mut forbidden = self.envelope.forbidden.clone();
        forbidden.sort();
        parts.extend(forbidden);
        parts.join("\u{1f}")
    }

    /// Whether `other` asks what this decision already settled.
    pub fn already_answers(&self, other: &Self) -> bool {
        self.is_authoritative() && self.fingerprint() == other.fingerprint()
    }
}

/// Reduce a question to its content, so rephrasing does not evade the check.
fn normalise(text: &str) -> String {
    text.trim()
        .trim_end_matches(['.', '!', '?'])
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A standing rule a room operates under.
///
/// The difference between this and a decision is who it binds. A decision
/// answers one question; a policy answers a class of them in advance, so the
/// question never has to be asked. "Internal evidence recovery does not need
/// the operator" is a policy — nothing about it is specific to any one task,
/// and re-deciding it every time is exactly the pestering worth removing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// A short stable name, referenced when a hold cites it.
    pub name: String,
    /// What the rule says, in words a bot reads.
    pub rule: String,
    /// What it permits, if it grants anything.
    #[serde(default)]
    pub envelope: Envelope,
    /// Which revision of this rule is in force.
    ///
    /// Stamped onto anything the policy governed, so a later reading can tell
    /// which version of the rules a decision was made under rather than
    /// judging old work by new rules.
    #[serde(default)]
    pub version: u32,
    /// Whether it is still in force. A withdrawn policy is kept so that work
    /// governed by it can still be understood.
    #[serde(default = "yes")]
    pub active: bool,
}

const fn yes() -> bool {
    true
}

impl Policy {
    /// A new rule at version one.
    pub fn new(name: impl Into<String>, rule: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            rule: rule.into(),
            envelope: Envelope::default(),
            version: 1,
            active: true,
        }
    }

    /// Builder: what this policy permits.
    pub fn granting(mut self, envelope: Envelope) -> Self {
        self.envelope = envelope;
        self
    }
}

/// A room's standing rules.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Governance {
    /// Every rule, current and withdrawn.
    #[serde(default)]
    pub policies: Vec<Policy>,
}

impl Governance {
    /// Add or replace a rule, bumping its version.
    ///
    /// Replacing rather than appending is what stops an obsolete rule
    /// resurfacing as live authority beside the one that replaced it.
    pub fn adopt(&mut self, policy: Policy) -> u32 {
        match self.policies.iter_mut().find(|p| p.name == policy.name) {
            Some(existing) => {
                let version = existing.version + 1;
                *existing = Policy {
                    version,
                    ..policy
                };
                version
            }
            None => {
                let version = policy.version.max(1);
                self.policies.push(Policy { version, ..policy });
                version
            }
        }
    }

    /// Withdraw a rule, keeping it for the record.
    pub fn withdraw(&mut self, name: &str) -> bool {
        match self.policies.iter_mut().find(|p| p.name == name) {
            Some(policy) => {
                policy.active = false;
                true
            }
            None => false,
        }
    }

    /// The rules currently in force.
    pub fn in_force(&self) -> impl Iterator<Item = &Policy> {
        self.policies.iter().filter(|policy| policy.active)
    }

    /// Which rule, if any, already permits `request`.
    pub fn permits(&self, request: &Request, now: Timestamp) -> Option<&Policy> {
        self.in_force()
            .find(|policy| policy.envelope.covers(request, now).is_ok())
    }

    /// A stamp naming the rules in force, for recording what governed an
    /// action.
    ///
    /// Kept as text rather than a structure because its only job is to be read
    /// back later and understood: "which rules applied when this happened?".
    pub fn stamp(&self) -> String {
        let mut named: Vec<String> = self
            .in_force()
            .map(|policy| format!("{}@{}", policy.name, policy.version))
            .collect();
        named.sort();
        named.join(",")
    }
}

/// Every decision a room has taken, and the authority that survives.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionLog {
    /// In the order they were asked.
    #[serde(default)]
    pub decisions: Vec<Decision>,
}

impl DecisionLog {
    /// Record a new question, or return the answered one that settles it.
    ///
    /// This is the whole anti-pestering mechanism: a room that asks the same
    /// thing twice gets the first answer back instead of a second prompt.
    pub fn ask(&mut self, decision: Decision) -> Settled {
        if let Some(existing) = self
            .decisions
            .iter()
            .find(|prior| prior.already_answers(&decision))
        {
            return Settled::Already(existing.clone());
        }
        // An identical question already waiting is not worth asking twice
        // either; the person will answer the one they already have.
        if let Some(pending) = self
            .decisions
            .iter()
            .find(|prior| prior.is_pending() && prior.fingerprint() == decision.fingerprint())
        {
            return Settled::AlreadyAsked(pending.clone());
        }
        let id = decision.id;
        self.decisions.push(decision);
        Settled::Asked(id)
    }

    /// Record an answer.
    pub fn answer(&mut self, id: DecisionId, answer: Answer) -> bool {
        match self.decisions.iter_mut().find(|d| d.id == id) {
            Some(decision) if decision.is_pending() => {
                decision.answer = Some(answer);
                true
            }
            _ => false,
        }
    }

    /// Make `later` replace `earlier`, so the old authority stops applying.
    pub fn supersede(&mut self, earlier: DecisionId, later: DecisionId) -> bool {
        if earlier == later || !self.decisions.iter().any(|d| d.id == later) {
            return false;
        }
        match self.decisions.iter_mut().find(|d| d.id == earlier) {
            Some(decision) => {
                decision.superseded_by = Some(later);
                true
            }
            None => false,
        }
    }

    /// Take back a question still waiting on a person. False when it is not
    /// pending, so an answer already given is never undone.
    pub fn withdraw(&mut self, decision: DecisionId, reason: &str) -> bool {
        match self.decisions.iter_mut().find(|d| d.id == decision && d.is_pending()) {
            Some(pending) => {
                pending.withdrawn = Some(reason.to_string());
                true
            }
            None => false,
        }
    }

    /// Whether standing authority already covers `request`.
    ///
    /// Any one authoritative decision is enough. The most specific refusal is
    /// reported when none of them do, so a bot is told the most useful reason
    /// rather than whichever envelope happened to be checked first.
    pub fn authorises(&self, request: &Request, now: Timestamp) -> Result<(), OutsideEnvelope> {
        let mut refusal = OutsideEnvelope::NotGranted {
            action: request.action.clone(),
        };
        for decision in self.decisions.iter().filter(|d| d.is_authoritative()) {
            match decision.envelope.covers(request, now) {
                Ok(()) => return Ok(()),
                // A standing refusal outranks every other reason: it is the
                // one answer that will not change by asking again.
                Err(reason @ OutsideEnvelope::Forbidden { .. }) => return Err(reason),
                Err(reason) => {
                    if matches!(refusal, OutsideEnvelope::NotGranted { .. }) {
                        refusal = reason;
                    }
                }
            }
        }
        Err(refusal)
    }

    /// Questions still waiting on a person.
    pub fn pending(&self) -> impl Iterator<Item = &Decision> {
        self.decisions.iter().filter(|d| d.is_pending())
    }
}

/// What happened when a room asked something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    /// A new question, now waiting on a person.
    Asked(DecisionId),
    /// Already answered. Use this; do not ask.
    Already(Decision),
    /// Already waiting. Do not ask a second time.
    AlreadyAsked(Decision),
}

impl Settled {
    /// Whether this actually needs a person's attention now.
    pub const fn needs_operator(&self) -> bool {
        matches!(self, Self::Asked(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room() -> RoomId {
        RoomId::new()
    }

    fn approved(room: RoomId, because: &str, envelope: Envelope) -> Decision {
        let mut decision = Decision::ask(room, because);
        decision.envelope = envelope;
        decision.answer = Some(Answer {
            chosen: "Approve".into(),
            note: String::new(),
            at: Timestamp::now(),
        });
        decision
    }

    fn docker_envelope() -> Envelope {
        Envelope {
            allowed: vec!["container:*".into(), "run-tests".into()],
            forbidden: vec!["container:publish".into()],
            environment: "same-vps-confined-docker".into(),
            expires_at: None,
        }
    }

    /// The approval that was given once and asked for twenty times.
    #[test]
    fn a_standing_approval_covers_work_inside_it() {
        let now = Timestamp::now();
        let envelope = docker_envelope();

        for action in ["container:rebuild", "container:recreate", "run-tests"] {
            assert_eq!(
                envelope.covers(
                    &Request::new(action, "same-vps-confined-docker"),
                    now
                ),
                Ok(()),
                "`{action}` was already approved"
            );
        }
    }

    #[test]
    fn a_carve_out_beats_a_broad_grant() {
        let now = Timestamp::now();
        let refusal = docker_envelope()
            .covers(
                &Request::new("container:publish", "same-vps-confined-docker"),
                now,
            )
            .unwrap_err();

        assert!(matches!(refusal, OutsideEnvelope::Forbidden { .. }));
        assert!(
            !refusal.worth_asking(),
            "a standing refusal must not send anyone back to the operator"
        );
        assert!(refusal.advice().contains("Do not ask again"));
    }

    /// Stepping outside the boundary is the one case worth a question.
    #[test]
    fn a_new_machine_falls_outside_and_is_worth_asking() {
        let now = Timestamp::now();
        let refusal = docker_envelope()
            .covers(&Request::new("container:rebuild", "a-new-vps"), now)
            .unwrap_err();

        assert!(matches!(
            refusal,
            OutsideEnvelope::WrongEnvironment { .. }
        ));
        assert!(refusal.worth_asking());
    }

    #[test]
    fn an_ungranted_action_is_refused_but_askable() {
        let now = Timestamp::now();
        let refusal = docker_envelope()
            .covers(
                &Request::new("delete-database", "same-vps-confined-docker"),
                now,
            )
            .unwrap_err();
        assert!(matches!(refusal, OutsideEnvelope::NotGranted { .. }));
        assert!(refusal.worth_asking());
    }

    #[test]
    fn an_expired_envelope_authorises_nothing() {
        let envelope = Envelope {
            expires_at: Some(Timestamp(1_000)),
            ..docker_envelope()
        };
        let refusal = envelope
            .covers(
                &Request::new("run-tests", "same-vps-confined-docker"),
                Timestamp(2_000),
            )
            .unwrap_err();
        assert!(matches!(refusal, OutsideEnvelope::Expired { .. }));
    }

    /// The headline behaviour: ask once, never again.
    #[test]
    fn the_same_question_is_not_asked_twice() {
        let room = room();
        let mut log = DecisionLog::default();
        log.decisions.push(approved(
            room,
            "this rebuilds the container on the approved VPS",
            docker_envelope(),
        ));

        let mut repeat = Decision::ask(room, "This rebuilds the container on the approved VPS.");
        repeat.envelope = docker_envelope();

        let settled = log.ask(repeat);
        assert!(
            !settled.needs_operator(),
            "the room already has this answer"
        );
        assert!(matches!(settled, Settled::Already(_)));
    }

    /// A model rarely phrases the same question the same way twice.
    #[test]
    fn rephrasing_does_not_evade_the_check() {
        let room = room();
        let first = approved(room, "this provisions a paid VPS", Envelope::default());
        let second = Decision::ask(room, "  This provisions a paid VPS!  ");
        assert_eq!(first.fingerprint(), second.fingerprint());
        assert!(first.already_answers(&second));
    }

    /// A wider request is a different question and must reach the operator.
    #[test]
    fn a_widened_scope_is_a_new_question() {
        let room = room();
        let mut log = DecisionLog::default();
        log.decisions.push(approved(
            room,
            "this rebuilds the container",
            docker_envelope(),
        ));

        let mut wider = Decision::ask(room, "this rebuilds the container");
        wider.envelope = Envelope {
            environment: "a-different-vps".into(),
            ..docker_envelope()
        };

        assert!(
            log.ask(wider).needs_operator(),
            "a new boundary deserves a new question"
        );
    }

    /// Asking twice before the first is answered must not produce two prompts.
    #[test]
    fn a_question_already_waiting_is_not_repeated() {
        let room = room();
        let mut log = DecisionLog::default();
        let first = Decision::ask(room, "deploy to production?");
        assert!(log.ask(first).needs_operator());

        let again = Decision::ask(room, "Deploy to production");
        let settled = log.ask(again);
        assert!(matches!(settled, Settled::AlreadyAsked(_)));
        assert!(!settled.needs_operator());
        assert_eq!(log.pending().count(), 1, "one prompt, not two");
    }

    /// A revoked approval must not come back and re-authorise anything.
    #[test]
    fn a_superseded_decision_loses_its_authority() {
        let room = room();
        let mut log = DecisionLog::default();
        let old = approved(room, "docker is fine here", docker_envelope());
        let old_id = old.id;
        log.decisions.push(old);

        let now = Timestamp::now();
        let request = Request::new("container:rebuild", "same-vps-confined-docker");
        assert_eq!(log.authorises(&request, now), Ok(()));

        // A later, narrower policy replaces it.
        let mut narrower = approved(
            room,
            "docker is fine but only for tests",
            Envelope {
                allowed: vec!["run-tests".into()],
                environment: "same-vps-confined-docker".into(),
                ..Default::default()
            },
        );
        narrower.id = DecisionId::new();
        let new_id = narrower.id;
        log.decisions.push(narrower);
        assert!(log.supersede(old_id, new_id));

        assert!(
            log.authorises(&request, now).is_err(),
            "the replaced approval must stop applying"
        );
        assert_eq!(
            log.authorises(&Request::new("run-tests", "same-vps-confined-docker"), now),
            Ok(())
        );
    }

    #[test]
    fn an_unanswered_decision_authorises_nothing() {
        let room = room();
        let mut log = DecisionLog::default();
        let mut pending = Decision::ask(room, "can we rebuild?");
        pending.envelope = docker_envelope();
        log.decisions.push(pending);

        assert!(log
            .authorises(
                &Request::new("container:rebuild", "same-vps-confined-docker"),
                Timestamp::now()
            )
            .is_err());
    }

    /// A standing refusal is reported ahead of vaguer reasons, because it is
    /// the one answer that will not change by asking.
    #[test]
    fn a_refusal_outranks_a_missing_grant() {
        let room = room();
        let mut log = DecisionLog::default();
        log.decisions.push(approved(
            room,
            "narrow grant",
            Envelope {
                allowed: vec!["run-tests".into()],
                ..Default::default()
            },
        ));
        log.decisions
            .push(approved(room, "with a carve-out", docker_envelope()));

        let refusal = log
            .authorises(
                &Request::new("container:publish", "same-vps-confined-docker"),
                Timestamp::now(),
            )
            .unwrap_err();
        assert!(matches!(refusal, OutsideEnvelope::Forbidden { .. }));
    }

    #[test]
    fn answering_settles_a_pending_decision_once() {
        let room = room();
        let mut log = DecisionLog::default();
        let decision = Decision::ask(room, "deploy?");
        let id = decision.id;
        log.decisions.push(decision);

        let answer = Answer {
            chosen: "Approve".into(),
            note: "go ahead".into(),
            at: Timestamp::now(),
        };
        assert!(log.answer(id, answer.clone()));
        assert!(!log.answer(id, answer), "it is already settled");
        assert_eq!(log.pending().count(), 0);
    }

    #[test]
    fn a_log_round_trips_through_json() {
        let room = room();
        let mut log = DecisionLog::default();
        log.decisions
            .push(approved(room, "docker is fine", docker_envelope()));
        let json = serde_json::to_string(&log).unwrap();
        let back: DecisionLog = serde_json::from_str(&json).unwrap();
        assert_eq!(back, log);
    }
}

#[cfg(test)]
mod governance_tests {
    use super::*;

    fn docker() -> Policy {
        Policy::new(
            "confined-docker",
            "Docker on the approved VPS is fine inside its boundary.",
        )
        .granting(Envelope {
            allowed: vec!["container:*".into()],
            environment: "same-vps-confined-docker".into(),
            ..Default::default()
        })
    }

    /// A rule answers a class of questions in advance, so they never get asked.
    #[test]
    fn a_standing_rule_permits_without_a_decision() {
        let mut governance = Governance::default();
        governance.adopt(docker());

        let permitted = governance.permits(
            &Request::new("container:rebuild", "same-vps-confined-docker"),
            Timestamp::now(),
        );
        assert_eq!(permitted.map(|p| p.name.as_str()), Some("confined-docker"));
    }

    #[test]
    fn a_rule_does_not_stretch_past_its_boundary() {
        let mut governance = Governance::default();
        governance.adopt(docker());

        assert!(governance
            .permits(
                &Request::new("container:rebuild", "a-different-vps"),
                Timestamp::now()
            )
            .is_none());
    }

    /// Replacing rather than appending is what stops an obsolete rule
    /// resurfacing beside the one that replaced it.
    #[test]
    fn adopting_the_same_rule_again_replaces_it_and_bumps_the_version() {
        let mut governance = Governance::default();
        assert_eq!(governance.adopt(docker()), 1);

        let narrower = Policy::new("confined-docker", "Only tests now.").granting(Envelope {
            allowed: vec!["run-tests".into()],
            environment: "same-vps-confined-docker".into(),
            ..Default::default()
        });
        assert_eq!(governance.adopt(narrower), 2);

        assert_eq!(governance.policies.len(), 1, "one rule, not two");
        assert!(
            governance
                .permits(
                    &Request::new("container:rebuild", "same-vps-confined-docker"),
                    Timestamp::now()
                )
                .is_none(),
            "the old grant must stop applying"
        );
    }

    /// A withdrawn rule is kept, so work it governed can still be understood.
    #[test]
    fn a_withdrawn_rule_stops_applying_but_stays_on_the_record() {
        let mut governance = Governance::default();
        governance.adopt(docker());
        assert!(governance.withdraw("confined-docker"));

        assert_eq!(governance.in_force().count(), 0);
        assert_eq!(governance.policies.len(), 1);
        assert!(governance
            .permits(
                &Request::new("container:rebuild", "same-vps-confined-docker"),
                Timestamp::now()
            )
            .is_none());
    }

    /// Which rules were in force is recorded, so old work is not judged by
    /// rules adopted after it happened.
    #[test]
    fn the_stamp_names_the_rules_and_their_versions() {
        let mut governance = Governance::default();
        governance.adopt(docker());
        governance.adopt(Policy::new("author-recovers", "The author re-emits."));

        let stamp = governance.stamp();
        assert!(stamp.contains("confined-docker@1"), "{stamp}");
        assert!(stamp.contains("author-recovers@1"), "{stamp}");

        governance.adopt(docker());
        assert!(governance.stamp().contains("confined-docker@2"));
    }

    #[test]
    fn a_withdrawn_rule_leaves_the_stamp() {
        let mut governance = Governance::default();
        governance.adopt(docker());
        governance.withdraw("confined-docker");
        assert_eq!(governance.stamp(), "");
    }

    #[test]
    fn governance_round_trips() {
        let mut governance = Governance::default();
        governance.adopt(docker());
        let json = serde_json::to_string(&governance).unwrap();
        assert_eq!(serde_json::from_str::<Governance>(&json).unwrap(), governance);
    }
}
