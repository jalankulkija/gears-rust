use bytes::Bytes;
use http::{HeaderMap, Method, StatusCode};

/// Generic, protocol-level view of an intercepted request — no domain
/// concept of "prompt" or "attachment" lives here. Handler plugins that
/// care about those interpret `body`/`headers` themselves.
///
/// Uses `http`'s transport-agnostic `Method`/`HeaderMap` (the same crate
/// `hyper`, `axum`, and `reqwest` all build on) rather than any
/// proxy-library-specific request type, so this stays a domain model
/// (`forward-proxy`'s `infra` layer converts to/from `hyper::Request` at
/// the boundary — see `docs/DESIGN.md`).
#[derive(Debug, Clone)]
pub struct ProxyRequest {
    pub method: Method,
    /// Full request URI as seen by the proxy (path + query for the
    /// forwarded request; the `CONNECT` target for the tunnel itself).
    pub uri: String,
    /// SNI / `Host` header target — what a handler matches against to
    /// decide whether it cares about this request.
    pub target_host: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

/// Generic, protocol-level view of an intercepted response.
#[derive(Debug, Clone)]
pub struct ProxyResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

/// What a handler plugin decides to do with a request or response it was
/// given. `Continue` may carry a modified value; `Block` short-circuits the
/// pipeline (no further handlers run, and — for a blocked request — nothing
/// is forwarded upstream).
#[derive(Debug, Clone)]
pub enum ProxyOutcome<T> {
    Continue(T),
    Block {
        status: StatusCode,
        /// Response headers to send with `body` — e.g. `content-type`.
        /// A blocked request that was itself a streaming request (`stream:
        /// true`) needs `text/event-stream` here, or the client's stream
        /// reader fails to parse `body` at all and shows its own generic
        /// "connection interrupted" fallback instead of anything from
        /// `body`.
        headers: HeaderMap,
        body: Bytes,
        /// Human-readable reason, surfaced in logs and (for handlers that
        /// use it) a banner shown to the end user.
        reason: String,
    },
}

impl<T> ProxyOutcome<T> {
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        matches!(self, Self::Block { .. })
    }
}
