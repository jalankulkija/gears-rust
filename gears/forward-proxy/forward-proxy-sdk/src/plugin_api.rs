//! Plugin API trait implemented by forward-proxy handler plugins.
//!
//! # Sequential composition, not fan-out
//!
//! Unlike `prompt-scanner`'s `ScannerBackendClient` (which fans out to every
//! enabled detector concurrently and aggregates independent findings),
//! handler plugins here compose **sequentially**, in ascending priority
//! order: each handler sees the request/response as the previous handler
//! left it, and may modify it for the next one. This is a middleware chain,
//! not a voting panel — see `docs/DESIGN.md` for why.
//!
//! Each handler plugin registers:
//! 1. A `PluginV1<ProxyHandlerPluginSpecV1>` instance in `types-registry`
//!    (discoverable by `forward-proxy`'s core).
//! 2. A scoped `ClientHub` entry for `dyn ProxyHandlerPluginClient`, keyed
//!    by the instance's GTS id (`ClientScope::gts_id`).
//!
//! A handler that has no interest in a given request's `target_host`
//! should return `ProxyOutcome::Continue` unchanged as cheaply as possible
//! — v1 has no registry-level host-pattern pre-filter (see "Open
//! questions" in `docs/DESIGN.md`), so every registered handler is invoked
//! for every request.

use async_trait::async_trait;

use crate::errors::ProxyHandlerError;
use crate::models::{ProxyOutcome, ProxyRequest, ProxyResponse};

/// Plugin API trait implemented by each forward-proxy handler.
#[async_trait]
pub trait ProxyHandlerPluginClient: Send + Sync {
    /// Unique id for logging (e.g. `"openai-interceptor"`). Defaults to a
    /// generic label so implementors aren't forced to override it.
    fn id(&self) -> &'static str {
        "unnamed-handler"
    }

    /// Called for every request before it's forwarded upstream, in
    /// ascending-priority order across registered handlers.
    async fn on_request(
        &self,
        req: ProxyRequest,
    ) -> Result<ProxyOutcome<ProxyRequest>, ProxyHandlerError>;

    /// Called for every response before it reaches the client. Non-blocking
    /// by convention in v1 (see `docs/DESIGN.md`'s "Response handling"
    /// section) — default implementation passes the response through
    /// unchanged, so handlers that only care about requests don't need to
    /// override this.
    async fn on_response(
        &self,
        resp: ProxyResponse,
    ) -> Result<ProxyOutcome<ProxyResponse>, ProxyHandlerError> {
        Ok(ProxyOutcome::Continue(resp))
    }
}
