use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Which typed extraction to use for a given output tensor — the model's
/// own documented output dtypes are declared here rather than guessed at
/// runtime (mirrors the `GraphContract` verification philosophy: know your
/// model ahead of time, fail loudly on mismatch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputDtype {
    I64,
    F32,
    U8,
    Bool,
}

/// One configured model: where its files live, its expected input/output
/// tensor contract, and how many session replicas to load (see
/// `docs/DESIGN.md`'s "pool of session replicas" decision).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    pub model_id: String,
    /// Directory containing `model.onnx` (or the configured `model_file`
    /// name) and `tokenizer.json`.
    pub model_dir: PathBuf,
    #[serde(default = "default_model_file")]
    pub model_file: String,
    #[serde(default = "default_tokenizer_file")]
    pub tokenizer_file: String,
    /// Optional metadata file carrying the model's `id2label` map. Absent for
    /// models that declare no fixed label set; never a load failure.
    #[serde(default = "default_labels_file")]
    pub labels_file: String,
    /// Expected SHA-256 of the model file — verified at load unless
    /// `skip_checksum` is set (local dev only).
    pub sha256: String,
    #[serde(default)]
    pub skip_checksum: bool,
    pub expected_inputs: Vec<String>,
    pub expected_outputs: Vec<String>,
    pub output_dtypes: BTreeMap<String, OutputDtype>,
    #[serde(default = "default_replicas")]
    pub replicas: usize,
    #[serde(default = "default_intra_threads")]
    pub intra_threads: usize,

    // ---- Bounds -----------------------------------------------------------
    //
    // Defaults are chosen to be safe for an untrusted caller, because defaults
    // are what people actually deploy, and because the content flowing through
    // this gear is attacker-controlled by construction. They are generous
    // enough that today's consumer never sees them.
    /// Maximum elements in any one input tensor.
    #[serde(default = "default_max_tensor_elements")]
    pub max_tensor_elements: usize,
    /// Maximum input tensors in one `run` call.
    #[serde(default = "default_max_input_tensors")]
    pub max_input_tensors: usize,
    /// Maximum extent of any single dimension.
    #[serde(default = "default_max_dimension")]
    pub max_dimension: usize,
    /// Maximum UTF-8 bytes accepted by `encode`/`encode_fragment`. Oversized text
    /// is rejected, never silently truncated — truncation is a policy this
    /// layer deliberately does not own.
    #[serde(default = "default_max_text_bytes")]
    pub max_text_bytes: usize,

    /// Wall-clock budget for one inference.
    ///
    /// On expiry the caller gets `Timeout`, but the work is *abandoned, not
    /// cancelled* — ONNX Runtime has no mid-graph cancellation, so the replica
    /// stays busy until the underlying call returns.
    #[serde(default = "default_inference_timeout", with = "humantime_serde")]
    pub inference_timeout: Duration,

    /// Concurrent inferences allowed for this model. Defaults to the replica
    /// count: more in flight than there are replicas is queueing, not
    /// parallelism, and unbounded queueing is what this cap exists to stop.
    #[serde(default)]
    pub max_in_flight: Option<usize>,

    /// What to do when `max_in_flight` is reached.
    #[serde(default)]
    pub on_overload: OverloadPolicy,
}

impl Default for ModelConfig {
    /// Mirrors the serde defaults, so a test (or a caller building a config
    /// programmatically) can set the fields it cares about and inherit the
    /// rest rather than restating every bound.
    fn default() -> Self {
        Self {
            model_id: String::new(),
            model_dir: PathBuf::new(),
            model_file: default_model_file(),
            tokenizer_file: default_tokenizer_file(),
            labels_file: default_labels_file(),
            sha256: String::new(),
            skip_checksum: false,
            expected_inputs: Vec::new(),
            expected_outputs: Vec::new(),
            output_dtypes: BTreeMap::new(),
            replicas: default_replicas(),
            intra_threads: default_intra_threads(),
            max_tensor_elements: default_max_tensor_elements(),
            max_input_tensors: default_max_input_tensors(),
            max_dimension: default_max_dimension(),
            max_text_bytes: default_max_text_bytes(),
            inference_timeout: default_inference_timeout(),
            max_in_flight: None,
            on_overload: OverloadPolicy::Queue,
        }
    }
}

/// What a caller gets when a model is already at capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OverloadPolicy {
    /// Wait for a permit, bounded by `inference_timeout`. The default: a
    /// consumer's own deadline is usually a better place to give up than an
    /// arbitrary queue depth here, and the timeout already bounds the wait.
    #[default]
    Queue,
    /// Fail immediately with `Overloaded`. For deployments that would rather
    /// shed load than let it accumulate.
    Reject,
}

fn default_model_file() -> String {
    "model.onnx".to_owned()
}

fn default_labels_file() -> String {
    "config.json".to_owned()
}

fn default_tokenizer_file() -> String {
    "tokenizer.json".to_owned()
}

fn default_replicas() -> usize {
    4
}

fn default_intra_threads() -> usize {
    1
}

/// 16Mi elements — 64 MiB as f32, 128 MiB as i64. Far above any real
/// tokenized batch, far below "exhaust the host".
fn default_max_tensor_elements() -> usize {
    16 * 1024 * 1024
}

fn default_max_input_tensors() -> usize {
    32
}

fn default_max_dimension() -> usize {
    1024 * 1024
}

/// 1 MiB of text. Well past any prompt; a document belongs in an attachment
/// path that chunks it, not in a single `encode` call.
fn default_max_text_bytes() -> usize {
    1024 * 1024
}

/// Generous enough that a large model on a cold CPU is not cut off, tight
/// enough to shed a pathological input. Tune per model once
/// `observability` makes real latencies visible.
fn default_inference_timeout() -> Duration {
    Duration::from_secs(30)
}

/// Configuration for the `onnx-runner` gear.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct OnnxRunnerConfig {
    #[serde(default)]
    pub models: Vec<ModelConfig>,
    /// Fail startup if any configured model cannot be loaded.
    ///
    /// Defaults to `false`: a bad artifact makes *that model* unavailable and
    /// leaves the rest serving, because this gear shares a process with every
    /// other gear and must not take them down with it. Consumers are expected
    /// to map `ModelUnavailable` to degraded coverage — a scan that could not
    /// run is not a clean scan — which is what makes the permissive default
    /// safe rather than merely convenient.
    ///
    /// Set `true` for deployments that would rather not start at all than
    /// start degraded.
    #[serde(default)]
    pub require_all_models: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The operator-facing reference config parses into the structs it claims to
    /// document, using the same duplicate-key-rejecting parser the server's config
    /// loader uses.
    ///
    /// This exists because a reference config is the one kind of documentation that
    /// can be wrong in a way nobody notices: it is never executed. `deny_unknown_fields`
    /// makes the test catch a renamed or removed field, which is exactly how a file
    /// like this rots.
    fn parse_example() -> OnnxRunnerConfig {
        let raw = include_str!("../../docs/config-example.yaml");
        let opts = serde_saphyr::options! {
            duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
        };
        serde_saphyr::from_str_with_options(raw, opts)
            .expect("docs/config-example.yaml must deserialize into OnnxRunnerConfig")
    }

    #[test]
    fn reference_config_parses() {
        let cfg = parse_example();

        assert!(!cfg.require_all_models);
        let ids: Vec<&str> = cfg.models.iter().map(|m| m.model_id.as_str()).collect();
        assert_eq!(ids, ["gliner2-pii", "piiranha-v1"]);
    }

    /// The annotated entry states a default in a comment beside almost every field.
    /// A comment claiming the wrong default is worse than no comment, so assert that
    /// the values written there are the values the code produces.
    #[test]
    fn annotated_entry_agrees_with_the_real_defaults() {
        let cfg = parse_example();
        let annotated = &cfg.models[0];
        let defaults = ModelConfig::default();

        assert_eq!(annotated.tokenizer_file, defaults.tokenizer_file);
        assert_eq!(annotated.labels_file, defaults.labels_file);
        assert_eq!(annotated.skip_checksum, defaults.skip_checksum);
        assert_eq!(annotated.replicas, defaults.replicas);
        assert_eq!(annotated.intra_threads, defaults.intra_threads);
        assert_eq!(annotated.max_tensor_elements, defaults.max_tensor_elements);
        assert_eq!(annotated.max_input_tensors, defaults.max_input_tensors);
        assert_eq!(annotated.max_dimension, defaults.max_dimension);
        assert_eq!(annotated.max_text_bytes, defaults.max_text_bytes);
        assert_eq!(annotated.inference_timeout, defaults.inference_timeout);
        assert_eq!(annotated.on_overload, defaults.on_overload);

        // `max_in_flight` is the one knob written out rather than left implicit:
        // unset means "the replica count", and the file spells that out.
        assert_eq!(annotated.max_in_flight, Some(annotated.replicas));

        // Deliberately not defaults — the whole point of showing them.
        assert_eq!(annotated.model_file, "model.quantized.onnx");
        assert_ne!(annotated.model_file, defaults.model_file);
    }

    /// The minimal entry must actually be minimal: everything it omits has to come
    /// back as the documented default, or the file is teaching a fiction.
    #[test]
    fn minimal_entry_inherits_every_default() {
        let cfg = parse_example();
        let minimal = &cfg.models[1];
        let defaults = ModelConfig::default();

        assert_eq!(minimal.tokenizer_file, defaults.tokenizer_file);
        assert_eq!(minimal.labels_file, defaults.labels_file);
        assert_eq!(minimal.replicas, defaults.replicas);
        assert_eq!(minimal.intra_threads, defaults.intra_threads);
        assert_eq!(minimal.inference_timeout, defaults.inference_timeout);
        assert_eq!(minimal.max_in_flight, None);
        assert_eq!(minimal.on_overload, OverloadPolicy::Queue);
        assert!(!minimal.skip_checksum);
    }

    /// Both entries pin a digest. A reference config that shipped `skip_checksum:
    /// true`, or an empty `sha256`, would be teaching operators to disable the only
    /// artifact-integrity control the gear has.
    #[test]
    fn reference_config_never_disables_integrity_checking() {
        for model in parse_example().models {
            assert!(
                !model.skip_checksum,
                "{} sets skip_checksum in the reference config",
                model.model_id
            );
            assert_eq!(
                model.sha256.len(),
                64,
                "{} needs a full hex sha256",
                model.model_id
            );
            assert!(model.sha256.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    /// Every declared output needs a dtype. This is a startup failure in the real
    /// loader; the reference config must not model the failure.
    #[test]
    fn every_declared_output_has_a_dtype() {
        for model in parse_example().models {
            for output in &model.expected_outputs {
                assert!(
                    model.output_dtypes.contains_key(output),
                    "{} declares output {output} with no dtype",
                    model.model_id
                );
            }
            assert_eq!(model.output_dtypes.len(), model.expected_outputs.len());
        }
    }
}
