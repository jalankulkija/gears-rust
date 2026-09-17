use serde::{Deserialize, Serialize};

/// Configuration for the `demo-policy-console` gear.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoPolicyConsoleConfig {
    /// Vendor id registered for this gear's `PolicyProviderSpecV1` instance.
    #[serde(default = "default_vendor")]
    pub vendor: String,
    /// Selection priority — lower wins (see `toolkit::plugins::choose_plugin_instance`
    /// and `Service::resolve_policy_provider`). Deliberately low (high
    /// priority) so this demo provider wins over any other registered
    /// `PolicyProviderSpecV1` instance without needing tuning.
    #[serde(default = "default_priority")]
    pub priority: i16,
    /// Initial value of the PII-detection toggle.
    #[serde(default = "default_pii_enabled")]
    pub pii_enabled: bool,
    /// Initial response action applied to `"pii."`-prefixed categories
    /// while the toggle is on.
    #[serde(default = "default_pii_action")]
    pub pii_action: prompt_scanner_sdk::VerdictAction,
    /// Scanner backend vendors the demo's `TenantPolicy` enables — since
    /// this gear becomes `prompt-scanner`'s resolved `PolicyProvider` (see
    /// `gear.rs`), *something* has to say which backends run. Defaults to
    /// the detectors the `GenAI` Security demo pipeline wires up (see
    /// `config/genai-security.yaml`, `scripts/run-genai-security.sh`).
    /// `always-allow-plugin` is deliberately absent: it is a test double
    /// that inspects nothing, so enabling it in a demo would add a backend
    /// to the audit trail that never contributes a verdict.
    #[serde(default = "default_enabled_backends")]
    pub enabled_backends: Vec<String>,
}

impl Default for DemoPolicyConsoleConfig {
    fn default() -> Self {
        Self {
            vendor: default_vendor(),
            priority: default_priority(),
            pii_enabled: default_pii_enabled(),
            pii_action: default_pii_action(),
            enabled_backends: default_enabled_backends(),
        }
    }
}

fn default_vendor() -> String {
    "cf.demo.policy_console".to_owned()
}

fn default_priority() -> i16 {
    0
}

fn default_pii_enabled() -> bool {
    true
}

fn default_pii_action() -> prompt_scanner_sdk::VerdictAction {
    prompt_scanner_sdk::VerdictAction::Redact
}

fn default_enabled_backends() -> Vec<String> {
    vec![
        "cf.builtin.regex_scanner".to_owned(),
        "cf.builtin.onnx_pii".to_owned(),
    ]
}
