use serde::{Deserialize, Serialize};

/// A single named tensor's data, covering the dtypes real models actually
/// need — dlp-bench only ever needed I64 in / F32 out, but attention masks
/// are sometimes Bool and some tokenizer outputs are U8.
///
/// # Wire format
///
/// An externally-tagged enum keyed by the dtype's canonical lowercase name,
/// carrying the elements as one flat array:
///
/// ```json
/// {"f32": [0.1, 0.2, 0.3]}
/// ```
///
/// The shape is deliberately *not* here — it lives on [`NamedTensor::shape`],
/// and this array is that shape flattened in row-major order. Keeping them
/// apart means a shape/data disagreement is one validation at the boundary
/// rather than a malformed value.
///
/// The dtype keys match the variant names in lowercase (`i64`, `f32`, `u8`,
/// `bool`), so the wire spelling and the ONNX dtype spelling agree.
///
/// ## Size, and when not to use this
///
/// Numeric JSON is verbose: a float costs a dozen-odd characters where it
/// costs four bytes in memory. A 512-token classifier output over 28 labels
/// is ~14k floats — on the order of 200 KB of JSON against 57 KB of raw
/// bytes, and the parse is not free either.
///
/// These derives exist to unblock an out-of-process transport
/// (`docs/toolkit_unified_system/09_oop_grpc_sdk_pattern.md`), **not** to
/// recommend one for bulk inference. In-process callers reach the gear
/// through `ClientHub` and move these types by value; nothing is serialized
/// on that path, and that is the path a throughput-sensitive consumer wants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TensorData {
    I64(Vec<i64>),
    F32(Vec<f32>),
    U8(Vec<u8>),
    Bool(Vec<bool>),
}

/// A single named tensor, in or out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedTensor {
    pub name: String,
    pub shape: Vec<usize>,
    pub data: TensorData,
}

impl NamedTensor {
    #[must_use]
    pub fn new(name: impl Into<String>, shape: Vec<usize>, data: TensorData) -> Self {
        Self {
            name: name.into(),
            shape,
            data,
        }
    }
}

/// Why a configured model is not usable.
///
/// A stable machine-readable classification, never a free-form diagnostic:
/// consumers branch on it, and it reaches logs and telemetry, so it must never
/// carry a path outside the configured model directory, file contents, or
/// tensor data.
///
/// # Wire format
///
/// Serializes as the same `snake_case` string [`Self::as_code`] returns, so a
/// reason read off the wire and the same reason read out of a log line are
/// spelled identically. A test pins that equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFailureReason {
    /// The model file could not be read.
    FileUnreadable,
    /// The file's SHA-256 did not match the configured digest.
    ChecksumMismatch,
    /// The loaded graph's tensor names differ from the configured contract.
    ContractMismatch,
    /// `tokenizer.json` was missing or could not be parsed.
    TokenizerInvalid,
    /// ONNX Runtime rejected the graph, or a session could not be rebuilt
    /// after being lost to a panicking inference.
    SessionBuildFailed,
    /// The model's configuration was rejected before any file was read.
    InvalidConfig,
}

impl ModelFailureReason {
    /// A stable `snake_case` code, for logs and telemetry.
    #[must_use]
    pub const fn as_code(self) -> &'static str {
        match self {
            Self::FileUnreadable => "file_unreadable",
            Self::ChecksumMismatch => "checksum_mismatch",
            Self::ContractMismatch => "contract_mismatch",
            Self::TokenizerInvalid => "tokenizer_invalid",
            Self::SessionBuildFailed => "session_build_failed",
            Self::InvalidConfig => "invalid_config",
        }
    }
}

impl std::fmt::Display for ModelFailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_code())
    }
}

/// Where a configured model stands.
///
/// Every *configured* model has a state, including one that never loaded —
/// that is the point. A caller that gets no entry at all is looking at a typo;
/// a caller that gets `Unavailable` is looking at an outage.
///
/// # Wire format
///
/// `"ready"` and `"loading"` are bare strings; the data-carrying variant is
/// externally tagged as `{"unavailable": {"reason": "checksum_mismatch"}}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelState {
    /// Loaded, verified, and serving.
    Ready,
    /// Loading, or rebuilding a session lost to a panic. Transient.
    Loading,
    /// Configured but not usable. `reason` is stable enough to branch on.
    Unavailable { reason: ModelFailureReason },
}

impl ModelState {
    #[must_use]
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// The parts of [`ModelInfo`] that only exist once a model has actually
/// loaded: the graph's real tensor names and the digest that was verified.
///
/// Separate rather than empty-when-unavailable, so a consumer cannot mistake
/// "this model declares no outputs" for "this model never loaded".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadedModelInfo {
    pub input_names: Vec<String>,
    pub output_names: Vec<String>,
    pub sha256: String,
    /// Replicas configured for this model.
    pub replicas: usize,
    /// The model's class labels, in class-id order, when it publishes an
    /// `id2label` map beside its weights.
    ///
    /// This is metadata carried in the model directory, exactly as
    /// `tokenizer.json` is — serving it is not knowledge of what the labels
    /// *mean*. Interpreting them (BIO merging, entity taxonomies, thresholds)
    /// remains the consumer's job.
    ///
    /// `None` means the model published no usable map, which is normal: a
    /// zero-shot span model takes its labels at request time and has none to
    /// declare. It is **not** a signal that the model failed to load.
    ///
    /// Indexing is positional — `labels[class_id]` — so consumers should check
    /// the length against the model's own class dimension before trusting it;
    /// the gear does not know which output carries classes.
    pub labels: Option<Vec<String>>,
}

/// Metadata about a configured model, for introspection (`list_models`) and
/// so a caller can fail loudly if it's pointed at the wrong model id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub model_id: String,
    pub state: ModelState,
    /// `Some` exactly when the model loaded. See [`LoadedModelInfo`].
    pub loaded: Option<LoadedModelInfo>,
}

impl ModelInfo {
    /// Whether this model can serve a request right now.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        self.state.is_ready()
    }
}

/// Result of tokenizing text against a configured tokenizer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenEncoding {
    pub ids: Vec<i64>,
    pub attention_mask: Vec<i64>,
    /// Byte offsets into the original text, per token — needed to map
    /// model-space spans back to the original payload.
    pub offsets: Vec<(u32, u32)>,
    pub special_tokens_mask: Vec<bool>,
}

#[cfg(test)]
mod serde_tests {
    use super::*;

    /// Round-trips through JSON and asserts equality. Every public type gets
    /// this: a derive that compiles is not evidence that a value survives the
    /// trip, and asymmetric attributes are the usual way it stops doing so.
    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).expect("serializes");
        let back: T = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(
            &back, value,
            "round-trip changed the value; json was {json}"
        );
        back
    }

    #[test]
    fn tensor_data_round_trips_every_dtype() {
        round_trip(&TensorData::I64(vec![i64::MIN, -1, 0, 1, i64::MAX]));
        round_trip(&TensorData::F32(vec![-1.5, 0.0, 0.1, 3.402_823_5e38]));
        round_trip(&TensorData::U8(vec![0, 127, 255]));
        round_trip(&TensorData::Bool(vec![true, false, true]));
    }

    #[test]
    fn tensor_data_round_trips_when_empty() {
        // An empty tensor is a legal zero-element shape, not a missing value,
        // so it must survive as `[]` rather than collapsing to null.
        round_trip(&TensorData::I64(vec![]));
        round_trip(&TensorData::F32(vec![]));
        round_trip(&TensorData::U8(vec![]));
        round_trip(&TensorData::Bool(vec![]));
    }

    #[test]
    fn tensor_data_wire_shape_is_dtype_keyed() {
        // Pinning the documented format, not just that it round-trips: a
        // rename here is a breaking change for any out-of-process consumer.
        let json = serde_json::to_string(&TensorData::F32(vec![0.5, 1.5])).expect("serializes");
        assert_eq!(json, r#"{"f32":[0.5,1.5]}"#);

        let json = serde_json::to_string(&TensorData::I64(vec![7])).expect("serializes");
        assert_eq!(json, r#"{"i64":[7]}"#);

        let json = serde_json::to_string(&TensorData::U8(vec![1])).expect("serializes");
        assert_eq!(json, r#"{"u8":[1]}"#);

        let json = serde_json::to_string(&TensorData::Bool(vec![true])).expect("serializes");
        assert_eq!(json, r#"{"bool":[true]}"#);
    }

    #[test]
    fn named_tensor_round_trips_and_keeps_shape_separate() {
        let t = NamedTensor::new("input_ids", vec![1, 3], TensorData::I64(vec![101, 7, 102]));
        let back = round_trip(&t);
        assert_eq!(back.shape, vec![1, 3]);

        let json = serde_json::to_string(&t).expect("serializes");
        assert!(
            json.contains(r#""shape":[1,3]"#),
            "shape stays on the tensor: {json}"
        );
    }

    #[test]
    fn large_shape_round_trips() {
        // The size case the doc comment warns about, exercised rather than
        // only described: 512 tokens x 28 labels.
        let n: u16 = 512 * 28;
        let data: Vec<f32> = (0..n).map(|i| f32::from(i) * 0.001).collect();
        let t = NamedTensor::new("logits", vec![1, 512, 28], TensorData::F32(data));

        let json = serde_json::to_string(&t).expect("serializes");
        assert!(
            json.len() > 100_000,
            "verbose as documented: {} bytes",
            json.len()
        );

        let back: NamedTensor = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, t);
    }

    #[test]
    fn failure_reason_wire_spelling_equals_as_code() {
        // The invariant the doc comment claims. If a variant is added with a
        // wire name that drifts from its telemetry code, this fails.
        for reason in [
            ModelFailureReason::FileUnreadable,
            ModelFailureReason::ChecksumMismatch,
            ModelFailureReason::ContractMismatch,
            ModelFailureReason::TokenizerInvalid,
            ModelFailureReason::SessionBuildFailed,
            ModelFailureReason::InvalidConfig,
        ] {
            round_trip(&reason);
            let json = serde_json::to_string(&reason).expect("serializes");
            assert_eq!(
                json,
                format!("\"{}\"", reason.as_code()),
                "wire spelling must match the telemetry code"
            );
        }
    }

    #[test]
    fn model_state_round_trips_including_the_data_variant() {
        round_trip(&ModelState::Ready);
        round_trip(&ModelState::Loading);
        round_trip(&ModelState::Unavailable {
            reason: ModelFailureReason::ChecksumMismatch,
        });

        assert_eq!(
            serde_json::to_string(&ModelState::Ready).expect("serializes"),
            r#""ready""#
        );
        assert_eq!(
            serde_json::to_string(&ModelState::Unavailable {
                reason: ModelFailureReason::SessionBuildFailed
            })
            .expect("serializes"),
            r#"{"unavailable":{"reason":"session_build_failed"}}"#
        );
    }

    #[test]
    fn loaded_model_info_round_trips_with_and_without_labels() {
        let with_labels = LoadedModelInfo {
            input_names: vec!["input_ids".to_owned(), "attention_mask".to_owned()],
            output_names: vec!["logits".to_owned()],
            sha256: "a".repeat(64),
            replicas: 4,
            labels: Some(vec!["O".to_owned(), "B-PER".to_owned()]),
        };
        round_trip(&with_labels);

        // `None` is normal — a zero-shot model declares no labels — so it must
        // survive as null rather than as an empty list.
        let without = LoadedModelInfo {
            labels: None,
            ..with_labels
        };
        let back = round_trip(&without);
        assert!(
            back.labels.is_none(),
            "absent labels stay absent, not empty"
        );
    }

    #[test]
    fn model_info_round_trips_loaded_and_unavailable() {
        let loaded = ModelInfo {
            model_id: "gliner2-pii".to_owned(),
            state: ModelState::Ready,
            loaded: Some(LoadedModelInfo {
                input_names: vec!["input_ids".to_owned()],
                output_names: vec!["logits".to_owned()],
                sha256: "b".repeat(64),
                replicas: 2,
                labels: None,
            }),
        };
        assert!(round_trip(&loaded).is_ready());

        // The distinction the type exists to preserve: a model that never
        // loaded carries a state and no `loaded` block, which must not read
        // back as "loaded with nothing in it".
        let unavailable = ModelInfo {
            model_id: "piiranha-v1".to_owned(),
            state: ModelState::Unavailable {
                reason: ModelFailureReason::FileUnreadable,
            },
            loaded: None,
        };
        let back = round_trip(&unavailable);
        assert!(!back.is_ready());
        assert!(back.loaded.is_none());
    }

    #[test]
    fn token_encoding_round_trips_with_offsets() {
        let e = TokenEncoding {
            ids: vec![101, 5, 102],
            attention_mask: vec![1, 1, 1],
            // Byte offsets, so the multi-byte case is the one worth carrying.
            offsets: vec![(0, 0), (0, 12), (0, 0)],
            special_tokens_mask: vec![true, false, true],
        };
        let back = round_trip(&e);
        assert_eq!(back.offsets[1], (0, 12), "offset pairs survive as pairs");
    }
}
