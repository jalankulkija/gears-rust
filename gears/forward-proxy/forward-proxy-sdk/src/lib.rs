//! forward-proxy SDK
//!
//! Public API for the `forward-proxy` gear — a generic forward/MITM HTTPS
//! proxy that dispatches intercepted traffic through an ordered chain of
//! handler plugins. See `gears/forward-proxy/docs/DESIGN.md` for the full
//! design.
//!
//! - [`ProxyHandlerPluginClient`] — plugin API trait, implemented by
//!   handler plugins (e.g. `openai-interceptor-plugin`).
//! - [`ProxyHandlerPluginSpecV1`] — GTS schema handler plugins register an
//!   instance of, for discovery via `types-registry`.
//! - Domain models: [`ProxyRequest`], [`ProxyResponse`], [`ProxyOutcome`].
//! - [`ProxyHandlerError`] — transport-agnostic error type.

#![forbid(unsafe_code)]

pub mod errors;
pub mod gts;
pub mod models;
pub mod plugin_api;

pub use errors::ProxyHandlerError;
pub use gts::ProxyHandlerPluginSpecV1;
pub use models::{ProxyOutcome, ProxyRequest, ProxyResponse};
pub use plugin_api::ProxyHandlerPluginClient;
