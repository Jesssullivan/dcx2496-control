//! Typed parameter writes: `write show|plan|verify` are offline planning
//! only. The non-default `typed-write-live` feature, which unattended builds
//! never enable (Legalab `dec-dcx-attended-software-writes-20261019` admits
//! attended software writes only), adds `write
//! apply|rollback`: apply rebuilds the carrier from its write plan under the
//! same policy flags and requires an exact match, rollback re-checks its
//! carrier, and both then run the ordinary `control apply|rollback`
//! transaction (fresh baseline, stale-plan refusal, one direct-parameter
//! frame, complete readback) and exit non-zero unless it verified.

use std::{error::Error, path::Path};

#[cfg(all(feature = "typed-write-live", target_os = "macos"))]
use std::path::PathBuf;

#[cfg(all(feature = "typed-write-live", target_os = "macos"))]
use dcx_core::RollbackPlanV1;
use dcx_core::{
    ApplyPlanV1, SnapshotV1,
    typed_write::{
        WritePlanV1, WritePolicyV1, decode_typed_state, plan_writes, verify_written_values,
    },
};

use crate::read_bounded;

const MAX_DOCUMENT_BYTES: usize = 256 * 1024;

/// Command-line policy limits; each can only tighten the defaults.
pub struct PolicyArgs {
    pub allow_routing: bool,
    pub allow_unmute: bool,
    pub allow_protection_change: bool,
    pub gain_ceiling_db: f64,
    pub max_gain_step_db: f64,
    pub max_actions: usize,
}

impl PolicyArgs {
    fn policy(&self) -> WritePolicyV1 {
        WritePolicyV1 {
            max_gain_step_db: self.max_gain_step_db,
            gain_ceiling_db: self.gain_ceiling_db,
            max_actions: self.max_actions,
            allow_routing: self.allow_routing,
            allow_unmute: self.allow_unmute,
            allow_protection_change: self.allow_protection_change,
        }
    }
}

fn load_snapshot(path: &Path, kind: &str) -> Result<SnapshotV1, Box<dyn Error>> {
    Ok(SnapshotV1::from_json(&read_bounded(
        path,
        MAX_DOCUMENT_BYTES,
        kind,
    )?)?)
}

fn load_apply_plan(path: &Path) -> Result<ApplyPlanV1, Box<dyn Error>> {
    Ok(ApplyPlanV1::from_json(&read_bounded(
        path,
        MAX_DOCUMENT_BYTES,
        "apply plan",
    )?)?)
}

pub fn show(snapshot: &Path) -> Result<(), Box<dyn Error>> {
    let snapshot = load_snapshot(snapshot, "control snapshot")?;
    println!(
        "{}",
        serde_json::to_string_pretty(&decode_typed_state(&snapshot)?)?
    );
    Ok(())
}

fn load_write_plan(path: &Path) -> Result<WritePlanV1, Box<dyn Error>> {
    Ok(WritePlanV1::from_json(&read_bounded(
        path,
        dcx_core::typed_write::MAX_WRITE_PLAN_BYTES,
        "write plan",
    )?)?)
}

pub fn plan(snapshot: &Path, plan: &Path, policy: &PolicyArgs) -> Result<(), Box<dyn Error>> {
    let baseline = load_snapshot(snapshot, "control snapshot")?;
    let plan = load_write_plan(plan)?;
    let transaction = plan_writes(&baseline, &plan, &policy.policy())?;
    println!("{}", serde_json::to_string_pretty(&transaction)?);
    Ok(())
}

/// Offline consistency gate. Returns whether the readback is consistent.
pub fn verify(plan: &Path, readback: &Path) -> Result<bool, Box<dyn Error>> {
    let plan = load_apply_plan(plan)?;
    let readback = load_snapshot(readback, "readback snapshot")?;
    let verification = verify_written_values(&plan, &readback)?;
    println!("{}", serde_json::to_string_pretty(&verification)?);
    Ok(verification.status == "consistent")
}

/// Rebuild the carrier from its write plan and the carrier's own baseline,
/// under the same command-line policy, and send it only if the rebuilt
/// carrier is identical. That restores the plan-level consents
/// (`allow_unmute`, `allow_routing`, `allow_protection_change`), the safe
/// in-frame order, and the rule that only carriers `write plan` emits are
/// sent; `check_policy` (inside `plan_writes`) re-checks the actions too.
#[cfg(all(feature = "typed-write-live", target_os = "macos"))]
pub fn apply(
    tty: PathBuf,
    expected_device: u8,
    write_plan_path: &Path,
    plan_path: &Path,
    policy: &PolicyArgs,
) -> Result<(), Box<dyn Error>> {
    let plan = load_apply_plan(plan_path)?;
    let document = load_write_plan(write_plan_path)?;
    let rebuilt = plan_writes(plan.baseline(), &document, &policy.policy())?;
    if rebuilt.apply_plan != plan {
        return Err(std::io::Error::other(
            "apply carrier is not the one write plan emits for this write plan and policy",
        )
        .into());
    }
    if crate::live_control::apply_loaded(tty, expected_device, &plan)? {
        Ok(())
    } else {
        Err(std::io::Error::other("apply did not verify; run dcxctl write rollback").into())
    }
}

#[cfg(all(feature = "typed-write-live", target_os = "macos"))]
pub fn rollback(tty: PathBuf, expected_device: u8, plan_path: &Path) -> Result<(), Box<dyn Error>> {
    let plan = RollbackPlanV1::from_json(&read_bounded(
        plan_path,
        MAX_DOCUMENT_BYTES,
        "rollback plan",
    )?)?;
    dcx_core::typed_write::check_rollback(&plan)?;
    if crate::live_control::rollback_loaded(tty, expected_device, &plan)? {
        Ok(())
    } else {
        Err(std::io::Error::other("rollback readback does not equal the baseline").into())
    }
}
