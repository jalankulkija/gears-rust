use serde::{Deserialize, Serialize};

/// Configuration for the `always-allow-plugin` gear.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlwaysAllowPluginConfig {
    #[serde(default = "default_vendor")]
    pub vendor: String,
    #[serde(default = "default_priority")]
    pub priority: i16,
}

impl Default for AlwaysAllowPluginConfig {
    fn default() -> Self {
        Self {
            vendor: default_vendor(),
            priority: default_priority(),
        }
    }
}

fn default_vendor() -> String {
    "cf.builtin.always_allow".to_owned()
}

fn default_priority() -> i16 {
    1000
}
