//! The one type every PII model backend produces, regardless of how it
//! decoded it (BIO tag merging, `GLiNER2` span probabilities, ...). See
//! `domain::backend::PiiBackend`.

/// A detected entity span, in byte offsets into the original scanned text.
pub struct Entity {
    /// The category name (e.g. `"EMAIL"`), lowercased and prefixed by
    /// `Service` into a `Finding.category` like `"pii.email"`.
    pub category: String,
    pub start: u32,
    pub end: u32,
    /// Model confidence in `[0.0, 1.0]`.
    pub confidence: f32,
}
