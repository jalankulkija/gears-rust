use thiserror::Error;

/// Transport-agnostic error type returned by [`crate::ProxyHandlerPluginClient`].
///
/// `forward-proxy`'s core treats any error the same way `prompt-scanner`
/// treats a failed detector call: log it and continue the pipeline as if
/// this handler had returned `ProxyOutcome::Continue` unchanged (fail
/// open), rather than blocking traffic because one handler misbehaved.
#[derive(Debug, Clone, Error)]
pub enum ProxyHandlerError {
    #[error("handler unavailable: {reason}")]
    Unavailable { reason: String },

    #[error("handler timed out")]
    Timeout,

    #[error("internal handler error: {0}")]
    Internal(String),
}
