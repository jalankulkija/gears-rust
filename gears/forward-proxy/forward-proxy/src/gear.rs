use std::sync::Arc;

use async_trait::async_trait;
use hudsucker::Proxy;
use hudsucker::certificate_authority::RcgenAuthority;
use hudsucker::rcgen::{Issuer, KeyPair};
use hudsucker::rustls::crypto::aws_lc_rs;
use tokio_util::sync::CancellationToken;
use toolkit::client_hub::ClientHub;
use toolkit::lifecycle::ReadySignal;
use toolkit::{Gear, GearCtx};
use tracing::info;

use crate::config::ForwardProxyConfig;
use crate::domain::ca;
use crate::domain::host_matcher::{HostMatcher, HostPattern};
use crate::domain::pipeline::Pipeline;
use crate::infra::ca_rcgen::RcgenCaGenerator;
use crate::infra::ca_store_fs::FsCaStore;
use crate::infra::handler_registry;
use crate::infra::hudsucker_handler::ForwardProxyHandler;

/// Generic forward/MITM HTTPS proxy gear. See `docs/DESIGN.md` for the
/// full design — this gear has no built-in knowledge of any specific
/// traffic; all interpretation happens in handler plugins discovered via
/// `types-registry` (see `infra::handler_registry`).
#[toolkit::gear(
    name = "forward-proxy",
    deps = [types_registry],
    capabilities = [stateful],
    lifecycle(entry = "serve", stop_timeout = "10s", await_ready)
)]
pub struct ForwardProxy {
    cfg: std::sync::OnceLock<ForwardProxyConfig>,
    hub: std::sync::OnceLock<Arc<ClientHub>>,
}

impl Default for ForwardProxy {
    fn default() -> Self {
        Self {
            cfg: std::sync::OnceLock::new(),
            hub: std::sync::OnceLock::new(),
        }
    }
}

#[async_trait]
impl Gear for ForwardProxy {
    async fn init(&self, ctx: &GearCtx) -> anyhow::Result<()> {
        let cfg: ForwardProxyConfig = ctx.config_or_default()?;
        info!(
            listen_addr = %cfg.listen_addr,
            intercept_hosts = ?cfg.intercept_hosts,
            intercept_host_suffixes = ?cfg.intercept_host_suffixes,
            "Loaded forward-proxy configuration"
        );

        self.cfg
            .set(cfg)
            .map_err(|_| anyhow::anyhow!("{} gear already initialized", Self::MODULE_NAME))?;
        self.hub
            .set(ctx.client_hub())
            .map_err(|_| anyhow::anyhow!("{} gear already initialized", Self::MODULE_NAME))?;

        Ok(())
    }
}

impl ForwardProxy {
    pub(crate) async fn serve(
        self: Arc<Self>,
        cancel: CancellationToken,
        ready: ReadySignal,
    ) -> anyhow::Result<()> {
        let cfg = self
            .cfg
            .get()
            .ok_or_else(|| anyhow::anyhow!("forward-proxy config not initialized"))?
            .clone();

        // --- CA material: load persisted root CA, or generate + persist a new one ---
        let store = FsCaStore::new(cfg.ca_cert_path.clone(), cfg.ca_key_path.clone());
        let generator = RcgenCaGenerator;
        let material = ca::load_or_generate(&store, &generator).map_err(|e| {
            anyhow::anyhow!("forward-proxy: failed to load/generate CA material: {e}")
        })?;

        let key_pair = KeyPair::from_pem(&material.key_pem)
            .map_err(|e| anyhow::anyhow!("forward-proxy: failed to parse persisted CA key: {e}"))?;
        let issuer = Issuer::from_ca_cert_pem(&material.cert_pem, key_pair).map_err(|e| {
            anyhow::anyhow!("forward-proxy: failed to parse persisted CA cert: {e}")
        })?;
        let ca = RcgenAuthority::new(issuer, 1_000, aws_lc_rs::default_provider());

        // --- Handler pipeline: discover registered handler plugins via types-registry ---
        let hub = self
            .hub
            .get()
            .ok_or_else(|| anyhow::anyhow!("forward-proxy ClientHub not initialized"))?;
        let handlers = handler_registry::resolve_handlers(hub).await.map_err(|e| {
            anyhow::anyhow!("forward-proxy: failed to resolve handler plugins: {e}")
        })?;
        info!(
            handler_count = handlers.len(),
            "forward-proxy: resolved handler plugins"
        );
        let pipeline = Arc::new(Pipeline::new(handlers));

        // --- Host matcher: which target hosts run the pipeline at all ---
        let mut patterns = Vec::new();
        patterns.extend(cfg.intercept_hosts.iter().cloned().map(HostPattern::Exact));
        patterns.extend(
            cfg.intercept_host_suffixes
                .iter()
                .cloned()
                .map(HostPattern::Suffix),
        );
        let host_matcher = Arc::new(HostMatcher::new(patterns));

        let handler = ForwardProxyHandler::new(pipeline, host_matcher);

        let addr: std::net::SocketAddr = cfg.listen_addr.parse().map_err(|e| {
            anyhow::anyhow!(
                "forward-proxy: invalid listen_addr '{}': {e}",
                cfg.listen_addr
            )
        })?;

        // hyper's server-side defaults (16KiB max H2 header-list size, 100
        // max header lines) are sized for typical API traffic, not a real
        // logged-in browser session — ChatGPT alone routinely carries
        // several KiB of cookies (Cloudflare's `__cf_bm`/`cf_clearance`,
        // OpenAI's own session JWT) plus a full set of `sec-ch-ua-*` client
        // hints, and blows past 16KiB easily. Once forward-proxy actually
        // decrypts this traffic (see `should_intercept_connect`/`_tls`
        // above), hyper's own request parser enforces that default *before*
        // the request ever reaches `ForwardProxyHandler` — the client sees
        // a bare 431 with no trace of it in our own logs. Raise both to
        // something a real browser session fits in.
        let mut server_builder = hudsucker::hyper_util::server::conn::auto::Builder::new(
            hudsucker::hyper_util::rt::TokioExecutor::new(),
        );
        server_builder.http1().max_headers(200);
        server_builder.http2().max_header_list_size(64 * 1024);

        let proxy = Proxy::builder()
            .with_addr(addr)
            .with_ca(ca)
            .with_rustls_connector(aws_lc_rs::default_provider())
            .with_http_handler(handler)
            .with_server(server_builder)
            .with_graceful_shutdown(cancel.cancelled_owned())
            .build()
            .map_err(|e| anyhow::anyhow!("forward-proxy: failed to build proxy: {e}"))?;

        info!(listen_addr = %addr, "forward-proxy: listening");
        ready.notify();

        proxy
            .start()
            .await
            .map_err(|e| anyhow::anyhow!("forward-proxy: proxy stopped with error: {e}"))
    }
}
