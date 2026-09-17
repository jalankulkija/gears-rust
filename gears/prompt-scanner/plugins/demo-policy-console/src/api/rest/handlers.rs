//! Axum handlers for `demo-policy-console`'s REST API.

use std::sync::Arc;

use axum::Extension;

use toolkit::api::canonical_prelude::*;
use toolkit_odata::ODataQuery;
use toolkit_security::SecurityContext;

use crate::api::rest::dto::{AuditEntryDto, DemoStateDto};
use crate::domain::Service;

/// Max audit rows returned in one page — this is a demo panel, not a
/// paginated admin table.
const AUDIT_PAGE_LIMIT: u64 = 50;

/// `GET /demo-policy-console/v1/state`.
///
/// # Errors
///
/// Never fails — infallible, `ApiResult` only for handler-signature
/// consistency with the other routes.
pub async fn get_state(
    Extension(svc): Extension<Arc<Service>>,
) -> ApiResult<JsonBody<DemoStateDto>> {
    Ok(Json(svc.state().await.into()))
}

/// `PUT /demo-policy-console/v1/state`.
///
/// # Errors
///
/// Never fails — infallible, `ApiResult` only for handler-signature
/// consistency with the other routes.
pub async fn put_state(
    Extension(svc): Extension<Arc<Service>>,
    Json(body): Json<DemoStateDto>,
) -> ApiResult<JsonBody<DemoStateDto>> {
    let updated = svc
        .set_state(body.pii_enabled, body.pii_action.into())
        .await;
    Ok(Json(updated.into()))
}

/// `GET /demo-policy-console/v1/audit` — recent scan verdicts, for the UI's
/// audit log panel.
///
/// # Errors
///
/// Returns an internal-error [`CanonicalError`] if `prompt-scanner` isn't
/// registered in `ClientHub` or the underlying `list_scans` call fails.
pub async fn get_audit(
    Extension(ctx): Extension<SecurityContext>,
    Extension(svc): Extension<Arc<Service>>,
) -> ApiResult<JsonBody<Vec<AuditEntryDto>>> {
    let query = ODataQuery::new().with_limit(AUDIT_PAGE_LIMIT);
    let page = svc
        .list_scans(&ctx, &query)
        .await
        .map_err(|e| CanonicalError::internal(e.to_string()).create())?;
    let entries = page.items.into_iter().map(AuditEntryDto::from).collect();
    Ok(Json(entries))
}
