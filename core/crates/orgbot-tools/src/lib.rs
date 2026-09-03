//! # orgbot-tools
//!
//! The tool surface a bot uses to act. There is no shell and no computer use:
//! a bot's entire capability is chat, the task board, handoffs, inspection, and
//! artifacts.
//!
//! Two design rules run through this crate.
//!
//! **Capability is granted by omission.** [`room::definitions_for`] filters the
//! tool list by the bot's autonomy, so a worker is never offered
//! `record_review`. The engine still refuses the call if it somehow arrives,
//! but withholding the tool is the primary control — a model cannot be tempted
//! by something it cannot see.
//!
//! **Errors are instructions.** Every rejection returns readable advice about
//! what to do instead, because the model's next turn reads it and can correct
//! itself.

#![forbid(unsafe_code)]
// See the note in orgbot-proto on `missing_docs` and data-carrier variants.
#![warn(missing_debug_implementations, rust_2018_idioms, unreachable_pub)]

pub mod room;
pub mod schema;

pub use room::{definitions_for, dispatch, ToolOutcome};
