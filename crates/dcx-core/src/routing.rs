//! Closed MVP routing: the digest-bound `dcx.desired-routing/v1` document and
//! a read-only decode of the reviewed routing addresses.
//!
//! Scope is exactly `dec-autonomous-muted-bench-20261007` (Linear TIN-5379
//! comment `d24dac45`): the O4, O5, and O6 output mutes (channels 8 through
//! 10, `0x03`), the O4 and O3 output sources (channels 8 and 7, `0x41`), and
//! the setup input sum type (channel 0, `0x02`) as off or A+B only. Actions
//! carry a fixed order, mutes first, then the input sum, then the O4 and O3
//! sources, so the O3 SUM source is written after the sum it consumes and a
//! rollback restores sources before the sum and the mutes. Every address is a
//! transcription from the pinned `DuinoDCX` tables, pending exact device
//! readback; nothing here is hardware evidence.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::{
    SnapshotV1,
    layout::{
        INPUT_SUM_A_PLUS_B_CODE, INPUT_SUM_OFF_CODE, INPUT_SUM_PARAMETER, MUTE_PARAMETER,
        SETUP_CHANNEL, SOURCE_PARAMETER, output_channel, reviewed_address, routing_rank,
    },
    protocol::DirectParameterAction,
    rew::{DesiredProfileError, MAX_DESIRED_PROFILE_BYTES},
};

/// Versioned closed routing desired-profile envelope.
pub const DESIRED_ROUTING_SCHEMA: &str = "dcx.desired-routing/v1";
/// Versioned decoded routing receipt.
pub const ROUTING_STATE_SCHEMA: &str = "dcx.routing-state/v1";
/// Number of reviewed routing addresses, and so the largest document.
pub const MAX_ROUTING_ACTIONS: usize = 6;

const DESIRED_ROUTING_DOMAIN: &[u8] = b"dcx2496.desired-routing/v1\0";

/// Output input source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputSource {
    /// Input A, code 0.
    A,
    /// Input B, code 1.
    B,
    /// Input C, code 2.
    C,
    /// Setup input sum, code 3.
    Sum,
}

impl OutputSource {
    /// Device code.
    pub const fn code(self) -> u16 {
        match self {
            Self::A => 0,
            Self::B => 1,
            Self::C => 2,
            Self::Sum => 3,
        }
    }

    /// Source of a device code, if it is one.
    pub const fn from_code(code: u16) -> Option<Self> {
        match code {
            0 => Some(Self::A),
            1 => Some(Self::B),
            2 => Some(Self::C),
            3 => Some(Self::Sum),
            _ => None,
        }
    }
}

/// The two admitted setup input sum types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSum {
    /// No input sum, code 0.
    Off,
    /// A+B, code 4.
    APlusB,
}

impl InputSum {
    /// Device code.
    pub const fn code(self) -> u16 {
        match self {
            Self::Off => INPUT_SUM_OFF_CODE,
            Self::APlusB => INPUT_SUM_A_PLUS_B_CODE,
        }
    }
}

/// Typed routing targets; every unset field is left untouched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoutingTargetsV1 {
    /// O4 mute.
    pub o4_muted: Option<bool>,
    /// O5 mute.
    pub o5_muted: Option<bool>,
    /// O6 mute.
    pub o6_muted: Option<bool>,
    /// Setup input sum.
    pub input_sum: Option<InputSum>,
    /// O4 source.
    pub o4_source: Option<OutputSource>,
    /// O3 source.
    pub o3_source: Option<OutputSource>,
}

impl RoutingTargetsV1 {
    /// The ruling's MVP routing: O5 and O6 muted, input sum A+B, O4 sourced
    /// from C, and O3 sourced from SUM. The O4 mute is left untouched.
    pub const fn mvp() -> Self {
        Self {
            o4_muted: None,
            o5_muted: Some(true),
            o6_muted: Some(true),
            input_sum: Some(InputSum::APlusB),
            o4_source: Some(OutputSource::C),
            o3_source: Some(OutputSource::Sum),
        }
    }

    /// Ordered direct actions for the set targets.
    ///
    /// # Errors
    ///
    /// Rejects an empty target set.
    pub fn actions(self) -> Result<Vec<DirectParameterAction>, RoutingError> {
        let mute = |output: u8, muted: bool| {
            DirectParameterAction::new(output_channel(output), MUTE_PARAMETER, u16::from(muted))
        };
        let source = |output: u8, source: OutputSource| {
            DirectParameterAction::new(output_channel(output), SOURCE_PARAMETER, source.code())
        };
        let ordered = [
            self.o4_muted.map(|muted| mute(4, muted)),
            self.o5_muted.map(|muted| mute(5, muted)),
            self.o6_muted.map(|muted| mute(6, muted)),
            self.input_sum.map(|sum| {
                DirectParameterAction::new(SETUP_CHANNEL, INPUT_SUM_PARAMETER, sum.code())
            }),
            self.o4_source.map(|value| source(4, value)),
            self.o3_source.map(|value| source(3, value)),
        ];
        let actions = ordered
            .into_iter()
            .flatten()
            .collect::<Result<Vec<_>, _>>()?;
        if actions.is_empty() {
            return Err(RoutingError::EmptyTargets);
        }
        Ok(actions)
    }
}

/// Ordered routing actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingDocumentV1 {
    /// Ordered mute, input sum, and source actions.
    pub actions: Vec<DirectParameterAction>,
}

/// One digest-bound desired routing state.
///
/// The digest covers a domain separator, the profile identity and revision,
/// and every ordered action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesiredRoutingProfileV1 {
    schema_version: String,
    #[serde(rename = "profileID")]
    profile_id: String,
    revision: String,
    digest: String,
    document: RoutingDocumentV1,
}

impl DesiredRoutingProfileV1 {
    /// Bind a routing document to its stable profile identity.
    ///
    /// # Errors
    ///
    /// Rejects unbounded identity strings, an empty or oversized action list,
    /// any address outside the reviewed routing set, a value outside its
    /// reviewed domain, and any order other than mutes, input sum, sources.
    pub fn new(
        profile_id: String,
        revision: String,
        document: RoutingDocumentV1,
    ) -> Result<Self, DesiredProfileError> {
        validate_text("profile_id", &profile_id)?;
        validate_text("revision", &revision)?;
        validate_document(&document)?;
        let digest = routing_digest(&profile_id, &revision, &document)?;
        Ok(Self {
            schema_version: DESIRED_ROUTING_SCHEMA.to_owned(),
            profile_id,
            revision,
            digest,
            document,
        })
    }

    /// Bind typed targets to a profile identity.
    ///
    /// # Errors
    ///
    /// Same as [`Self::new`], plus an empty target set.
    pub fn from_targets(
        profile_id: String,
        revision: String,
        targets: RoutingTargetsV1,
    ) -> Result<Self, RoutingError> {
        Ok(Self::new(
            profile_id,
            revision,
            RoutingDocumentV1 {
                actions: targets.actions()?,
            },
        )?)
    }

    /// Parse and fully revalidate one serialized envelope.
    ///
    /// # Errors
    ///
    /// Rejects oversized/malformed JSON, unknown fields, unsupported schema,
    /// invalid actions, or digest mismatch.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DesiredProfileError> {
        if bytes.len() > MAX_DESIRED_PROFILE_BYTES {
            return Err(DesiredProfileError::JsonTooLarge(bytes.len()));
        }
        let wire: RoutingProfileWire = serde_json::from_slice(bytes)?;
        if wire.schema_version != DESIRED_ROUTING_SCHEMA {
            return Err(DesiredProfileError::UnsupportedSchema(wire.schema_version));
        }
        let actions = wire
            .document
            .actions
            .into_iter()
            .map(|action| {
                DirectParameterAction::new(action.channel, action.parameter, action.value)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let rebuilt = Self::new(
            wire.profile_id,
            wire.revision,
            RoutingDocumentV1 { actions },
        )?;
        if rebuilt.digest != wire.digest {
            return Err(DesiredProfileError::DigestMismatch {
                expected: rebuilt.digest,
                actual: wire.digest,
            });
        }
        Ok(rebuilt)
    }

    /// Serialize the strict envelope.
    ///
    /// # Errors
    ///
    /// Returns an encoding error only for an in-memory invariant failure.
    pub fn to_json(&self) -> Result<Vec<u8>, DesiredProfileError> {
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > MAX_DESIRED_PROFILE_BYTES {
            return Err(DesiredProfileError::JsonTooLarge(bytes.len()));
        }
        Ok(bytes)
    }

    /// Stable profile identifier.
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Stable profile revision.
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// Canonical digest.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Reviewed document.
    pub const fn document(&self) -> &RoutingDocumentV1 {
        &self.document
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RoutingProfileWire {
    schema_version: String,
    #[serde(rename = "profileID")]
    profile_id: String,
    revision: String,
    digest: String,
    document: RoutingDocumentWire,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoutingDocumentWire {
    actions: Vec<ActionWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionWire {
    channel: u8,
    parameter: u8,
    value: u16,
}

fn validate_text(field: &'static str, value: &str) -> Result<(), DesiredProfileError> {
    if value.is_empty() || value.len() > 128 || !value.is_ascii() {
        Err(DesiredProfileError::InvalidIdentity { field })
    } else {
        Ok(())
    }
}

fn validate_document(document: &RoutingDocumentV1) -> Result<(), DesiredProfileError> {
    if document.actions.is_empty() || document.actions.len() > MAX_ROUTING_ACTIONS {
        return Err(DesiredProfileError::UnsupportedDocument(
            "routing profile must carry 1 through 6 actions",
        ));
    }
    let mut previous: Option<u8> = None;
    for action in &document.actions {
        let Some(address) = reviewed_address(action.channel(), action.parameter())
            .filter(|address| address.field.is_routing())
        else {
            return Err(DesiredProfileError::UnsupportedDocument(
                "routing action escapes the O4/O5/O6 mutes, O3/O4 sources, and input sum",
            ));
        };
        let rank = routing_rank(address).ok_or(DesiredProfileError::UnsupportedDocument(
            "routing action has no apply rank",
        ))?;
        if previous.is_some_and(|previous| rank <= previous) {
            return Err(DesiredProfileError::UnsupportedDocument(
                "routing actions must be distinct, mutes then input sum then O4 and O3 sources",
            ));
        }
        previous = Some(rank);
        if !address.field.admits(action.value()) {
            return Err(DesiredProfileError::UnsupportedDocument(
                "routing action value is outside its reviewed domain",
            ));
        }
    }
    Ok(())
}

fn routing_digest(
    profile_id: &str,
    revision: &str,
    document: &RoutingDocumentV1,
) -> Result<String, DesiredProfileError> {
    let mut hasher = Sha256::new();
    hasher.update(DESIRED_ROUTING_DOMAIN);
    for text in [profile_id, revision] {
        let length = u8::try_from(text.len())
            .map_err(|_| DesiredProfileError::InvalidIdentity { field: "text" })?;
        hasher.update([length]);
        hasher.update(text.as_bytes());
    }
    let count = u8::try_from(document.actions.len())
        .map_err(|_| DesiredProfileError::UnsupportedDocument("too many routing actions"))?;
    hasher.update([count]);
    for action in &document.actions {
        hasher.update([action.channel(), action.parameter()]);
        hasher.update(action.value().to_be_bytes());
    }
    Ok(format!("sha256/{:x}", hasher.finalize()))
}

/// Decoded reviewed routing fields of one snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingStateV1 {
    /// Always [`ROUTING_STATE_SCHEMA`].
    pub schema_version: &'static str,
    /// Evidence class of the decoded addresses.
    pub evidence_class: &'static str,
    /// Exact snapshot the fields were decoded from.
    pub snapshot_digest: String,
    /// O4 mute.
    pub o4_muted: bool,
    /// O5 mute.
    pub o5_muted: bool,
    /// O6 mute.
    pub o6_muted: bool,
    /// Raw setup input sum type code (0 off, 1 A, 2 B, 3 C, 4 A+B, 5 A+C, 6 B+C).
    pub input_sum_code: u16,
    /// O4 source.
    pub o4_source: OutputSource,
    /// O3 source.
    pub o3_source: OutputSource,
}

/// Decode the reviewed routing fields from the transcribed layout.
///
/// # Errors
///
/// Rejects any mute other than 0 or 1, any source above SUM, and any input sum
/// type above B+C; each is evidence against the transcription.
pub fn decode_routing(snapshot: &SnapshotV1) -> Result<RoutingStateV1, RoutingError> {
    let read = |channel: u8, parameter: u8| -> Result<u16, RoutingError> {
        let address = reviewed_address(channel, parameter)
            .filter(|address| address.field.is_routing())
            .ok_or(RoutingError::Unreviewed { channel, parameter })?;
        Ok(snapshot.read_reviewed(address))
    };
    let muted = |output: u8| -> Result<bool, RoutingError> {
        let channel = output_channel(output);
        match read(channel, MUTE_PARAMETER)? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(RoutingError::OutOfDomain {
                channel,
                parameter: MUTE_PARAMETER,
                value,
            }),
        }
    };
    let source = |output: u8| -> Result<OutputSource, RoutingError> {
        let channel = output_channel(output);
        let value = read(channel, SOURCE_PARAMETER)?;
        OutputSource::from_code(value).ok_or(RoutingError::OutOfDomain {
            channel,
            parameter: SOURCE_PARAMETER,
            value,
        })
    };
    let input_sum_code = read(SETUP_CHANNEL, INPUT_SUM_PARAMETER)?;
    if input_sum_code > 6 {
        return Err(RoutingError::OutOfDomain {
            channel: SETUP_CHANNEL,
            parameter: INPUT_SUM_PARAMETER,
            value: input_sum_code,
        });
    }
    Ok(RoutingStateV1 {
        schema_version: ROUTING_STATE_SCHEMA,
        evidence_class: "transcribed_layout_unverified_on_named_device",
        snapshot_digest: snapshot.digest().to_owned(),
        o4_muted: muted(4)?,
        o5_muted: muted(5)?,
        o6_muted: muted(6)?,
        input_sum_code,
        o4_source: source(4)?,
        o3_source: source(3)?,
    })
}

/// Routing construction and decoding failures.
#[derive(Debug, Error)]
pub enum RoutingError {
    /// No routing target was set.
    #[error("routing targets are empty")]
    EmptyTargets,
    /// An address escaped the reviewed routing set.
    #[error("channel {channel} parameter {parameter:#04x} is not a reviewed routing address")]
    Unreviewed { channel: u8, parameter: u8 },
    /// A stored value is outside the documented device domain.
    #[error("channel {channel} parameter {parameter:#04x} holds unexpected value {value}")]
    OutOfDomain {
        channel: u8,
        parameter: u8,
        value: u16,
    },
    /// The document failed validation.
    #[error(transparent)]
    Profile(#[from] DesiredProfileError),
    /// A typed action could not be represented.
    #[error(transparent)]
    Protocol(#[from] crate::protocol::ProtocolError),
}

#[cfg(test)]
mod tests;
