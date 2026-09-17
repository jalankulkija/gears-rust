use std::sync::Arc;

use forward_proxy_sdk::{ProxyHandlerPluginClient, ProxyOutcome, ProxyRequest, ProxyResponse};
use toolkit_macros::domain_model;

/// Runs registered handler plugins in order against each intercepted
/// request/response. Sequential composition, not fan-out — see
/// `gears/forward-proxy/docs/DESIGN.md` for why this differs from
/// `prompt-scanner`'s detector fan-out.
///
/// A handler that errors is logged and skipped (fails open) rather than
/// aborting the whole pipeline — one misbehaving handler should not take
/// down proxying for every other handler and for traffic that handler
/// doesn't even care about.
#[domain_model]
#[derive(Clone)]
pub struct Pipeline {
    /// Ascending priority order (lowest first) — callers are responsible
    /// for sorting before constructing (see
    /// `infra::handler_registry::resolve_handlers`).
    handlers: Vec<Arc<dyn ProxyHandlerPluginClient>>,
}

impl Pipeline {
    #[must_use]
    pub fn new(handlers: Vec<Arc<dyn ProxyHandlerPluginClient>>) -> Self {
        Self { handlers }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    /// Runs every handler's `on_request` in order. Returns `Block` from the
    /// first handler that blocks (no later handlers run); otherwise returns
    /// `Continue` with the request as modified by every handler in turn.
    pub async fn run_request(&self, req: ProxyRequest) -> ProxyOutcome<ProxyRequest> {
        let mut current = req;
        for handler in &self.handlers {
            match handler.on_request(current.clone()).await {
                Ok(ProxyOutcome::Continue(next)) => current = next,
                Ok(block @ ProxyOutcome::Block { .. }) => return block,
                Err(err) => {
                    tracing::warn!(
                        handler = handler.id(),
                        error = %err,
                        "forward-proxy: handler errored on request, failing open"
                    );
                }
            }
        }
        ProxyOutcome::Continue(current)
    }

    /// Runs every handler's `on_response` in order. Same fail-open behavior
    /// as `run_request`. Non-blocking by convention in v1 (see
    /// `docs/DESIGN.md`'s "Response handling" section) — built-in handlers
    /// only use this for observation/rewriting, never `Block`, but the
    /// pipeline itself doesn't enforce that; it just executes whatever a
    /// handler returns.
    pub async fn run_response(&self, resp: ProxyResponse) -> ProxyOutcome<ProxyResponse> {
        let mut current = resp;
        for handler in &self.handlers {
            match handler.on_response(current.clone()).await {
                Ok(ProxyOutcome::Continue(next)) => current = next,
                Ok(block @ ProxyOutcome::Block { .. }) => return block,
                Err(err) => {
                    tracing::warn!(
                        handler = handler.id(),
                        error = %err,
                        "forward-proxy: handler errored on response, failing open"
                    );
                }
            }
        }
        ProxyOutcome::Continue(current)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use async_trait::async_trait;
    use bytes::Bytes;
    use forward_proxy_sdk::ProxyHandlerError;
    use http::{HeaderMap, Method, StatusCode};

    use super::*;

    fn sample_request() -> ProxyRequest {
        ProxyRequest {
            method: Method::POST,
            uri: "/v1/chat/completions".to_owned(),
            target_host: "api.openai.com".to_owned(),
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"hello"),
        }
    }

    fn sample_response() -> ProxyResponse {
        ProxyResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"world"),
        }
    }

    /// Appends its id to the request body — lets tests assert both *that*
    /// and *in what order* handlers ran.
    struct AppendingHandler {
        id: &'static str,
    }

    #[async_trait]
    impl ProxyHandlerPluginClient for AppendingHandler {
        fn id(&self) -> &'static str {
            self.id
        }

        async fn on_request(
            &self,
            mut req: ProxyRequest,
        ) -> Result<ProxyOutcome<ProxyRequest>, ProxyHandlerError> {
            let mut body = req.body.to_vec();
            body.extend_from_slice(self.id.as_bytes());
            req.body = Bytes::from(body);
            Ok(ProxyOutcome::Continue(req))
        }

        async fn on_response(
            &self,
            mut resp: ProxyResponse,
        ) -> Result<ProxyOutcome<ProxyResponse>, ProxyHandlerError> {
            let mut body = resp.body.to_vec();
            body.extend_from_slice(self.id.as_bytes());
            resp.body = Bytes::from(body);
            Ok(ProxyOutcome::Continue(resp))
        }
    }

    struct BlockingHandler;

    #[async_trait]
    impl ProxyHandlerPluginClient for BlockingHandler {
        fn id(&self) -> &'static str {
            "blocking"
        }

        async fn on_request(
            &self,
            _req: ProxyRequest,
        ) -> Result<ProxyOutcome<ProxyRequest>, ProxyHandlerError> {
            Ok(ProxyOutcome::Block {
                status: StatusCode::FORBIDDEN,
                headers: HeaderMap::new(),
                body: Bytes::from_static(b"blocked"),
                reason: "test block".to_owned(),
            })
        }
    }

    struct ErroringHandler;

    #[async_trait]
    impl ProxyHandlerPluginClient for ErroringHandler {
        fn id(&self) -> &'static str {
            "erroring"
        }

        async fn on_request(
            &self,
            _req: ProxyRequest,
        ) -> Result<ProxyOutcome<ProxyRequest>, ProxyHandlerError> {
            Err(ProxyHandlerError::Internal("boom".to_owned()))
        }
    }

    #[tokio::test]
    async fn empty_pipeline_passes_request_through_unchanged() {
        let pipeline = Pipeline::new(vec![]);
        let req = sample_request();
        let original_body = req.body.clone();

        let outcome = pipeline.run_request(req).await;

        match outcome {
            ProxyOutcome::Continue(req) => assert_eq!(req.body, original_body),
            ProxyOutcome::Block { .. } => panic!("expected Continue"),
        }
    }

    #[tokio::test]
    async fn handlers_run_in_registration_order() {
        let pipeline = Pipeline::new(vec![
            Arc::new(AppendingHandler { id: "a" }),
            Arc::new(AppendingHandler { id: "b" }),
        ]);

        let outcome = pipeline.run_request(sample_request()).await;

        match outcome {
            ProxyOutcome::Continue(req) => {
                assert_eq!(req.body.as_ref(), b"helloab");
            }
            ProxyOutcome::Block { .. } => panic!("expected Continue"),
        }
    }

    #[tokio::test]
    async fn blocking_handler_short_circuits_remaining_handlers() {
        let pipeline = Pipeline::new(vec![
            Arc::new(AppendingHandler { id: "a" }),
            Arc::new(BlockingHandler),
            Arc::new(AppendingHandler {
                id: "should-not-run",
            }),
        ]);

        let outcome = pipeline.run_request(sample_request()).await;

        match outcome {
            ProxyOutcome::Block { status, reason, .. } => {
                assert_eq!(status, StatusCode::FORBIDDEN);
                assert_eq!(reason, "test block");
            }
            ProxyOutcome::Continue(_) => panic!("expected Block"),
        }
    }

    #[tokio::test]
    async fn erroring_handler_fails_open_and_pipeline_continues() {
        let pipeline = Pipeline::new(vec![
            Arc::new(ErroringHandler),
            Arc::new(AppendingHandler { id: "a" }),
        ]);

        let outcome = pipeline.run_request(sample_request()).await;

        match outcome {
            ProxyOutcome::Continue(req) => assert_eq!(req.body.as_ref(), b"helloa"),
            ProxyOutcome::Block { .. } => panic!("expected Continue (fail-open)"),
        }
    }

    #[tokio::test]
    async fn run_response_runs_handlers_in_order() {
        let pipeline = Pipeline::new(vec![
            Arc::new(AppendingHandler { id: "x" }),
            Arc::new(AppendingHandler { id: "y" }),
        ]);

        let outcome = pipeline.run_response(sample_response()).await;

        match outcome {
            ProxyOutcome::Continue(resp) => assert_eq!(resp.body.as_ref(), b"worldxy"),
            ProxyOutcome::Block { .. } => panic!("expected Continue"),
        }
    }

    #[test]
    fn is_empty_reflects_handler_count() {
        assert!(Pipeline::new(vec![]).is_empty());
        assert!(!Pipeline::new(vec![Arc::new(BlockingHandler)]).is_empty());
    }
}
