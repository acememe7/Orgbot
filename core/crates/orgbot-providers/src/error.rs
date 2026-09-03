//! Provider failures, classified by whether retrying could help.

use serde::{Deserialize, Serialize};

/// Something went wrong talking to a model backend.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// No usable credential was found.
    #[error("no credential for {provider}: {detail}")]
    MissingCredential {
        /// Which backend.
        provider: String,
        /// Where the resolver looked.
        detail: String,
    },

    /// The credential was rejected.
    #[error("{provider} rejected the credential (HTTP {status})")]
    Unauthorized {
        /// Which backend.
        provider: String,
        /// The status returned.
        status: u16,
    },

    /// Rate limited. Retry after the given delay if one was supplied.
    #[error("{provider} rate limited this request")]
    RateLimited {
        /// Which backend.
        provider: String,
        /// Seconds to wait, from the `retry-after` header.
        retry_after_secs: Option<u64>,
    },

    /// The request was malformed.
    #[error("{provider} rejected the request: {message}")]
    BadRequest {
        /// Which backend.
        provider: String,
        /// The backend's explanation.
        message: String,
    },

    /// The backend failed on its side.
    #[error("{provider} returned HTTP {status}: {message}")]
    Upstream {
        /// Which backend.
        provider: String,
        /// The status returned.
        status: u16,
        /// The body, truncated.
        message: String,
    },

    /// The network failed.
    #[error("transport error talking to {provider}: {source}")]
    Transport {
        /// Which backend.
        provider: String,
        /// The underlying failure.
        #[source]
        source: reqwest::Error,
    },

    /// The response did not match the shape the adapter expected.
    #[error("could not decode {provider} response: {detail}")]
    Decode {
        /// Which backend.
        provider: String,
        /// What was wrong.
        detail: String,
    },

    /// A spawned agent CLI failed, timed out, or was killed.
    #[error("{provider} failed: {detail}")]
    Process {
        /// Which CLI.
        provider: String,
        /// What went wrong, including the exit status or stderr tail.
        detail: String,
    },

    /// The configuration names something this build cannot do.
    #[error("{0}")]
    Unsupported(String),
}

impl ProviderError {
    /// Whether retrying the identical request could plausibly succeed.
    ///
    /// The agent loop uses this to decide between backing off and faulting the
    /// bot: a 429 or a 503 is worth another attempt, a 400 never is.
    pub const fn is_retryable(&self) -> bool {
        match self {
            Self::RateLimited { .. } | Self::Transport { .. } => true,
            Self::Upstream { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// How long to wait before retrying, when the backend said.
    pub const fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::RateLimited {
                retry_after_secs, ..
            } => *retry_after_secs,
            _ => None,
        }
    }
}

/// Shorthand for provider results.
pub type Result<T> = std::result::Result<T, ProviderError>;

/// A machine-readable summary of a failure, for the event log and the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorReport {
    /// Which backend failed.
    pub provider: String,
    /// A short classification.
    pub kind: String,
    /// The human-readable message.
    pub message: String,
    /// Whether the caller may retry.
    pub retryable: bool,
}

impl From<&ProviderError> for ErrorReport {
    fn from(error: &ProviderError) -> Self {
        let (provider, kind) = match error {
            ProviderError::MissingCredential { provider, .. } => (provider.clone(), "credential"),
            ProviderError::Unauthorized { provider, .. } => (provider.clone(), "unauthorized"),
            ProviderError::RateLimited { provider, .. } => (provider.clone(), "rate_limited"),
            ProviderError::BadRequest { provider, .. } => (provider.clone(), "bad_request"),
            ProviderError::Upstream { provider, .. } => (provider.clone(), "upstream"),
            ProviderError::Transport { provider, .. } => (provider.clone(), "transport"),
            ProviderError::Decode { provider, .. } => (provider.clone(), "decode"),
            ProviderError::Process { provider, .. } => (provider.clone(), "process"),
            ProviderError::Unsupported(_) => ("unknown".to_string(), "unsupported"),
        };
        Self {
            provider,
            kind: kind.to_string(),
            message: error.to_string(),
            retryable: error.is_retryable(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_errors_and_rate_limits_are_retryable() {
        assert!(ProviderError::RateLimited {
            provider: "anthropic".into(),
            retry_after_secs: Some(3),
        }
        .is_retryable());
        assert!(ProviderError::Upstream {
            provider: "anthropic".into(),
            status: 503,
            message: String::new(),
        }
        .is_retryable());
    }

    #[test]
    fn client_errors_are_not_retryable() {
        for fatal in [
            ProviderError::BadRequest {
                provider: "deepseek".into(),
                message: "bad schema".into(),
            },
            ProviderError::Unauthorized {
                provider: "deepseek".into(),
                status: 401,
            },
            ProviderError::Upstream {
                provider: "deepseek".into(),
                status: 404,
                message: String::new(),
            },
        ] {
            assert!(!fatal.is_retryable(), "{fatal}");
        }
    }

    #[test]
    fn the_retry_delay_is_surfaced_when_the_backend_supplies_one() {
        let error = ProviderError::RateLimited {
            provider: "anthropic".into(),
            retry_after_secs: Some(12),
        };
        assert_eq!(error.retry_after_secs(), Some(12));
        assert_eq!(ErrorReport::from(&error).kind, "rate_limited");
        assert!(ErrorReport::from(&error).retryable);
    }
}
