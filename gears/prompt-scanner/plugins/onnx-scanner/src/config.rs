use serde::{Deserialize, Serialize};

/// Which PII model family `onnx-scanner` runs, and that family's own
/// config — see `domain::backend::PiiBackend` for why this is a trait
/// rather than a hardcoded model.
///
/// `model_id` must match a model already configured (and loaded) in
/// `onnx-runner` — see `gears/onnx-runner/docs/DESIGN.md`. This plugin
/// never loads or fetches a model itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PiiBackendConfig {
    /// A plain BIO token-classification model (`minilm-l6-pii` by
    /// default) — see `gears/onnx-runner/docs/DESIGN.md`'s original
    /// "Which PII model" decision.
    BioTokenClassifier {
        #[serde(default = "default_bio_model_id")]
        model_id: String,
        /// BIO label set, in the exact order the model's output logits are
        /// indexed (e.g. `["O", "B-EMAIL", "I-EMAIL", ...]`).
        #[serde(default = "default_bio_labels")]
        labels: Vec<String>,
    },
    /// Fastino's `GLiNER2`, a zero-shot span model specialized for PII — see
    /// `domain::gliner2`'s module docs.
    Gliner2 {
        #[serde(default = "default_gliner2_model_id")]
        model_id: String,
        /// Entity labels to condition the schema prompt on (e.g.
        /// `["person", "email", ...]`) — also used directly as
        /// `Finding.category` (lowercase, no further mapping).
        #[serde(default = "default_gliner2_labels")]
        labels: Vec<String>,
        /// Spans scoring below this are never emitted, independent of
        /// `redact_threshold`/`warn_threshold` below (which only ever see
        /// scores this backend actually reports). The model card's
        /// evaluated operating points are 0.3/0.5/0.7.
        #[serde(default = "default_gliner2_decode_threshold")]
        decode_threshold: f32,
    },
}

impl Default for PiiBackendConfig {
    fn default() -> Self {
        Self::BioTokenClassifier {
            model_id: default_bio_model_id(),
            labels: default_bio_labels(),
        }
    }
}

/// Configuration for the `onnx-scanner` gear.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnnxScannerConfig {
    #[serde(default = "default_vendor")]
    pub vendor: String,
    #[serde(default = "default_priority")]
    pub priority: i16,
    #[serde(default)]
    pub backend: PiiBackendConfig,
    /// Prefix for `Finding.category` (e.g. `"pii"` -> `"pii.email"`).
    #[serde(default = "default_category_prefix")]
    pub category_prefix: String,
    /// Payload word count above which `detect` returns
    /// `BackendOutcome::Defer` instead of calling the backend — see
    /// `domain::service`'s module docs for the known limitation this
    /// implies.
    #[serde(default = "default_defer_threshold_words")]
    pub defer_threshold_words: usize,
    /// Confidence at/above which a finding suggests `VerdictAction::Redact`.
    #[serde(default = "default_redact_threshold")]
    pub redact_threshold: f32,
    /// Confidence at/above which a finding suggests `VerdictAction::Warn`
    /// (ignored once `redact_threshold` is reached).
    #[serde(default = "default_warn_threshold")]
    pub warn_threshold: f32,
}

impl Default for OnnxScannerConfig {
    fn default() -> Self {
        Self {
            vendor: default_vendor(),
            priority: default_priority(),
            backend: PiiBackendConfig::default(),
            category_prefix: default_category_prefix(),
            defer_threshold_words: default_defer_threshold_words(),
            redact_threshold: default_redact_threshold(),
            warn_threshold: default_warn_threshold(),
        }
    }
}

fn default_vendor() -> String {
    "cf.builtin.onnx_pii".to_owned()
}

fn default_priority() -> i16 {
    1000
}

fn default_bio_model_id() -> String {
    "minilm-l6-pii".to_owned()
}

/// `Negative-Star-Innovators/MiniLM-L6-finetuned-pii-detection-v2`'s label
/// set — see `gears/onnx-runner/docs/DESIGN.md`'s "Which PII model"
/// decision. Operators pointing `onnx-runner` at a different BIO model
/// must override this to match.
fn default_bio_labels() -> Vec<String> {
    [
        "O",
        "B-EMAIL",
        "I-EMAIL",
        "B-PHONE",
        "I-PHONE",
        "B-PERSON",
        "I-PERSON",
        "B-ADDRESS",
        "I-ADDRESS",
        "B-DATE",
        "I-DATE",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

fn default_gliner2_model_id() -> String {
    "gliner2-pii".to_owned()
}

/// The label schema `dlp-bench` validated as `GLINER2_PII` — see that
/// repo's `src/labels.rs`.
fn default_gliner2_labels() -> Vec<String> {
    [
        "person",
        "email",
        "phone_number",
        "address",
        "date_of_birth",
        "national_id_number",
        "passport_number",
        "drivers_license_number",
        "card_number",
        "bank_account",
        "iban",
        "ip_address",
        "username",
        "api_key",
        "access_token",
        "password",
        "secret",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

fn default_gliner2_decode_threshold() -> f32 {
    0.5
}

fn default_category_prefix() -> String {
    "pii".to_owned()
}

fn default_defer_threshold_words() -> usize {
    2_000
}

fn default_redact_threshold() -> f32 {
    0.6
}

fn default_warn_threshold() -> f32 {
    0.3
}
