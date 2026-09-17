//! Regex Scanner Plugin
//!
//! Deterministic `ScannerBackendClient` implementation for `prompt-scanner`:
//! regex + validator (Luhn, IBAN mod-97-10, `std::net` IP parsing, JWT
//! structural check, Shannon entropy) detection of PII and secret shapes.
//! No ONNX/ML dependency — see `gears/onnx-runner/docs/DESIGN.md`'s
//! "Tier-0 deterministic detection" rationale.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod config;
pub mod domain;
pub mod gear;

pub use gear::RegexScanner;
