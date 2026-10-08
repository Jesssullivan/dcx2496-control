//! Deterministic, versioned DCX snapshot and fail-closed apply planning.
//!
//! Dump payload semantics remain broadly unestablished on the named device.
//! This module preserves exact validated wire frames and exposes only the
//! closed reviewed projections in [`crate::layout`], transcribed from the
//! pinned MIT `DuinoDCX` `00b9d70` layout: PEQ on/off, PEQ band count, and the
//! nine PEQ bands of every output, plus the O4 output mute. O1/PEQ9 has
//! named-device readback; the O4 mute address is fixture-derived and pending
//! the Legalab WORD-FS-A silent mute-frame rehearsal, O4 PEQ on/off, band
//! count and band 1 frequency/Q/gain/slope have exact device readback
//! (2026-10-08), and every other PEQ address is pending its first exact device
//! readback. Projection preserves the observed modulo-128 balance of the
//! device-maintained Dump0 and Dump1 trailers; every other dump
//! byte remains opaque and unappliable. Apply plans accept only explicit
//! checked direct-parameter actions with cut-only PEQ gains and exact
//! inverses; they never accept caller-supplied frames.

use std::{fmt, fmt::Write as _};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::{
    layout::{
        OutputField, PACKED_PAYLOAD_START, ReviewedAddress, UNITY_GAIN_CODE, reviewed_address,
    },
    protocol::{
        DUMP0_RESPONSE_LEN, DUMP1_RESPONSE_LEN, DecodedMessage, DeviceId, DirectParameterAction,
        DirectParameterCommand, DumpPart, MAX_DIRECT_PARAMETER_ACTIONS, ProtocolError,
        SearchResponse26, decode, parse_frame,
    },
};

/// Wire/schema version of [`SnapshotV1`].
pub const SNAPSHOT_SCHEMA_VERSION: u16 = 1;
/// Wire/schema version of [`SnapshotDiffV1`].
pub const SNAPSHOT_DIFF_SCHEMA_VERSION: u16 = 1;
/// Wire/schema version of [`ApplyPlanV1`] and [`RollbackPlanV1`].
pub const APPLY_PLAN_SCHEMA_VERSION: u16 = 1;
/// Hard input ceiling for one serialized raw snapshot carrier.
pub const MAX_SNAPSHOT_JSON_BYTES: usize = 64 * 1_024;
/// Hard input ceiling for a standalone apply or rollback carrier.
pub const MAX_PLAN_JSON_BYTES: usize = 256 * 1_024;
/// Maximum typed actions in one bounded direct-parameter apply frame.
pub const MAX_APPLY_ACTIONS: usize = MAX_DIRECT_PARAMETER_ACTIONS;

const DIGEST_PREFIX: &str = "sha256/";
const SNAPSHOT_DOMAIN: &[u8] = b"dcx2496.snapshot/v1\0";
const APPLY_PLAN_DOMAIN: &[u8] = b"dcx2496.apply-plan/v1\0";
const ROLLBACK_PLAN_DOMAIN: &[u8] = b"dcx2496.rollback-plan/v1\0";
#[cfg(test)]
const DUMP0_PACKED_PAYLOAD_START: usize = PACKED_PAYLOAD_START;
#[cfg(test)]
const DUMP0_DERIVED_TRAILER_OFFSET: usize = DUMP0_RESPONSE_LEN - 2;

/// One fixed component of a complete DCX snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotSection {
    /// Exact 26-byte Search identity frame.
    Identity,
    /// Exact first dump response frame.
    Dump0,
    /// Exact second dump response frame.
    Dump1,
}

impl SnapshotSection {
    const fn tag(self) -> u8 {
        match self {
            Self::Identity => 0,
            Self::Dump0 => 1,
            Self::Dump1 => 2,
        }
    }
}

/// Exact raw wire image for one snapshot component.
///
/// `Debug` redacts the frame. Serialization intentionally preserves it for a
/// local immutable rollback artifact; callers must not log or commit snapshots
/// captured from hardware.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotImage {
    section: SnapshotSection,
    frame: Vec<u8>,
    digest: String,
}

impl SnapshotImage {
    fn new(section: SnapshotSection, frame: &[u8]) -> Self {
        Self {
            section,
            frame: frame.to_vec(),
            digest: digest_bytes(frame),
        }
    }
}

impl fmt::Debug for SnapshotImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotImage")
            .field("section", &self.section)
            .field("frame_len", &self.frame.len())
            .field("digest", &self.digest)
            .field("frame", &"[redacted]")
            .finish()
    }
}

/// Complete, versioned, immutable DCX wire snapshot.
///
/// The digest covers a domain separator, schema version, device ID, section
/// tags, lengths, and exact raw frames in identity/Dump0/Dump1 order. It does
/// not depend on JSON key ordering or serializer behavior.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotV1 {
    schema_version: u16,
    device_id: DeviceId,
    identity: SnapshotImage,
    dump0: SnapshotImage,
    dump1: SnapshotImage,
    snapshot_digest: String,
}

impl SnapshotV1 {
    /// Validate three exact response frames and construct a complete snapshot.
    ///
    /// # Errors
    ///
    /// Fails for malformed frames, wrong response functions or parts, or any
    /// identity mismatch across Search, Dump0, and Dump1.
    pub fn from_frames(
        identity_frame: &[u8],
        dump0_frame: &[u8],
        dump1_frame: &[u8],
    ) -> Result<Self, SnapshotError> {
        let identity = SearchResponse26::parse(identity_frame)?;
        let device_id = identity.device();
        validate_dump(dump0_frame, device_id, DumpPart::Part0)?;
        validate_dump(dump1_frame, device_id, DumpPart::Part1)?;

        let identity = SnapshotImage::new(SnapshotSection::Identity, identity_frame);
        let dump0 = SnapshotImage::new(SnapshotSection::Dump0, dump0_frame);
        let dump1 = SnapshotImage::new(SnapshotSection::Dump1, dump1_frame);
        let snapshot_digest = snapshot_digest(device_id, [&identity, &dump0, &dump1]);
        Ok(Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            device_id,
            identity,
            dump0,
            dump1,
            snapshot_digest,
        })
    }

    /// Parse a serialized snapshot and revalidate every frame and digest.
    ///
    /// # Errors
    ///
    /// Fails closed for malformed JSON, unknown fields, unsupported schema,
    /// mismatched metadata, invalid frames, or any digest mismatch.
    pub fn from_json(bytes: &[u8]) -> Result<Self, SnapshotError> {
        if bytes.len() > MAX_SNAPSHOT_JSON_BYTES {
            return Err(SnapshotError::JsonTooLarge(bytes.len()));
        }
        let wire: SnapshotWire = serde_json::from_slice(bytes)?;
        if wire.schema_version != SNAPSHOT_SCHEMA_VERSION {
            return Err(SnapshotError::UnsupportedSchema(wire.schema_version));
        }
        validate_wire_image(&wire.identity, SnapshotSection::Identity)?;
        validate_wire_image(&wire.dump0, SnapshotSection::Dump0)?;
        validate_wire_image(&wire.dump1, SnapshotSection::Dump1)?;

        let snapshot =
            Self::from_frames(&wire.identity.frame, &wire.dump0.frame, &wire.dump1.frame)?;
        if snapshot.device_id != wire.device_id {
            return Err(SnapshotError::DeviceMismatch {
                section: SnapshotSection::Identity,
                expected: wire.device_id.get(),
                actual: snapshot.device_id.get(),
            });
        }
        require_digest("identity", &snapshot.identity.digest, &wire.identity.digest)?;
        require_digest("dump0", &snapshot.dump0.digest, &wire.dump0.digest)?;
        require_digest("dump1", &snapshot.dump1.digest, &wire.dump1.digest)?;
        require_digest("snapshot", &snapshot.snapshot_digest, &wire.snapshot_digest)?;
        Ok(snapshot)
    }

    /// Serialize a local snapshot artifact without affecting its digest.
    ///
    /// # Errors
    ///
    /// Returns a serializer error if the in-memory representation cannot be
    /// encoded, which is an invariant failure for this fixed schema.
    pub fn to_json(&self) -> Result<Vec<u8>, SnapshotError> {
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > MAX_SNAPSHOT_JSON_BYTES {
            return Err(SnapshotError::JsonTooLarge(bytes.len()));
        }
        Ok(bytes)
    }

    /// Snapshot schema version, always one for this type.
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Exact device address shared by all three responses.
    pub const fn device(&self) -> DeviceId {
        self.device_id
    }

    /// Canonical SHA-256 digest of the complete ordered snapshot.
    pub fn digest(&self) -> &str {
        &self.snapshot_digest
    }

    /// Exact validated raw frame for a section.
    ///
    /// Hardware-derived bytes may contain device-specific material and must
    /// remain in bounded local state rather than logs or repository fixtures.
    pub fn frame(&self, section: SnapshotSection) -> &[u8] {
        &self.image(section).frame
    }

    /// SHA-256 digest of one exact section frame.
    pub fn section_digest(&self, section: SnapshotSection) -> &str {
        &self.image(section).digest
    }

    /// Compare an observed snapshot with an exact desired snapshot.
    ///
    /// # Errors
    ///
    /// Different physical addresses are not a diff; they are a binding error.
    pub fn diff(&self, desired: &Self) -> Result<SnapshotDiffV1, SnapshotDiffError> {
        if self.device_id != desired.device_id {
            return Err(SnapshotDiffError::DeviceMismatch {
                observed: self.device_id.get(),
                desired: desired.device_id.get(),
            });
        }
        let mut changes = Vec::with_capacity(3);
        for section in [
            SnapshotSection::Identity,
            SnapshotSection::Dump0,
            SnapshotSection::Dump1,
        ] {
            let observed = self.image(section);
            let desired = desired.image(section);
            if observed.frame != desired.frame {
                changes.push(SnapshotSectionChange {
                    section,
                    observed_digest: observed.digest.clone(),
                    desired_digest: desired.digest.clone(),
                    observed_len: observed.frame.len(),
                    desired_len: desired.frame.len(),
                });
            }
        }
        Ok(SnapshotDiffV1 {
            schema_version: SNAPSHOT_DIFF_SCHEMA_VERSION,
            device_id: self.device_id,
            observed_snapshot_digest: self.snapshot_digest.clone(),
            desired_snapshot_digest: desired.snapshot_digest.clone(),
            changes,
        })
    }

    /// Project the reviewed direct actions into an exact desired dump.
    ///
    /// This is intentionally not a general dump mapper. It accepts only the
    /// closed allowlist in [`crate::layout::reviewed_address`]: PEQ on/off,
    /// PEQ band count, and the nine PEQ bands on outputs O1 through O6, plus
    /// the O4 output mute (fixture-derived, pending hardware confirmation by
    /// the Legalab WORD-FS-A rehearsal). Each value must lie inside the
    /// documented device domain of its field. Every other address fails
    /// closed. The identity bytes remain unchanged, both dump trailers keep
    /// their baseline modulo-128 balance, and a fresh snapshot digest is
    /// calculated.
    ///
    /// # Errors
    ///
    /// Rejects empty, duplicate, unmapped, or out-of-range projected actions,
    /// plus any invariant failure while rebuilding the exact snapshot.
    pub fn project_direct_actions(
        &self,
        actions: &[DirectParameterAction],
    ) -> Result<Self, SnapshotProjectionError> {
        if actions.is_empty() {
            return Err(SnapshotProjectionError::EmptyActions);
        }
        let mut addresses = std::collections::BTreeSet::new();
        let mut dump0 = self.dump0.frame.clone();
        let mut dump1 = self.dump1.frame.clone();
        for action in actions {
            if !addresses.insert((action.channel(), action.parameter())) {
                return Err(SnapshotProjectionError::DuplicateAction {
                    channel: action.channel(),
                    parameter: action.parameter(),
                });
            }
            let address = require_reviewed(*action)?;
            require_device_domain(*action, address.field, action.value())?;
            let frame = match address.location.part() {
                DumpPart::Part0 => &mut dump0,
                DumpPart::Part1 => &mut dump1,
            };
            address.location.write(frame, action.value()).map_err(|_| {
                SnapshotProjectionError::ValueTooWide {
                    parameter: action.parameter(),
                    value: action.value(),
                }
            })?;
        }
        preserve_trailer_balance(&self.dump0.frame, &mut dump0);
        // DUMP1 TRAILER: the named-device modulo-128 balance is observed for
        // Dump0 and, since the 2026-10-08 O4 round trips, for Dump1 (byte 909
        // read back exactly). A projected Dump1 keeps its baseline balance; an
        // unchanged payload leaves the trailer byte unchanged. Any future
        // inexact readback still only forces the bound rollback.
        preserve_trailer_balance(&self.dump1.frame, &mut dump1);
        Ok(Self::from_frames(&self.identity.frame, &dump0, &dump1)?)
    }

    /// Extract exact inverse values for a reviewed action address set.
    ///
    /// Returned actions preserve caller order but replace each value with the
    /// value decoded from this immutable snapshot at the transcribed location.
    /// A baseline value outside the documented device domain is evidence
    /// against the transcription and fails closed. This supports rollback
    /// derivation without caller-supplied inverse bytes.
    ///
    /// # Errors
    ///
    /// Rejects empty, duplicate, or unreviewed action addresses and baseline
    /// values outside the reviewed device domain.
    pub fn inverse_actions_for(
        &self,
        actions: &[DirectParameterAction],
    ) -> Result<Vec<DirectParameterAction>, SnapshotProjectionError> {
        if actions.is_empty() {
            return Err(SnapshotProjectionError::EmptyActions);
        }
        let mut addresses = std::collections::BTreeSet::new();
        let mut inverse = Vec::with_capacity(actions.len());
        for action in actions {
            if !addresses.insert((action.channel(), action.parameter())) {
                return Err(SnapshotProjectionError::DuplicateAction {
                    channel: action.channel(),
                    parameter: action.parameter(),
                });
            }
            let address = require_reviewed(*action)?;
            let value = self.read_reviewed(address);
            require_device_domain(*action, address.field, value)?;
            inverse.push(DirectParameterAction::new(
                action.channel(),
                action.parameter(),
                value,
            )?);
        }
        Ok(inverse)
    }

    /// Exact rollback actions for an apply action list.
    ///
    /// The inverse values come from [`Self::inverse_actions_for`]; the order
    /// is reversed so a rollback first restores the PEQ enable and band count
    /// and only then restores band values that are again inactive.
    ///
    /// # Errors
    ///
    /// Same as [`Self::inverse_actions_for`].
    pub fn rollback_actions_for(
        &self,
        actions: &[DirectParameterAction],
    ) -> Result<Vec<DirectParameterAction>, SnapshotProjectionError> {
        let mut inverse = self.inverse_actions_for(actions)?;
        inverse.reverse();
        Ok(inverse)
    }

    /// Decode one reviewed address from this snapshot without domain checks.
    pub fn read_reviewed(&self, address: ReviewedAddress) -> u16 {
        let frame = match address.location.part() {
            DumpPart::Part0 => &self.dump0.frame,
            DumpPart::Part1 => &self.dump1.frame,
        };
        address.location.read(frame)
    }

    fn image(&self, section: SnapshotSection) -> &SnapshotImage {
        match section {
            SnapshotSection::Identity => &self.identity,
            SnapshotSection::Dump0 => &self.dump0,
            SnapshotSection::Dump1 => &self.dump1,
        }
    }
}

fn require_reviewed(
    action: DirectParameterAction,
) -> Result<ReviewedAddress, SnapshotProjectionError> {
    reviewed_address(action.channel(), action.parameter()).ok_or(
        SnapshotProjectionError::UnmappedAction {
            channel: action.channel(),
            parameter: action.parameter(),
        },
    )
}

fn require_device_domain(
    action: DirectParameterAction,
    field: OutputField,
    value: u16,
) -> Result<(), SnapshotProjectionError> {
    if value <= field.device_max() {
        Ok(())
    } else if field.is_switch() {
        Err(SnapshotProjectionError::ValueNotOnOff {
            parameter: action.parameter(),
            value,
        })
    } else {
        Err(SnapshotProjectionError::ValueOutOfRange {
            channel: action.channel(),
            parameter: action.parameter(),
            value,
            maximum: field.device_max(),
        })
    }
}

fn preserve_trailer_balance(before: &[u8], after: &mut [u8]) {
    // Named firmware 1.17 evidence and the independent pinned `domenut`
    // `97cdcca` Dump0 establish that the packed payload plus its penultimate
    // trailer preserves one modulo-128 balance across direct edits. Preserve
    // the baseline's own balance rather than assigning meaning to opaque data.
    // Dump1 reuses the rule as the documented hypothesis above.
    let trailer = after.len() - 2;
    let baseline_balance = seven_bit_sum(&before[PACKED_PAYLOAD_START..=trailer]);
    let projected_payload = seven_bit_sum(&after[PACKED_PAYLOAD_START..trailer]);
    after[trailer] = baseline_balance.wrapping_sub(projected_payload) & 0x7f;
}

fn seven_bit_sum(bytes: &[u8]) -> u8 {
    bytes
        .iter()
        .fold(0_u8, |sum, byte| sum.wrapping_add(*byte) & 0x7f)
}

impl fmt::Debug for SnapshotV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotV1")
            .field("schema_version", &self.schema_version)
            .field("device_id", &self.device_id)
            .field("identity", &self.identity)
            .field("dump0", &self.dump0)
            .field("dump1", &self.dump1)
            .field("snapshot_digest", &self.snapshot_digest)
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotWire {
    schema_version: u16,
    device_id: DeviceId,
    identity: SnapshotImageWire,
    dump0: SnapshotImageWire,
    dump1: SnapshotImageWire,
    snapshot_digest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotImageWire {
    section: SnapshotSectionWire,
    frame: Vec<u8>,
    digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SnapshotSectionWire {
    Identity,
    Dump0,
    Dump1,
}

impl From<SnapshotSectionWire> for SnapshotSection {
    fn from(value: SnapshotSectionWire) -> Self {
        match value {
            SnapshotSectionWire::Identity => Self::Identity,
            SnapshotSectionWire::Dump0 => Self::Dump0,
            SnapshotSectionWire::Dump1 => Self::Dump1,
        }
    }
}

fn validate_wire_image(
    image: &SnapshotImageWire,
    expected: SnapshotSection,
) -> Result<(), SnapshotError> {
    let actual = SnapshotSection::from(image.section);
    if actual == expected {
        Ok(())
    } else {
        Err(SnapshotError::WrongSerializedSection { expected, actual })
    }
}

fn validate_dump(
    frame: &[u8],
    expected_device: DeviceId,
    expected_part: DumpPart,
) -> Result<(), SnapshotError> {
    let expected_len = match expected_part {
        DumpPart::Part0 => DUMP0_RESPONSE_LEN,
        DumpPart::Part1 => DUMP1_RESPONSE_LEN,
    };
    if frame.len() != expected_len {
        return Err(SnapshotError::InvalidDumpLength {
            part: expected_part,
            expected: expected_len,
            actual: frame.len(),
        });
    }
    match decode(parse_frame(frame)?)? {
        DecodedMessage::DumpResponse { device, part, .. } => {
            if device != expected_device {
                return Err(SnapshotError::DeviceMismatch {
                    section: match expected_part {
                        DumpPart::Part0 => SnapshotSection::Dump0,
                        DumpPart::Part1 => SnapshotSection::Dump1,
                    },
                    expected: expected_device.get(),
                    actual: device.get(),
                });
            }
            if part != expected_part {
                return Err(SnapshotError::WrongDumpPart {
                    expected: expected_part,
                    actual: part,
                });
            }
            Ok(())
        }
        other => Err(SnapshotError::UnexpectedMessage {
            section: match expected_part {
                DumpPart::Part0 => SnapshotSection::Dump0,
                DumpPart::Part1 => SnapshotSection::Dump1,
            },
            kind: message_kind(&other),
        }),
    }
}

const fn message_kind(message: &DecodedMessage) -> &'static str {
    match message {
        DecodedMessage::SearchResponse(_) => "search_response",
        DecodedMessage::PingResponse { .. } => "ping_response",
        DecodedMessage::DumpResponse { .. } => "dump_response",
        DecodedMessage::DirectParameters { .. } => "direct_parameters",
        DecodedMessage::Unknown(_) => "unknown",
    }
}

fn digest_bytes(bytes: &[u8]) -> String {
    digest_hasher(Sha256::digest(bytes))
}

fn snapshot_digest(device: DeviceId, images: [&SnapshotImage; 3]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(SNAPSHOT_DOMAIN);
    hasher.update(SNAPSHOT_SCHEMA_VERSION.to_be_bytes());
    hasher.update([device.get()]);
    for image in images {
        hasher.update([image.section.tag()]);
        let length = u32::try_from(image.frame.len()).expect("protocol frame length fits u32");
        hasher.update(length.to_be_bytes());
        hasher.update(&image.frame);
    }
    digest_hasher(hasher.finalize())
}

fn digest_hasher(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut digest = String::with_capacity(DIGEST_PREFIX.len() + bytes.len() * 2);
    digest.push_str(DIGEST_PREFIX);
    for byte in bytes {
        write!(&mut digest, "{byte:02x}").expect("writing to String cannot fail");
    }
    digest
}

fn require_digest(field: &'static str, expected: &str, actual: &str) -> Result<(), SnapshotError> {
    if expected == actual {
        Ok(())
    } else {
        Err(SnapshotError::DigestMismatch {
            field,
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        })
    }
}

/// Snapshot construction and strict decoding failures.
#[derive(Debug, Error)]
pub enum SnapshotError {
    /// A frame failed bounded protocol validation.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// JSON was malformed or outside the strict schema.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Serialized raw snapshot exceeded its bounded carrier limit.
    #[error("snapshot JSON has {0} bytes; maximum is 65536")]
    JsonTooLarge(usize),
    /// Serialized snapshot schema is not supported.
    #[error("unsupported snapshot schema version {0}")]
    UnsupportedSchema(u16),
    /// A dump response or serialized identity belonged to another device.
    #[error("{section:?} device mismatch: expected {expected}, found {actual}")]
    DeviceMismatch {
        /// Component in which the mismatch occurred.
        section: SnapshotSection,
        /// Bound address.
        expected: u8,
        /// Observed address.
        actual: u8,
    },
    /// A dump response declared the other segment.
    #[error("wrong dump part: expected {expected:?}, found {actual:?}")]
    WrongDumpPart {
        /// Required segment.
        expected: DumpPart,
        /// Received segment.
        actual: DumpPart,
    },
    /// Complete dump segments have fixed part-specific frame lengths.
    #[error("{part:?} dump has {actual} bytes; expected exactly {expected}")]
    InvalidDumpLength {
        /// Expected dump part.
        part: DumpPart,
        /// Exact required total frame length.
        expected: usize,
        /// Received total frame length.
        actual: usize,
    },
    /// A section contained a valid but wrong message class.
    #[error("unexpected {kind} in {section:?} snapshot section")]
    UnexpectedMessage {
        /// Expected section.
        section: SnapshotSection,
        /// Conservative decoded class.
        kind: &'static str,
    },
    /// Serialized section label did not match its fixed position.
    #[error("wrong serialized section: expected {expected:?}, found {actual:?}")]
    WrongSerializedSection {
        /// Required section label.
        expected: SnapshotSection,
        /// Supplied section label.
        actual: SnapshotSection,
    },
    /// Serialized digest did not match recomputed exact bytes.
    #[error("{field} digest mismatch: expected {expected}, found {actual}")]
    DigestMismatch {
        /// Digest-bearing field.
        field: &'static str,
        /// Recomputed digest.
        expected: String,
        /// Serialized digest.
        actual: String,
    },
}

/// One deterministic desired-versus-observed section change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotSectionChange {
    section: SnapshotSection,
    observed_digest: String,
    desired_digest: String,
    observed_len: usize,
    desired_len: usize,
}

impl SnapshotSectionChange {
    /// Changed section.
    pub const fn section(&self) -> SnapshotSection {
        self.section
    }

    /// Digest of the observed raw frame.
    pub fn observed_digest(&self) -> &str {
        &self.observed_digest
    }

    /// Digest of the desired raw frame.
    pub fn desired_digest(&self) -> &str {
        &self.desired_digest
    }

    /// Observed frame length.
    pub const fn observed_len(&self) -> usize {
        self.observed_len
    }

    /// Desired frame length.
    pub const fn desired_len(&self) -> usize {
        self.desired_len
    }
}

/// Typed, ordered, deterministic desired-versus-observed snapshot diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotDiffV1 {
    schema_version: u16,
    device_id: DeviceId,
    observed_snapshot_digest: String,
    desired_snapshot_digest: String,
    changes: Vec<SnapshotSectionChange>,
}

impl SnapshotDiffV1 {
    /// Whether observed and desired are byte-for-byte identical.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Ordered section changes: identity, Dump0, then Dump1.
    pub fn changes(&self) -> &[SnapshotSectionChange] {
        &self.changes
    }

    /// Observed complete snapshot digest.
    pub fn observed_snapshot_digest(&self) -> &str {
        &self.observed_snapshot_digest
    }

    /// Desired complete snapshot digest.
    pub fn desired_snapshot_digest(&self) -> &str {
        &self.desired_snapshot_digest
    }
}

/// Snapshot diff binding failures.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SnapshotDiffError {
    /// Desired state was captured from a different device address.
    #[error("snapshot device mismatch: observed {observed}, desired {desired}")]
    DeviceMismatch { observed: u8, desired: u8 },
}

/// Fail-closed errors from the reviewed dump projection.
#[derive(Debug, Error)]
pub enum SnapshotProjectionError {
    /// Projection requires at least one explicit action.
    #[error("snapshot projection action set cannot be empty")]
    EmptyActions,
    /// Address is outside the closed reviewed allowlist.
    #[error("unmapped snapshot action channel {channel}, parameter {parameter:#04x}")]
    UnmappedAction { channel: u8, parameter: u8 },
    /// One opaque address cannot be projected twice.
    #[error("duplicate snapshot action channel {channel}, parameter {parameter:#04x}")]
    DuplicateAction { channel: u8, parameter: u8 },
    /// A low-only field cannot represent a high-bit value.
    #[error("parameter {parameter:#04x} cannot project value {value} into a seven-bit field")]
    ValueTooWide { parameter: u8, value: u16 },
    /// A reviewed on/off field carries only zero or one.
    #[error("parameter {parameter:#04x} reviewed on/off field cannot carry value {value}")]
    ValueNotOnOff { parameter: u8, value: u16 },
    /// A reviewed field value lies outside its documented device domain.
    #[error(
        "channel {channel} parameter {parameter:#04x} value {value} exceeds device maximum {maximum}"
    )]
    ValueOutOfRange {
        channel: u8,
        parameter: u8,
        value: u16,
        maximum: u16,
    },
    /// A reconstructed typed inverse could not be represented.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// Reconstructed snapshot failed exact validation.
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
}

/// Closed, bounded apply plan bound to exact observed and desired snapshots.
///
/// Actions are explicit opaque wire-level tuples, not inferred semantic
/// mappings. The only outbound representation is [`DirectParameterCommand`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyPlanV1 {
    schema_version: u16,
    device_id: DeviceId,
    baseline: SnapshotV1,
    desired: SnapshotV1,
    baseline_snapshot_digest: String,
    desired_snapshot_digest: String,
    command: Option<DirectParameterCommand>,
    plan_digest: String,
}

impl ApplyPlanV1 {
    /// Bind explicit typed actions to exact baseline and desired snapshots.
    ///
    /// This function does not infer action semantics from opaque dump bytes.
    /// The caller must supply actions from a separately evidenced semantic
    /// mapping. Empty actions are accepted only for exact snapshot equality.
    ///
    /// # Errors
    ///
    /// Rejects another device, identity changes, missing or surplus actions,
    /// and any invalid direct-parameter command.
    pub fn new(
        baseline: &SnapshotV1,
        desired: &SnapshotV1,
        actions: Vec<DirectParameterAction>,
    ) -> Result<Self, ApplyPlanError> {
        let diff = baseline.diff(desired)?;
        if diff
            .changes
            .iter()
            .any(|change| change.section == SnapshotSection::Identity)
        {
            return Err(ApplyPlanError::IdentityChangeUnappliable);
        }
        let command = if diff.is_empty() {
            if actions.is_empty() {
                None
            } else {
                return Err(ApplyPlanError::ActionsWithoutChanges(actions.len()));
            }
        } else if actions.is_empty() {
            return Err(ApplyPlanError::MissingActions(
                diff.changes.iter().map(|change| change.section).collect(),
            ));
        } else {
            let command = DirectParameterCommand::new(diff.device_id, actions)?;
            require_cut_only(command.actions())?;
            let projected = baseline.project_direct_actions(command.actions())?;
            if projected != *desired {
                return Err(ApplyPlanError::DesiredProjectionMismatch {
                    projected: projected.digest().to_owned(),
                    desired: desired.digest().to_owned(),
                });
            }
            Some(command)
        };
        let plan_digest = plan_digest(
            APPLY_PLAN_DOMAIN,
            diff.device_id,
            &diff.observed_snapshot_digest,
            &diff.desired_snapshot_digest,
            command.as_ref(),
        );
        Ok(Self {
            schema_version: APPLY_PLAN_SCHEMA_VERSION,
            device_id: diff.device_id,
            baseline: baseline.clone(),
            desired: desired.clone(),
            baseline_snapshot_digest: diff.observed_snapshot_digest.clone(),
            desired_snapshot_digest: diff.desired_snapshot_digest.clone(),
            command,
            plan_digest,
        })
    }

    /// Bound device address.
    pub const fn device(&self) -> DeviceId {
        self.device_id
    }

    /// Complete immutable baseline carried by the plan.
    pub const fn baseline(&self) -> &SnapshotV1 {
        &self.baseline
    }

    /// Complete exact desired readback carried by the plan.
    pub const fn desired(&self) -> &SnapshotV1 {
        &self.desired
    }

    /// Exact baseline digest against which the plan was produced.
    pub fn baseline_snapshot_digest(&self) -> &str {
        &self.baseline_snapshot_digest
    }

    /// Exact desired readback digest.
    pub fn desired_snapshot_digest(&self) -> &str {
        &self.desired_snapshot_digest
    }

    /// Exact checked direct command, or `None` for equality/no-op.
    pub const fn command(&self) -> Option<&DirectParameterCommand> {
        self.command.as_ref()
    }

    /// Number of typed mutation actions.
    pub fn action_count(&self) -> usize {
        self.command
            .as_ref()
            .map_or(0, |command| command.actions().len())
    }

    /// Canonical digest binding snapshots and ordered typed actions.
    pub fn digest(&self) -> &str {
        &self.plan_digest
    }

    /// Serialize the strict durable apply carrier.
    ///
    /// # Errors
    ///
    /// Returns a JSON encoding error only for an in-memory invariant failure.
    pub fn to_json(&self) -> Result<Vec<u8>, PlanCarrierError> {
        bounded_plan_json(self)
    }

    /// Parse and fully revalidate a durable apply carrier.
    ///
    /// Both complete snapshots are reparsed, every action is reconstructed by
    /// its checked constructor, and all redundant bindings/digests must match a
    /// freshly rebuilt plan.
    ///
    /// # Errors
    ///
    /// Rejects malformed/unknown JSON, unsupported schema, invalid snapshots,
    /// unchecked actions, binding drift, or digest tampering.
    pub fn from_json(bytes: &[u8]) -> Result<Self, PlanCarrierError> {
        require_plan_json_bound(bytes)?;
        let wire: ApplyPlanWire = serde_json::from_slice(bytes)?;
        require_plan_schema("apply", wire.schema_version)?;
        let baseline = snapshot_from_value(&wire.baseline)?;
        let desired = snapshot_from_value(&wire.desired)?;
        let actions = actions_from_wire(wire.command, wire.device_id)?;
        let rebuilt = Self::new(&baseline, &desired, actions)?;
        require_carrier_field(
            "device_id",
            &wire.device_id.get().to_string(),
            &rebuilt.device_id.get().to_string(),
        )?;
        require_carrier_field(
            "baseline_snapshot_digest",
            &wire.baseline_snapshot_digest,
            &rebuilt.baseline_snapshot_digest,
        )?;
        require_carrier_field(
            "desired_snapshot_digest",
            &wire.desired_snapshot_digest,
            &rebuilt.desired_snapshot_digest,
        )?;
        require_carrier_field("plan_digest", &wire.plan_digest, &rebuilt.plan_digest)?;
        Ok(rebuilt)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyPlanWire {
    schema_version: u16,
    device_id: DeviceId,
    baseline: serde_json::Value,
    desired: serde_json::Value,
    baseline_snapshot_digest: String,
    desired_snapshot_digest: String,
    command: Option<DirectCommandWire>,
    plan_digest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectCommandWire {
    device: DeviceId,
    actions: Vec<DirectActionWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectActionWire {
    channel: u8,
    parameter: u8,
    value: u16,
}

fn actions_from_wire(
    command: Option<DirectCommandWire>,
    expected_device: DeviceId,
) -> Result<Vec<DirectParameterAction>, PlanCarrierError> {
    let Some(command) = command else {
        return Ok(Vec::new());
    };
    if command.device != expected_device {
        return Err(PlanCarrierError::CommandDeviceMismatch {
            expected: expected_device.get(),
            actual: command.device.get(),
        });
    }
    let actions = command
        .actions
        .into_iter()
        .map(|action| DirectParameterAction::new(action.channel, action.parameter, action.value))
        .collect::<Result<Vec<_>, _>>()?;
    DirectParameterCommand::new(expected_device, actions.clone())?;
    Ok(actions)
}

fn require_cut_only(actions: &[DirectParameterAction]) -> Result<(), ApplyPlanError> {
    for action in actions {
        let boost = reviewed_address(action.channel(), action.parameter())
            .is_some_and(|address| address.field.is_gain() && action.value() > UNITY_GAIN_CODE);
        if boost {
            return Err(ApplyPlanError::BoostNotAdmitted {
                channel: action.channel(),
                parameter: action.parameter(),
                value: action.value(),
            });
        }
    }
    Ok(())
}

fn snapshot_from_value(value: &serde_json::Value) -> Result<SnapshotV1, PlanCarrierError> {
    Ok(SnapshotV1::from_json(&serde_json::to_vec(value)?)?)
}

fn bounded_plan_json(value: &impl Serialize) -> Result<Vec<u8>, PlanCarrierError> {
    let bytes = serde_json::to_vec(value)?;
    require_plan_json_bound(&bytes)?;
    Ok(bytes)
}

fn require_plan_json_bound(bytes: &[u8]) -> Result<(), PlanCarrierError> {
    if bytes.len() > MAX_PLAN_JSON_BYTES {
        Err(PlanCarrierError::JsonTooLarge(bytes.len()))
    } else {
        Ok(())
    }
}

fn require_plan_schema(kind: &'static str, schema: u16) -> Result<(), PlanCarrierError> {
    if schema == APPLY_PLAN_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(PlanCarrierError::UnsupportedSchema { kind, schema })
    }
}

fn require_carrier_field(
    field: &'static str,
    actual: &str,
    expected: &str,
) -> Result<(), PlanCarrierError> {
    if actual == expected {
        Ok(())
    } else {
        Err(PlanCarrierError::BindingMismatch {
            field,
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        })
    }
}

/// Fail-closed apply planning errors.
#[derive(Debug, Error)]
pub enum ApplyPlanError {
    /// Snapshot binding failed.
    #[error(transparent)]
    Diff(#[from] SnapshotDiffError),
    /// The Search identity changed and cannot be written by direct parameters.
    #[error("identity changes cannot be applied by direct-parameter actions")]
    IdentityChangeUnappliable,
    /// Opaque dump changes require an explicit externally evidenced mapping.
    #[error("changed sections require explicit typed actions: {0:?}")]
    MissingActions(Vec<SnapshotSection>),
    /// Actions were supplied although desired and baseline are already equal.
    #[error("{0} actions were supplied for an empty snapshot diff")]
    ActionsWithoutChanges(usize),
    /// A checked direct command could not be built.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// Reviewed snapshot projection failed.
    #[error(transparent)]
    Projection(#[from] SnapshotProjectionError),
    /// The apply path admits only PEQ cuts; boosts have no apply lane.
    #[error(
        "channel {channel} parameter {parameter:#04x} gain code {value} is a boost; apply is cut-only"
    )]
    BoostNotAdmitted {
        channel: u8,
        parameter: u8,
        value: u16,
    },
    /// Explicit typed actions did not project to the exact desired snapshot.
    #[error("typed action projection {projected} did not match desired snapshot {desired}")]
    DesiredProjectionMismatch { projected: String, desired: String },
    /// Apply and rollback must address exactly the same opaque parameters.
    #[error("rollback parameter addresses do not match apply parameter addresses")]
    RollbackAddressMismatch,
    /// A changing plan must carry explicit inverse actions.
    #[error("non-empty apply plan requires explicit rollback actions")]
    MissingRollbackActions,
    /// A no-op apply cannot carry rollback actions.
    #[error("{0} rollback actions were supplied for a no-op apply")]
    RollbackActionsWithoutApply(usize),
    /// Serialized rollback baseline did not equal the apply baseline.
    #[error("rollback baseline does not match the bound apply baseline")]
    RollbackBaselineMismatch,
    /// Explicit inverse actions did not restore the immutable baseline.
    #[error("typed rollback projection {projected} did not match baseline {baseline}")]
    RollbackProjectionMismatch { projected: String, baseline: String },
}

/// Strict durable apply/rollback carrier failures.
#[derive(Debug, Error)]
pub enum PlanCarrierError {
    /// JSON was malformed or outside the strict schema.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Serialized plan exceeded its bounded carrier limit.
    #[error("plan JSON has {0} bytes; maximum is 262144")]
    JsonTooLarge(usize),
    /// A carried snapshot failed full frame/digest validation.
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
    /// A carried plan failed closed construction.
    #[error(transparent)]
    Plan(#[from] ApplyPlanError),
    /// A carried direct action was invalid.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// Apply/rollback carrier schema is unsupported.
    #[error("unsupported {kind} plan schema version {schema}")]
    UnsupportedSchema { kind: &'static str, schema: u16 },
    /// Redundant durable binding did not match reconstruction.
    #[error("{field} binding mismatch: expected {expected}, found {actual}")]
    BindingMismatch {
        /// Mismatched field.
        field: &'static str,
        /// Recomputed value.
        expected: String,
        /// Serialized value.
        actual: String,
    },
    /// Nested command addressed another device.
    #[error("command device mismatch: expected {expected}, found {actual}")]
    CommandDeviceMismatch { expected: u8, actual: u8 },
}

/// Immutable rollback binding retained before any apply begins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackPlanV1 {
    schema_version: u16,
    apply_plan: ApplyPlanV1,
    apply_plan_digest: String,
    command: Option<DirectParameterCommand>,
    plan_digest: String,
}

impl RollbackPlanV1 {
    fn new(
        baseline: &SnapshotV1,
        apply: &ApplyPlanV1,
        actions: Vec<DirectParameterAction>,
    ) -> Result<Self, ApplyPlanError> {
        if baseline != &apply.baseline {
            return Err(ApplyPlanError::RollbackBaselineMismatch);
        }
        let command = match apply.command() {
            None if actions.is_empty() => None,
            None => return Err(ApplyPlanError::RollbackActionsWithoutApply(actions.len())),
            Some(_) if actions.is_empty() => return Err(ApplyPlanError::MissingRollbackActions),
            Some(apply_command) => {
                let rollback = DirectParameterCommand::new(apply.device_id, actions)?;
                let mut apply_addresses: Vec<_> = apply_command
                    .actions()
                    .iter()
                    .map(|action| (action.channel(), action.parameter()))
                    .collect();
                let mut rollback_addresses: Vec<_> = rollback
                    .actions()
                    .iter()
                    .map(|action| (action.channel(), action.parameter()))
                    .collect();
                apply_addresses.sort_unstable();
                rollback_addresses.sort_unstable();
                if apply_addresses != rollback_addresses {
                    return Err(ApplyPlanError::RollbackAddressMismatch);
                }
                let projected = apply.desired.project_direct_actions(rollback.actions())?;
                if projected != *baseline {
                    return Err(ApplyPlanError::RollbackProjectionMismatch {
                        projected: projected.digest().to_owned(),
                        baseline: baseline.digest().to_owned(),
                    });
                }
                Some(rollback)
            }
        };
        let plan_digest = plan_digest(
            ROLLBACK_PLAN_DOMAIN,
            baseline.device_id,
            apply.digest(),
            baseline.digest(),
            command.as_ref(),
        );
        Ok(Self {
            schema_version: APPLY_PLAN_SCHEMA_VERSION,
            apply_plan: apply.clone(),
            apply_plan_digest: apply.digest().to_owned(),
            command,
            plan_digest,
        })
    }

    /// Exact immutable pre-apply snapshot.
    pub const fn baseline(&self) -> &SnapshotV1 {
        self.apply_plan.baseline()
    }

    /// Complete apply plan to which this standalone inverse is bound.
    pub const fn apply_plan(&self) -> &ApplyPlanV1 {
        &self.apply_plan
    }

    /// Exact checked inverse direct command, or `None` for equality/no-op.
    pub const fn command(&self) -> Option<&DirectParameterCommand> {
        self.command.as_ref()
    }

    /// Number of typed inverse actions.
    pub fn action_count(&self) -> usize {
        self.command
            .as_ref()
            .map_or(0, |command| command.actions().len())
    }

    /// Canonical digest binding baseline and ordered inverse actions.
    pub fn digest(&self) -> &str {
        &self.plan_digest
    }

    /// Digest of the exact apply plan this inverse is bound to.
    pub fn apply_plan_digest(&self) -> &str {
        &self.apply_plan_digest
    }

    /// Serialize the strict durable rollback carrier.
    ///
    /// # Errors
    ///
    /// Returns a JSON encoding error only for an in-memory invariant failure.
    pub fn to_json(&self) -> Result<Vec<u8>, PlanCarrierError> {
        bounded_plan_json(self)
    }

    /// Parse and fully revalidate a standalone rollback carrier.
    ///
    /// # Errors
    ///
    /// Rejects malformed/unknown JSON, unsupported schema, invalid baseline,
    /// unchecked inverse actions, address-set drift, or digest tampering.
    pub fn from_json(bytes: &[u8]) -> Result<Self, PlanCarrierError> {
        require_plan_json_bound(bytes)?;
        let wire: RollbackPlanWire = serde_json::from_slice(bytes)?;
        require_plan_schema("rollback", wire.schema_version)?;
        let apply = ApplyPlanV1::from_json(&serde_json::to_vec(&wire.apply_plan)?)?;
        let actions = actions_from_wire(wire.command, apply.device())?;
        let rebuilt = Self::new(apply.baseline(), &apply, actions)?;
        require_carrier_field(
            "apply_plan_digest",
            &wire.apply_plan_digest,
            &rebuilt.apply_plan_digest,
        )?;
        require_carrier_field("plan_digest", &wire.plan_digest, &rebuilt.plan_digest)?;
        Ok(rebuilt)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RollbackPlanWire {
    schema_version: u16,
    apply_plan: serde_json::Value,
    apply_plan_digest: String,
    command: Option<DirectCommandWire>,
    plan_digest: String,
}

fn plan_digest(
    domain: &[u8],
    device: DeviceId,
    from_digest: &str,
    to_digest: &str,
    command: Option<&DirectParameterCommand>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(APPLY_PLAN_SCHEMA_VERSION.to_be_bytes());
    hasher.update([device.get()]);
    hasher.update(from_digest.as_bytes());
    hasher.update(to_digest.as_bytes());
    if let Some(command) = command {
        for action in command.actions() {
            hasher.update([action.channel(), action.parameter()]);
            hasher.update(action.value().to_be_bytes());
        }
    }
    digest_hasher(hasher.finalize())
}

/// Exact desired/readback comparison receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReadbackVerificationV1 {
    device_id: DeviceId,
    expected_snapshot_digest: String,
    observed_snapshot_digest: String,
    exact: bool,
}

impl ReadbackVerificationV1 {
    fn compare(expected: &SnapshotV1, observed: &SnapshotV1) -> Result<Self, SnapshotDiffError> {
        if expected.device_id != observed.device_id {
            return Err(SnapshotDiffError::DeviceMismatch {
                observed: observed.device_id.get(),
                desired: expected.device_id.get(),
            });
        }
        Ok(Self {
            device_id: expected.device_id,
            expected_snapshot_digest: expected.snapshot_digest.clone(),
            observed_snapshot_digest: observed.snapshot_digest.clone(),
            exact: expected == observed,
        })
    }

    /// Whether every exact identity and dump byte matched.
    pub const fn is_exact(&self) -> bool {
        self.exact
    }

    /// Expected complete snapshot digest.
    pub fn expected_snapshot_digest(&self) -> &str {
        &self.expected_snapshot_digest
    }

    /// Observed complete snapshot digest.
    pub fn observed_snapshot_digest(&self) -> &str {
        &self.observed_snapshot_digest
    }
}

/// Lifecycle of one exact-snapshot apply/readback/rollback transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyTransactionState {
    /// Immutable baseline, desired state, and plans are bound.
    Staged,
    /// The caller reports allowlisted steps are being executed.
    Applying,
    /// Apply readback exactly matched desired state.
    Verified,
    /// Apply readback differed; rollback to the immutable baseline is required.
    RollbackRequired,
    /// The caller reports rollback steps are being executed.
    RollingBack,
    /// Complete rollback readback exactly matched the baseline.
    RolledBack,
    /// Rollback readback did not restore exact baseline equality.
    Faulted,
}

/// Pure state holder binding apply and rollback to exact snapshots.
#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyTransactionV1 {
    state: ApplyTransactionState,
    baseline: SnapshotV1,
    desired: SnapshotV1,
    apply_plan: ApplyPlanV1,
    rollback_plan: RollbackPlanV1,
    apply_readback: Option<ReadbackVerificationV1>,
    rollback_readback: Option<ReadbackVerificationV1>,
}

impl ApplyTransactionV1 {
    /// Bind an immutable baseline, exact desired state, and closed plans.
    ///
    /// # Errors
    ///
    /// Fails for another device, an identity change, any changed snapshot
    /// without explicit typed actions/inverses, or invalid action bindings.
    pub fn stage(
        baseline: SnapshotV1,
        desired: SnapshotV1,
        apply_actions: Vec<DirectParameterAction>,
        rollback_actions: Vec<DirectParameterAction>,
    ) -> Result<Self, ApplyTransactionError> {
        let apply_plan = ApplyPlanV1::new(&baseline, &desired, apply_actions)?;
        let rollback_plan = RollbackPlanV1::new(&baseline, &apply_plan, rollback_actions)?;
        Ok(Self {
            state: ApplyTransactionState::Staged,
            baseline,
            desired,
            apply_plan,
            rollback_plan,
            apply_readback: None,
            rollback_readback: None,
        })
    }

    /// Derive desired state and exact inverse values from one immutable baseline.
    ///
    /// This is the coherent reviewed mapping path: apply actions are projected
    /// into the desired snapshot, inverse actions are decoded from the baseline,
    /// and both plans are then bound and validated by [`Self::stage`].
    ///
    /// # Errors
    ///
    /// Rejects empty, duplicate, unreviewed, or invalid actions and any plan
    /// binding failure.
    pub fn stage_projected(
        baseline: SnapshotV1,
        apply_actions: Vec<DirectParameterAction>,
    ) -> Result<Self, ApplyTransactionError> {
        let desired = baseline
            .project_direct_actions(&apply_actions)
            .map_err(ApplyPlanError::from)?;
        let rollback_actions = baseline
            .rollback_actions_for(&apply_actions)
            .map_err(ApplyPlanError::from)?;
        Self::stage(baseline, desired, apply_actions, rollback_actions)
    }

    /// Reconstruct a staged transaction from one fully validated apply plan.
    ///
    /// Exact inverse values are decoded from the immutable carried baseline.
    /// The resulting plan must equal the supplied plan byte-for-byte at the
    /// typed representation level; no caller-provided mutation state is added.
    ///
    /// # Errors
    ///
    /// Rejects a plan whose reviewed actions cannot be inverted or whose
    /// complete reconstruction differs from the supplied plan.
    pub fn from_apply_plan(apply_plan: &ApplyPlanV1) -> Result<Self, ApplyTransactionError> {
        let apply_actions = apply_plan
            .command()
            .map_or_else(Vec::new, |command| command.actions().to_vec());
        let rollback_actions = if apply_actions.is_empty() {
            Vec::new()
        } else {
            apply_plan
                .baseline()
                .rollback_actions_for(&apply_actions)
                .map_err(ApplyPlanError::from)?
        };
        let transaction = Self::stage(
            apply_plan.baseline().clone(),
            apply_plan.desired().clone(),
            apply_actions,
            rollback_actions,
        )?;
        if transaction.apply_plan != *apply_plan {
            return Err(ApplyTransactionError::PlanReconstructionMismatch { kind: "apply" });
        }
        Ok(transaction)
    }

    /// Reconstruct a transaction directly at the rollback-required boundary.
    ///
    /// This consumes no synthetic apply event. Both nested plans are rebuilt
    /// from their complete snapshots and checked actions, then required to
    /// exactly equal the standalone durable rollback carrier before the state
    /// is moved from `Staged` to `RollbackRequired`.
    ///
    /// # Errors
    ///
    /// Rejects any apply or rollback reconstruction mismatch.
    pub fn resume_rollback(rollback_plan: &RollbackPlanV1) -> Result<Self, ApplyTransactionError> {
        let apply_plan = rollback_plan.apply_plan();
        let apply_actions = apply_plan
            .command()
            .map_or_else(Vec::new, |command| command.actions().to_vec());
        let rollback_actions = rollback_plan
            .command()
            .map_or_else(Vec::new, |command| command.actions().to_vec());
        let mut transaction = Self::stage(
            apply_plan.baseline().clone(),
            apply_plan.desired().clone(),
            apply_actions,
            rollback_actions,
        )?;
        if transaction.apply_plan != *apply_plan {
            return Err(ApplyTransactionError::PlanReconstructionMismatch { kind: "apply" });
        }
        if transaction.rollback_plan != *rollback_plan {
            return Err(ApplyTransactionError::PlanReconstructionMismatch { kind: "rollback" });
        }
        transaction.state = ApplyTransactionState::RollbackRequired;
        Ok(transaction)
    }

    /// Current pure transaction state.
    pub const fn state(&self) -> ApplyTransactionState {
        self.state
    }

    /// Closed exact apply plan.
    pub const fn apply_plan(&self) -> &ApplyPlanV1 {
        &self.apply_plan
    }

    /// Immutable rollback plan and baseline.
    pub const fn rollback_plan(&self) -> &RollbackPlanV1 {
        &self.rollback_plan
    }

    /// Mark execution started. State changes only from `Staged`.
    ///
    /// # Errors
    ///
    /// Returns an illegal-transition error otherwise.
    pub fn begin_apply(&mut self) -> Result<(), ApplyTransactionError> {
        self.require_state(ApplyTransactionState::Staged)?;
        self.state = ApplyTransactionState::Applying;
        Ok(())
    }

    /// Compare complete post-apply readback to desired state.
    ///
    /// Exact equality verifies the transaction. Any same-device mismatch moves
    /// directly to `RollbackRequired`; wrong-device evidence is rejected with
    /// no state change.
    ///
    /// # Errors
    ///
    /// Returns a binding or lifecycle error.
    pub fn verify_apply_readback(
        &mut self,
        observed: &SnapshotV1,
    ) -> Result<&ReadbackVerificationV1, ApplyTransactionError> {
        self.require_state(ApplyTransactionState::Applying)?;
        let receipt = ReadbackVerificationV1::compare(&self.desired, observed)?;
        self.state = if receipt.is_exact() {
            ApplyTransactionState::Verified
        } else {
            ApplyTransactionState::RollbackRequired
        };
        Ok(self.apply_readback.insert(receipt))
    }

    /// Require rollback after an outer-boundary apply/readback failure.
    ///
    /// # Errors
    ///
    /// Returns an illegal-transition error unless apply is in progress.
    pub fn require_rollback_after_error(&mut self) -> Result<(), ApplyTransactionError> {
        self.require_state(ApplyTransactionState::Applying)?;
        self.state = ApplyTransactionState::RollbackRequired;
        Ok(())
    }

    /// Enter rollback execution after an apply mismatch.
    ///
    /// # Errors
    ///
    /// Returns an illegal-transition error unless rollback is required.
    pub fn begin_rollback(&mut self) -> Result<(), ApplyTransactionError> {
        self.require_state(ApplyTransactionState::RollbackRequired)?;
        self.state = ApplyTransactionState::RollingBack;
        Ok(())
    }

    /// Compare complete rollback readback to the immutable baseline.
    ///
    /// A same-device mismatch is terminal `Faulted`; wrong-device evidence is
    /// rejected without changing state.
    ///
    /// # Errors
    ///
    /// Returns a binding or lifecycle error.
    pub fn verify_rollback_readback(
        &mut self,
        observed: &SnapshotV1,
    ) -> Result<&ReadbackVerificationV1, ApplyTransactionError> {
        self.require_state(ApplyTransactionState::RollingBack)?;
        let receipt = ReadbackVerificationV1::compare(&self.baseline, observed)?;
        self.state = if receipt.is_exact() {
            ApplyTransactionState::RolledBack
        } else {
            ApplyTransactionState::Faulted
        };
        Ok(self.rollback_readback.insert(receipt))
    }

    /// Mark rollback terminally faulted after an outer-boundary failure.
    ///
    /// # Errors
    ///
    /// Returns an illegal-transition error unless rollback is in progress.
    pub fn fault_rollback_after_error(&mut self) -> Result<(), ApplyTransactionError> {
        self.require_state(ApplyTransactionState::RollingBack)?;
        self.state = ApplyTransactionState::Faulted;
        Ok(())
    }

    fn require_state(&self, required: ApplyTransactionState) -> Result<(), ApplyTransactionError> {
        if self.state == required {
            Ok(())
        } else {
            Err(ApplyTransactionError::IllegalTransition {
                state: self.state,
                required,
            })
        }
    }
}

/// Pure apply transaction failures.
#[derive(Debug, Error)]
pub enum ApplyTransactionError {
    /// Snapshot binding failure.
    #[error(transparent)]
    Diff(#[from] SnapshotDiffError),
    /// No closed mutation maps the requested snapshot difference.
    #[error(transparent)]
    Plan(#[from] ApplyPlanError),
    /// A strict durable plan differed from its freshly rebuilt representation.
    #[error("{kind} plan differed from exact transaction reconstruction")]
    PlanReconstructionMismatch {
        /// Plan kind that failed exact reconstruction.
        kind: &'static str,
    },
    /// Operation was attempted out of sequence.
    #[error("illegal apply transition from {state:?}; required {required:?}")]
    IllegalTransition {
        /// Actual state.
        state: ApplyTransactionState,
        /// Required source state.
        required: ApplyTransactionState,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(device: u8) -> Vec<u8> {
        let mut frame = vec![0xf0, 0x00, 0x20, 0x32, device, 0x0e, 0x00];
        frame.extend_from_slice(b"SYNTHETIC-IDENTITY");
        frame.push(0xf7);
        frame
    }

    fn dump(device: u8, part: u8, marker: u8) -> Vec<u8> {
        let length = match part {
            0 => DUMP0_RESPONSE_LEN,
            1 => DUMP1_RESPONSE_LEN,
            _ => panic!("synthetic test part must be zero or one"),
        };
        let mut frame = vec![0; length];
        frame[..7].copy_from_slice(&[0xf0, 0x00, 0x20, 0x32, device, 0x0e, 0x10]);
        frame[7] = marker;
        frame[12] = part;
        frame[length - 1] = 0xf7;
        frame
    }

    fn snapshot(device: u8, marker0: u8, marker1: u8) -> SnapshotV1 {
        SnapshotV1::from_frames(
            &identity(device),
            &dump(device, 0, marker0),
            &dump(device, 1, marker1),
        )
        .unwrap()
    }

    #[test]
    fn complete_snapshot_preserves_exact_frames_and_round_trips_strict_json() {
        let snapshot = snapshot(0, 1, 2);
        assert_eq!(snapshot.schema_version(), SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(snapshot.device(), DeviceId::new(0).unwrap());
        assert_eq!(snapshot.frame(SnapshotSection::Identity), identity(0));
        assert_eq!(snapshot.frame(SnapshotSection::Dump0), dump(0, 0, 1));
        assert_eq!(snapshot.frame(SnapshotSection::Dump1), dump(0, 1, 2));
        assert!(snapshot.digest().starts_with(DIGEST_PREFIX));
        assert_eq!(snapshot.digest().len(), DIGEST_PREFIX.len() + 64);

        let json = snapshot.to_json().unwrap();
        assert_eq!(SnapshotV1::from_json(&json).unwrap(), snapshot);
    }

    #[test]
    fn snapshot_digest_is_stable_and_domain_separates_ordered_sections() {
        let first = snapshot(0, 1, 2);
        let second = snapshot(0, 1, 2);
        let changed0 = snapshot(0, 3, 2);
        let changed1 = snapshot(0, 1, 3);
        assert_eq!(first.digest(), second.digest());
        assert_ne!(first.digest(), changed0.digest());
        assert_ne!(first.digest(), changed1.digest());
        assert_ne!(changed0.digest(), changed1.digest());
    }

    #[test]
    fn wrong_dump_identity_part_or_message_class_fails_closed() {
        assert!(matches!(
            SnapshotV1::from_frames(&identity(0), &dump(1, 0, 1), &dump(0, 1, 2)),
            Err(SnapshotError::DeviceMismatch {
                section: SnapshotSection::Dump0,
                expected: 0,
                actual: 1,
            })
        ));
        let mut wrong_part = dump(0, 0, 1);
        wrong_part[12] = 1;
        assert!(matches!(
            SnapshotV1::from_frames(&identity(0), &wrong_part, &dump(0, 1, 2)),
            Err(SnapshotError::WrongDumpPart { .. })
        ));
        assert!(matches!(
            SnapshotV1::from_frames(&identity(0), &identity(0), &dump(0, 1, 2)),
            Err(SnapshotError::InvalidDumpLength {
                part: DumpPart::Part0,
                actual: 26,
                ..
            })
        ));

        let mut wrong_kind = vec![0; DUMP0_RESPONSE_LEN];
        wrong_kind[..7].copy_from_slice(&[0xf0, 0, 0x20, 0x32, 0, 0x0e, 0x04]);
        wrong_kind[DUMP0_RESPONSE_LEN - 1] = 0xf7;
        assert!(matches!(
            SnapshotV1::from_frames(&identity(0), &wrong_kind, &dump(0, 1, 2)),
            Err(SnapshotError::UnexpectedMessage {
                section: SnapshotSection::Dump0,
                ..
            })
        ));
    }

    #[test]
    fn serialized_byte_or_digest_tampering_is_detected() {
        let snapshot = snapshot(0, 1, 2);
        let mut value: serde_json::Value =
            serde_json::from_slice(&snapshot.to_json().unwrap()).unwrap();
        value["dump0"]["digest"] =
            serde_json::Value::String(format!("{DIGEST_PREFIX}{}", "0".repeat(64)));
        assert!(matches!(
            SnapshotV1::from_json(&serde_json::to_vec(&value).unwrap()),
            Err(SnapshotError::DigestMismatch { field: "dump0", .. })
        ));

        let mut value: serde_json::Value =
            serde_json::from_slice(&snapshot.to_json().unwrap()).unwrap();
        value["dump0"]["frame"][7] = serde_json::Value::from(7);
        assert!(matches!(
            SnapshotV1::from_json(&serde_json::to_vec(&value).unwrap()),
            Err(SnapshotError::DigestMismatch { field: "dump0", .. })
        ));
    }

    #[test]
    fn diff_order_is_stable_and_changes_require_explicit_typed_actions() {
        let observed = snapshot(0, 1, 2);
        let desired = snapshot(0, 3, 4);
        let diff = observed.diff(&desired).unwrap();
        assert_eq!(
            diff.changes()
                .iter()
                .map(SnapshotSectionChange::section)
                .collect::<Vec<_>>(),
            [SnapshotSection::Dump0, SnapshotSection::Dump1]
        );
        assert!(matches!(
            ApplyPlanV1::new(&observed, &desired, Vec::new()),
            Err(ApplyPlanError::MissingActions(changes))
                if changes == [SnapshotSection::Dump0, SnapshotSection::Dump1]
        ));

        let baseline = snapshot(0, 0, 0);
        let action = DirectParameterAction::new(5, 0x3c, 40).unwrap();
        let desired = baseline.project_direct_actions(&[action]).unwrap();
        let plan = ApplyPlanV1::new(&baseline, &desired, vec![action]).unwrap();
        assert_eq!(plan.action_count(), 1);
        assert_eq!(plan.command().unwrap().actions(), [action]);
        assert!(plan.digest().starts_with(DIGEST_PREFIX));
    }

    #[test]
    fn exact_equality_builds_a_bounded_zero_action_transaction() {
        let baseline = snapshot(0, 1, 2);
        let desired = baseline.clone();
        let mut transaction =
            ApplyTransactionV1::stage(baseline.clone(), desired, Vec::new(), Vec::new()).unwrap();
        assert_eq!(MAX_APPLY_ACTIONS, 127);
        assert_eq!(transaction.apply_plan().action_count(), 0);
        assert_eq!(transaction.rollback_plan().action_count(), 0);
        assert_eq!(transaction.rollback_plan().baseline(), &baseline);
        transaction.begin_apply().unwrap();
        assert!(
            transaction
                .verify_apply_readback(&baseline)
                .unwrap()
                .is_exact()
        );
        assert_eq!(transaction.state(), ApplyTransactionState::Verified);
    }

    #[test]
    fn readback_mismatch_requires_rollback_and_baseline_equality_closes_it() {
        let baseline = snapshot(0, 1, 2);
        let drift = snapshot(0, 1, 3);
        let mut transaction =
            ApplyTransactionV1::stage(baseline.clone(), baseline.clone(), Vec::new(), Vec::new())
                .unwrap();
        transaction.begin_apply().unwrap();
        assert!(
            !transaction
                .verify_apply_readback(&drift)
                .unwrap()
                .is_exact()
        );
        assert_eq!(transaction.state(), ApplyTransactionState::RollbackRequired);
        transaction.begin_rollback().unwrap();
        assert!(
            transaction
                .verify_rollback_readback(&baseline)
                .unwrap()
                .is_exact()
        );
        assert_eq!(transaction.state(), ApplyTransactionState::RolledBack);
    }

    #[test]
    fn failed_rollback_readback_is_terminal() {
        let baseline = snapshot(0, 1, 2);
        let drift = snapshot(0, 1, 3);
        let mut transaction =
            ApplyTransactionV1::stage(baseline.clone(), baseline.clone(), Vec::new(), Vec::new())
                .unwrap();
        transaction.begin_apply().unwrap();
        transaction.verify_apply_readback(&drift).unwrap();
        transaction.begin_rollback().unwrap();
        assert!(
            !transaction
                .verify_rollback_readback(&drift)
                .unwrap()
                .is_exact()
        );
        assert_eq!(transaction.state(), ApplyTransactionState::Faulted);
    }

    #[test]
    fn debug_redacts_all_raw_frames() {
        let debug = format!("{:?}", snapshot(0, 1, 2));
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("SYNTHETIC"));
    }

    #[test]
    fn changing_transaction_requires_matching_explicit_inverse_addresses() {
        let baseline = snapshot(0, 0, 0);
        let apply = DirectParameterAction::new(5, 0x3c, 40).unwrap();
        let inverse = DirectParameterAction::new(5, 0x3c, 0).unwrap();
        let desired = baseline.project_direct_actions(&[apply]).unwrap();
        let transaction = ApplyTransactionV1::stage(
            baseline.clone(),
            desired.clone(),
            vec![apply],
            vec![inverse],
        )
        .unwrap();
        assert_eq!(
            transaction.apply_plan().command().unwrap().actions(),
            [apply]
        );
        assert_eq!(
            transaction.rollback_plan().command().unwrap().actions(),
            [inverse]
        );
        assert_eq!(transaction.rollback_plan().baseline(), &baseline);

        assert!(matches!(
            ApplyTransactionV1::stage(baseline.clone(), desired.clone(), vec![apply], Vec::new()),
            Err(ApplyTransactionError::Plan(
                ApplyPlanError::MissingRollbackActions
            ))
        ));
        assert!(matches!(
            ApplyTransactionV1::stage(
                baseline,
                desired,
                vec![apply],
                vec![DirectParameterAction::new(5, 0x3d, 0).unwrap()],
            ),
            Err(ApplyTransactionError::Plan(
                ApplyPlanError::RollbackAddressMismatch
            ))
        ));

        let projected = ApplyTransactionV1::stage_projected(
            snapshot(0, 0, 0),
            vec![DirectParameterAction::new(5, 0x3c, 40).unwrap()],
        )
        .unwrap();
        assert_eq!(
            projected.rollback_plan().command().unwrap().actions(),
            [DirectParameterAction::new(5, 0x3c, 0).unwrap()]
        );
    }

    #[test]
    fn reviewed_o1_peq9_projection_patches_only_exact_dump0_locations() {
        let baseline = snapshot(0, 0, 0);
        // 300 = 0x12c: low 0x2c, carrier bit clear, high 1.
        // 150 = 0x096: low 0x16, carrier bit set, high 0.
        let actions = [
            DirectParameterAction::new(5, 0x3b, 300).unwrap(),
            DirectParameterAction::new(5, 0x3c, 40).unwrap(),
            DirectParameterAction::new(5, 0x3d, 150).unwrap(),
            DirectParameterAction::new(5, 0x3e, 1).unwrap(),
        ];
        let desired = baseline.project_direct_actions(&actions).unwrap();
        let before = baseline.frame(SnapshotSection::Dump0);
        let after = desired.frame(SnapshotSection::Dump0);

        assert_eq!(after[843], 0x2c);
        assert_eq!(after[844] & (1 << 6), 0);
        assert_eq!(after[845], 1);
        assert_eq!(after[846], 40);
        assert_eq!(after[848], 0x16);
        assert_eq!(after[852] & (1 << 3), 1 << 3);
        assert_eq!(after[849], 0);
        assert_eq!(after[850], 1);
        assert_eq!(
            seven_bit_sum(&after[DUMP0_PACKED_PAYLOAD_START..=DUMP0_DERIVED_TRAILER_OFFSET]),
            seven_bit_sum(&before[DUMP0_PACKED_PAYLOAD_START..=DUMP0_DERIVED_TRAILER_OFFSET])
        );
        for index in 0..DUMP0_RESPONSE_LEN {
            if ![
                843,
                844,
                845,
                846,
                848,
                849,
                850,
                852,
                DUMP0_DERIVED_TRAILER_OFFSET,
            ]
            .contains(&index)
            {
                assert_eq!(after[index], before[index], "unexpected patch at {index}");
            }
        }
        assert_eq!(
            desired.frame(SnapshotSection::Identity),
            baseline.frame(SnapshotSection::Identity)
        );
        assert_eq!(
            desired.frame(SnapshotSection::Dump1),
            baseline.frame(SnapshotSection::Dump1)
        );
        assert_ne!(desired.digest(), baseline.digest());

        let inverse = [
            DirectParameterAction::new(5, 0x3b, 0).unwrap(),
            DirectParameterAction::new(5, 0x3c, 0).unwrap(),
            DirectParameterAction::new(5, 0x3d, 0).unwrap(),
            DirectParameterAction::new(5, 0x3e, 0).unwrap(),
        ];
        assert_eq!(desired.project_direct_actions(&inverse).unwrap(), baseline);
    }

    #[test]
    fn named_o1_peq9_projection_updates_the_observed_dump0_trailer_exactly() {
        let mut baseline_dump0 = dump(0, 0, 0);
        // Carry the omitted opaque body's sanitized residue so this minimized
        // fixture retains the named and independent Dump0 balance of 23.
        baseline_dump0[DUMP0_PACKED_PAYLOAD_START] = 104;
        baseline_dump0[843] = 52;
        baseline_dump0[844] = 65;
        baseline_dump0[846] = 20;
        baseline_dump0[848] = 22;
        baseline_dump0[850] = 1;
        baseline_dump0[852] = 8;
        baseline_dump0[DUMP0_DERIVED_TRAILER_OFFSET] = 7;
        let baseline =
            SnapshotV1::from_frames(&identity(0), &baseline_dump0, &dump(0, 1, 0)).unwrap();
        let actions = [
            DirectParameterAction::new(5, 0x3b, 53).unwrap(),
            DirectParameterAction::new(5, 0x3c, 32).unwrap(),
            DirectParameterAction::new(5, 0x3d, 118).unwrap(),
        ];

        let desired = baseline.project_direct_actions(&actions).unwrap();
        let after = desired.frame(SnapshotSection::Dump0);
        assert_eq!(after[843], 53);
        assert_eq!(after[844], 1);
        assert_eq!(after[846], 32);
        assert_eq!(after[848], 118);
        assert_eq!(after[852], 0);
        assert_eq!(after[DUMP0_DERIVED_TRAILER_OFFSET], 98);
        assert_eq!(
            seven_bit_sum(&after[DUMP0_PACKED_PAYLOAD_START..=DUMP0_DERIVED_TRAILER_OFFSET]),
            23
        );

        let inverse = baseline.inverse_actions_for(&actions).unwrap();
        assert_eq!(desired.project_direct_actions(&inverse).unwrap(), baseline);
    }

    #[test]
    fn snapshot_projection_rejects_every_unreviewed_address_and_wide_low_field() {
        let baseline = snapshot(0, 0, 0);
        // Input sum, output gain, mute off O4, PEQ editor index, dynamic EQ,
        // crossover, output name, and setup/input channels all fail closed.
        for (channel, parameter) in [
            (4, 0x3b),
            (5, 0x02),
            (5, 0x03),
            (5, 0x08),
            (5, 0x09),
            (5, 0x12),
            (5, 0x40),
            (5, 0x42),
            (8, 0x45),
            (0, 0x06),
            (1, 0x13),
        ] {
            let action = DirectParameterAction::new(channel, parameter, 1).unwrap();
            assert!(matches!(
                baseline.project_direct_actions(&[action]),
                Err(SnapshotProjectionError::UnmappedAction { .. })
            ));
        }
        let wide_q = DirectParameterAction::new(5, 0x3c, 128).unwrap();
        assert!(matches!(
            baseline.project_direct_actions(&[wide_q]),
            Err(SnapshotProjectionError::ValueOutOfRange {
                parameter: 0x3c,
                value: 128,
                maximum: 40,
                ..
            })
        ));
        for (parameter, maximum) in [(0x3b, 320), (0x3d, 300), (0x3e, 2), (0x3f, 1), (0x07, 9)] {
            let action = DirectParameterAction::new(6, parameter, maximum + 1).unwrap();
            assert!(matches!(
                baseline.project_direct_actions(&[action]),
                Err(SnapshotProjectionError::ValueOutOfRange { .. })
            ));
            let action = DirectParameterAction::new(6, parameter, maximum).unwrap();
            assert!(baseline.project_direct_actions(&[action]).is_ok());
        }
    }

    // The O4 output-mute address (channel 8, parameter 0x03, Dump1 byte 223)
    // is fixture-derived from the pinned references and remains pending
    // hardware confirmation by the Legalab WORD-FS-A silent mute-frame
    // rehearsal. These tests establish the offline shape only.

    fn muted_o4_baseline() -> SnapshotV1 {
        let mut baseline_dump1 = dump(0, 1, 0);
        // O4 mute value 1 = muted at the transcribed Dump1 location.
        baseline_dump1[223] = 1;
        SnapshotV1::from_frames(&identity(0), &dump(0, 0, 0), &baseline_dump1).unwrap()
    }

    #[test]
    fn reviewed_o4_mute_projection_patches_only_the_exact_dump1_byte() {
        let baseline = muted_o4_baseline();
        let unmute = DirectParameterAction::new(8, 0x03, 0).unwrap();
        let desired = baseline.project_direct_actions(&[unmute]).unwrap();

        let before = baseline.frame(SnapshotSection::Dump1);
        let after = desired.frame(SnapshotSection::Dump1);
        assert_eq!(after[223], 0);
        for index in 0..DUMP1_RESPONSE_LEN - 2 {
            if index != 223 {
                assert_eq!(after[index], before[index], "unexpected patch at {index}");
            }
        }
        // Dump1 trailer (named-device observed 2026-10-08): the trailer keeps
        // the baseline modulo-128 balance, so lowering byte 223 by one raises
        // the trailer by one.
        assert_eq!(
            after[DUMP1_RESPONSE_LEN - 2],
            (before[DUMP1_RESPONSE_LEN - 2] + 1) & 0x7f
        );
        assert_eq!(
            seven_bit_sum(&after[PACKED_PAYLOAD_START..=DUMP1_RESPONSE_LEN - 2]),
            seven_bit_sum(&before[PACKED_PAYLOAD_START..=DUMP1_RESPONSE_LEN - 2])
        );
        assert_eq!(
            desired.frame(SnapshotSection::Identity),
            baseline.frame(SnapshotSection::Identity)
        );
        assert_eq!(
            desired.frame(SnapshotSection::Dump0),
            baseline.frame(SnapshotSection::Dump0)
        );
        assert_ne!(desired.digest(), baseline.digest());

        let inverse = baseline.inverse_actions_for(&[unmute]).unwrap();
        assert_eq!(inverse, [DirectParameterAction::new(8, 0x03, 1).unwrap()]);
        assert_eq!(desired.project_direct_actions(&inverse).unwrap(), baseline);
    }

    #[test]
    fn one_change_o4_unmute_plan_diffs_to_exactly_one_field() {
        let baseline = muted_o4_baseline();
        let unmute = DirectParameterAction::new(8, 0x03, 0).unwrap();
        let desired = baseline.project_direct_actions(&[unmute]).unwrap();

        // Section-level semantic diff: exactly one changed section, Dump1.
        let diff = baseline.diff(&desired).unwrap();
        assert_eq!(diff.changes().len(), 1);
        assert_eq!(diff.changes()[0].section, SnapshotSection::Dump1);

        // Byte-level: the mute byte and the balanced Dump1 trailer differ.
        let mut changed = 0_usize;
        for section in [
            SnapshotSection::Identity,
            SnapshotSection::Dump0,
            SnapshotSection::Dump1,
        ] {
            let before = baseline.frame(section);
            let after = desired.frame(section);
            assert_eq!(before.len(), after.len());
            changed += before
                .iter()
                .zip(after.iter())
                .filter(|(before_byte, after_byte)| before_byte != after_byte)
                .count();
        }
        assert_eq!(changed, 2);

        // Plan-level: one typed action out, exact mute=1 inverse back.
        let transaction =
            ApplyTransactionV1::stage_projected(baseline.clone(), vec![unmute]).unwrap();
        assert_eq!(transaction.apply_plan().action_count(), 1);
        assert_eq!(
            transaction.apply_plan().command().unwrap().actions(),
            [unmute]
        );
        assert_eq!(transaction.apply_plan().desired(), &desired);
        assert_eq!(
            transaction.rollback_plan().command().unwrap().actions(),
            [DirectParameterAction::new(8, 0x03, 1).unwrap()]
        );
    }

    #[test]
    fn o4_mute_projection_rejects_other_channels_unreviewed_params_and_wide_values() {
        let baseline = muted_o4_baseline();
        // A mute address on any other channel does not match O4's reviewed
        // address: O1/O2/O3/O5/O6 mutes and the setup channel fail closed, as
        // do O4 parameters adjacent to mute and the O4 crossover.
        for (channel, parameter) in [
            (0, 0x03),
            (5, 0x03),
            (6, 0x03),
            (7, 0x03),
            (9, 0x03),
            (10, 0x03),
            (8, 0x02),
            (8, 0x04),
            (8, 0x42),
        ] {
            let action = DirectParameterAction::new(channel, parameter, 1).unwrap();
            assert!(matches!(
                baseline.project_direct_actions(&[action]),
                Err(SnapshotProjectionError::UnmappedAction { .. })
            ));
            assert!(matches!(
                baseline.inverse_actions_for(&[action]),
                Err(SnapshotProjectionError::UnmappedAction { .. })
            ));
        }
        // The on/off domain is closed: 2 is not a mute state.
        let wide = DirectParameterAction::new(8, 0x03, 2).unwrap();
        assert!(matches!(
            baseline.project_direct_actions(&[wide]),
            Err(SnapshotProjectionError::ValueNotOnOff {
                parameter: 0x03,
                value: 2,
            })
        ));
        // A baseline byte outside the on/off domain refutes the transcription
        // and must fail closed on inverse derivation.
        let mut suspect_dump1 = dump(0, 1, 0);
        suspect_dump1[223] = 5;
        let suspect =
            SnapshotV1::from_frames(&identity(0), &dump(0, 0, 0), &suspect_dump1).unwrap();
        assert!(matches!(
            suspect.inverse_actions_for(&[DirectParameterAction::new(8, 0x03, 0).unwrap()]),
            Err(SnapshotProjectionError::ValueNotOnOff {
                parameter: 0x03,
                value: 5,
            })
        ));
    }

    #[test]
    fn durable_apply_and_rollback_carriers_round_trip_only_after_revalidation() {
        let baseline = snapshot(0, 0, 0);
        let apply = DirectParameterAction::new(5, 0x3c, 40).unwrap();
        let inverse = DirectParameterAction::new(5, 0x3c, 0).unwrap();
        let desired = baseline.project_direct_actions(&[apply]).unwrap();
        let transaction =
            ApplyTransactionV1::stage(baseline, desired, vec![apply], vec![inverse]).unwrap();

        let apply_json = transaction.apply_plan().to_json().unwrap();
        let decoded_apply = ApplyPlanV1::from_json(&apply_json).unwrap();
        assert_eq!(&decoded_apply, transaction.apply_plan());
        let reconstructed = ApplyTransactionV1::from_apply_plan(&decoded_apply).unwrap();
        assert_eq!(reconstructed.state(), ApplyTransactionState::Staged);
        assert_eq!(reconstructed.apply_plan(), &decoded_apply);

        let rollback_json = transaction.rollback_plan().to_json().unwrap();
        let decoded_rollback = RollbackPlanV1::from_json(&rollback_json).unwrap();
        assert_eq!(&decoded_rollback, transaction.rollback_plan());
        let resumed = ApplyTransactionV1::resume_rollback(&decoded_rollback).unwrap();
        assert_eq!(resumed.state(), ApplyTransactionState::RollbackRequired);
        assert_eq!(resumed.apply_plan(), decoded_rollback.apply_plan());
        assert_eq!(resumed.rollback_plan(), &decoded_rollback);
    }

    #[test]
    fn durable_plan_tampering_and_unknown_fields_fail_closed() {
        let baseline = snapshot(0, 0, 0);
        let apply = DirectParameterAction::new(5, 0x3c, 40).unwrap();
        let inverse = DirectParameterAction::new(5, 0x3c, 0).unwrap();
        let desired = baseline.project_direct_actions(&[apply]).unwrap();
        let transaction =
            ApplyTransactionV1::stage(baseline, desired, vec![apply], vec![inverse]).unwrap();

        let mut wire: serde_json::Value =
            serde_json::from_slice(&transaction.apply_plan().to_json().unwrap()).unwrap();
        wire["plan_digest"] =
            serde_json::Value::String(format!("{DIGEST_PREFIX}{}", "0".repeat(64)));
        assert!(matches!(
            ApplyPlanV1::from_json(&serde_json::to_vec(&wire).unwrap()),
            Err(PlanCarrierError::BindingMismatch {
                field: "plan_digest",
                ..
            })
        ));

        let mut wire: serde_json::Value =
            serde_json::from_slice(&transaction.rollback_plan().to_json().unwrap()).unwrap();
        wire["unexpected"] = serde_json::Value::Bool(true);
        assert!(matches!(
            RollbackPlanV1::from_json(&serde_json::to_vec(&wire).unwrap()),
            Err(PlanCarrierError::Json(_))
        ));
    }
}
