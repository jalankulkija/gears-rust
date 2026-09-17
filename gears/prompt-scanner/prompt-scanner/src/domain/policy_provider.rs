//! Built-in [`PolicyProvider`] — the default `Service` falls back to when no
//! `PolicyProviderSpecV1` plugin instance is registered.
//!
//! prompt-scanner stores no tenant policies: they belong to whatever policy
//! service a provider plugin adapts. Without one, every tenant gets the same
//! policy from the gear's config (`fallback_policy`), or, when that is unset,
//! `None` and so the permissive `TenantPolicy::default_for_tenant`. Writes are
//! refused. See `docs/POLICY_PROVIDER_AND_DEMO_CONSOLE.md`.

use async_trait::async_trait;
use prompt_scanner_sdk::{PolicyProvider, PromptScannerError, TenantPolicy};
use toolkit_macros::domain_model;
use toolkit_security::SecurityContext;
use uuid::Uuid;

/// Default `PolicyProvider`: one read-only policy for every tenant.
#[domain_model]
pub struct ConfigPolicyProvider {
    /// The policy served to every tenant; its `tenant_id` is replaced by the
    /// one asked for.
    policy: Option<TenantPolicy>,
}

impl ConfigPolicyProvider {
    #[must_use]
    pub fn new(policy: Option<TenantPolicy>) -> Self {
        Self { policy }
    }
}

#[async_trait]
impl PolicyProvider for ConfigPolicyProvider {
    async fn get_policy(
        &self,
        _ctx: &SecurityContext,
        tenant_id: Uuid,
    ) -> Result<Option<TenantPolicy>, PromptScannerError> {
        Ok(self.policy.clone().map(|p| TenantPolicy { tenant_id, ..p }))
    }

    async fn put_policy(
        &self,
        _ctx: &SecurityContext,
        _policy: TenantPolicy,
    ) -> Result<TenantPolicy, PromptScannerError> {
        Err(PromptScannerError::validation(
            "tenant policies are read-only: no policy provider plugin is registered, \
             and the fallback comes from prompt-scanner's `fallback_policy` config",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_the_configured_policy_to_every_tenant_and_refuses_writes() {
        let ctx = SecurityContext::anonymous();
        let tenant = Uuid::now_v7();

        let none = ConfigPolicyProvider::new(None);
        assert_eq!(none.get_policy(&ctx, tenant).await.unwrap(), None);

        let mut template = TenantPolicy::default_for_tenant(Uuid::nil());
        template.enabled_backends = vec!["cf.builtin.regex_scanner".to_owned()];
        let some = ConfigPolicyProvider::new(Some(template.clone()));
        let got = some.get_policy(&ctx, tenant).await.unwrap().unwrap();
        assert_eq!(got.tenant_id, tenant);
        assert_eq!(got.enabled_backends, template.enabled_backends);

        assert!(matches!(
            some.put_policy(&ctx, got).await,
            Err(PromptScannerError::Validation { .. })
        ));
    }
}
