//! Strict offline planning plus explicit macOS persistent-session execution.

use std::{error::Error, io, path::Path};

#[cfg(all(feature = "live-control", target_os = "macos"))]
use std::{
    convert::Infallible,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use crate::read_bounded;
#[cfg(all(feature = "live-control", target_os = "macos"))]
use dcx_core::ApplyTransactionState;
use dcx_core::{
    ApplyPlanV1, ApplyTransactionV1, DirectParameterAction, SnapshotV1, layout::reviewed_address,
    peq_bank::DesiredProfile,
};
#[cfg(all(feature = "live-control", target_os = "macos"))]
use dcx_core::{RollbackPlanV1, protocol::DeviceId};
#[cfg(all(feature = "live-control", target_os = "macos"))]
use dcx_darwin_tty::{
    DarwinCarrierError, DarwinSearchSession, PrivateTtyBinding, recover_receive_direct_known_38400,
};
#[cfg(all(feature = "live-control", target_os = "macos"))]
use dcx_transport::{
    RepeatPacer,
    snapshot::{
        ApplyReadbackOutcome, ApplyReadbackReport, ReadbackRetryStop, RollbackReadbackOutcome,
        SnapshotCaptureError, TransactionExecutionError,
        execute_apply_readback_with_snapshot_retry, execute_persistent_snapshot,
        execute_rollback_readback,
    },
};

const MAX_CONTROL_DOCUMENT_BYTES: usize = 256 * 1024;

#[cfg(all(feature = "live-control", target_os = "macos"))]
struct SystemPacer {
    start: Instant,
}

#[cfg(all(feature = "live-control", target_os = "macos"))]
impl SystemPacer {
    fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

#[cfg(all(feature = "live-control", target_os = "macos"))]
impl RepeatPacer for SystemPacer {
    type Error = Infallible;

    fn elapsed(&mut self) -> Duration {
        self.start.elapsed()
    }

    fn wait(&mut self, minimum: Duration) -> Result<(), Self::Error> {
        thread::sleep(minimum);
        Ok(())
    }
}

#[cfg(all(feature = "live-control", target_os = "macos"))]
pub fn recover_receive_direct(tty: PathBuf, expected_device: u8) -> Result<(), Box<dyn Error>> {
    let device = DeviceId::new(expected_device)?;
    let binding = PrivateTtyBinding::new(tty)?;
    let receipt = recover_receive_direct_known_38400(&binding, device)?;
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}

#[cfg(all(feature = "live-control", target_os = "macos"))]
pub fn capture(tty: PathBuf, expected_device: u8) -> Result<(), Box<dyn Error>> {
    let expected_device = DeviceId::new(expected_device)?;
    let binding = PrivateTtyBinding::new(tty)?;
    let session = DarwinSearchSession::open_known_38400(binding)?;
    let mut pacer = SystemPacer::new();
    let captured = execute_persistent_snapshot(session, &mut pacer, expected_device)?;
    println!("{}", serde_json::to_string_pretty(captured.snapshot())?);
    Ok(())
}

/// Approved-target bulk refresh; the caller validates the saved approval first.
#[cfg(all(feature = "live-control", target_os = "macos"))]
pub fn capture_approved(
    tty: PathBuf,
    admission: &dcx_core::room_verification::ApprovedRoomRefreshAdmission,
) -> Result<dcx_transport::snapshot::ApprovedTargetCapture, Box<dyn Error>> {
    let binding = PrivateTtyBinding::new(tty)?;
    let session = DarwinSearchSession::open_known_38400(binding)?;
    let mut pacer = SystemPacer::new();
    match dcx_transport::snapshot::execute_approved_target_snapshot(session, &mut pacer, admission)
    {
        Ok(captured) => Ok(captured),
        Err(error) => {
            let failure = approved_capture_failure_receipt(error.failure());
            let envelope = serde_json::json!({
                "schemaVersion": "dcx.approved-room-refresh/v1",
                "status": "failed",
                "snapshot": null,
                "comparison": null,
                "searchAttempts": error.search_attempts(),
                "searchEmptyTimeouts": error.empty_search_timeouts(),
                "staleSearchRepliesDiscarded": {
                    "dump0": error.stale_search_replies().dump0,
                    "dump1": error.stale_search_replies().dump1,
                },
                "captureElapsedMilliseconds": u64::try_from(error.elapsed().as_millis())?,
                "closeVerified": failure["sessionClosedVerified"],
                "failure": failure,
                "error": error.to_string(),
            });
            let window_line = error
                .failure_window()
                .map(dcx_transport::snapshot::FailureWindow::private_record_line);
            write_approved_failure(
                &mut io::stdout().lock(),
                &mut io::stderr().lock(),
                &envelope,
                window_line.as_deref(),
            )?;
            Err(error.into())
        }
    }
}

pub fn diff(snapshot: &Path, profile: &Path) -> Result<(), Box<dyn Error>> {
    let baseline = SnapshotV1::from_json(&read_bounded(
        snapshot,
        MAX_CONTROL_DOCUMENT_BYTES,
        "control snapshot",
    )?)?;
    let profile = DesiredProfile::from_json(&read_bounded(
        profile,
        MAX_CONTROL_DOCUMENT_BYTES,
        "desired control profile",
    )?)?;
    let desired_actions = profile.actions().to_vec();
    let baseline_actions = baseline.inverse_actions_for(&desired_actions)?;
    let changed_actions = desired_actions
        .iter()
        .zip(&baseline_actions)
        .filter(|(desired, observed)| desired.value() != observed.value())
        .map(|(desired, _)| *desired)
        .collect::<Vec<_>>();
    let changes = if changed_actions.is_empty() {
        Vec::new()
    } else if matches!(profile, DesiredProfile::V1(_)) {
        vec![serde_json::json!({
            "path": "$.outputs[0].peq[8]",
            "before": peq_slot_value(&baseline_actions)?,
            "after": peq_slot_value(&desired_actions)?,
        })]
    } else {
        field_changes(&desired_actions, &baseline_actions)?
    };
    let transaction = if changed_actions.is_empty() {
        ApplyTransactionV1::stage(baseline.clone(), baseline, Vec::new(), Vec::new())?
    } else {
        ApplyTransactionV1::stage_projected(baseline, changed_actions)?
    };
    let output = serde_json::json!({
        "baseline_snapshot_digest": transaction.apply_plan().baseline_snapshot_digest(),
        "desired_profile_schema": profile.schema(),
        "desired_profile_digest": profile.digest(),
        "desired_snapshot_digest": transaction.apply_plan().desired_snapshot_digest(),
        "changes": changes,
        "apply_plan": transaction.apply_plan(),
        "rollback_plan": transaction.rollback_plan(),
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

/// Offline containment gate: every bit that differs between an apply plan's
/// bound baseline and a complete readback must lie inside the plan's projected
/// bits (plus the touched dump trailers), or inside a follower field that
/// moved exactly as a modelled linked crossover predicts; those are listed
/// under `linked_changes`. Returns whether it does.
pub fn verify_containment(plan: &Path, readback: &Path) -> Result<bool, Box<dyn Error>> {
    let plan = ApplyPlanV1::from_json(&read_bounded(
        plan,
        MAX_CONTROL_DOCUMENT_BYTES,
        "apply plan",
    )?)?;
    let readback = SnapshotV1::from_json(&read_bounded(
        readback,
        MAX_CONTROL_DOCUMENT_BYTES,
        "readback snapshot",
    )?)?;
    let uncontained = plan.uncontained_changes(&readback)?;
    let linked = plan.linked_changes(&readback)?;
    let projected = plan
        .projected_masks()
        .into_iter()
        .map(|((section, offset), mask)| {
            serde_json::json!({"section": section, "offset": offset, "mask": mask})
        })
        .collect::<Vec<_>>();
    let contained = uncontained.is_empty();
    let output = serde_json::json!({
        "status": if contained { "contained" } else { "uncontained" },
        "transaction_id": plan.digest(),
        "baseline_snapshot_digest": plan.baseline_snapshot_digest(),
        "desired_snapshot_digest": plan.desired_snapshot_digest(),
        "readback_snapshot_digest": readback.digest(),
        "readback_matches_desired": readback.digest() == plan.desired_snapshot_digest(),
        "projected_offsets": projected,
        "uncontained_changes": uncontained,
        "linked_changes": linked,
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(contained)
}

#[cfg(all(feature = "live-control", target_os = "macos"))]
pub fn apply(tty: PathBuf, expected_device: u8, plan: &Path) -> Result<(), Box<dyn Error>> {
    let plan = ApplyPlanV1::from_json(&read_bounded(
        plan,
        MAX_CONTROL_DOCUMENT_BYTES,
        "apply plan",
    )?)?;
    refuse_typed_lane(plan.command(), "write apply")?;
    apply_loaded(tty, expected_device, &plan).map(|_| ())
}

/// Run one already-validated apply carrier (see [`apply`]). Returns whether
/// the readback matched the desired snapshot; `control apply` keeps its
/// historical exit 0 after printing a `rollback_required` status.
///
/// A post-apply readback that fails with a transport or read error is
/// repeated only as a fresh read-only snapshot on the same exact callout, at
/// most three readbacks in total and 20 s apart (fixed constants in
/// `dcx_transport::snapshot`); no write frame is ever re-sent. The output
/// records `readback_attempts`.
#[cfg(all(feature = "live-control", target_os = "macos"))]
pub fn apply_loaded(
    tty: PathBuf,
    expected_device: u8,
    plan: &ApplyPlanV1,
) -> Result<bool, Box<dyn Error>> {
    let (output, verified) = apply_loaded_report(tty, expected_device, plan)?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(verified)
}

/// [`apply_loaded`] without printing: the apply receipt and whether the
/// readback matched the desired snapshot. `dcxctl profile apply` collects one
/// receipt per chain step.
#[cfg(all(feature = "live-control", target_os = "macos"))]
pub fn apply_loaded_report(
    tty: PathBuf,
    expected_device: u8,
    plan: &ApplyPlanV1,
) -> Result<(serde_json::Value, bool), Box<dyn Error>> {
    let expected = DeviceId::new(expected_device)?;
    require_device(expected, plan.device())?;
    let mut transaction = ApplyTransactionV1::from_apply_plan(plan)?;
    let binding = PrivateTtyBinding::new(tty.clone())?;
    let session = DarwinSearchSession::open_known_38400(binding)?;
    let mut pacer = SystemPacer::new();
    // Each read-only retry re-validates and re-binds the same exact callout.
    let open_readback_session =
        move || DarwinSearchSession::open_known_38400(PrivateTtyBinding::new(tty.clone())?);
    let ApplyReadbackReport {
        result,
        readback_attempts,
        earlier_failures,
        retry_stop,
    } = execute_apply_readback_with_snapshot_retry(
        session,
        &mut pacer,
        &mut transaction,
        open_readback_session,
    );
    let mut readback_failures: Vec<serde_json::Value> = earlier_failures
        .iter()
        .map(capture_failure_receipt)
        .collect();
    let retry_stop = retry_stop.map(ReadbackRetryStop::label);
    let (readback, readback_matches_desired) = match result {
        Ok(ApplyReadbackOutcome::Verified(captured)) => (captured, true),
        Ok(ApplyReadbackOutcome::RollbackRequired(captured)) => (captured, false),
        Err(error) if transaction.state() == ApplyTransactionState::RollbackRequired => {
            readback_failures.push(transaction_failure_receipt(&error));
            let output = serde_json::json!({
                "status": "rollback_required",
                "transaction_id": plan.digest(),
                "baseline_snapshot_digest": plan.baseline_snapshot_digest(),
                "desired_snapshot_digest": plan.desired_snapshot_digest(),
                "readback": null,
                "readback_attempts": readback_attempts,
                "readback_failures": readback_failures,
                "readback_retry_stop": retry_stop,
                "rollback_available": plan.command().is_some(),
                "readback_matches_desired": false,
                "failure": "post_apply_state_uncertain",
            });
            return Ok((output, false));
        }
        Err(error) => return Err(error.into()),
    };
    let output = serde_json::json!({
        "status": if readback_matches_desired { "verified" } else { "rollback_required" },
        "transaction_id": plan.digest(),
        "baseline_snapshot_digest": plan.baseline_snapshot_digest(),
        "desired_snapshot_digest": plan.desired_snapshot_digest(),
        "readback": readback.snapshot(),
        "readback_attempts": readback_attempts,
        "readback_failures": readback_failures,
        "readback_retry_stop": retry_stop,
        "rollback_available": plan.command().is_some(),
        "readback_matches_desired": readback_matches_desired,
    });
    Ok((output, readback_matches_desired))
}

/// Sanitized class of one carrier failure: a fixed label, never bytes or
/// paths.
#[cfg(all(feature = "live-control", target_os = "macos"))]
fn carrier_failure_kind(error: &DarwinCarrierError) -> &'static str {
    match error {
        DarwinCarrierError::UnexpectedTrailingFrame { .. } => "unexpected_trailing_frame",
        DarwinCarrierError::InputRecoveryRequired => "input_recovery_required",
        DarwinCarrierError::Deadline { .. } => "deadline",
        DarwinCarrierError::Overflow { .. } => "overflow",
        DarwinCarrierError::EndOfFile { .. } => "end_of_file",
        DarwinCarrierError::PreexistingInput { .. } => "preexisting_input",
        DarwinCarrierError::ShortWrite { .. } => "short_write",
        DarwinCarrierError::System { .. } => "system",
        DarwinCarrierError::Cleanup { .. } => "cleanup",
        DarwinCarrierError::Binding(_) => "binding",
        _ => "other",
    }
}

/// Sanitized receipt of one failed readback capture: its class, the
/// operation, the carrier failure class, and whether that session closed
/// verified (the precondition for a retry).
#[cfg(all(feature = "live-control", target_os = "macos"))]
fn capture_failure_receipt(
    error: &SnapshotCaptureError<DarwinCarrierError, Infallible>,
) -> serde_json::Value {
    let value = dcx_transport::capture_failure::capture_failure_summary(error);
    serde_json::json!({
        "kind": value.category.as_str(),
        "operation": value.operation.map(|operation| format!("{operation:?}")),
        "carrier": value.carrier.map(carrier_failure_kind),
        "session_closed_verified": value.session_closed_verified,
        "close_failure": value.close_failure.map(carrier_failure_kind),
    })
}

/// Versioned approved-room failure metadata. No Debug text is wire authority.
#[cfg(all(feature = "live-control", target_os = "macos"))]
fn approved_capture_failure_receipt(
    error: &SnapshotCaptureError<DarwinCarrierError, Infallible>,
) -> serde_json::Value {
    let value = dcx_transport::capture_failure::capture_failure_summary(error);
    serde_json::json!({
        "schemaVersion": "dcx.approved-room-capture-failure/v1",
        "category": value.category.as_str(),
        "operation": value.operation.map(dcx_transport::capture_failure::operation_label),
        "received": value.received,
        "sessionClosedVerified": value.session_closed_verified,
        "closeFailure": value.close_failure.map(carrier_failure_kind),
    })
}

/// Sanitized receipt of the final apply failure.
#[cfg(all(feature = "live-control", target_os = "macos"))]
fn transaction_failure_receipt(
    error: &TransactionExecutionError<DarwinCarrierError, Infallible>,
) -> serde_json::Value {
    match error {
        TransactionExecutionError::Capture(capture) => capture_failure_receipt(capture),
        TransactionExecutionError::ReadbackRetryOpen { attempt, source } => serde_json::json!({
            "kind": "retry_open_failed",
            "attempt": attempt,
            "carrier": carrier_failure_kind(source),
        }),
        TransactionExecutionError::ReadbackRetryPacing { attempt, .. } => serde_json::json!({
            "kind": "retry_pacing_failed",
            "attempt": attempt,
        }),
        TransactionExecutionError::Mutation {
            source,
            finish_error,
            ..
        } => serde_json::json!({
            "kind": "mutation",
            "carrier": carrier_failure_kind(source),
            "session_closed_verified": finish_error.is_none(),
        }),
        TransactionExecutionError::Transaction { .. } => serde_json::json!({"kind": "transaction"}),
        TransactionExecutionError::StaleBaseline { .. } => {
            serde_json::json!({"kind": "stale_baseline"})
        }
    }
}

#[cfg(all(feature = "live-control", target_os = "macos"))]
pub fn rollback(tty: PathBuf, expected_device: u8, plan: &Path) -> Result<(), Box<dyn Error>> {
    let plan = RollbackPlanV1::from_json(&read_bounded(
        plan,
        MAX_CONTROL_DOCUMENT_BYTES,
        "rollback plan",
    )?)?;
    refuse_typed_lane(plan.command(), "write rollback")?;
    rollback_loaded(tty, expected_device, &plan).map(|_| ())
}

/// `control apply|rollback` refuse carriers holding a typed-write-lane
/// address (the typed fields and the O1-O3 mutes; see
/// [`dcx_core::layout::ReviewedAddress::is_typed_lane`]). PEQ and O4-mute
/// carriers on O1-O4 still pass here under the core rules only, as before the
/// typed-write lane existed; they get no write-policy check. Typed-lane
/// carriers go only through `dcxctl write apply|rollback`, which no shipped
/// binary builds (`typed-write-live`).
#[cfg(all(feature = "live-control", target_os = "macos"))]
fn refuse_typed_lane(
    command: Option<&dcx_core::DirectParameterCommand>,
    replacement: &str,
) -> Result<(), io::Error> {
    let typed = command
        .map_or(&[][..], dcx_core::DirectParameterCommand::actions)
        .iter()
        .any(|action| {
            reviewed_address(action.channel(), action.parameter())
                .is_some_and(dcx_core::layout::ReviewedAddress::is_typed_lane)
        });
    if typed {
        Err(invalid_input(&format!(
            "carrier holds typed-write-lane addresses; use dcxctl {replacement}"
        )))
    } else {
        Ok(())
    }
}

/// Run one already-validated rollback carrier (see [`rollback`]). Returns
/// whether the restored readback equals the baseline; `control rollback`
/// keeps its historical exit 0 after printing `state_unresolved` with a
/// captured readback.
#[cfg(all(feature = "live-control", target_os = "macos"))]
pub fn rollback_loaded(
    tty: PathBuf,
    expected_device: u8,
    plan: &RollbackPlanV1,
) -> Result<bool, Box<dyn Error>> {
    let expected = DeviceId::new(expected_device)?;
    require_device(expected, plan.baseline().device())?;
    let mut transaction = ApplyTransactionV1::resume_rollback(plan)?;
    let binding = PrivateTtyBinding::new(tty)?;
    let session = DarwinSearchSession::open_known_38400(binding)?;
    let mut pacer = SystemPacer::new();
    let (restored, equals_baseline) =
        match execute_rollback_readback(session, &mut pacer, &mut transaction) {
            Ok(RollbackReadbackOutcome::RolledBack(captured)) => (captured, true),
            Ok(RollbackReadbackOutcome::Faulted(captured)) => (captured, false),
            Err(_) if transaction.state() == ApplyTransactionState::Faulted => {
                let output = serde_json::json!({
                    "status": "state_unresolved",
                    "transaction_id": plan.apply_plan_digest(),
                    "baseline_snapshot_digest": plan.baseline().digest(),
                    "restored": null,
                    "equals_baseline": false,
                    "failure": "post_rollback_state_uncertain",
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
                return Err(io::Error::other("post_rollback_state_uncertain").into());
            }
            Err(error) => return Err(error.into()),
        };
    let output = serde_json::json!({
        "status": if equals_baseline { "rolled_back" } else { "state_unresolved" },
        "transaction_id": plan.apply_plan_digest(),
        "baseline_snapshot_digest": plan.baseline().digest(),
        "restored": restored.snapshot(),
        "equals_baseline": equals_baseline,
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(equals_baseline)
}

fn field_changes(
    desired: &[DirectParameterAction],
    baseline: &[DirectParameterAction],
) -> Result<Vec<serde_json::Value>, io::Error> {
    desired
        .iter()
        .zip(baseline)
        .filter(|(desired, observed)| desired.value() != observed.value())
        .map(|(desired, observed)| {
            let address =
                reviewed_address(desired.channel(), desired.parameter()).ok_or_else(|| {
                    invalid_input("unmapped action escaped desired-profile validation")
                })?;
            Ok(serde_json::json!({
                "output": address.output,
                "field": address.field.label(),
                "channel": desired.channel(),
                "parameter": desired.parameter(),
                "before": observed.value(),
                "after": desired.value(),
            }))
        })
        .collect()
}

fn peq_slot_value(actions: &[DirectParameterAction]) -> Result<serde_json::Value, io::Error> {
    let [frequency, q, gain, kind] = actions else {
        return Err(invalid_input(
            "O1/PEQ9 semantic value requires exactly four actions",
        ));
    };
    for (action, expected_parameter) in actions.iter().zip(0x3b..=0x3e) {
        if action.channel() != 5 || action.parameter() != expected_parameter {
            return Err(invalid_input(
                "unmapped action escaped desired-profile validation",
            ));
        }
    }
    Ok(serde_json::json!({
        "frequencyCode": frequency.value(),
        "qCode": q.value(),
        "gainCode": gain.value(),
        "filterKindCode": kind.value(),
    }))
}

#[cfg(all(feature = "live-control", target_os = "macos"))]
fn require_device(expected: DeviceId, actual: DeviceId) -> Result<(), io::Error> {
    if expected == actual {
        Ok(())
    } else {
        Err(invalid_input(&format!(
            "configured device {} does not match plan device {}",
            expected.get(),
            actual.get()
        )))
    }
}

/// Writes the failed approved-refresh envelope to `stdout`, then the failed
/// window line to `stderr` when the capture kept one. TIN-6964.
///
/// The window bytes go to stderr only. The helper keeps child stderr in its
/// private failure record, and the envelope that it parses from stdout never
/// carries them. The envelope is one line of compact JSON, the same bytes
/// that `println!("{}", envelope)` wrote before, because the helper parses
/// this stdout and records its digest and byte count. A failed stderr write is
/// ignored so it cannot change the result. Only the failure path calls this,
/// so a success writes no line.
#[cfg_attr(
    not(all(feature = "live-control", target_os = "macos")),
    allow(dead_code)
)]
fn write_approved_failure(
    stdout: &mut impl io::Write,
    stderr: &mut impl io::Write,
    envelope: &serde_json::Value,
    window_line: Option<&str>,
) -> io::Result<()> {
    writeln!(stdout, "{}", serde_json::to_string(envelope)?)?;
    if let Some(line) = window_line {
        let _ = writeln!(stderr, "{line}");
    }
    Ok(())
}

fn invalid_input(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::write_approved_failure;
    use std::io;

    // The shape `FailureWindow::private_record_line` writes. The host build
    // of dcxctl has no transport crate, so the line is spelled out here. The
    // hex is the 26-byte synthetic identity frame the transport tests use.
    const WINDOW_HEX: &str = "f0002032000e0053594e5448455449432d4944454e54495459f7";
    const WINDOW_LINE: &str = "{\"schemaVersion\":\"dcx.failure-window-bytes/v1\",\
        \"operation\":\"dump0\",\"sequence\":null,\"failure\":\"validation\",\
        \"received\":26,\"kept\":26,\"truncated\":false,\
        \"hex\":\"f0002032000e0053594e5448455449432d4944454e54495459f7\"}";

    fn envelope() -> serde_json::Value {
        serde_json::json!({
            "schemaVersion": "dcx.approved-room-refresh/v1",
            "status": "failed",
            "snapshot": null,
            "error": "approved-target bulk capture failed",
        })
    }

    struct BrokenWriter;

    impl io::Write for BrokenWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("closed"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("closed"))
        }
    }

    #[test]
    fn window_line_fixture_agrees_with_its_schema() {
        let line: serde_json::Value = serde_json::from_str(WINDOW_LINE).unwrap();
        let hex = line["hex"].as_str().unwrap();
        assert_eq!(hex, WINDOW_HEX);
        assert_eq!(
            line["kept"].as_u64().unwrap() * 2,
            u64::try_from(hex.len()).unwrap()
        );
        assert_eq!(line["received"], line["kept"]);
        assert_eq!(line["truncated"], false);
        assert_eq!(line["failure"], "validation");
    }

    #[test]
    fn failure_window_line_goes_to_stderr_only() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        write_approved_failure(&mut stdout, &mut stderr, &envelope(), Some(WINDOW_LINE)).unwrap();
        let expected = format!("{}\n", serde_json::to_string(&envelope()).unwrap());
        assert_eq!(String::from_utf8(stdout).unwrap(), expected);
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            format!("{WINDOW_LINE}\n")
        );
    }

    #[test]
    fn stdout_envelope_is_the_same_with_or_without_a_window() {
        let mut with_window = Vec::new();
        let mut without_window = Vec::new();
        write_approved_failure(
            &mut with_window,
            &mut Vec::new(),
            &envelope(),
            Some(WINDOW_LINE),
        )
        .unwrap();
        write_approved_failure(&mut without_window, &mut Vec::new(), &envelope(), None).unwrap();
        assert_eq!(with_window, without_window);
        let text = String::from_utf8(with_window).unwrap();
        assert!(!text.contains("failure-window-bytes"), "{text}");
        assert!(!text.contains(WINDOW_HEX), "{text}");
    }

    #[test]
    fn stdout_envelope_matches_the_compact_println_form() {
        // Before this writer, capture_approved printed the envelope with
        // `println!("{}", value)`. The helper parses that stdout, so its
        // bytes stay the same: one compact line and one newline.
        let mut stdout = Vec::new();
        write_approved_failure(&mut stdout, &mut Vec::new(), &envelope(), Some(WINDOW_LINE))
            .unwrap();
        let text = String::from_utf8(stdout).unwrap();
        assert_eq!(text, format!("{}\n", envelope()));
        assert_eq!(text.matches('\n').count(), 1, "{text}");
        assert!(text.ends_with("}\n"), "{text}");
        assert!(!text.contains(": "), "{text}");
        let parsed: serde_json::Value = serde_json::from_str(text.trim_end()).unwrap();
        assert_eq!(parsed, envelope());
    }

    #[test]
    fn no_window_writes_no_stderr_line() {
        let mut stderr = Vec::new();
        write_approved_failure(&mut Vec::new(), &mut stderr, &envelope(), None).unwrap();
        assert!(stderr.is_empty());
    }

    #[test]
    fn a_failed_stderr_write_changes_nothing_else() {
        let mut stdout = Vec::new();
        write_approved_failure(
            &mut stdout,
            &mut BrokenWriter,
            &envelope(),
            Some(WINDOW_LINE),
        )
        .unwrap();
        assert!(!stdout.is_empty());
    }

    #[test]
    fn a_failed_stdout_write_is_returned() {
        let mut stderr = Vec::new();
        let result = write_approved_failure(
            &mut BrokenWriter,
            &mut stderr,
            &envelope(),
            Some(WINDOW_LINE),
        );
        assert!(result.is_err());
    }
}
