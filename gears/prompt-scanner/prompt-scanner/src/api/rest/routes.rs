//! REST route registration for the prompt-scanner gear.
//!
//! Endpoints (`/prompt-scanner/v1/...`), per `docs/DESIGN.md`:
//!
//! | Method | Path              | Purpose                                  |
//! |--------|-------------------|-------------------------------------------|
//! | POST   | `/scans`          | Sync scan -> verdict                      |
//! | POST   | `/scans/async`    | Queue large upload scan -> job id         |
//! | GET    | `/scans/{trace_id}` | Fetch a historical verdict              |
//! | GET    | `/scans`          | `OData`-filterable audit search           |
//! | GET    | `/policy`         | Get tenant's backend policy                |
//! | PUT    | `/policy`         | Update tenant's backend policy             |
//! | GET    | `/backends`       | List registered scanner backend instances |

use std::sync::Arc;

use axum::Router;
use http::StatusCode;
use toolkit::api::OpenApiRegistry;
use toolkit::api::operation_builder::{
    CORE_GLOBAL_BASE_LICENSE_FEATURE, LicenseFeature, OperationBuilder, OperationBuilderODataExt,
};

use prompt_scanner_sdk::odata::ScanRecordFilterField;

use crate::api::rest::{dto, handlers};
use crate::gear::ConcreteService;

const API_TAG: &str = "Scan Gateway";

pub(super) struct License;

impl AsRef<str> for License {
    fn as_ref(&self) -> &'static str {
        CORE_GLOBAL_BASE_LICENSE_FEATURE
    }
}

impl LicenseFeature for License {}

/// Register all routes for the `prompt_scanner` gear.
pub(crate) fn register_routes(
    mut router: Router,
    openapi: &dyn OpenApiRegistry,
    service: Arc<ConcreteService>,
) -> Router {
    router = OperationBuilder::post("/prompt-scanner/v1/scans")
        .operation_id("prompt_scanner.create_scan")
        .summary("Synchronously scan a piece of content")
        .description(
            "Fans out to every enabled scanner backend for the tenant, aggregates findings \
             into a single verdict, and persists the scan record.",
        )
        .tag(API_TAG)
        .authenticated()
        .require_license_features::<License>([])
        .json_request::<dto::ScanRequestDto>(openapi, "Content to scan")
        .handler(handlers::create_scan)
        .json_response_with_schema::<dto::ScanVerdictDto>(
            openapi,
            StatusCode::CREATED,
            "Scan verdict",
        )
        .error_400(openapi)
        .error_401(openapi)
        .error_403(openapi)
        .error_500(openapi)
        .register(router, openapi);

    router = OperationBuilder::post("/prompt-scanner/v1/scans/async")
        .operation_id("prompt_scanner.create_scan_async")
        .summary("Queue a scan for asynchronous processing")
        .description("For large uploads. Returns a job handle; poll GET /scans/{trace_id}.")
        .tag(API_TAG)
        .authenticated()
        .require_license_features::<License>([])
        .json_request::<dto::ScanRequestDto>(openapi, "Content to scan")
        .handler(handlers::create_scan_async)
        .json_response_with_schema::<dto::ScanJobHandleDto>(
            openapi,
            StatusCode::ACCEPTED,
            "Queued scan job handle",
        )
        .error_400(openapi)
        .error_401(openapi)
        .error_500(openapi)
        .register(router, openapi);

    router = OperationBuilder::get("/prompt-scanner/v1/scans/{trace_id}")
        .operation_id("prompt_scanner.get_scan")
        .summary("Fetch a historical verdict by trace id")
        .tag(API_TAG)
        .authenticated()
        .require_license_features::<License>([])
        .path_param("trace_id", "Scan trace id")
        .handler(handlers::get_scan)
        .json_response_with_schema::<dto::ScanVerdictDto>(openapi, StatusCode::OK, "Scan verdict")
        .error_401(openapi)
        .error_403(openapi)
        .error_404(openapi)
        .error_500(openapi)
        .register(router, openapi);

    router = OperationBuilder::get("/prompt-scanner/v1/scans")
        .operation_id("prompt_scanner.list_scans")
        .summary("OData-filterable audit search over recorded verdicts")
        .tag(API_TAG)
        .authenticated()
        .require_license_features::<License>([])
        .handler(handlers::list_scans)
        .json_response_with_schema::<toolkit_odata::Page<dto::ScanVerdictDto>>(
            openapi,
            StatusCode::OK,
            "Paginated list of scan verdicts",
        )
        .with_odata_filter::<ScanRecordFilterField>()
        .with_odata_select()
        .with_odata_orderby::<ScanRecordFilterField>()
        .error_400(openapi)
        .error_401(openapi)
        .error_500(openapi)
        .register(router, openapi);

    router = OperationBuilder::get("/prompt-scanner/v1/policy")
        .operation_id("prompt_scanner.get_policy")
        .summary("Get the calling tenant's backend policy")
        .tag(API_TAG)
        .authenticated()
        .require_license_features::<License>([])
        .handler(handlers::get_policy)
        .json_response_with_schema::<dto::TenantPolicyDto>(
            openapi,
            StatusCode::OK,
            "Tenant backend policy",
        )
        .error_401(openapi)
        .error_403(openapi)
        .error_500(openapi)
        .register(router, openapi);

    router = OperationBuilder::put("/prompt-scanner/v1/policy")
        .operation_id("prompt_scanner.put_policy")
        .summary("Update the calling tenant's backend policy")
        .tag(API_TAG)
        .authenticated()
        .require_license_features::<License>([])
        .json_request::<dto::TenantPolicyDto>(openapi, "New tenant backend policy")
        .handler(handlers::put_policy)
        .json_response_with_schema::<dto::TenantPolicyDto>(
            openapi,
            StatusCode::OK,
            "Updated tenant backend policy",
        )
        .error_400(openapi)
        .error_401(openapi)
        .error_403(openapi)
        .error_500(openapi)
        .register(router, openapi);

    router = OperationBuilder::get("/prompt-scanner/v1/backends")
        .operation_id("prompt_scanner.list_backends")
        .summary("List registered scanner backend instances and their availability")
        .tag(API_TAG)
        .authenticated()
        .require_license_features::<License>([])
        .handler(handlers::list_backends)
        .json_array_response_with_schema::<dto::BackendInstanceDto>(
            openapi,
            StatusCode::OK,
            "Registered scanner backend instances",
        )
        .error_401(openapi)
        .error_500(openapi)
        .register(router, openapi);

    router.layer(axum::Extension(service))
}
