//! The polymorphic seam between `onnx-scanner`'s orchestration
//! (`domain::service::Service` — deferral, threshold-to-action mapping,
//! `Finding` construction) and any specific PII model family.
//!
//! Two implementations exist:
//! - [`crate::domain::bio_backend::BioTokenClassifierBackend`] — a plain
//!   BIO token-classification model (`minilm-l6-pii`).
//! - [`crate::domain::gliner2::Gliner2Backend`] — Fastino's `GLiNER2`, a
//!   zero-shot span model specialized for PII.
//!
//! `config::BackendConfig` selects which one `gear.rs` constructs; `Service`
//! itself never branches on which — that would defeat the point of the
//! trait. See `gears/prompt-scanner/docs/POLICY_PROVIDER_AND_DEMO_CONSOLE.md`
//! for why this shape (config-selected, single active backend per plugin
//! instance) rather than running both at once.

use async_trait::async_trait;
use prompt_scanner_sdk::PromptScannerError;
use toolkit_security::SecurityContext;

use crate::domain::entity::Entity;

/// A PII model backend: text in, detected entities out.
#[async_trait]
pub trait PiiBackend: Send + Sync {
    /// Detect PII entities in `text`. Returned spans are byte offsets into
    /// `text`.
    ///
    /// # Errors
    ///
    /// Returns [`PromptScannerError`] on model/runtime failure (e.g.
    /// `onnx-runner` unreachable, unexpected tensor shape).
    async fn detect_entities(
        &self,
        ctx: &SecurityContext,
        text: &str,
    ) -> Result<Vec<Entity>, PromptScannerError>;
}
