//! Stable failure metadata for an already-finished capture, never retry policy.

use std::error::Error;

use dcx_core::protocol::MAX_FRAME_LEN;

use crate::snapshot::{SnapshotCaptureError, SnapshotOperationKind};

/// Closed category names shared by approved-room failure consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFailureCategory {
    /// Search pacing failed.
    Pacing,
    /// Whole capture budget was exhausted.
    BudgetExceeded,
    /// The bounded Search attempts did not produce the required identity count.
    SearchQualificationIncomplete,
    /// The carrier failed an exchange.
    Transport,
    /// A typed telemetry-mode operation failed.
    RemoteMode,
    /// The response deadline expired.
    Timeout,
    /// The response exceeded its bound.
    ResponseLimit,
    /// Frame, identity or section validation failed.
    Validation,
    /// Session restoration or close was not verified.
    Finish,
}

impl CaptureFailureCategory {
    /// Stable wire label, independent of Rust Debug formatting.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pacing => "pacing",
            Self::BudgetExceeded => "budget_exceeded",
            Self::SearchQualificationIncomplete => "search_qualification_incomplete",
            Self::Transport => "transport",
            Self::RemoteMode => "remote_mode",
            Self::Timeout => "timeout",
            Self::ResponseLimit => "response_limit",
            Self::Validation => "validation",
            Self::Finish => "finish",
        }
    }
}

/// Metadata derived from the executor's actual body and consuming finish result.
/// Error references stay private to the caller's explicit carrier-class mapping.
#[derive(Debug)]
pub struct CaptureFailureSummary<'a, E> {
    /// Closed primary category.
    pub category: CaptureFailureCategory,
    /// The operation, when the original failure identifies one.
    pub operation: Option<SnapshotOperationKind>,
    /// Actual response count, omitted if an injected invalid count exceeds bounds.
    pub received: Option<usize>,
    /// Primary carrier error, if this was a carrier or mode failure.
    pub carrier: Option<&'a E>,
    /// True only when the executor's consuming finish succeeded.
    pub session_closed_verified: bool,
    /// Actual finish error, never inferred from the primary category.
    pub close_failure: Option<&'a E>,
}

/// Stable operation label. Search sequence numbers never become wire enum names.
pub const fn operation_label(operation: SnapshotOperationKind) -> &'static str {
    match operation {
        SnapshotOperationKind::Search { .. } => "search",
        SnapshotOperationKind::Dump0 => "dump0",
        SnapshotOperationKind::Dump1 => "dump1",
        SnapshotOperationKind::Meter { .. } => "meter",
    }
}

/// Summarize a completed capture error without changing sequencing or retry rules.
pub fn capture_failure_summary<E, P>(
    error: &SnapshotCaptureError<E, P>,
) -> CaptureFailureSummary<'_, E>
where
    E: Error + Send + Sync + 'static,
    P: Error + Send + Sync + 'static,
{
    let (category, operation, received, carrier, close_failure) = match error {
        SnapshotCaptureError::Pacing { finish_error, .. } => (
            CaptureFailureCategory::Pacing,
            None,
            None,
            None,
            finish_error.as_ref(),
        ),
        SnapshotCaptureError::BudgetExceeded {
            operation,
            finish_error,
        } => (
            CaptureFailureCategory::BudgetExceeded,
            Some(*operation),
            None,
            None,
            finish_error.as_ref(),
        ),
        SnapshotCaptureError::SearchQualificationIncomplete { finish_error, .. } => (
            CaptureFailureCategory::SearchQualificationIncomplete,
            None,
            None,
            None,
            finish_error.as_ref(),
        ),
        SnapshotCaptureError::Transport {
            operation,
            source,
            finish_error,
        } => (
            CaptureFailureCategory::Transport,
            Some(*operation),
            None,
            Some(source),
            finish_error.as_ref(),
        ),
        SnapshotCaptureError::RemoteMode {
            source,
            finish_error,
            ..
        } => (
            CaptureFailureCategory::RemoteMode,
            None,
            None,
            Some(source),
            finish_error.as_ref(),
        ),
        SnapshotCaptureError::Timeout {
            operation,
            received,
            finish_error,
        } => (
            CaptureFailureCategory::Timeout,
            Some(*operation),
            Some(*received),
            None,
            finish_error.as_ref(),
        ),
        SnapshotCaptureError::ResponseLimit {
            operation,
            received,
            finish_error,
            ..
        } => (
            CaptureFailureCategory::ResponseLimit,
            Some(*operation),
            Some(*received),
            None,
            finish_error.as_ref(),
        ),
        SnapshotCaptureError::Validation {
            operation,
            finish_error,
            ..
        } => (
            CaptureFailureCategory::Validation,
            Some(*operation),
            None,
            None,
            finish_error.as_ref(),
        ),
        SnapshotCaptureError::Finish { source } => (
            CaptureFailureCategory::Finish,
            None,
            None,
            Some(source),
            Some(source),
        ),
    };
    CaptureFailureSummary {
        category,
        operation,
        received: received.filter(|count| *count <= MAX_FRAME_LEN),
        carrier,
        session_closed_verified: close_failure.is_none(),
        close_failure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    type Failure = SnapshotCaptureError<io::Error, io::Error>;

    #[test]
    fn dump_zero_timeout_preserves_actual_count_and_verified_finish() {
        let error = Failure::Timeout {
            operation: SnapshotOperationKind::Dump0,
            received: 0,
            finish_error: None,
        };
        let value = capture_failure_summary(&error);
        assert_eq!(value.category.as_str(), "timeout");
        assert_eq!(value.operation.map(operation_label), Some("dump0"));
        assert_eq!(value.received, Some(0));
        assert!(value.session_closed_verified);
        assert!(value.close_failure.is_none());
    }

    #[test]
    fn timeout_with_failed_finish_never_claims_verified_close() {
        let error = Failure::Timeout {
            operation: SnapshotOperationKind::Dump1,
            received: 17,
            finish_error: Some(io::Error::other("SYNTHETIC close")),
        };
        let value = capture_failure_summary(&error);
        assert_eq!(value.received, Some(17));
        assert!(!value.session_closed_verified);
        assert!(value.close_failure.is_some());
    }

    #[test]
    fn finish_failure_cannot_become_success_or_a_primary_timeout() {
        let error = Failure::Finish {
            source: io::Error::other("SYNTHETIC close"),
        };
        let value = capture_failure_summary(&error);
        assert_eq!(value.category.as_str(), "finish");
        assert!(value.operation.is_none());
        assert!(value.received.is_none());
        assert!(!value.session_closed_verified);
        assert!(value.carrier.is_some());
    }

    #[test]
    fn search_and_meter_labels_never_encode_debug_sequences() {
        for sequence in [0, 1, 3, usize::MAX] {
            assert_eq!(
                operation_label(SnapshotOperationKind::Search { sequence }),
                "search"
            );
            assert_eq!(
                operation_label(SnapshotOperationKind::Meter { sequence }),
                "meter"
            );
        }
    }

    #[test]
    fn bounded_counts_remain_exact_and_invalid_injected_counts_are_not_clipped() {
        for received in 0..=MAX_FRAME_LEN + 1 {
            let error = Failure::ResponseLimit {
                operation: SnapshotOperationKind::Dump0,
                received,
                limit: 1015,
                finish_error: None,
            };
            let value = capture_failure_summary(&error);
            assert_eq!(
                value.received,
                (received <= MAX_FRAME_LEN).then_some(received)
            );
        }
    }

    #[test]
    fn all_failure_categories_have_closed_labels_without_debug_text() {
        use dcx_core::{RemoteMode, SnapshotError, protocol::DumpPart};
        let errors: [(Failure, &str); 6] = [
            (
                Failure::Pacing {
                    trial: 2,
                    source: io::Error::other("SYNTHETIC"),
                    finish_error: None,
                },
                "pacing",
            ),
            (
                Failure::BudgetExceeded {
                    operation: SnapshotOperationKind::Dump0,
                    finish_error: None,
                },
                "budget_exceeded",
            ),
            (
                Failure::SearchQualificationIncomplete {
                    valid: 0,
                    required: 1,
                    attempts: 3,
                    finish_error: None,
                },
                "search_qualification_incomplete",
            ),
            (
                Failure::RemoteMode {
                    mode: RemoteMode::Transmit,
                    source: io::Error::other("SYNTHETIC"),
                    finish_error: None,
                },
                "remote_mode",
            ),
            (
                Failure::Validation {
                    operation: SnapshotOperationKind::Dump0,
                    source: SnapshotError::InvalidDumpLength {
                        part: DumpPart::Part0,
                        expected: 1015,
                        actual: 12,
                    },
                    finish_error: None,
                },
                "validation",
            ),
            (
                Failure::ResponseLimit {
                    operation: SnapshotOperationKind::Dump0,
                    received: 1016,
                    limit: 1015,
                    finish_error: None,
                },
                "response_limit",
            ),
        ];
        for (error, expected) in errors {
            let value = capture_failure_summary(&error);
            assert_eq!(value.category.as_str(), expected);
            assert!(value.session_closed_verified);
            assert!(value.close_failure.is_none());
        }
    }

    #[test]
    fn primary_transport_failure_does_not_imply_failed_close() {
        let error = Failure::Transport {
            operation: SnapshotOperationKind::Dump0,
            source: io::Error::other("SYNTHETIC transport"),
            finish_error: None,
        };
        let value = capture_failure_summary(&error);
        assert_eq!(value.category.as_str(), "transport");
        assert!(value.carrier.is_some());
        assert!(value.session_closed_verified);
    }
}
