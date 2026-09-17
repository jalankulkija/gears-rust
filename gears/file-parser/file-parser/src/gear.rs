use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use toolkit::api::OpenApiRegistry;
use toolkit::{Gear, GearCtx, RestApiCapability};
use tracing::{debug, info};

use file_parser_sdk::FileParserClientV1;

use crate::config::FileParserConfig;
use crate::domain::local_client::FileParserLocalClient;
use crate::domain::service::{FileParserService, ServiceConfig};
use crate::infra::parsers::{
    DocxParser, ImageParser, KreuzbergParser, PlainTextParser, StubParser,
};

/// Main gear struct for file parsing
#[toolkit::gear(
    name = "file-parser",
    capabilities = [rest]
)]
pub struct FileParserGear {
    service: OnceLock<Arc<FileParserService>>,
}

impl Default for FileParserGear {
    fn default() -> Self {
        Self {
            service: OnceLock::new(),
        }
    }
}

/// Why the Magika content-type detector failed to initialize.
///
/// The mechanism, and the two variants that oblige the caller to terminate the
/// process, are documented once in [`ort_bootstrap`]. `onnx-runner` has the
/// same problem for the same reason and uses the same helper.
#[cfg(feature = "magika")]
pub type MagikaInitError = ort_bootstrap::BootstrapError<magika::Error>;

/// Loads the Magika content-type detector eagerly at gear startup, so a
/// missing model/runtime fails gear startup rather than the first request.
///
/// Bounded by a timeout because `ort` 2.0.0-rc.12 **hangs forever instead of
/// erroring** when the ONNX Runtime library it `dlopen`s via `ORT_DYLIB_PATH`
/// cannot be loaded. Reproduced: against a nonexistent path,
/// `magika::Session::new()` returns neither `Ok` nor `Err` nor panics in 45 s.
/// [`ort_bootstrap`] holds the mechanism, the two measured facts that constrain
/// any simplification of it, and why the wedged thread is abandoned rather than
/// joined. Provision a compatible runtime with `scripts/fetch-onnxruntime.sh`.
///
/// # Visibility
///
/// `#[doc(hidden)] pub` only so `tests/magika_timeout_tests.rs` can drive this
/// exact path; an integration test cannot reach `pub(crate)`. Not public API.
///
/// # Errors
///
/// See [`MagikaInitError`] — two variants oblige the caller to terminate.
#[cfg(feature = "magika")]
#[doc(hidden)]
pub async fn init_magika_detector(
    parsers: &[Arc<dyn crate::domain::parser::FileParserBackend>],
    intra_op_threads: Option<std::num::NonZeroUsize>,
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<Arc<dyn crate::domain::detector::ContentTypeDetector>, MagikaInitError> {
    // Flat, not scaled — one session means one model load. A healthy one builds
    // in well under a second and the hang never completes, so a longer timeout
    // would only delay a legible startup failure.
    const MAGIKA_INIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    let supported_extensions: Vec<String> = parsers
        .iter()
        .flat_map(|p| p.supported_extensions().iter().map(|ext| (*ext).to_owned()))
        .collect();

    let detector =
        ort_bootstrap::Bootstrap::new("Magika content-type detector", MAGIKA_INIT_TIMEOUT)
            .run(
                move || {
                    crate::infra::MagikaDetector::with_config(
                        supported_extensions,
                        intra_op_threads,
                    )
                },
                cancellation_token,
            )
            .await?;

    info!("Magika content-type detection enabled");
    Ok(Arc::new(detector) as Arc<dyn crate::domain::detector::ContentTypeDetector>)
}

#[async_trait]
impl Gear for FileParserGear {
    #[allow(clippy::cast_possible_truncation)]
    async fn init(&self, ctx: &GearCtx) -> anyhow::Result<()> {
        const BYTES_IN_MB: u64 = 1024_u64 * 1024;

        // Load gear configuration
        let cfg: FileParserConfig = ctx.config()?;
        debug!(
            "Loaded file-parser config: max_file_size_mb={}",
            cfg.max_file_size_mb
        );

        let max_file_size_bytes = cfg.max_file_size_mb.saturating_mul(BYTES_IN_MB);

        // The two backends that read a whole local file into memory get the
        // configured ceiling, so their bounded reads agree with the
        // service-level check rather than a compiled-in default.
        let parsers: Vec<Arc<dyn crate::domain::parser::FileParserBackend>> = vec![
            Arc::new(PlainTextParser::new().with_max_bytes(max_file_size_bytes)),
            Arc::new(KreuzbergParser::new()),
            Arc::new(DocxParser::new()),
            Arc::new(ImageParser::new()),
            Arc::new(StubParser::new().with_max_bytes(max_file_size_bytes)),
        ];

        info!("Registered {} parser backends", parsers.len());

        // Canonicalize at startup so we only do it once.
        let allowed_local_base_dir = cfg.allowed_local_base_dir.canonicalize().map_err(|e| {
            anyhow::anyhow!(
                "allowed_local_base_dir '{}' cannot be resolved: {e}",
                cfg.allowed_local_base_dir.display()
            )
        })?;
        if !allowed_local_base_dir.is_dir() {
            return Err(anyhow::anyhow!(
                "allowed_local_base_dir '{}' is not a directory",
                allowed_local_base_dir.display()
            ));
        }
        info!(
            allowed_local_base_dir = %allowed_local_base_dir.display(),
            "Local file parsing restricted to base directory"
        );

        // Create service config from gear config
        let service_config = ServiceConfig {
            max_file_size_bytes: usize::try_from(max_file_size_bytes).unwrap_or(usize::MAX),
            allowed_local_base_dir,
        };

        // With the `magika` feature, load the content-type detector eagerly
        // so a missing model/runtime fails gear startup, not the first
        // request. See `init_magika_detector` for why this is bounded by a
        // timeout instead of a plain `.await`.
        // `Err` from `Gear::init` terminates the process, which is what
        // `leaked_init_thread` requires. Making this recoverable means dealing
        // with the leaked thread.
        #[cfg(feature = "magika")]
        let detector = match init_magika_detector(
            &parsers,
            cfg.magika_intra_op_threads,
            ctx.cancellation_token(),
        )
        .await
        {
            Ok(detector) => detector,
            Err(e) => {
                if e.leaked_init_thread() {
                    tracing::error!(
                        error = %e,
                        "Magika init left an abandoned OS thread; this is only bounded because \
                         gear init failure terminates the process"
                    );
                }
                return Err(e.into());
            }
        };

        // Fail startup on a bad threshold rather than clamping a value the
        // operator plainly did not mean (e.g. a percentage typed as `90`).
        let detection_confidence_threshold =
            crate::domain::detector::Confidence::new(cfg.detection_confidence_threshold)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "detection_confidence_threshold ({}) must be a number in [0.0, 1.0]",
                        cfg.detection_confidence_threshold
                    )
                })?;

        // Create file parser service
        #[allow(unused_mut)]
        let mut service = FileParserService::new(parsers, service_config)
            .with_detection_confidence_threshold(detection_confidence_threshold);
        #[cfg(feature = "magika")]
        {
            service = service.with_detector(detector);
        }
        let file_parser_service = Arc::new(service);

        // Register the in-process ClientHub client so other gears can call
        // file-parser without going over HTTP.
        let client: Arc<dyn FileParserClientV1> =
            Arc::new(FileParserLocalClient::new(file_parser_service.clone()));
        ctx.client_hub().register::<dyn FileParserClientV1>(client);

        // Store service for REST usage
        self.service
            .set(file_parser_service)
            .map_err(|_| anyhow::anyhow!("{} gear already initialized", Self::MODULE_NAME))?;

        Ok(())
    }
}

impl RestApiCapability for FileParserGear {
    fn register_rest(
        &self,
        _ctx: &GearCtx,
        router: axum::Router,
        openapi: &dyn OpenApiRegistry,
    ) -> anyhow::Result<axum::Router> {
        info!("Registering file-parser REST routes");

        let service = self
            .service
            .get()
            .ok_or_else(|| anyhow::anyhow!("Service not initialized"))?
            .clone();

        let router = crate::api::rest::routes::register_routes(router, openapi, service);

        info!("File parser REST routes registered successfully");
        Ok(router)
    }
}
