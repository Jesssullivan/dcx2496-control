//! Legalab output profiles: `profile plan|verify` are offline. The
//! non-default `typed-write-live` feature adds `profile apply`, which
//! re-plans the chain from the same profile, snapshot and policy, refuses
//! unless the chain id matches the reviewed one, and runs each step as one
//! ordinary `write apply` transaction (fresh baseline, stale refusal, one
//! frame, complete readback with the read-only readback retry), stopping at
//! the first step that does not verify. Legalab
//! `dec-scene-dcx-apply-20261021` admits PEQ, gain and crossover diffs under
//! the 2026-10-19 attended-write budget (5 remote-mode writes per apply, 3
//! per rollback).

use std::{error::Error, path::Path};

#[cfg(all(feature = "typed-write-live", target_os = "macos"))]
use std::{path::PathBuf, thread, time::Duration};

use dcx_core::{
    SnapshotV1,
    output_profile::{
        DcxOutputProfileV1, MAX_OUTPUT_PROFILE_BYTES, plan_profile_chain, profile_digest,
        verify_profile,
    },
    typed_write::WritePolicyV1,
};

use crate::read_bounded;

const MAX_SNAPSHOT_BYTES: usize = 256 * 1024;

/// Fixed pause between two chain transactions, the G1 pacing; not caller
/// input.
#[cfg(all(feature = "typed-write-live", target_os = "macos"))]
const INTER_TRANSACTION_GAP: Duration = Duration::from_secs(20);

/// Chain policy limits; each can only tighten the default.
pub struct ChainPolicyArgs {
    pub allow_protection_change: bool,
    pub gain_ceiling_db: f64,
    pub max_gain_step_db: f64,
    pub max_actions: usize,
}

impl ChainPolicyArgs {
    fn policy(&self) -> WritePolicyV1 {
        WritePolicyV1 {
            max_gain_step_db: self.max_gain_step_db,
            gain_ceiling_db: self.gain_ceiling_db,
            max_actions: self.max_actions,
            allow_routing: false,
            allow_unmute: false,
            allow_protection_change: self.allow_protection_change,
        }
    }
}

fn load(
    profile: &Path,
    snapshot: &Path,
) -> Result<(DcxOutputProfileV1, String, SnapshotV1), Box<dyn Error>> {
    let bytes = read_bounded(profile, MAX_OUTPUT_PROFILE_BYTES, "output profile")?;
    let digest = profile_digest(&bytes);
    let profile = DcxOutputProfileV1::from_json(&bytes)?;
    let snapshot = SnapshotV1::from_json(&read_bounded(
        snapshot,
        MAX_SNAPSHOT_BYTES,
        "control snapshot",
    )?)?;
    Ok((profile, digest, snapshot))
}

/// Dry run: print the `dcx.profile-chain/v1` chain. Opens no device.
pub fn plan(
    profile: &Path,
    snapshot: &Path,
    policy: &ChainPolicyArgs,
) -> Result<(), Box<dyn Error>> {
    let (profile, digest, baseline) = load(profile, snapshot)?;
    let chain = plan_profile_chain(&baseline, &profile, &digest, &policy.policy())?;
    println!("{}", serde_json::to_string_pretty(&chain)?);
    Ok(())
}

/// Offline gate. Returns whether every reviewed profiled field matches.
pub fn verify(profile: &Path, snapshot: &Path) -> Result<bool, Box<dyn Error>> {
    let (profile, digest, snapshot) = load(profile, snapshot)?;
    let verification = verify_profile(&snapshot, &profile, &digest)?;
    println!("{}", serde_json::to_string_pretty(&verification)?);
    Ok(verification.matches())
}

/// Steps `from` through `to` (1-based, inclusive) of a chain re-planned from
/// the same profile, snapshot and policy, after an exact chain-id match.
#[cfg(all(feature = "typed-write-live", target_os = "macos"))]
pub struct ApplyArgs<'a> {
    pub tty: PathBuf,
    pub expected_device: u8,
    pub profile: &'a Path,
    pub snapshot: &'a Path,
    pub chain_id: &'a str,
    pub from_step: usize,
    pub to_step: Option<usize>,
}

#[cfg(all(feature = "typed-write-live", target_os = "macos"))]
#[allow(clippy::too_many_lines)]
pub fn apply(args: &ApplyArgs<'_>, policy: &ChainPolicyArgs) -> Result<(), Box<dyn Error>> {
    let (profile, digest, baseline) = load(args.profile, args.snapshot)?;
    let chain = plan_profile_chain(&baseline, &profile, &digest, &policy.policy())?;
    if chain.chain_id != args.chain_id {
        return Err(std::io::Error::other(format!(
            "re-planned chain {} is not the reviewed chain {}; re-plan and review",
            chain.chain_id, args.chain_id
        ))
        .into());
    }
    let steps = chain.steps.len();
    let to_step = args.to_step.unwrap_or(steps);
    if steps == 0 {
        if args.from_step != 1 || args.to_step.is_some_and(|to| to != 0) {
            return Err(std::io::Error::other("the chain has no steps to select").into());
        }
        // Offline: nothing was sent and nothing was read from the device.
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema": "dcx.profile-apply/v1",
                "status": "already_matching",
                "offline": true,
                "chain_id": chain.chain_id,
                "steps": [],
                "next": "nothing was sent; confirm a live snapshot with dcxctl profile verify",
            }))?
        );
        return Ok(());
    }
    if args.from_step == 0 || args.from_step > to_step || to_step > steps {
        return Err(std::io::Error::other(format!(
            "steps {} through {to_step} are outside the chain's 1 through {steps}",
            args.from_step
        ))
        .into());
    }
    let mut receipts = Vec::new();
    let mut last_readback = None;
    for step in &chain.steps[args.from_step - 1..to_step] {
        if !receipts.is_empty() {
            thread::sleep(INTER_TRANSACTION_GAP);
        }
        let carrier = &step.transaction.apply_plan;
        let header = |receipts: Vec<serde_json::Value>, reason: String| {
            serde_json::json!({
                "schema": "dcx.profile-apply/v1",
                "status": "stopped",
                "chain_id": chain.chain_id,
                "stopped_at_step": step.step,
                "reason": reason,
                "steps": receipts,
            })
        };
        match crate::live_control::apply_loaded_report(
            args.tty.clone(),
            args.expected_device,
            carrier,
        ) {
            Ok((receipt, true)) => {
                last_readback = Some(receipt["readback"].clone());
                receipts.push(serde_json::json!({"step": step.step, "receipt": receipt}));
            }
            Ok((receipt, false)) => {
                receipts.push(serde_json::json!({"step": step.step, "receipt": receipt}));
                // The step's write was attempted: its exact rollback applies,
                // but only to this step's own addresses.
                let mut output = header(receipts, "rollback_required".to_owned());
                output["applied"] = "rollback_required".into();
                output["apply_plan"] = serde_json::to_value(carrier)?;
                output["rollback_plan"] = serde_json::to_value(&step.transaction.rollback_plan)?;
                output["rollback_rise_db"] = step.rollback_rise_db.into();
                output["next"] = "take a fresh control snapshot. Equal to this step's baseline: nothing changed, re-plan. Equal to its predicted snapshot: it landed, re-plan from it. Otherwise control verify-containment --plan (the apply_plan above) must pass before write rollback --plan with this rollback_plan".into();
                println!("{}", serde_json::to_string_pretty(&output)?);
                return Err(std::io::Error::other(format!(
                    "chain step {} did not verify; the chain stopped",
                    step.step
                ))
                .into());
            }
            Err(error) => {
                // Every failure after the write starts moves the transaction
                // to rollback_required (the Ok(false) arm above), so an error
                // here came before the write: this step wrote nothing and its
                // rollback must not be run.
                let mut output = header(receipts, error.to_string());
                output["applied"] = "not_applied".into();
                output["next"] = "this step wrote nothing; do not run its rollback. Take a fresh snapshot and re-plan from it".into();
                println!("{}", serde_json::to_string_pretty(&output)?);
                return Err(error);
            }
        }
    }
    // Every run step has written by now: a verification error becomes part
    // of the receipt instead of losing it.
    let (verification, verification_error) = match (to_step == steps, last_readback) {
        (true, Some(readback)) => match serde_json::to_vec(&readback)
            .map_err(|error| error.to_string())
            .and_then(|bytes| SnapshotV1::from_json(&bytes).map_err(|error| error.to_string()))
            .and_then(|readback| {
                verify_profile(&readback, &profile, &digest).map_err(|error| error.to_string())
            }) {
            Ok(verification) => (Some(verification), None),
            Err(error) => (None, Some(error)),
        },
        _ => (None, None),
    };
    let matches = verification_error.is_none()
        && verification
            .as_ref()
            .is_none_or(|verification| verification.matches_holding_mute(chain.held_muted));
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "schema": "dcx.profile-apply/v1",
            "status": if !matches {
                "profile_drift"
            } else if to_step == steps {
                "verified"
            } else {
                "verified_partial"
            },
            "chain_id": chain.chain_id,
            "steps_run": [args.from_step, to_step],
            "held_muted": chain.held_muted,
            "steps": receipts,
            "profile_verification": verification,
            "verification_error": verification_error,
        }))?
    );
    if matches {
        Ok(())
    } else {
        Err(std::io::Error::other("the final readback does not match the profile").into())
    }
}
