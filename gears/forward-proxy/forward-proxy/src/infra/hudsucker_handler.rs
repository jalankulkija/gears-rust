//! `hudsucker::HttpHandler` implementation. This is the **only** place
//! `hyper`/`hudsucker` types are allowed to exist — everything past this
//! boundary is `forward_proxy_sdk`'s transport-agnostic `ProxyRequest`/
//! `ProxyResponse`, run through `domain::pipeline::Pipeline`.
//!
//! `HttpHandler` requires `Clone + Send + Sync + 'static` and `&mut self`
//! methods (hudsucker clones the handler once per intercepted connection).
//! Nothing here actually needs mutation — `Pipeline` and `HostMatcher` are
//! wrapped in `Arc` and cloned cheaply, so `&mut self` is trivially
//! satisfied without any real interior mutability.

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::BodyExt;
use hudsucker::hyper::{Request, Response};
use hudsucker::{Body, HttpContext, HttpHandler, RequestOrResponse};

use forward_proxy_sdk::{ProxyOutcome, ProxyRequest, ProxyResponse};

use crate::domain::host_matcher::HostMatcher;
use crate::domain::pipeline::Pipeline;

#[derive(Clone)]
pub struct ForwardProxyHandler {
    pipeline: Arc<Pipeline>,
    host_matcher: Arc<HostMatcher>,
}

impl ForwardProxyHandler {
    #[must_use]
    pub fn new(pipeline: Arc<Pipeline>, host_matcher: Arc<HostMatcher>) -> Self {
        Self {
            pipeline,
            host_matcher,
        }
    }

    /// Whether `host` is one this gear's config says to MITM at all — the
    /// shared gate behind `should_intercept_connect`/`should_intercept_tls`
    /// (pulled out as a plain method, not just inlined, so it's testable
    /// without needing a real `hudsucker::HttpContext`, which has no public
    /// constructor).
    ///
    /// Logs at `debug` (not `info`) for the pass-through case specifically
    /// — this fires for *every* non-matched host a proxied browser talks to
    /// (every ad/tracker/CDN on every page), so at `info` it would drown
    /// out everything else. Turn it on with `RUST_LOG=forward_proxy=debug`
    /// when you need to see what's being filtered out.
    fn wants_to_intercept(&self, host: &str) -> bool {
        let intercept = self.host_matcher.matches(host);
        if intercept {
            tracing::debug!(host, "forward-proxy: intercepting (MITM + scan)");
        } else {
            tracing::debug!(
                host,
                "forward-proxy: not intercepting (host not in intercept_hosts/intercept_host_suffixes) - passing through as an opaque tunnel"
            );
        }
        intercept
    }

    /// Turns the pipeline's verdict into what hudsucker actually sends —
    /// pulled out of `handle_request` to keep that function's cognitive
    /// complexity down.
    fn request_outcome_to_response(outcome: ProxyOutcome<ProxyRequest>) -> RequestOrResponse {
        match outcome {
            ProxyOutcome::Continue(proxy_req) => match rebuild_request(proxy_req) {
                Ok(req) => req.into(),
                Err(err) => {
                    tracing::error!(error = %err, "forward-proxy: failed to rebuild request after pipeline");
                    RequestOrResponse::Response(fallback_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        http::HeaderMap::new(),
                        Bytes::new(),
                    ))
                }
            },
            ProxyOutcome::Block {
                status,
                headers,
                body,
                reason,
            } => {
                tracing::info!(reason = %reason, "forward-proxy: request blocked");
                RequestOrResponse::Response(fallback_response(status, headers, body))
            }
        }
    }

    fn target_host(req: &Request<Body>) -> String {
        req.uri()
            .host()
            .map(str::to_owned)
            .or_else(|| {
                req.headers()
                    .get(http::header::HOST)
                    .and_then(|v| v.to_str().ok())
                    .map(|h| h.split(':').next().unwrap_or(h).to_owned())
            })
            .unwrap_or_default()
    }
}

async fn buffer_request_body(req: Request<Body>) -> (http::request::Parts, Bytes) {
    let (parts, body) = req.into_parts();
    let bytes = body
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (parts, bytes)
}

async fn buffer_response_body(resp: Response<Body>) -> (http::response::Parts, Bytes) {
    let (parts, body) = resp.into_parts();
    let bytes = body
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (parts, bytes)
}

/// Rebuilds a `hyper::Request` from a (possibly handler-modified)
/// `ProxyRequest`. The only failure mode is a handler producing an
/// unparsable method/URI, which we treat as an internal-error block rather
/// than propagating a panic.
fn rebuild_request(proxy_req: ProxyRequest) -> Result<Request<Body>, http::Error> {
    let mut builder = Request::builder()
        .method(proxy_req.method)
        .uri(proxy_req.uri);
    if let Some(headers) = builder.headers_mut() {
        *headers = proxy_req.headers;
    }
    builder.body(Body::from(proxy_req.body))
}

/// Rebuilds a `hyper::Response` from a (possibly handler-modified)
/// `ProxyResponse`. Same failure-mode reasoning as `rebuild_request`.
fn rebuild_response(proxy_resp: ProxyResponse) -> Result<Response<Body>, http::Error> {
    let mut builder = Response::builder().status(proxy_resp.status);
    if let Some(headers) = builder.headers_mut() {
        *headers = proxy_resp.headers;
    }
    builder.body(Body::from(proxy_resp.body))
}

/// Infallible fallback response — used only when rebuilding a
/// handler-modified request/response fails, or a handler's own `Block`
/// response fails to construct. `Response::new` and setting `status_mut()`
/// on an already-valid `StatusCode` cannot fail, so this needs no
/// `Result`/`expect`.
fn fallback_response(
    status: http::StatusCode,
    headers: http::HeaderMap,
    body: Bytes,
) -> Response<Body> {
    let mut resp = Response::new(Body::from(body));
    *resp.status_mut() = status;
    resp.headers_mut().extend(headers);
    resp
}

impl HttpHandler for ForwardProxyHandler {
    /// Whether to even open a MITM tunnel for this `CONNECT`. `hudsucker`
    /// defaults to `true` for every connection — without this override,
    /// forward-proxy decrypts and re-parses *all* HTTPS traffic through it
    /// (any site, any WebSocket), not just `intercept_hosts`. That's not a
    /// no-op: hudsucker's generic frame-relay code doesn't understand
    /// extensions like WebSocket's `permessage-deflate`, so MITM'ing
    /// connections we never intended to touch (e.g. `ws.chatgpt.com`, which
    /// isn't in `intercept_hosts`) corrupts frames the real endpoints would
    /// have handled fine as an opaque byte tunnel.
    async fn should_intercept_connect(&mut self, _ctx: &HttpContext, req: &Request<Body>) -> bool {
        self.wants_to_intercept(&Self::target_host(req))
    }

    /// Same gate as `should_intercept_connect`, applied to the TLS
    /// `ClientHello`'s SNI — belt and suspenders in case a caller's
    /// `HttpHandler` setup ever reaches this without going through
    /// `should_intercept_connect` first.
    async fn should_intercept_tls(
        &mut self,
        _ctx: &HttpContext,
        client_hello: hudsucker::rustls::server::ClientHello<'_>,
    ) -> bool {
        client_hello
            .server_name()
            .is_some_and(|name| self.wants_to_intercept(name))
    }

    async fn handle_request(
        &mut self,
        _ctx: &HttpContext,
        req: Request<Body>,
    ) -> RequestOrResponse {
        // CONNECT requests must pass through untouched. hudsucker calls
        // `handle_request` for the CONNECT itself (before the TLS tunnel
        // exists), then relies on hyper's `OnUpgrade` extension on *this
        // same request* to hand off the raw connection for MITM. Buffering
        // the body and rebuilding a fresh `Request` (as below, for real
        // HTTP requests) drops that extension — `hyper::upgrade::on` then
        // fails with "no upgrade available" and every tunnel breaks. There's
        // nothing to scan on a CONNECT anyway (no body, just "open a tunnel
        // to host:port"); the actual HTTP requests worth scanning arrive as
        // separate `handle_request` calls once hudsucker has decrypted the
        // tunnel.
        if req.method() == http::Method::CONNECT {
            return req.into();
        }

        let target_host = Self::target_host(&req);
        if !self.host_matcher.matches(&target_host) {
            return req.into();
        }

        let method = req.method().clone();
        let uri = req.uri().to_string();
        // Access-log line for every intercepted request, logged *before*
        // running the pipeline — unlike the block-only log below, this
        // fires regardless of verdict, so "is traffic even reaching this
        // plugin at all" is answerable by grepping the log instead of
        // inferring it from silence.
        tracing::info!(host = %target_host, %method, %uri, "forward-proxy: intercepted request");

        let (parts, body) = buffer_request_body(req).await;
        let proxy_req = ProxyRequest {
            method: parts.method.clone(),
            uri: parts.uri.to_string(),
            target_host,
            headers: parts.headers.clone(),
            body,
        };

        Self::request_outcome_to_response(self.pipeline.run_request(proxy_req).await)
    }

    async fn handle_response(
        &mut self,
        _ctx: &HttpContext,
        resp: Response<Body>,
    ) -> Response<Body> {
        let (parts, body) = buffer_response_body(resp).await;
        let proxy_resp = ProxyResponse {
            status: parts.status,
            headers: parts.headers.clone(),
            body,
        };

        match self.pipeline.run_response(proxy_resp).await {
            ProxyOutcome::Continue(proxy_resp) => rebuild_response(proxy_resp).unwrap_or_else(|err| {
                tracing::error!(error = %err, "forward-proxy: failed to rebuild response after pipeline");
                fallback_response(http::StatusCode::INTERNAL_SERVER_ERROR, http::HeaderMap::new(), Bytes::new())
            }),
            ProxyOutcome::Block {
                status,
                headers,
                body,
                reason,
            } => {
                tracing::info!(reason = %reason, "forward-proxy: response blocked");
                fallback_response(status, headers, body)
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crate::domain::host_matcher::HostPattern;

    use super::*;

    fn handler(patterns: Vec<HostPattern>) -> ForwardProxyHandler {
        ForwardProxyHandler::new(
            Arc::new(Pipeline::new(Vec::new())),
            Arc::new(HostMatcher::new(patterns)),
        )
    }

    fn connect_request(authority: &str) -> Request<Body> {
        Request::builder()
            .method(http::Method::CONNECT)
            .uri(authority)
            .body(Body::empty())
            .expect("valid CONNECT request")
    }

    /// This is the regression test for the bug that made forward-proxy MITM
    /// *every* HTTPS connection through it (breaking unrelated sites'
    /// `WebSockets` — see `should_intercept_connect`'s doc comment) instead of
    /// just `intercept_hosts`. `should_intercept_connect`/`should_intercept_tls`
    /// themselves aren't directly testable here — `hudsucker::HttpContext` has
    /// no public constructor — so this exercises the shared
    /// `wants_to_intercept` gate behind both instead.
    #[test]
    fn only_intercepts_matched_hosts() {
        let handler = handler(vec![HostPattern::Exact("chatgpt.com".to_owned())]);

        assert!(
            handler.wants_to_intercept(&ForwardProxyHandler::target_host(&connect_request(
                "chatgpt.com:443"
            )))
        );
        assert!(
            !handler.wants_to_intercept(&ForwardProxyHandler::target_host(&connect_request(
                "ws.chatgpt.com:443"
            )))
        );
        assert!(
            !handler.wants_to_intercept(&ForwardProxyHandler::target_host(&connect_request(
                "example.com:443"
            )))
        );
    }

    #[test]
    fn intercept_defaults_to_false_with_no_patterns() {
        let handler = handler(Vec::new());

        assert!(
            !handler.wants_to_intercept(&ForwardProxyHandler::target_host(&connect_request(
                "chatgpt.com:443"
            )))
        );
    }
}
