//! Always-Allow Scanner Backend Plugin
//!
//! Minimal `ScannerBackendClient` implementation for `prompt-scanner`: never
//! finds anything, always suggests `VerdictAction::Allow`. Exists purely so
//! `prompt-scanner`'s fan-out resolution logic has a real plugin instance to
//! discover and call in integration tests. See
//! `gears/prompt-scanner/docs/DESIGN.md`.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod config;
pub mod domain;
pub mod gear;

pub use gear::AlwaysAllowPlugin;
