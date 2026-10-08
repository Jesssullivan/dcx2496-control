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
#[cfg(all(feature = "live-control", target_os = "macos"))]
use dcx_core::{ApplyPlanV1, RollbackPlanV1, protocol::DeviceId};
use dcx_core::{
    ApplyTransactionV1, DirectParameterAction, SnapshotV1, layout::reviewed_address,
    peq_bank::DesiredProfile,
};
#[cfg(all(feature = "live-control", target_os = "macos"))]
use dcx_darwin_tty::{DarwinSearchSession, PrivateTtyBinding, recover_receive_direct_known_38400};
#[cfg(all(feature = "live-control", target_os = "macos"))]
use dcx_transport::{
    RepeatPacer,
    snapshot::{
        ApplyReadbackOutcome, RollbackReadbackOutcome, execute_apply_readback,
        execute_persistent_snapshot, execute_rollback_readback,
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

#[cfg(all(feature = "live-control", target_os = "macos"))]
pub fn apply(tty: PathBuf, expected_device: u8, plan: &Path) -> Result<(), Box<dyn Error>> {
    let plan = ApplyPlanV1::from_json(&read_bounded(
        plan,
        MAX_CONTROL_DOCUMENT_BYTES,
        "apply plan",
    )?)?;
    let expected = DeviceId::new(expected_device)?;
    require_device(expected, plan.device())?;
    let mut transaction = ApplyTransactionV1::from_apply_plan(&plan)?;
    let binding = PrivateTtyBinding::new(tty)?;
    let session = DarwinSearchSession::open_known_38400(binding)?;
    let mut pacer = SystemPacer::new();
    let (readback, readback_matches_desired) =
        match execute_apply_readback(session, &mut pacer, &mut transaction) {
            Ok(ApplyReadbackOutcome::Verified(captured)) => (captured, true),
            Ok(ApplyReadbackOutcome::RollbackRequired(captured)) => (captured, false),
            Err(_) if transaction.state() == ApplyTransactionState::RollbackRequired => {
                let output = serde_json::json!({
                    "status": "rollback_required",
                    "transaction_id": plan.digest(),
                    "baseline_snapshot_digest": plan.baseline_snapshot_digest(),
                    "desired_snapshot_digest": plan.desired_snapshot_digest(),
                    "readback": null,
                    "rollback_available": plan.command().is_some(),
                    "readback_matches_desired": false,
                    "failure": "post_apply_state_uncertain",
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
    let output = serde_json::json!({
        "status": if readback_matches_desired { "verified" } else { "rollback_required" },
        "transaction_id": plan.digest(),
        "baseline_snapshot_digest": plan.baseline_snapshot_digest(),
        "desired_snapshot_digest": plan.desired_snapshot_digest(),
        "readback": readback.snapshot(),
        "rollback_available": plan.command().is_some(),
        "readback_matches_desired": readback_matches_desired,
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

#[cfg(all(feature = "live-control", target_os = "macos"))]
pub fn rollback(tty: PathBuf, expected_device: u8, plan: &Path) -> Result<(), Box<dyn Error>> {
    let plan = RollbackPlanV1::from_json(&read_bounded(
        plan,
        MAX_CONTROL_DOCUMENT_BYTES,
        "rollback plan",
    )?)?;
    let expected = DeviceId::new(expected_device)?;
    require_device(expected, plan.baseline().device())?;
    let mut transaction = ApplyTransactionV1::resume_rollback(&plan)?;
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
    Ok(())
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

fn invalid_input(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}
