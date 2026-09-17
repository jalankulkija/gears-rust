//! ONNX graph generation, for test fixtures and benchmark model ladders.
//!
//! Behind the `fixture-gen` feature so it never ships in the gear: this is a
//! *producer* of models, and the gear is deliberately a consumer that loads
//! only what an operator placed on disk
//! (`cpt-cf-onnx-runner-constraint-no-caller-paths`).
//!
//! # Why hand-rolled protobuf
//!
//! ONNX's own authoring tooling is Python, and this program has none
//! (`cpt-cf-onnx-runner-constraint-no-python`). ONNX *is* protobuf, and the
//! subset needed for these graphs is a few dozen `prost` message fields. The
//! field numbers below come from the ONNX specification and are not ours to
//! choose.
//!
//! # Two kinds of graph
//!
//! - [`tiny_graph`] — the committed test fixture: two inputs, two outputs, two
//!   dtypes, `Identity` nodes. Bit-stable everywhere, so tests assert exact
//!   equality rather than picking a tolerance.
//! - [`compute_graph`] — a chain of `MatMul` layers with sized weights, so
//!   compute cost becomes a *controlled variable* the benchmark can dial
//!   independently of artifact size. A real model's cost is whatever it happens
//!   to be; this one is whatever we ask for.

use prost::Message;

// --- The subset of onnx.proto these graphs need ---------------------------

#[derive(Clone, PartialEq, Message)]
pub struct ModelProto {
    #[prost(int64, tag = "1")]
    pub ir_version: i64,
    #[prost(string, tag = "2")]
    pub producer_name: String,
    #[prost(message, optional, tag = "7")]
    pub graph: Option<GraphProto>,
    #[prost(message, repeated, tag = "8")]
    pub opset_import: Vec<OperatorSetIdProto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct OperatorSetIdProto {
    #[prost(string, tag = "1")]
    pub domain: String,
    #[prost(int64, tag = "2")]
    pub version: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct GraphProto {
    #[prost(message, repeated, tag = "1")]
    pub node: Vec<NodeProto>,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, repeated, tag = "5")]
    pub initializer: Vec<TensorProto>,
    #[prost(message, repeated, tag = "11")]
    pub input: Vec<ValueInfoProto>,
    #[prost(message, repeated, tag = "12")]
    pub output: Vec<ValueInfoProto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct NodeProto {
    #[prost(string, repeated, tag = "1")]
    pub input: Vec<String>,
    #[prost(string, repeated, tag = "2")]
    pub output: Vec<String>,
    #[prost(string, tag = "3")]
    pub name: String,
    #[prost(string, tag = "4")]
    pub op_type: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct TensorProto {
    #[prost(int64, repeated, tag = "1")]
    pub dims: Vec<i64>,
    #[prost(int32, tag = "2")]
    pub data_type: i32,
    #[prost(string, tag = "8")]
    pub name: String,
    #[prost(bytes = "vec", tag = "9")]
    pub raw_data: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ValueInfoProto {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(message, optional, tag = "2")]
    pub r#type: Option<TypeProto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TypeProto {
    #[prost(message, optional, tag = "1")]
    pub tensor_type: Option<TensorTypeProto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TensorTypeProto {
    #[prost(int32, tag = "1")]
    pub elem_type: i32,
    #[prost(message, optional, tag = "2")]
    pub shape: Option<TensorShapeProto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TensorShapeProto {
    #[prost(message, repeated, tag = "1")]
    pub dim: Vec<Dimension>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Dimension {
    #[prost(int64, tag = "1")]
    pub dim_value: i64,
    /// A symbolic dimension, so a graph accepts any batch or length.
    #[prost(string, tag = "2")]
    pub dim_param: String,
}

/// `TensorProto.DataType` values from the ONNX specification.
pub const ELEM_FLOAT: i32 = 1;
pub const ELEM_INT64: i32 = 7;
pub const ELEM_BOOL: i32 = 9;
pub const ELEM_UINT8: i32 = 2;

fn symbolic(dims: &[&str]) -> TensorShapeProto {
    TensorShapeProto {
        dim: dims
            .iter()
            .map(|d| Dimension {
                dim_value: 0,
                dim_param: (*d).to_owned(),
            })
            .collect(),
    }
}

fn value(name: &str, elem_type: i32, dims: &[&str]) -> ValueInfoProto {
    ValueInfoProto {
        name: name.to_owned(),
        r#type: Some(TypeProto {
            tensor_type: Some(TensorTypeProto {
                elem_type,
                shape: Some(symbolic(dims)),
            }),
        }),
    }
}

fn node(op_type: &str, name: &str, inputs: &[&str], outputs: &[&str]) -> NodeProto {
    NodeProto {
        input: inputs.iter().map(|s| (*s).to_owned()).collect(),
        output: outputs.iter().map(|s| (*s).to_owned()).collect(),
        name: name.to_owned(),
        op_type: op_type.to_owned(),
    }
}

fn model(graph: GraphProto) -> ModelProto {
    ModelProto {
        ir_version: 8,
        producer_name: "cf-gears-onnx-runner fixture generator".to_owned(),
        graph: Some(graph),
        opset_import: vec![OperatorSetIdProto {
            domain: String::new(),
            version: 13,
        }],
    }
}

/// The committed test fixture: `ids` (i64) and `weights` (f32) echoed through
/// `Identity` nodes.
///
/// Two names and two dtypes so contract verification and typed extraction are
/// both meaningful; `Identity` so results are bit-stable on every platform.
#[must_use]
pub fn tiny_graph() -> Vec<u8> {
    model(GraphProto {
        node: vec![
            node("Identity", "echo_ids", &["ids"], &["ids_out"]),
            node("Identity", "echo_scores", &["weights"], &["scores"]),
        ],
        name: "cf_gears_test_fixture".to_owned(),
        initializer: Vec::new(),
        input: vec![
            value("ids", ELEM_INT64, &["batch", "seq"]),
            value("weights", ELEM_FLOAT, &["batch", "seq"]),
        ],
        output: vec![
            value("ids_out", ELEM_INT64, &["batch", "seq"]),
            value("scores", ELEM_FLOAT, &["batch", "seq"]),
        ],
    })
    .encode_to_vec()
}

/// A second committed fixture, deliberately unlike [`tiny_graph`] in every
/// dimension a per-model contract can differ along.
///
/// Multi-model hosting is only meaningfully tested when the two models
/// *disagree*: two copies of the same graph would pass a contract-isolation
/// test that a single shared contract would also pass. So this one differs in
/// tensor names (`tokens`/`marks` rather than `ids`/`weights`), in arity (one
/// input, two outputs), and in dtypes (`u8` and `bool`, the two the SDK
/// supports that `tiny_graph` never exercises).
///
/// `Identity` throughout, for the same reason as [`tiny_graph`]: bit-stable on
/// every platform, so tests assert exact equality rather than a tolerance.
#[must_use]
pub fn other_graph() -> Vec<u8> {
    model(GraphProto {
        node: vec![
            node("Identity", "echo_tokens", &["tokens"], &["tokens_out"]),
            node("Identity", "echo_marks", &["marks"], &["marks_out"]),
        ],
        name: "cf_gears_test_fixture_other".to_owned(),
        initializer: Vec::new(),
        input: vec![
            value("tokens", ELEM_UINT8, &["batch", "seq"]),
            value("marks", ELEM_BOOL, &["batch", "seq"]),
        ],
        output: vec![
            value("tokens_out", ELEM_UINT8, &["batch", "seq"]),
            value("marks_out", ELEM_BOOL, &["batch", "seq"]),
        ],
    })
    .encode_to_vec()
}

/// A chain of `MatMul` layers over square weight matrices.
///
/// Compute scales as `batch × dim² × layers`, and artifact size as
/// `dim² × layers × 4` bytes — so the benchmark can hold one constant while
/// varying the other, which is the point of a synthetic model.
///
/// Weights are filled deterministically rather than randomly, so a regenerated
/// ladder is byte-identical and results stay comparable across runs.
///
/// Input `x` is `[batch, dim]`, output `y` is `[batch, dim]`.
#[must_use]
pub fn compute_graph(dim: usize, layers: usize) -> Vec<u8> {
    let mut nodes = Vec::with_capacity(layers);
    let mut initializers = Vec::with_capacity(layers);
    let mut current = "x".to_owned();

    for layer in 0..layers {
        let weight_name = format!("w{layer}");
        let output_name = if layer + 1 == layers {
            "y".to_owned()
        } else {
            format!("h{layer}")
        };

        let dim_i64 = i64::try_from(dim).unwrap_or(i64::MAX);
        initializers.push(TensorProto {
            dims: vec![dim_i64, dim_i64],
            data_type: ELEM_FLOAT,
            name: weight_name.clone(),
            raw_data: weight_bytes(dim, layer),
        });
        nodes.push(node(
            "MatMul",
            &format!("matmul{layer}"),
            &[&current, &weight_name],
            &[&output_name],
        ));
        current = output_name;
    }

    model(GraphProto {
        node: nodes,
        name: format!("cf_gears_compute_d{dim}_l{layers}"),
        initializer: initializers,
        input: vec![value("x", ELEM_FLOAT, &["batch", "dim"])],
        output: vec![value("y", ELEM_FLOAT, &["batch", "dim"])],
    })
    .encode_to_vec()
}

/// Deterministic near-identity weights.
///
/// Near-identity rather than arbitrary: a chain of random matrices drives
/// activations to zero or infinity, and denormal floats are *slower* to
/// multiply on most hardware — which would make the benchmark measure
/// floating-point edge cases instead of matrix throughput.
fn weight_bytes(dim: usize, layer: usize) -> Vec<u8> {
    let mut raw = Vec::with_capacity(dim * dim * 4);
    for row in 0..dim {
        for col in 0..dim {
            let v: f32 = if row == col {
                1.0
            } else {
                // Small, deterministic, and varied per layer so no two weight
                // matrices are identical.
                let seed = u16::try_from((row * 31 + col * 17 + layer * 7) % 1000).unwrap_or(0);
                f32::from(seed) * 1e-6
            };
            raw.extend_from_slice(&v.to_le_bytes());
        }
    }
    raw
}
