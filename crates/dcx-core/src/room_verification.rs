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

/// Scene-bound expectation. The helper installed today reads only this form.
pub const ROOM_EXPECTATION_V1: &str = "dcx.room-expectation/v1";
/// Preset-bound expectation, one declared file per DCX preset (TIN-5385 step 6).
pub const ROOM_EXPECTATION_V2: &str = "dcx.room-expectation/v2";

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
        if value.schema_version != ROOM_EXPECTATION_V1
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
        {
            return Err(RoomVerificationError::Invalid(
                "invalid room identity, digests or reserve mutes",
            ));
        }
        validate_room_shape(value.output_mutes, &value.profiles)?;
        Ok(value)
    }
}

/// One declared DCX preset image. The file names the preset, the Legalab
/// digests a receipt carries to the LS56 agent and the exact image a device
/// readback must equal. It binds no scene and no scene revision, so every
/// scene that compiles to the same preset and `dcx` block shares one receipt.
///
/// `approved` is false until a named-device readback equals `snapshotDigest`
/// and Legalab records that readback in `basis`. An unapproved file can be
/// compared offline but never admits a refresh or publishes a receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoomExpectationV2 {
    /// Always `dcx.room-expectation/v2`.
    pub schema_version: String,
    /// Legalab preset name in `profiles/dcx-scene-map.json`, such as `practice-room`.
    pub preset_id: String,
    /// Catalog `dcxPresetDigest`: canonical digest of the compiled DCX state.
    /// Required when approved.
    pub preset_digest: Option<String>,
    /// Canonical digest of the catalog scene's compiled `dcx` block. Required when approved.
    pub dcx_block_digest: Option<String>,
    /// Exact whole-device image this preset is on the named device.
    pub snapshot_digest: String,
    /// True only after a device readback equals `snapshot_digest`.
    pub approved: bool,
    /// The readback receipt when approved, or why approval is pending.
    pub basis: String,
    /// Expected physical output mutes, O1 through O6.
    pub output_mutes: [bool; 6],
    /// Typed profile checks supplement complete image equality.
    pub profiles: Vec<DcxOutputProfileV1>,
}

impl RoomExpectationV2 {
    /// Decode a bounded, strict preset expectation and validate nested profiles.
    ///
    /// # Errors
    /// Rejects unknown keys, malformed digests or preset names, an approved
    /// file without both Legalab digests, unsafe reserve mutes and invalid profiles.
    pub fn from_json(bytes: &[u8]) -> Result<Self, RoomVerificationError> {
        if bytes.len() > MAX_ROOM_EXPECTATION_BYTES {
            return Err(RoomVerificationError::Invalid(
                "expectation exceeds size bound",
            ));
        }
        let value: Self = serde_json::from_slice(bytes)?;
        let optional_digests = [&value.preset_digest, &value.dcx_block_digest];
        if value.schema_version != ROOM_EXPECTATION_V2
            || !valid_name(&value.preset_id)
            || !valid_digest(&value.snapshot_digest)
            || !optional_digests
                .iter()
                .all(|digest| digest.as_deref().is_none_or(valid_digest))
            || (value.approved && !optional_digests.iter().all(|digest| digest.is_some()))
            || value.basis.is_empty()
            || value.basis.len() > 512
            || !value.basis.bytes().all(|byte| (0x20..0x7f).contains(&byte))
        {
            return Err(RoomVerificationError::Invalid(
                "invalid preset identity, digests, approval or basis",
            ));
        }
        validate_room_shape(value.output_mutes, &value.profiles)?;
        Ok(value)
    }
}

/// Either expectation form, chosen by its exact `schemaVersion`.
#[derive(Debug, Clone)]
pub enum RoomExpectation {
    /// Scene-bound form, read by installed helpers.
    V1(RoomExpectationV1),
    /// Preset-bound declared form.
    V2(RoomExpectationV2),
}

impl RoomExpectation {
    /// Decode either strict form.
    ///
    /// # Errors
    /// Rejects an unknown schema and every refusal of the selected form.
    pub fn from_json(bytes: &[u8]) -> Result<Self, RoomVerificationError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Header {
            schema_version: String,
        }
        if bytes.len() > MAX_ROOM_EXPECTATION_BYTES {
            return Err(RoomVerificationError::Invalid(
                "expectation exceeds size bound",
            ));
        }
        let header: Header = serde_json::from_slice(bytes)?;
        match header.schema_version.as_str() {
            ROOM_EXPECTATION_V1 => RoomExpectationV1::from_json(bytes).map(Self::V1),
            ROOM_EXPECTATION_V2 => RoomExpectationV2::from_json(bytes).map(Self::V2),
            _ => Err(RoomVerificationError::Invalid(
                "unknown room expectation schema",
            )),
        }
    }

    /// A v1 document has no approval field: its binding in the helper
    /// configuration is the approval. A v2 document states it.
    pub const fn approved(&self) -> bool {
        match self {
            Self::V1(_) => true,
            Self::V2(value) => value.approved,
        }
    }

    fn snapshot_digest(&self) -> &str {
        match self {
            Self::V1(value) => &value.approved_snapshot_digest,
            Self::V2(value) => &value.snapshot_digest,
        }
    }

    fn output_mutes(&self) -> &[bool; 6] {
        match self {
            Self::V1(value) => &value.output_mutes,
            Self::V2(value) => &value.output_mutes,
        }
    }

    fn profiles(&self) -> &[DcxOutputProfileV1] {
        match self {
            Self::V1(value) => &value.profiles,
            Self::V2(value) => &value.profiles,
        }
    }
}

fn validate_room_shape(
    output_mutes: [bool; 6],
    profiles: &[DcxOutputProfileV1],
) -> Result<(), RoomVerificationError> {
    if !output_mutes[4] || !output_mutes[5] || profiles.is_empty() || profiles.len() > 4 {
        return Err(RoomVerificationError::Invalid(
            "invalid reserve mutes or profile count",
        ));
    }
    let mut outputs = BTreeSet::new();
    for profile in profiles {
        profile.validate()?;
        if !outputs.insert(profile.output)
            || profile.state.muted != output_mutes[usize::from(profile.output - 1)]
        {
            return Err(RoomVerificationError::Invalid(
                "duplicate output or contradictory mute",
            ));
        }
    }
    Ok(())
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
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
        if !RoomExpectation::from_json(bytes)?.approved() {
            return Err(RoomVerificationError::Invalid(
                "preset expectation is not approved",
            ));
        }
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

/// Comparison of a preset-bound expectation. It names no scene.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomVerificationV2 {
    /// Always `dcx.room-comparison/v2`.
    pub schema_version: &'static str,
    /// Exact expectation bytes.
    pub expectation_digest: String,
    /// Declared preset name.
    pub preset_id: String,
    /// Catalog preset digest, null while unapproved and undeclared.
    pub preset_digest: Option<String>,
    /// Catalog `dcx` block digest, null while unapproved and undeclared.
    pub dcx_block_digest: Option<String>,
    /// Whether the declared file is approved. A match on an unapproved file
    /// is the readback evidence for approval, never a receipt.
    pub approved: bool,
    /// Actual image identity.
    pub snapshot_digest: String,
    /// Declared image identity.
    pub expected_snapshot_digest: String,
    /// Complete image and all typed checks match.
    pub matches: bool,
    /// Every explicitly requested field was readable.
    pub readable: bool,
    /// Exact-image comparison plus six mute checks and typed profile fields.
    pub compared_fields: usize,
    /// Typed verification details.
    pub profiles: Vec<ProfileVerificationV1>,
}

/// The comparison form follows the expectation form. v1 output is unchanged.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum RoomVerification {
    /// `dcx.room-comparison/v1`, from a v1 expectation.
    V1(RoomVerificationV1),
    /// `dcx.room-comparison/v2`, from a v2 expectation.
    V2(RoomVerificationV2),
}

impl RoomVerification {
    /// Complete image and all typed checks match.
    pub const fn matches(&self) -> bool {
        match self {
            Self::V1(value) => value.matches,
            Self::V2(value) => value.matches,
        }
    }

    /// Digest of the exact expectation bytes compared.
    pub fn expectation_digest(&self) -> &str {
        match self {
            Self::V1(value) => &value.expectation_digest,
            Self::V2(value) => &value.expectation_digest,
        }
    }
}

/// Compare one complete snapshot without device, clock or filesystem access.
///
/// # Errors
/// Rejects malformed expectations or a missing reviewed mute mapping.
pub fn verify_room(
    snapshot: &SnapshotV1,
    bytes: &[u8],
) -> Result<RoomVerification, RoomVerificationError> {
    let expectation = RoomExpectation::from_json(bytes)?;
    let mut mutes_match = true;
    for (index, muted) in expectation.output_mutes().iter().enumerate() {
        let output =
            u8::try_from(index + 1).map_err(|_| RoomVerificationError::Invalid("output index"))?;
        let address = reviewed_address(output_channel(output), MUTE_PARAMETER)
            .ok_or(RoomVerificationError::Invalid("unreviewed output mute"))?;
        mutes_match &= snapshot.read_reviewed(address) == u16::from(*muted);
    }
    let mut profiles = Vec::new();
    for profile in expectation.profiles() {
        let encoded = serde_json::to_vec(profile)?;
        profiles.push(verify_profile(
            snapshot,
            profile,
            &profile_digest(&encoded),
        )?);
    }
    let matches = snapshot.digest() == expectation.snapshot_digest()
        && mutes_match
        && profiles.iter().all(ProfileVerificationV1::matches);
    let compared_fields = 7 + profiles
        .iter()
        .map(|value| value.fields.len())
        .sum::<usize>();
    Ok(match expectation {
        RoomExpectation::V1(expectation) => RoomVerification::V1(RoomVerificationV1 {
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
            compared_fields,
            profiles,
        }),
        RoomExpectation::V2(expectation) => RoomVerification::V2(RoomVerificationV2 {
            schema_version: "dcx.room-comparison/v2",
            expectation_digest: profile_digest(bytes),
            preset_id: expectation.preset_id,
            preset_digest: expectation.preset_digest,
            dcx_block_digest: expectation.dcx_block_digest,
            approved: expectation.approved,
            snapshot_digest: snapshot.digest().to_owned(),
            expected_snapshot_digest: expectation.snapshot_digest,
            matches,
            readable: true,
            compared_fields,
            profiles,
        }),
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
