//! Scan Gateway SDK
//!
//! Public API for the `prompt-scanner` gear — a tenant-aware content-scanning
//! / verdict microservice. See `gears/prompt-scanner/docs/DESIGN.md` for the
//! full design.
//!
//! - [`PromptScannerClientV1`] — public API trait, consumed by connectors
//!   (endpoint agents, `llm-gateway`'s `scan-hook-plugin`, MCP proxies, ...).
//! - [`ScannerBackendClient`] — plugin API trait, implemented by scanner
//!   backends (DLP, secrets, prompt injection, harmful content, malicious
//!   code/URL, ...).
//! - [`ScannerBackendSpecV1`] — GTS schema scanner backend plugins register
//!   an instance of, for discovery via `types-registry`.
//! - Domain models: [`ScanRequest`], [`ScanVerdict`], [`Finding`],
//!   [`VerdictAction`], [`TenantPolicy`], ...
//! - [`PromptScannerError`] — transport-agnostic error type.
//!
//! ## Usage
//!
//! ```ignore
//! use prompt_scanner_sdk::{PromptScannerClientV1, ScanRequest, ContentKind};
//!
//! let client = hub.get::<dyn PromptScannerClientV1>()?;
//! let verdict = client.scan(&ctx, ScanRequest {
//!     tenant_id,
//!     connector_id: "llm-gateway".into(),
//!     content_kind: ContentKind::Prompt,
//!     payload: "...".into(),
//!     metadata: Default::default(),
//! }).await?;
//! ```

#![forbid(unsafe_code)]

pub mod api;
pub mod errors;
pub mod gts;
pub mod models;
pub mod odata;
pub mod plugin_api;
pub mod redaction;

// Re-export main types at crate root
pub use api::PromptScannerClientV1;
pub use errors::PromptScannerError;
pub use gts::{PolicyProviderSpecV1, ScannerBackendSpecV1};
pub use models::{
    Attachment, BackendOutcome, ContentKind, Finding, ScanInput, ScanJobHandle, ScanJobStatus,
    ScanRequest, ScanVerdict, Severity, Span, TenantPolicy, VerdictAction,
};
pub use plugin_api::{PolicyProvider, ScannerBackendClient};
pub use redaction::redact_spans;
