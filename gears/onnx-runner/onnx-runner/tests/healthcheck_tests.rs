#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Operator-visible readiness, end to end over real model state.
//!
//! Two things are under test, and the second matters as much as the first:
//! that the check reports what actually happened to each model, and that
//! implementing `RestApiCapability` to get the healthcheck hook has **not**
//! quietly given this gear an HTTP surface
//! (`cpt-cf-onnx-runner-constraint-no-http-surface`).

mod common;

use std::sync::Arc;

use axum::Router;
use common::{fixture_config, fixture_with, no_metrics};
use onnx_runner::OnnxRunner;
use onnx_runner::domain::OnnxRunnerMetricsPort;
use onnx_runner::gear::{OnnxRunnerLocalClient, load_models};
use onnx_runner::health::ModelReadiness;
use tokio_util::sync::CancellationToken;
use toolkit::contracts::{OpenApiRegistry, RestApiCapability};
use toolkit::{ClientHub, GearCtx, Healthcheck, HealthcheckStatus};

/// Loads the given configs the way the gear does, then wraps them in the real
/// check. Nothing here reimplements the projection under test.
async fn readiness_over(configs: Vec<onnx_runner::config::ModelConfig>) -> ModelReadiness {
    let models = load_models(configs, false, no_metrics(), &CancellationToken::new())
        .await
        .expect("permissive load never fails");
    ModelReadiness::new(Arc::new(OnnxRunnerLocalClient::new(models)))
}

#[tokio::test]
async fn a_loaded_model_reports_healthy() {
    let check = readiness_over(vec![fixture_config()]).await;
    let result = check.check().await;

    assert_eq!(result.status, HealthcheckStatus::Healthy);
    assert_eq!(
        check.name(),
        "onnx-runner-models",
        "name is on /health verbatim"
    );
}

#[tokio::test]
async fn a_gear_serving_zero_models_reports_unhealthy() {
    // The defect this feature exists to close: before the check existed, this
    // deployment was indistinguishable from a working one.
    let check = readiness_over(vec![]).await;
    let result = check.check().await;

    assert_eq!(result.status, HealthcheckStatus::Unhealthy);
    assert_eq!(result.code.as_deref(), Some("no_models_configured"));
}

#[tokio::test]
async fn a_model_that_failed_its_checksum_is_named_and_unhealthy() {
    // Only model configured, and it cannot load — nothing left to serve.
    let bad = fixture_with(|c| {
        c.model_id = "tampered".to_owned();
        c.sha256 = "f".repeat(64);
    });
    let result = readiness_over(vec![bad]).await.check().await;

    assert_eq!(result.status, HealthcheckStatus::Unhealthy);
    assert_eq!(result.code.as_deref(), Some("no_models_ready"));

    let message = result.message.expect("unhealthy explains itself");
    assert!(message.contains("tampered"), "names the model: {message}");
    assert!(
        message.contains("checksum_mismatch"),
        "names the stable reason code: {message}"
    );
}

#[tokio::test]
async fn one_bad_model_among_good_ones_degrades_rather_than_fails() {
    // Degraded keeps the pod in rotation, which is correct: `tiny` still
    // serves, and a consumer asking for `tampered` gets a typed
    // `ModelUnavailable` it is obliged to map to failed coverage.
    let good = fixture_config();
    let bad = fixture_with(|c| {
        c.model_id = "tampered".to_owned();
        c.sha256 = "f".repeat(64);
    });
    let result = readiness_over(vec![good, bad]).await.check().await;

    assert_eq!(result.status, HealthcheckStatus::Degraded);
    assert_eq!(result.code.as_deref(), Some("models_unavailable"));

    let message = result.message.expect("degraded explains itself");
    assert!(message.contains("tampered"), "{message}");
    assert!(
        !message.contains("tiny"),
        "working models are not listed: {message}"
    );
}

#[tokio::test]
async fn the_check_runs_no_inference() {
    // Requirement: readiness reads recorded state. A probe that ran a real
    // inference every few seconds would occupy a replica and could cause the
    // overload it is meant to detect. Asserted through the metrics port,
    // which records every inference the gear performs.
    let recorder = Arc::new(common::RecordingMetrics::default());
    let models = load_models(
        vec![fixture_config()],
        false,
        Arc::clone(&recorder) as Arc<dyn OnnxRunnerMetricsPort>,
        &CancellationToken::new(),
    )
    .await
    .expect("loads");

    let check = ModelReadiness::new(Arc::new(OnnxRunnerLocalClient::new(models)));
    for _ in 0..5 {
        assert_eq!(check.check().await.status, HealthcheckStatus::Healthy);
    }

    assert!(
        recorder.inferences().is_empty(),
        "readiness must not run inferences"
    );
}

// ── The no-HTTP-surface guard ────────────────────────────────────────

struct NoopOpenApiRegistry;

impl OpenApiRegistry for NoopOpenApiRegistry {
    fn register_operation(&self, _spec: &toolkit::api::operation_builder::OperationSpec) {}

    fn ensure_schema_raw(
        &self,
        name: &str,
        _schemas: Vec<(String, utoipa::openapi::RefOr<utoipa::openapi::Schema>)>,
    ) -> String {
        name.to_owned()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct EmptyConfigProvider;

impl toolkit::config::ConfigProvider for EmptyConfigProvider {
    fn get_gear_config(&self, _gear_name: &str) -> Option<&serde_json::Value> {
        None
    }
}

fn test_ctx() -> GearCtx {
    GearCtx::new(
        "onnx-runner",
        uuid::Uuid::new_v4(),
        Arc::new(EmptyConfigProvider),
        Arc::new(ClientHub::default()),
        CancellationToken::new(),
    )
}

#[tokio::test]
async fn registers_no_rest_routes() {
    // `capabilities = [rest]` is declared for the healthcheck hook alone. If
    // someone later adds a route here, this fails — which is the point:
    // the no-HTTP-surface constraint is enforced, not merely documented.
    use tower::ServiceExt as _;

    let gear = OnnxRunner::default();
    let router: Router = gear
        .register_rest(&test_ctx(), Router::new(), &NoopOpenApiRegistry)
        .expect("wiring is a no-op and cannot fail");

    for path in ["/", "/models", "/onnx-runner/v1/models", "/health"] {
        let response = router
            .clone()
            .oneshot(
                http::Request::builder()
                    .uri(path)
                    .body(axum::body::Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router responds");

        assert_eq!(
            response.status(),
            http::StatusCode::NOT_FOUND,
            "{path} must not be served by this gear"
        );
    }
}

#[tokio::test]
async fn no_healthcheck_is_offered_before_init() {
    // Honest rather than optimistic: with no models loaded yet there is no
    // state to report, and the platform reads an absent check as an opt-out
    // rather than as health.
    let gear = OnnxRunner::default();
    assert!(gear.healthcheck(&test_ctx()).is_none());
}
