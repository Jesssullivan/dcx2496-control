//! Offline closed MVP routing commands: bind a `dcx.desired-routing/v1`
//! profile and decode the reviewed routing fields of a snapshot. Neither opens
//! a device; `control diff`, `apply`, and `rollback` carry the live path.

use std::{error::Error, path::Path};

use dcx_core::{
    SnapshotV1,
    routing::{DesiredRoutingProfileV1, InputSum, OutputSource, RoutingTargetsV1, decode_routing},
};

use crate::read_bounded;

const MAX_SNAPSHOT_FILE_BYTES: usize = 256 * 1024;

/// Requested routing targets; `None` leaves a field untouched.
pub struct Targets {
    pub mvp: bool,
    pub o4_mute: Option<bool>,
    pub o5_mute: Option<bool>,
    pub o6_mute: Option<bool>,
    pub input_sum: Option<InputSum>,
    pub o4_source: Option<OutputSource>,
    pub o3_source: Option<OutputSource>,
}

pub fn desired_profile(
    targets: &Targets,
    profile_id: String,
    revision: String,
) -> Result<(), Box<dyn Error>> {
    let targets = if targets.mvp {
        RoutingTargetsV1::mvp()
    } else {
        RoutingTargetsV1 {
            o4_muted: targets.o4_mute,
            o5_muted: targets.o5_mute,
            o6_muted: targets.o6_mute,
            input_sum: targets.input_sum,
            o4_source: targets.o4_source,
            o3_source: targets.o3_source,
        }
    };
    let profile = DesiredRoutingProfileV1::from_targets(profile_id, revision, targets)?;
    println!("{}", serde_json::to_string_pretty(&profile)?);
    Ok(())
}

pub fn inspect(snapshot: &Path) -> Result<(), Box<dyn Error>> {
    let snapshot = SnapshotV1::from_json(&read_bounded(
        snapshot,
        MAX_SNAPSHOT_FILE_BYTES,
        "control snapshot",
    )?)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&decode_routing(&snapshot)?)?
    );
    Ok(())
}
