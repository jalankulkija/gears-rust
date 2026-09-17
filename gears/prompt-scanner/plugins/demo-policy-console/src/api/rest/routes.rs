//! REST route registration for `demo-policy-console`.
//!
//! Endpoints (`/demo-policy-console/v1/...`):
//!
//! | Method | Path      | Purpose                                          |
//! |--------|-----------|---------------------------------------------------|
//! | GET    | `/state`  | Current `{ pii_enabled, pii_action }`             |
//! | PUT    | `/state`  | Update the toggle/response-action                |
//! | GET    | `/audit`  | Recent scan verdicts (passthrough to prompt-scanner) |
//!
//! Plus a plain `GET /demo-policy-console/` serving the control-panel HTML
//! page (see `api::web`) — not part of the `OpenAPI` surface, same pattern
//! `api-gateway`'s `/docs` page uses.

use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use http::StatusCode;
use toolkit::api::OpenApiRegistry;
use toolkit::api::operation_builder::{
    CORE_GLOBAL_BASE_LICENSE_FEATURE, LicenseFeature, OperationBuilder,
};

use crate::api::rest::{dto, handlers};
use crate::api::web;
use crate::domain::Service;

const API_TAG: &str = "Demo Policy Console";

struct License;

impl AsRef<str> for License {
    fn as_ref(&self) -> &'static str {
        CORE_GLOBAL_BASE_LICENSE_FEATURE
    }
}

impl LicenseFeature for License {}

/// Register all routes for the `demo-policy-console` gear.
pub(crate) fn register_routes(
    mut router: Router,
    openapi: &dyn OpenApiRegistry,
    service: Arc<Service>,
) -> Router {
    router = OperationBuilder::get("/demo-policy-console/v1/state")
        .operation_id("demo_policy_console.get_state")
        .summary("Get the demo's current PII-detection toggle state")
        .tag(API_TAG)
        .authenticated()
        .require_license_features::<License>([])
        .handler(handlers::get_state)
        .json_response_with_schema::<dto::DemoStateDto>(openapi, StatusCode::OK, "Current state")
        .error_401(openapi)
        .error_500(openapi)
        .register(router, openapi);

    router = OperationBuilder::put("/demo-policy-console/v1/state")
        .operation_id("demo_policy_console.put_state")
        .summary("Update the demo's PII-detection toggle state")
        .tag(API_TAG)
        .authenticated()
        .require_license_features::<License>([])
        .json_request::<dto::DemoStateDto>(openapi, "New state")
        .handler(handlers::put_state)
        .json_response_with_schema::<dto::DemoStateDto>(openapi, StatusCode::OK, "Updated state")
        .error_400(openapi)
        .error_401(openapi)
        .error_500(openapi)
        .register(router, openapi);

    router = OperationBuilder::get("/demo-policy-console/v1/audit")
        .operation_id("demo_policy_console.get_audit")
        .summary("Recent scan verdicts, for the audit log panel")
        .tag(API_TAG)
        .authenticated()
        .require_license_features::<License>([])
        .handler(handlers::get_audit)
        .json_array_response_with_schema::<dto::AuditEntryDto>(
            openapi,
            StatusCode::OK,
            "Recent scan verdicts",
        )
        .error_401(openapi)
        .error_500(openapi)
        .register(router, openapi);

    router = router.route("/demo-policy-console/", get(web::index));

    router.layer(axum::Extension(service))
}
