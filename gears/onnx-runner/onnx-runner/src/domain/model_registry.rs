use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};

use toolkit_macros::domain_model;

use crate::domain::error::DomainError;

/// Verifies a loaded ONNX session's actual input/output tensor names match
/// what was configured — "transposed-axis protection" (dlp-bench's term):
/// fail loudly at load time rather than silently misinterpreting tensors
/// later. Order-independent (ONNX doesn't guarantee name ordering, and
/// `ort` takes named, not positional, inputs).
pub fn verify_contract(
    model_id: &str,
    expected_inputs: &[String],
    expected_outputs: &[String],
    actual_inputs: &[String],
    actual_outputs: &[String],
) -> Result<(), DomainError> {
    verify_names(model_id, "input", expected_inputs, actual_inputs)?;
    verify_names(model_id, "output", expected_outputs, actual_outputs)?;
    Ok(())
}

fn verify_names(
    model_id: &str,
    kind: &str,
    expected: &[String],
    actual: &[String],
) -> Result<(), DomainError> {
    let mut expected_sorted = expected.to_vec();
    expected_sorted.sort();
    let mut actual_sorted = actual.to_vec();
    actual_sorted.sort();

    if expected_sorted != actual_sorted {
        return Err(DomainError::ContractMismatch {
            model_id: model_id.to_owned(),
            message: format!(
                "{kind} names mismatch: expected {expected_sorted:?}, session has {actual_sorted:?}"
            ),
        });
    }
    Ok(())
}

/// Verifies a computed SHA-256 hex digest matches the configured expected
/// value (case-insensitive).
pub fn verify_checksum(model_id: &str, expected: &str, actual: &str) -> Result<(), DomainError> {
    if expected.eq_ignore_ascii_case(actual) {
        Ok(())
    } else {
        Err(DomainError::ChecksumMismatch {
            model_id: model_id.to_owned(),
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        })
    }
}

/// Limits applied to one call's inputs. Values come from `ModelConfig`; this
/// layer knows nothing about where they were configured.
#[domain_model]
#[derive(Debug, Clone, Copy)]
pub struct InputLimits {
    /// Maximum elements in any one tensor.
    pub elements_per_tensor: usize,
    /// Maximum tensors in one call.
    pub tensors_per_call: usize,
    /// Maximum extent of any single dimension.
    pub extent_per_dimension: usize,
    /// Maximum UTF-8 bytes of text accepted for tokenization.
    pub text_bytes: usize,
}

/// Why an input was rejected. Carries shapes, names and sizes — never tensor
/// contents or tokenized text.
#[domain_model]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputRejection {
    pub tensor: String,
    pub message: String,
}

/// Validates one tensor's declared shape against the data it carries, and
/// against the configured limits.
///
/// Pure arithmetic, deliberately: this is the check that must run *before* any
/// allocation proportional to the declared shape, so it cannot depend on
/// having built anything.
///
/// # Errors
///
/// Returns an [`InputRejection`] naming the tensor and what was wrong.
pub fn validate_tensor(
    name: &str,
    shape: &[usize],
    element_count: usize,
    limits: InputLimits,
) -> Result<(), InputRejection> {
    let reject = |message: String| InputRejection {
        tensor: name.to_owned(),
        message,
    };

    if shape.is_empty() {
        return Err(reject(
            "shape is empty; a tensor needs at least rank 1".to_owned(),
        ));
    }

    for (axis, &dim) in shape.iter().enumerate() {
        if dim == 0 {
            return Err(reject(format!("dimension {axis} is zero")));
        }
        if dim > limits.extent_per_dimension {
            return Err(reject(format!(
                "dimension {axis} is {dim}, over the limit of {}",
                limits.extent_per_dimension
            )));
        }
    }

    // Checked, so a shape whose product overflows is a validation error rather
    // than a wrap into a plausible-looking small number.
    let declared = shape
        .iter()
        .try_fold(1_usize, |acc, &d| acc.checked_mul(d))
        .ok_or_else(|| reject(format!("shape {shape:?} overflows when multiplied out")))?;

    if declared != element_count {
        return Err(reject(format!(
            "shape {shape:?} declares {declared} elements but data has {element_count}"
        )));
    }

    if declared > limits.elements_per_tensor {
        return Err(reject(format!(
            "{declared} elements, over the limit of {}",
            limits.elements_per_tensor
        )));
    }

    Ok(())
}

/// Checks the set of supplied input names against the model's declared
/// contract: every expected name present, and nothing extra.
///
/// # Errors
///
/// Returns a [`DomainError::ContractMismatch`] citing both sets.
pub fn verify_input_names(
    model_id: &str,
    expected: &[String],
    supplied: &[String],
) -> Result<(), DomainError> {
    verify_names(model_id, "input", expected, supplied)
}

/// Round-robin selection across a fixed number of loaded session replicas
/// (see `docs/DESIGN.md`'s "pool of session replicas, not one shared
/// mutex" decision). Pure counter logic — the actual `Session`s live in
/// `infra::ort_session`.
///
/// The replica count is a [`NonZeroUsize`] fixed at construction, so an
/// empty pool is unrepresentable rather than asserted against on every
/// call. Callers build the selector from the pool they just filled, which
/// is what keeps the two in step.
#[domain_model]
pub struct ReplicaSelector {
    next: AtomicUsize,
    replica_count: NonZeroUsize,
}

impl ReplicaSelector {
    #[must_use]
    pub fn new(replica_count: NonZeroUsize) -> Self {
        Self {
            next: AtomicUsize::new(0),
            replica_count,
        }
    }

    /// Returns the index of the next replica to use, always in
    /// `0..replica_count`. Cannot panic: the divisor is non-zero by type.
    #[must_use]
    pub fn next_index(&self) -> usize {
        self.next.fetch_add(1, Ordering::Relaxed) % self.replica_count
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn verify_contract_passes_when_names_match_regardless_of_order() {
        let result = verify_contract(
            "m1",
            &["input_ids".to_owned(), "attention_mask".to_owned()],
            &["logits".to_owned()],
            &["attention_mask".to_owned(), "input_ids".to_owned()],
            &["logits".to_owned()],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn verify_contract_fails_on_missing_input() {
        let result = verify_contract(
            "m1",
            &["input_ids".to_owned(), "attention_mask".to_owned()],
            &["logits".to_owned()],
            &["input_ids".to_owned()],
            &["logits".to_owned()],
        );
        assert!(matches!(result, Err(DomainError::ContractMismatch { .. })));
    }

    #[test]
    fn verify_contract_fails_on_extra_output() {
        let result = verify_contract(
            "m1",
            &["input_ids".to_owned()],
            &["logits".to_owned()],
            &["input_ids".to_owned()],
            &["logits".to_owned(), "extra".to_owned()],
        );
        assert!(matches!(result, Err(DomainError::ContractMismatch { .. })));
    }

    #[test]
    fn verify_checksum_is_case_insensitive() {
        assert!(verify_checksum("m1", "ABCDEF", "abcdef").is_ok());
        assert!(verify_checksum("m1", "abcdef", "abcdef").is_ok());
    }

    #[test]
    fn verify_checksum_fails_on_mismatch() {
        let err = verify_checksum("m1", "abcdef", "123456").unwrap_err();
        assert!(matches!(err, DomainError::ChecksumMismatch { .. }));
    }

    fn nonzero(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).unwrap_or(NonZeroUsize::MIN)
    }

    fn limits() -> InputLimits {
        InputLimits {
            elements_per_tensor: 100,
            tensors_per_call: 4,
            extent_per_dimension: 50,
            text_bytes: 1000,
        }
    }

    #[test]
    fn shape_matching_data_is_accepted() {
        assert!(validate_tensor("input_ids", &[2, 3], 6, limits()).is_ok());
    }

    #[test]
    fn shape_disagreeing_with_data_is_rejected_naming_both() {
        let err = validate_tensor("input_ids", &[2, 3], 5, limits()).unwrap_err();
        assert_eq!(err.tensor, "input_ids");
        assert!(
            err.message.contains('6'),
            "states the declared count: {}",
            err.message
        );
        assert!(
            err.message.contains('5'),
            "states the actual count: {}",
            err.message
        );
    }

    /// The check that must not become a wrap into a plausible small number.
    /// Needs a permissive per-dimension limit, since that guard would
    /// otherwise reject these dimensions before the multiply is reached.
    #[test]
    fn overflowing_shape_is_a_rejection_not_a_panic() {
        let permissive = InputLimits {
            elements_per_tensor: usize::MAX,
            extent_per_dimension: usize::MAX,
            ..limits()
        };
        // Two of these multiply past usize::MAX.
        let huge = 1_usize << (usize::BITS - 2);
        let err = validate_tensor("t", &[huge, 4], 8, permissive).unwrap_err();
        assert!(err.message.contains("overflow"), "{}", err.message);
    }

    #[test]
    fn zero_and_oversized_dimensions_are_rejected() {
        assert!(validate_tensor("t", &[2, 0], 0, limits()).is_err());
        let err = validate_tensor("t", &[51], 51, limits()).unwrap_err();
        assert!(err.message.contains("51"), "{}", err.message);
    }

    #[test]
    fn element_count_over_the_limit_is_rejected() {
        let err = validate_tensor("t", &[20, 10], 200, limits()).unwrap_err();
        assert!(err.message.contains("200"), "{}", err.message);
    }

    #[test]
    fn empty_shape_is_rejected() {
        assert!(validate_tensor("t", &[], 1, limits()).is_err());
    }

    /// Rejection messages reach logs and consumers, so they carry shapes,
    /// names and counts — and cannot carry tensor contents, because
    /// `validate_tensor` is never given them: it takes an element *count*, not
    /// the elements. This test pins the useful half; the safety half holds by
    /// signature.
    #[test]
    fn rejection_messages_name_the_tensor_and_its_shape() {
        let err = validate_tensor("input_ids", &[2, 2], 3, limits()).unwrap_err();
        assert_eq!(err.tensor, "input_ids");
        assert!(
            err.message.contains("[2, 2]"),
            "states the shape: {}",
            err.message
        );
    }

    #[test]
    fn input_names_are_checked_against_the_contract() {
        let expected = ["input_ids".to_owned(), "attention_mask".to_owned()];
        assert!(verify_input_names("m1", &expected, &expected).is_ok());

        let extra = [
            "input_ids".to_owned(),
            "attention_mask".to_owned(),
            "surprise".to_owned(),
        ];
        let err = verify_input_names("m1", &expected, &extra).unwrap_err();
        assert!(matches!(err, DomainError::ContractMismatch { .. }));
        assert!(
            err.to_string().contains("surprise"),
            "cites the offender: {err}"
        );

        let missing = ["input_ids".to_owned()];
        assert!(verify_input_names("m1", &expected, &missing).is_err());
    }

    #[test]
    fn replica_selector_cycles_round_robin() {
        let selector = ReplicaSelector::new(nonzero(3));
        let picks: Vec<usize> = (0..7).map(|_| selector.next_index()).collect();
        assert_eq!(picks, vec![0, 1, 2, 0, 1, 2, 0]);
    }

    #[test]
    fn replica_selector_with_one_replica_always_returns_zero() {
        let selector = ReplicaSelector::new(nonzero(1));
        for _ in 0..5 {
            assert_eq!(selector.next_index(), 0);
        }
    }

    #[test]
    fn replica_selector_stays_in_bounds_across_counter_wrap() {
        let count = nonzero(3);
        let selector = ReplicaSelector::new(count);
        for _ in 0..1000 {
            assert!(selector.next_index() < count.get());
        }
    }
}
