//! The backend trait.

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::error::Result;
use crate::types::{CompletionRequest, CompletionResponse, StreamEvent};

/// A model backend.
///
/// One method covers both modes: pass a `sink` to receive incremental updates
/// while the turn runs, or `None` to just wait for the result. Either way the
/// return value is the complete assembled turn, so the agent loop does not care
/// whether the UI was watching.
#[async_trait]
pub trait Provider: Send + Sync + std::fmt::Debug {
    /// A stable name for logs and error messages.
    fn name(&self) -> &str;

    /// Run one model turn.
    async fn complete(
        &self,
        request: CompletionRequest,
        sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> Result<CompletionResponse>;
}

/// Emit a stream event, ignoring a closed receiver.
///
/// A dropped UI subscriber must not abort a turn that is otherwise fine, so
/// send failures here are deliberately swallowed.
pub(crate) async fn emit(sink: &Option<mpsc::Sender<StreamEvent>>, event: StreamEvent) {
    if let Some(tx) = sink {
        let _ = tx.send(event).await;
    }
}
