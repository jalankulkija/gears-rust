//! Axum handlers for the prompt-scanner REST API.

use std::sync::Arc;

use axum::Extension;
use axum::extract::Path;
use axum::http::Uri;
use axum::response::IntoResponse;
use uuid::Uuid;

use toolkit::api::canonical_prelude::*;
use toolkit_security::SecurityContext;

use crate::api::rest::dto::{
    BackendInstanceDto, ScanJobHandleDto, ScanRequestDto, ScanVerdictDto, TenantPolicyDto,
};
use crate::gear::ConcreteService;

/// `POST /prompt-scanner/v1/scans` — synchronous scan.
pub async fn create_scan(
    uri: Uri,
    Extension(ctx): Extension<SecurityContext>,
    Extension(svc): Extension<Arc<ConcreteService>>,
    Json(req_body): Json<ScanRequestDto>,
) -> ApiResult<impl IntoResponse> {
    let verdict = svc.scan(&ctx, req_body.into()).await?;
    let trace_id = verdict.trace_id.to_string();
    Ok(created_json(ScanVerdictDto::from(verdict), &uri, &trace_id).into_response())
}

/// `POST /prompt-scanner/v1/scans/async` — queue a scan for async processing.
pub async fn create_scan_async(
    Extension(ctx): Extension<SecurityContext>,
    Extension(svc): Extension<Arc<ConcreteService>>,
    Json(req_body): Json<ScanRequestDto>,
) -> ApiResult<JsonBody<ScanJobHandleDto>> {
    let handle = svc.scan_async(&ctx, req_body.into()).await?;
    Ok(Json(handle.into()))
}

/// `GET /prompt-scanner/v1/scans/{trace_id}` — fetch a historical verdict.
pub async fn get_scan(
    Extension(ctx): Extension<SecurityContext>,
    Extension(svc): Extension<Arc<ConcreteService>>,
    Path(trace_id): Path<Uuid>,
) -> ApiResult<JsonBody<ScanVerdictDto>> {
    let verdict = svc.get_verdict(&ctx, trace_id).await?;
    Ok(Json(verdict.into()))
}

/// `GET /prompt-scanner/v1/scans` — `OData`-filterable audit search.
pub async fn list_scans(
    Extension(ctx): Extension<SecurityContext>,
    Extension(svc): Extension<Arc<ConcreteService>>,
    OData(query): OData,
) -> ApiResult<JsonPage<ScanVerdictDto>> {
    let page = svc.list_scans(&ctx, &query).await?;
    Ok(Json(page.map_items(ScanVerdictDto::from)))
}

/// `GET /prompt-scanner/v1/policy` — get the calling tenant's backend policy.
pub async fn get_policy(
    Extension(ctx): Extension<SecurityContext>,
    Extension(svc): Extension<Arc<ConcreteService>>,
) -> ApiResult<JsonBody<TenantPolicyDto>> {
    let policy = svc.get_policy(&ctx).await?;
    Ok(Json(policy.into()))
}

/// `PUT /prompt-scanner/v1/policy` — update the calling tenant's backend policy.
pub async fn put_policy(
    Extension(ctx): Extension<SecurityContext>,
    Extension(svc): Extension<Arc<ConcreteService>>,
    Json(req_body): Json<TenantPolicyDto>,
) -> ApiResult<JsonBody<TenantPolicyDto>> {
    let policy = svc.put_policy(&ctx, req_body.into()).await?;
    Ok(Json(policy.into()))
}

/// `GET /prompt-scanner/v1/backends` — list registered scanner backend
/// plugin instances and their availability.
pub async fn list_backends(
    Extension(svc): Extension<Arc<ConcreteService>>,
) -> ApiResult<JsonBody<Vec<BackendInstanceDto>>> {
    let instances = svc.list_backend_instances().await?;
    let dtos = instances
        .into_iter()
        .map(|i| BackendInstanceDto {
            gts_id: i.gts_id,
            vendor: i.vendor,
            priority: i.priority,
            available: i.available,
        })
        .collect();
    Ok(Json(dtos))
}
