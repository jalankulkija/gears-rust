//! Scan Gateway Gear Implementation
//!
//! The public API is defined in `prompt-scanner-sdk` and re-exported here. See
//! `docs/DESIGN.md` for the full design (tenant-aware content-scanning /
//! verdict microservice, fan-out-to-all-enabled-backends architecture).

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

// === PUBLIC API (from SDK) ===
pub use prompt_scanner_sdk::{
    BackendOutcome, ContentKind, Finding, PromptScannerClientV1, PromptScannerError, ScanInput,
    ScanJobHandle, ScanJobStatus, ScanRequest, ScanVerdict, ScannerBackendClient,
    ScannerBackendSpecV1, Severity, Span, TenantPolicy, VerdictAction,
};

// === MODULE DEFINITION ===
pub mod gear;
pub use gear::PromptScanner;

// === INTERNAL MODULES ===
#[doc(hidden)]
pub mod api;
#[doc(hidden)]
pub mod config;
#[doc(hidden)]
pub mod domain;
#[doc(hidden)]
pub mod infra;
