use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenAiInterceptorConfig {
    #[serde(default = "default_vendor")]
    pub vendor: String,
    #[serde(default = "default_priority")]
    pub priority: i16,
    #[serde(default = "default_prompt_scanner_tenant_id")]
    pub tenant_id: uuid::Uuid,
    #[serde(default = "default_connector_id")]
    pub connector_id: String,
    /// Product name this handler's block messages are attributed to.
    ///
    /// The message is injected into the end user's chat session, so whoever
    /// deploys the proxy decides what it names — it is deliberately not a
    /// literal in the code. See `domain::Service::block_message`.
    #[serde(default = "default_product_name")]
    pub product_name: String,
}

impl Default for OpenAiInterceptorConfig {
    fn default() -> Self {
        Self {
            vendor: default_vendor(),
            priority: default_priority(),
            tenant_id: default_prompt_scanner_tenant_id(),
            connector_id: default_connector_id(),
            product_name: default_product_name(),
        }
    }
}

fn default_vendor() -> String {
    "cf.builtin.openai_interceptor".to_owned()
}

fn default_priority() -> i16 {
    1000
}

fn default_prompt_scanner_tenant_id() -> uuid::Uuid {
    uuid::Uuid::nil()
}

fn default_connector_id() -> String {
    "forward-proxy.openai-interceptor".to_owned()
}

fn default_product_name() -> String {
    "GenAI Security Scanner".to_owned()
}
