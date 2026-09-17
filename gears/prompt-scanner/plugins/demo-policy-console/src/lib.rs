//! Demo Policy Console — temporary demo control panel for `prompt-scanner`'s
//! PII detection.
//!
//! **Explicitly temporary.** Exists to make the `GenAI` Security demo
//! interactive (a toggle + a set of radio buttons + an audit log), not as a
//! real integration with an external policy service. See
//! `gears/prompt-scanner/docs/POLICY_PROVIDER_AND_DEMO_CONSOLE.md` for the
//! full design and why this can be deleted outright once a real
//! `PolicyProvider` adapter lands, without `prompt-scanner` needing any
//! change.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod api;
pub mod config;
pub mod domain;
pub mod gear;

pub use gear::DemoPolicyConsole;
