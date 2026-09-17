use std::collections::HashMap;

use prompt_scanner_sdk::{ContentKind, TenantPolicy, VerdictAction};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Configuration for the `prompt_scanner` gear.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptScannerConfig {
    #[serde(default = "default_page_size")]
    pub default_page_size: u32,
    #[serde(default = "default_max_page_size")]
    pub max_page_size: u32,
    /// Per-backend call timeout, in milliseconds. A backend that doesn't
    /// respond within this window degrades to "unscored" (see
    /// `domain::service::Service::run_backends`) unless the tenant policy
    /// marks it `required`.
    #[serde(default = "default_backend_timeout_ms")]
    pub backend_timeout_ms: u64,
    /// Interval, in milliseconds, between drains of the in-memory
    /// `scan_async` job queue by the gear's stateful background task.
    #[serde(default = "default_job_poll_interval_ms")]
    pub job_poll_interval_ms: u64,
    /// The policy every tenant gets when no `PolicyProvider` plugin is
    /// registered. Unset: the permissive `TenantPolicy::default_for_tenant`.
    #[serde(default)]
    pub fallback_policy: Option<FallbackPolicy>,
}

/// A `TenantPolicy` without its tenant: the same for every tenant. Field
/// meanings and defaults are `TenantPolicy`'s.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FallbackPolicy {
    #[serde(default)]
    pub enabled_backends: Vec<String>,
    #[serde(default)]
    pub required_backends: Vec<String>,
    #[serde(default)]
    pub thresholds: HashMap<String, f32>,
    #[serde(default)]
    pub default_action: VerdictAction,
    #[serde(default)]
    pub category_action_overrides: HashMap<String, VerdictAction>,
    #[serde(default = "ContentKind::all")]
    pub content_kinds: Vec<ContentKind>,
}

impl FallbackPolicy {
    /// As a `TenantPolicy` for the nil tenant; the provider substitutes the
    /// tenant asked for.
    #[must_use]
    pub fn into_template(self) -> TenantPolicy {
        TenantPolicy {
            tenant_id: Uuid::nil(),
            enabled_backends: self.enabled_backends,
            required_backends: self.required_backends,
            thresholds: self.thresholds,
            default_action: self.default_action,
            category_action_overrides: self.category_action_overrides,
            content_kinds: self.content_kinds,
        }
    }
}

impl Default for PromptScannerConfig {
    fn default() -> Self {
        Self {
            default_page_size: default_page_size(),
            max_page_size: default_max_page_size(),
            backend_timeout_ms: default_backend_timeout_ms(),
            job_poll_interval_ms: default_job_poll_interval_ms(),
            fallback_policy: None,
        }
    }
}

fn default_page_size() -> u32 {
    50
}

fn default_max_page_size() -> u32 {
    1000
}

fn default_backend_timeout_ms() -> u64 {
    5_000
}

fn default_job_poll_interval_ms() -> u64 {
    200
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_policy_fields_default_like_tenant_policy() {
        let cfg: PromptScannerConfig = serde_json::from_value(serde_json::json!({
            "fallback_policy": {"enabled_backends": ["cf.builtin.regex_scanner"]},
        }))
        .unwrap();
        let policy = cfg.fallback_policy.unwrap().into_template();
        assert_eq!(policy.enabled_backends, ["cf.builtin.regex_scanner"]);
        assert_eq!(policy.default_action, VerdictAction::Allow);
        assert_eq!(policy.content_kinds, ContentKind::ALL);

        let cfg: PromptScannerConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(cfg.fallback_policy.is_none());
    }
}
