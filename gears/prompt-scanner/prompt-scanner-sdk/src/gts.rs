//! GTS type definitions for prompt-scanner scanner backend plugins.
//!
//! This gear defines the GTS type for scanner backend plugin instances.
//! Scanner backend plugins register instances of this type with
//! `types-registry` to be discovered by `prompt-scanner`'s fan-out
//! resolution logic.

use toolkit::gts::PluginV1;
use toolkit_gts::gts_type_schema;

/// GTS type definition for prompt-scanner scanner backend plugin instances.
///
/// Each scanner backend plugin registers an instance of this type with its
/// vendor-specific instance id. `prompt-scanner`'s domain service discovers
/// *all* matching instances (not just one) and filters them by the calling
/// tenant's `TenantPolicy.enabled_backends` before fanning out.
///
/// # Instance ID Format
///
/// ```text
/// gts.cf.toolkit.plugins.plugin.v1~cf.core.prompt_scanner.scanner_backend.v1~<vendor>.<package>.detector.v1~
/// ```
///
/// # Example
///
/// ```ignore
/// let (instance_id, instance_json) = PluginV1::<ScannerBackendSpecV1>::build_registration(
///     "cf.builtin.always_allow.detector.v1",
///     cfg.vendor.clone(),
///     cfg.priority,
/// )?;
///
/// let results = registry.register(vec![instance_json]).await?;
/// RegisterResult::ensure_all_ok(&results)?;
/// ```
#[derive(Default)]
#[gts_type_schema(
    dir_path = "schemas",
    base = PluginV1,
    type_id = gts_id!("cf.toolkit.plugins.plugin.v1~cf.core.prompt_scanner.scanner_backend.v1~"),
    description = "prompt-scanner scanner backend plugin specification",
    properties = "",
)]
pub struct ScannerBackendSpecV1;

/// GTS type definition for prompt-scanner `PolicyProvider` plugin instances.
///
/// Unlike [`ScannerBackendSpecV1`], `prompt-scanner` resolves at most **one**
/// instance of this type — the highest-priority registered instance — via
/// `toolkit::plugins::choose_plugin_instance`-style single-instance
/// selection, falling back to its own built-in DB-backed provider when none
/// is registered. See `crate::plugin_api::PolicyProvider` and
/// `docs/POLICY_PROVIDER_AND_DEMO_CONSOLE.md`.
#[derive(Default)]
#[gts_type_schema(
    dir_path = "schemas",
    base = PluginV1,
    type_id = gts_id!("cf.toolkit.plugins.plugin.v1~cf.core.prompt_scanner.policy_provider.v1~"),
    description = "prompt-scanner tenant policy provider plugin specification",
    properties = "",
)]
pub struct PolicyProviderSpecV1;
