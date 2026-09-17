// ToolKit needs access to the gear struct for instantiation
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

// === PUBLIC API (from SDK) ===
pub use forward_proxy_sdk::{
    ProxyHandlerError, ProxyHandlerPluginClient, ProxyOutcome, ProxyRequest, ProxyResponse,
};

// === MODULE DEFINITION ===
pub mod gear;
pub use gear::ForwardProxy;

// === INTERNAL MODULES ===
// WARNING: internal implementation details, exposed only for testing —
// external consumers should depend on `cf-gears-forward-proxy-sdk`.
#[doc(hidden)]
pub mod config;
#[doc(hidden)]
pub mod domain;
#[doc(hidden)]
pub mod infra;
