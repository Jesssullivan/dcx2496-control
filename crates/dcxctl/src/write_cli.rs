//! Typed parameter writes: `write show|plan|verify` are offline; the
//! `live-control` feature adds `write apply|rollback`, which re-validate the
//! carrier against the write policy and then run the ordinary `control
//! apply|rollback` transaction (fresh baseline, stale-plan refusal, one
//! direct-parameter frame, complete readback).

use std::{error::Error, path::Path};

#[cfg(all(feature = "live-control", target_os = "macos"))]
use std::path::PathBuf;

#[cfg(all(feature = "live-control", target_os = "macos"))]
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

pub fn plan(snapshot: &Path, plan: &Path, policy: &PolicyArgs) -> Result<(), Box<dyn Error>> {
    let baseline = load_snapshot(snapshot, "control snapshot")?;
    let plan = WritePlanV1::from_json(&read_bounded(
        plan,
        dcx_core::typed_write::MAX_WRITE_PLAN_BYTES,
        "write plan",
    )?)?;
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

#[cfg(all(feature = "live-control", target_os = "macos"))]
pub fn apply(
    tty: PathBuf,
    expected_device: u8,
    plan_path: &Path,
    policy: &PolicyArgs,
) -> Result<(), Box<dyn Error>> {
    let plan = load_apply_plan(plan_path)?;
    let actions = plan
        .command()
        .map_or(&[][..], dcx_core::DirectParameterCommand::actions);
    dcx_core::typed_write::check_policy(plan.baseline(), actions, &policy.policy())?;
    crate::live_control::apply_loaded(tty, expected_device, &plan)
}

#[cfg(all(feature = "live-control", target_os = "macos"))]
pub fn rollback(tty: PathBuf, expected_device: u8, plan_path: &Path) -> Result<(), Box<dyn Error>> {
    let plan = RollbackPlanV1::from_json(&read_bounded(
        plan_path,
        MAX_DOCUMENT_BYTES,
        "rollback plan",
    )?)?;
    dcx_core::typed_write::check_rollback(&plan)?;
    crate::live_control::rollback_loaded(tty, expected_device, &plan)
}
