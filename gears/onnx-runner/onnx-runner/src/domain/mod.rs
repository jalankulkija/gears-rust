pub mod error;
pub mod metrics;
pub mod model_registry;

pub use error::DomainError;
pub use metrics::{InferenceOutcome, LoadOutcome, NoopMetrics, OnnxRunnerMetricsPort};
pub use model_registry::{
    InputLimits, InputRejection, ReplicaSelector, validate_tensor, verify_checksum,
    verify_contract, verify_input_names,
};
