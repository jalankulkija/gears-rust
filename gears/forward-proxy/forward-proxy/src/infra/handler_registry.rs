//! Discovers registered `ProxyHandlerPluginClient` instances via
//! `types-registry`, mirroring `prompt-scanner`'s own detector-plugin
//! discovery (`gears/prompt-scanner/prompt-scanner/src/domain/service.rs`'s
//! `resolve_backends`) — except there's no tenant policy to filter by
//! here: every registered handler is resolved, sorted into ascending
//! priority order (lower = higher priority, same convention as
//! `PluginV1::priority`), ready to hand to `domain::pipeline::Pipeline`.

use std::sync::Arc;

use forward_proxy_sdk::{ProxyHandlerPluginClient, ProxyHandlerPluginSpecV1};
use toolkit::client_hub::{ClientHub, ClientScope};
use toolkit::gts::PluginV1;
use types_registry_sdk::{InstanceQuery, TypesRegistryClient};

/// Resolves every registered handler plugin instance, in ascending
/// priority order. Instances that fail to deserialize, or whose scoped
/// `ClientHub` entry isn't registered yet, are skipped with a warning
/// rather than failing the whole resolution.
pub async fn resolve_handlers(
    hub: &ClientHub,
) -> anyhow::Result<Vec<Arc<dyn ProxyHandlerPluginClient>>> {
    let registry = hub.get::<dyn TypesRegistryClient>()?;
    let type_id = ProxyHandlerPluginSpecV1::gts_type_id().clone();

    let instances = registry
        .list_instances(InstanceQuery::new().with_pattern(format!("{type_id}*")))
        .await?;

    let mut resolved: Vec<(i16, Arc<dyn ProxyHandlerPluginClient>)> = Vec::new();
    for inst in &instances {
        let plugin: PluginV1<ProxyHandlerPluginSpecV1> =
            match serde_json::from_value(inst.object.clone()) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(
                        gts_id = %inst.id,
                        error = %e,
                        "forward-proxy: failed to deserialize handler plugin instance"
                    );
                    continue;
                }
            };

        let scope = ClientScope::gts_id(&inst.id);
        match hub.try_get_scoped::<dyn ProxyHandlerPluginClient>(&scope) {
            Some(client) => resolved.push((plugin.priority, client)),
            None => {
                tracing::warn!(
                    vendor = %plugin.vendor,
                    gts_id = %inst.id,
                    "forward-proxy: handler plugin client not registered yet"
                );
            }
        }
    }

    resolved.sort_by_key(|(priority, _)| *priority);
    Ok(resolved.into_iter().map(|(_, client)| client).collect())
}
