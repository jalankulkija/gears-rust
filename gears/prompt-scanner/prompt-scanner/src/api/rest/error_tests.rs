use super::*;
use prompt_scanner_sdk::PromptScannerError;
use uuid::Uuid;

#[test]
fn policy_unavailable_maps_to_503_without_the_reason() {
    let tenant_id = Uuid::now_v7();
    // As a policy provider returns it, through the domain.
    let err = DomainError::from(PromptScannerError::policy_unavailable(
        tenant_id,
        "GET https://grpm.internal:8443/applications: connection refused",
    ));
    assert!(matches!(err, DomainError::PolicyUnavailable { tenant_id: t, .. } if t == tenant_id));

    let ce = CanonicalError::from(err);
    assert_eq!(ce.status_code(), 503);
    assert!(ce.detail().contains(&tenant_id.to_string()));
    assert!(!ce.detail().contains("grpm.internal"));
}
