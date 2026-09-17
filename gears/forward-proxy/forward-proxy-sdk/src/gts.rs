//! GTS type definitions for forward-proxy handler plugins.
//!
//! This gear defines the GTS type for handler plugin instances. Handler
//! plugins register instances of this type with `types-registry` to be
//! discovered by `forward-proxy`'s core dispatch loop.

use toolkit::gts::PluginV1;
use toolkit_gts::gts_type_schema;

/// GTS type definition for forward-proxy handler plugin instances.
///
/// Each handler plugin registers an instance of this type with its
/// vendor-specific instance id. `forward-proxy`'s core discovers *all*
/// registered instances and runs them in ascending-priority order (see
/// `plugin_api` module docs — sequential composition, not fan-out).
///
/// # Instance ID Format
///
/// ```text
/// gts.cf.toolkit.plugins.plugin.v1~cf.core.forward_proxy.handler_plugin.v1~<vendor>.<package>.handler.v1~
/// ```
#[derive(Default)]
#[gts_type_schema(
    dir_path = "schemas",
    base = PluginV1,
    type_id = gts_id!("cf.toolkit.plugins.plugin.v1~cf.core.forward_proxy.handler_plugin.v1~"),
    description = "forward-proxy handler plugin specification",
    properties = "",
)]
pub struct ProxyHandlerPluginSpecV1;
