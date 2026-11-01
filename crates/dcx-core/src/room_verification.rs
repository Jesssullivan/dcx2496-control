//! Read-only admission against an explicitly approved complete device image.
//!
//! Image equality protects bytes whose parameter mapping remains unreviewed.
//! Capturing an image does not approve it. Legalab owns the named-device
//! acceptance that supplies this immutable expectation.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    SnapshotV1,
    layout::{MUTE_PARAMETER, output_channel, reviewed_address},
    output_profile::{DcxOutputProfileV1, ProfileVerificationV1, profile_digest, verify_profile},
    protocol::DeviceId,
};

/// Maximum expectation size, including embedded typed profiles.
pub const MAX_ROOM_EXPECTATION_BYTES: usize = 256 * 1024;

/// A user-approved image and typed profile binding. This document grants no writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoomExpectationV1 {
    /// Always `dcx.room-expectation/v1`.
    pub schema_version: String,
    /// Named Legalab scene.
    pub scene_id: String,
    /// Exact scene revision.
    pub scene_revision: u32,
    /// Exact compiled LS56 scene digest.
    pub compiled_digest: String,
    /// Digest of the shared DCX preset contract.
    pub preset_digest: String,
    /// Complete image accepted through named-device qualification, never auto-capture.
    pub approved_snapshot_digest: String,
    /// Expected physical output mutes, O1 through O6.
    pub output_mutes: [bool; 6],
    /// Typed profile checks supplement complete image equality.
    pub profiles: Vec<DcxOutputProfileV1>,
}

impl RoomExpectationV1 {
    /// Decode a bounded, strict expectation and validate all nested profiles.
    ///
    /// # Errors
    /// Rejects unknown keys, malformed digests, duplicate outputs, unsafe reserve
    /// mutes and invalid typed profiles.
    pub fn from_json(bytes: &[u8]) -> Result<Self, RoomVerificationError> {
        if bytes.len() > MAX_ROOM_EXPECTATION_BYTES {
            return Err(RoomVerificationError::Invalid(
                "expectation exceeds size bound",
            ));
        }
        let value: Self = serde_json::from_slice(bytes)?;
        if value.schema_version != "dcx.room-expectation/v1"
            || value.scene_revision == 0
            || value.scene_id.is_empty()
            || value.scene_id.len() > 96
            || !value
                .scene_id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || ![
                &value.compiled_digest,
                &value.preset_digest,
                &value.approved_snapshot_digest,
            ]
            .into_iter()
            .all(|digest| valid_digest(digest))
            || !value.output_mutes[4]
            || !value.output_mutes[5]
            || value.profiles.is_empty()
            || value.profiles.len() > 4
        {
            return Err(RoomVerificationError::Invalid(
                "invalid room identity, digests or reserve mutes",
            ));
        }
        let mut outputs = BTreeSet::new();
        for profile in &value.profiles {
            profile.validate()?;
            if !outputs.insert(profile.output)
                || profile.state.muted != value.output_mutes[usize::from(profile.output - 1)]
            {
                return Err(RoomVerificationError::Invalid(
                    "duplicate output or contradictory mute",
                ));
            }
        }
        Ok(value)
    }
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256/").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Admission for the explicit approved-target bulk refresh. The caller owns
/// the independent approval and exact tty binding. This type validates the
/// pinned raw document before a transport session may be opened.
#[derive(Debug)]
pub struct ApprovedRoomRefreshAdmission {
    expectation: Vec<u8>,
    expected_device: DeviceId,
}

impl ApprovedRoomRefreshAdmission {
    /// Validate the strict approval and its exact reviewed byte digest.
    ///
    /// # Errors
    /// Rejects malformed or unapproved documents and any digest mismatch.
    pub fn from_json(
        bytes: &[u8],
        expected_digest: &str,
        expected_device: DeviceId,
    ) -> Result<Self, RoomVerificationError> {
        RoomExpectationV1::from_json(bytes)?;
        if !valid_digest(expected_digest) || profile_digest(bytes) != expected_digest {
            return Err(RoomVerificationError::Invalid(
                "approved expectation digest differs",
            ));
        }
        Ok(Self {
            expectation: bytes.to_vec(),
            expected_device,
        })
    }

    /// Exact address supplied by the helper's separately checked target binding.
    pub const fn expected_device(&self) -> DeviceId {
        self.expected_device
    }

    /// The exact approved bytes. Capture never replaces or edits these bytes.
    pub fn expectation(&self) -> &[u8] {
        &self.expectation
    }
}

/// Pure verification result. It has no timestamp or live-device claim.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomVerificationV1 {
    /// Always `dcx.room-comparison/v1`.
    pub schema_version: &'static str,
    /// Exact approved expectation bytes.
    pub expectation_digest: String,
    /// Named scene and revision.
    pub scene_id: String,
    /// Named revision.
    pub scene_revision: u32,
    /// Compiled scene binding.
    pub compiled_digest: String,
    /// Shared room preset binding.
    pub preset_digest: String,
    /// Actual image identity.
    pub snapshot_digest: String,
    /// Approved image identity.
    pub expected_snapshot_digest: String,
    /// Complete image and all typed checks match.
    pub matches: bool,
    /// Every explicitly requested field was readable.
    pub readable: bool,
    /// Exact-image comparison plus six mute checks and typed profile fields.
    pub compared_fields: usize,
    /// Typed verification details, not a claim about unreviewed source maps.
    pub profiles: Vec<ProfileVerificationV1>,
}

/// Compare one complete snapshot without device, clock or filesystem access.
///
/// # Errors
/// Rejects malformed expectations or a missing reviewed mute mapping.
pub fn verify_room(
    snapshot: &SnapshotV1,
    bytes: &[u8],
) -> Result<RoomVerificationV1, RoomVerificationError> {
    let expectation = RoomExpectationV1::from_json(bytes)?;
    let mut mutes_match = true;
    for (index, muted) in expectation.output_mutes.iter().enumerate() {
        let output =
            u8::try_from(index + 1).map_err(|_| RoomVerificationError::Invalid("output index"))?;
        let address = reviewed_address(output_channel(output), MUTE_PARAMETER)
            .ok_or(RoomVerificationError::Invalid("unreviewed output mute"))?;
        mutes_match &= snapshot.read_reviewed(address) == u16::from(*muted);
    }
    let mut profiles = Vec::new();
    for profile in &expectation.profiles {
        let encoded = serde_json::to_vec(profile)?;
        profiles.push(verify_profile(
            snapshot,
            profile,
            &profile_digest(&encoded),
        )?);
    }
    let matches = snapshot.digest() == expectation.approved_snapshot_digest
        && mutes_match
        && profiles.iter().all(ProfileVerificationV1::matches);
    Ok(RoomVerificationV1 {
        schema_version: "dcx.room-comparison/v1",
        expectation_digest: profile_digest(bytes),
        scene_id: expectation.scene_id,
        scene_revision: expectation.scene_revision,
        compiled_digest: expectation.compiled_digest,
        preset_digest: expectation.preset_digest,
        snapshot_digest: snapshot.digest().to_owned(),
        expected_snapshot_digest: expectation.approved_snapshot_digest,
        matches,
        readable: true,
        compared_fields: 7 + profiles
            .iter()
            .map(|value| value.fields.len())
            .sum::<usize>(),
        profiles,
    })
}

/// Read-only room admission errors.
#[derive(Debug, Error)]
pub enum RoomVerificationError {
    /// Strict JSON decoding failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Invalid embedded profile.
    #[error(transparent)]
    Profile(#[from] crate::output_profile::OutputProfileError),
    /// Invalid expectation or unavailable field.
    #[error("room expectation refused: {0}")]
    Invalid(&'static str),
}
