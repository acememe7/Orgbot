//! Re-reading a sign-in the service refused.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use orgbot_proto::agent::ProviderRef;
use tokio::sync::mpsc;

use crate::error::{ProviderError, Result};
use crate::provider::Provider;
use crate::types::{CompletionRequest, CompletionResponse, StreamEvent};

/// An API provider that, when its credential is refused, builds itself again
/// from the current sign-in and tries once more.
///
/// A sign-in kept in a file (`~/.codex/auth.json`, an `ant auth login`
/// profile, Grok's) is refreshed there by its own CLI; a room that read it at
/// start kept sending the old token and failed every turn after it expired.
#[derive(Debug)]
pub(crate) struct ReloadOnUnauthorized {
    reference: ProviderRef,
    name: String,
    current: RwLock<Arc<dyn Provider>>,
}

impl ReloadOnUnauthorized {
    pub(crate) fn new(reference: ProviderRef, provider: Arc<dyn Provider>) -> Self {
        Self {
            reference,
            name: provider.name().to_string(),
            current: RwLock::new(provider),
        }
    }

    fn current(&self) -> Arc<dyn Provider> {
        self.current.read().map(|guard| Arc::clone(&guard)).unwrap_or_else(|poisoned| Arc::clone(&poisoned.into_inner()))
    }
}

#[async_trait]
impl Provider for ReloadOnUnauthorized {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> Result<CompletionResponse> {
        let provider = self.current();
        let retry = request.clone();
        let first = provider.complete(request, sink.clone()).await;
        if !matches!(first, Err(ProviderError::Unauthorized { .. })) {
            return first;
        }
        let Ok(fresh) = crate::build_once(&self.reference) else {
            return first;
        };
        if let Ok(mut guard) = self.current.write() {
            *guard = Arc::clone(&fresh);
        }
        fresh.complete(retry, sink).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Refuses every request, as a service does an expired token.
    #[derive(Debug)]
    struct Refused;

    #[async_trait]
    impl Provider for Refused {
        fn name(&self) -> &str {
            "refused"
        }

        async fn complete(&self, _request: CompletionRequest, _sink: Option<mpsc::Sender<StreamEvent>>) -> Result<CompletionResponse> {
            Err(ProviderError::Unauthorized { provider: "refused".into(), status: 401, message: String::new() })
        }
    }

    /// A refused sign-in is read again and the request tried once more.
    #[tokio::test]
    async fn a_refused_sign_in_is_read_again_and_tried_once_more() {
        let reference = ProviderRef::Scripted { script: "fresh".into() };
        let provider = ReloadOnUnauthorized::new(reference, Arc::new(Refused));
        let reply = provider.complete(CompletionRequest::new("m", "", Vec::new()), None).await;
        assert!(reply.is_ok(), "{reply:?}");
        assert!(provider.complete(CompletionRequest::new("m", "", Vec::new()), None).await.is_ok(), "the fresh one is kept");
    }
}
