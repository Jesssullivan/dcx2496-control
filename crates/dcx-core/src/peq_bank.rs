//! Multi-band output PEQ state: device code grids, snapshot decoding, and the
//! digest-bound `dcx.desired-profile/v2` document.
//!
//! The code grids follow the public protocol description already used by the
//! REW importer: frequency `20 * 2^(code / 32)` Hz for codes 0 through 320, Q
//! `0.1 * 10^(code / 20)` for codes 0 through 40, and gain `code / 10 - 15` dB
//! for codes 0 through 300. These remain implementation hypotheses until exact
//! device readback confirms a written value.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::{
    SnapshotV1,
    layout::{
        BandField, MAX_FREQUENCY_CODE, MAX_GAIN_CODE, MAX_Q_CODE, OUTPUT_COUNT, OutputField,
        PEQ_BANDS, UNITY_GAIN_CODE, band_parameter, output_channel, reviewed_address,
    },
    protocol::DirectParameterAction,
    rew::{
        DESIRED_PROFILE_SCHEMA, DesiredPeqProfileV1, DesiredProfileError, MAX_DESIRED_PROFILE_BYTES,
    },
};

/// Versioned multi-action output PEQ desired-profile envelope.
pub const DESIRED_PROFILE_V2_SCHEMA: &str = "dcx.desired-profile/v2";
/// Versioned decoded PEQ bank receipt.
pub const PEQ_BANK_STATE_SCHEMA: &str = "dcx.peq-bank-state/v1";
/// Largest v2 document: on/off, count, and all five fields of nine bands.
pub const MAX_BANK_ACTIONS: usize = 2 + PEQ_BANDS as usize * 5;

const DESIRED_PROFILE_V2_DOMAIN: &[u8] = b"dcx2496.desired-profile/v2\0";

/// Nearest device frequency code for a frequency in hertz.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn frequency_code(frequency_hz: f64) -> u16 {
    (32.0 * (frequency_hz / 20.0).log2())
        .round()
        .clamp(0.0, f64::from(MAX_FREQUENCY_CODE)) as u16
}

/// Frequency represented by a device frequency code.
pub fn frequency_hz(code: u16) -> f64 {
    20.0 * 2.0_f64.powf(f64::from(code) / 32.0)
}

/// Nearest device Q code for a Q value.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn q_code(q: f64) -> u16 {
    (20.0 * (q / 0.1).log10())
        .round()
        .clamp(0.0, f64::from(MAX_Q_CODE)) as u16
}

/// Q represented by a device Q code.
pub fn q_value(code: u16) -> f64 {
    0.1 * 10.0_f64.powf(f64::from(code) / 20.0)
}

/// Nearest device gain code for a gain in decibels.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn gain_code(gain_db: f64) -> u16 {
    ((gain_db + 15.0) * 10.0)
        .round()
        .clamp(0.0, f64::from(MAX_GAIN_CODE)) as u16
}

/// Gain in decibels represented by a device gain code.
pub fn gain_db(code: u16) -> f64 {
    f64::from(code) / 10.0 - 15.0
}

/// Exact device codes of one PEQ band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandCodes {
    /// Frequency code.
    pub frequency_code: u16,
    /// Q code.
    pub q_code: u16,
    /// Gain code.
    pub gain_code: u16,
    /// Filter kind code.
    pub kind_code: u16,
    /// Shelf slope code.
    pub slope_code: u16,
}

impl BandCodes {
    /// Code for one field.
    pub const fn get(self, field: BandField) -> u16 {
        match field {
            BandField::Frequency => self.frequency_code,
            BandField::Q => self.q_code,
            BandField::Gain => self.gain_code,
            BandField::Kind => self.kind_code,
            BandField::Slope => self.slope_code,
        }
    }

    /// Ordered direct actions writing all five fields of one band.
    ///
    /// # Errors
    ///
    /// Rejects an invalid output, band, or out-of-domain value.
    pub fn actions(self, output: u8, band: u8) -> Result<Vec<DirectParameterAction>, PeqBankError> {
        if !(1..=OUTPUT_COUNT).contains(&output) {
            return Err(PeqBankError::InvalidOutput(output));
        }
        if !(1..=PEQ_BANDS).contains(&band) {
            return Err(PeqBankError::InvalidBand(band));
        }
        BandField::all()
            .into_iter()
            .map(|field| {
                let value = self.get(field);
                if value > field.device_max() {
                    return Err(PeqBankError::OutOfDomain {
                        label: OutputField::Band { band, field }.label(),
                        value,
                        maximum: field.device_max(),
                    });
                }
                Ok(DirectParameterAction::new(
                    output_channel(output),
                    band_parameter(band, field),
                    value,
                )?)
            })
            .collect()
    }
}

/// Decoded state of one PEQ band.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeqBandStateV1 {
    /// Band number, 1 through 9.
    pub band: u8,
    /// Whether the band lies inside the active band count.
    pub active: bool,
    /// Exact device codes.
    pub codes: BandCodes,
    /// Frequency represented by the code.
    pub frequency_hz: f64,
    /// Q represented by the code.
    pub q: f64,
    /// Gain represented by the code.
    pub gain_db: f64,
    /// Filter kind name.
    pub kind: &'static str,
}

/// Decoded PEQ bank of one output in one snapshot.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeqBankStateV1 {
    /// Always [`PEQ_BANK_STATE_SCHEMA`].
    pub schema_version: &'static str,
    /// Evidence class of the decoded addresses.
    pub evidence_class: &'static str,
    /// Exact snapshot the bank was decoded from.
    pub snapshot_digest: String,
    /// Physical output, 1 through 6.
    pub target_output: u8,
    /// Direct parameter channel.
    pub parameter_channel: u8,
    /// PEQ on/off.
    pub eq_enabled: bool,
    /// Active band count, 0 through 9.
    pub eq_count: u8,
    /// All nine bands, active or not.
    pub bands: Vec<PeqBandStateV1>,
}

impl PeqBankStateV1 {
    /// Codes of one band, 1 through 9.
    ///
    /// # Panics
    ///
    /// Never for a band from 1 through 9.
    pub fn band(&self, band: u8) -> BandCodes {
        self.bands[usize::from(band - 1)].codes
    }
}

/// Decode the PEQ bank of one output from the transcribed layout.
///
/// # Errors
///
/// Rejects an invalid output, or any stored value outside its documented
/// device domain, which is evidence against the transcription.
pub fn decode_peq_bank(snapshot: &SnapshotV1, output: u8) -> Result<PeqBankStateV1, PeqBankError> {
    if !(1..=OUTPUT_COUNT).contains(&output) {
        return Err(PeqBankError::InvalidOutput(output));
    }
    let channel = output_channel(output);
    let read = |parameter: u8| -> Result<u16, PeqBankError> {
        let address =
            reviewed_address(channel, parameter).ok_or(PeqBankError::Unreviewed(parameter))?;
        let value = snapshot.read_reviewed(address);
        if value > address.field.device_max() {
            return Err(PeqBankError::OutOfDomain {
                label: address.field.label(),
                value,
                maximum: address.field.device_max(),
            });
        }
        Ok(value)
    };
    let eq_enabled = read(crate::layout::EQ_ENABLED_PARAMETER)? == 1;
    let eq_count = u8::try_from(read(crate::layout::EQ_COUNT_PARAMETER)?)
        .map_err(|_| PeqBankError::Unreviewed(crate::layout::EQ_COUNT_PARAMETER))?;
    let mut bands = Vec::with_capacity(usize::from(PEQ_BANDS));
    for band in 1..=PEQ_BANDS {
        let codes = BandCodes {
            frequency_code: read(band_parameter(band, BandField::Frequency))?,
            q_code: read(band_parameter(band, BandField::Q))?,
            gain_code: read(band_parameter(band, BandField::Gain))?,
            kind_code: read(band_parameter(band, BandField::Kind))?,
            slope_code: read(band_parameter(band, BandField::Slope))?,
        };
        bands.push(PeqBandStateV1 {
            band,
            active: band <= eq_count,
            codes,
            frequency_hz: frequency_hz(codes.frequency_code),
            q: q_value(codes.q_code),
            gain_db: gain_db(codes.gain_code),
            kind: match codes.kind_code {
                0 => "low_shelf",
                1 => "bell",
                _ => "high_shelf",
            },
        });
    }
    Ok(PeqBankStateV1 {
        schema_version: PEQ_BANK_STATE_SCHEMA,
        evidence_class: if output == 1 {
            "transcribed_layout_o1_peq9_device_observed_other_bands_unverified"
        } else {
            "transcribed_layout_unverified_on_named_device"
        },
        snapshot_digest: snapshot.digest().to_owned(),
        target_output: output,
        parameter_channel: channel,
        eq_enabled,
        eq_count,
        bands,
    })
}

/// PEQ bank decoding and construction failures.
#[derive(Debug, Error)]
pub enum PeqBankError {
    /// Output must be one through six.
    #[error("invalid DCX output {0}; expected 1 through 6")]
    InvalidOutput(u8),
    /// Band must be one through nine.
    #[error("invalid PEQ band {0}; expected 1 through 9")]
    InvalidBand(u8),
    /// A PEQ parameter escaped the reviewed allowlist.
    #[error("PEQ parameter {0:#04x} is not reviewed")]
    Unreviewed(u8),
    /// A stored or requested value is outside the device domain.
    #[error("{label} value {value} exceeds device maximum {maximum}")]
    OutOfDomain {
        /// Field label.
        label: String,
        /// Observed value.
        value: u16,
        /// Device maximum.
        maximum: u16,
    },
    /// A typed action could not be represented.
    #[error(transparent)]
    Protocol(#[from] crate::protocol::ProtocolError),
}

/// One output's ordered PEQ direct actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeqBankDocumentV2 {
    /// Physical output number, one through six.
    pub target_output: u8,
    /// DCX direct-parameter channel, five through ten.
    pub parameter_channel: u8,
    /// Ordered PEQ on/off, band count, and band-field actions.
    pub actions: Vec<DirectParameterAction>,
}

/// One digest-bound desired multi-band PEQ state for one output.
///
/// The digest covers a domain separator, the profile identity and revision,
/// the output/channel pair, and every ordered action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesiredPeqBankProfileV2 {
    schema_version: String,
    #[serde(rename = "profileID")]
    profile_id: String,
    revision: String,
    digest: String,
    document: PeqBankDocumentV2,
}

impl DesiredPeqBankProfileV2 {
    /// Bind a reviewed PEQ bank document to its stable profile identity.
    ///
    /// # Errors
    ///
    /// Rejects unbounded identity strings, a mismatched output/channel pair,
    /// an empty or oversized action list, duplicate addresses, any address
    /// outside PEQ on/off, band count, and band fields, out-of-domain values,
    /// and every boost.
    pub fn new(
        profile_id: String,
        revision: String,
        document: PeqBankDocumentV2,
    ) -> Result<Self, DesiredProfileError> {
        validate_text("profile_id", &profile_id)?;
        validate_text("revision", &revision)?;
        validate_bank_document(&document)?;
        let digest = bank_digest(&profile_id, &revision, &document)?;
        Ok(Self {
            schema_version: DESIRED_PROFILE_V2_SCHEMA.to_owned(),
            profile_id,
            revision,
            digest,
            document,
        })
    }

    /// Parse and fully revalidate one serialized v2 envelope.
    ///
    /// # Errors
    ///
    /// Rejects oversized/malformed JSON, unknown fields, unsupported schema,
    /// invalid actions, or digest mismatch.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DesiredProfileError> {
        if bytes.len() > MAX_DESIRED_PROFILE_BYTES {
            return Err(DesiredProfileError::JsonTooLarge(bytes.len()));
        }
        let wire: BankProfileWire = serde_json::from_slice(bytes)?;
        if wire.schema_version != DESIRED_PROFILE_V2_SCHEMA {
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
            PeqBankDocumentV2 {
                target_output: wire.document.target_output,
                parameter_channel: wire.document.parameter_channel,
                actions,
            },
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
    pub const fn document(&self) -> &PeqBankDocumentV2 {
        &self.document
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BankProfileWire {
    schema_version: String,
    #[serde(rename = "profileID")]
    profile_id: String,
    revision: String,
    digest: String,
    document: BankDocumentWire,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BankDocumentWire {
    target_output: u8,
    parameter_channel: u8,
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

fn validate_bank_document(document: &PeqBankDocumentV2) -> Result<(), DesiredProfileError> {
    if !(1..=OUTPUT_COUNT).contains(&document.target_output)
        || document.parameter_channel != output_channel(document.target_output)
    {
        return Err(DesiredProfileError::UnsupportedDocument(
            "v2 profile output must be 1 through 6 with its exact parameter channel",
        ));
    }
    if document.actions.is_empty() || document.actions.len() > MAX_BANK_ACTIONS {
        return Err(DesiredProfileError::UnsupportedDocument(
            "v2 profile must carry 1 through 47 PEQ actions",
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for action in &document.actions {
        if action.channel() != document.parameter_channel || !seen.insert(action.parameter()) {
            return Err(DesiredProfileError::UnsupportedDocument(
                "v2 profile actions must address distinct parameters on the document channel",
            ));
        }
        let Some(address) = reviewed_address(action.channel(), action.parameter()) else {
            return Err(DesiredProfileError::UnsupportedDocument(
                "v2 profile action escapes PEQ on/off, band count, and band fields",
            ));
        };
        if address.field == OutputField::Mute {
            return Err(DesiredProfileError::UnsupportedDocument(
                "v2 PEQ profile cannot carry the output mute",
            ));
        }
        if action.value() > address.field.device_max() {
            return Err(DesiredProfileError::UnsupportedDocument(
                "v2 profile action value is outside its device domain",
            ));
        }
        if address.field.is_gain() && action.value() > UNITY_GAIN_CODE {
            return Err(DesiredProfileError::UnsupportedDocument(
                "v2 profile is cut-only; boosts have no lane",
            ));
        }
    }
    Ok(())
}

fn bank_digest(
    profile_id: &str,
    revision: &str,
    document: &PeqBankDocumentV2,
) -> Result<String, DesiredProfileError> {
    let mut hasher = Sha256::new();
    hasher.update(DESIRED_PROFILE_V2_DOMAIN);
    for text in [profile_id, revision] {
        let length = u8::try_from(text.len())
            .map_err(|_| DesiredProfileError::InvalidIdentity { field: "text" })?;
        hasher.update([length]);
        hasher.update(text.as_bytes());
    }
    hasher.update([document.target_output, document.parameter_channel]);
    let count = u8::try_from(document.actions.len())
        .map_err(|_| DesiredProfileError::UnsupportedDocument("too many desired actions"))?;
    hasher.update([count]);
    for action in &document.actions {
        hasher.update([action.channel(), action.parameter()]);
        hasher.update(action.value().to_be_bytes());
    }
    Ok(format!("sha256/{:x}", hasher.finalize()))
}

/// Either supported desired-profile envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesiredProfile {
    /// Exact O1/PEQ9 v1 envelope.
    V1(DesiredPeqProfileV1),
    /// Multi-band output PEQ v2 envelope.
    V2(DesiredPeqBankProfileV2),
}

#[derive(Deserialize)]
struct SchemaPeek {
    #[serde(rename = "schemaVersion")]
    schema_version: String,
}

impl DesiredProfile {
    /// Parse either envelope by its declared schema.
    ///
    /// # Errors
    ///
    /// Rejects an unknown schema and every error of the selected parser.
    pub fn from_json(bytes: &[u8]) -> Result<Self, DesiredProfileError> {
        if bytes.len() > MAX_DESIRED_PROFILE_BYTES {
            return Err(DesiredProfileError::JsonTooLarge(bytes.len()));
        }
        let peek: SchemaPeek = serde_json::from_slice(bytes)?;
        match peek.schema_version.as_str() {
            DESIRED_PROFILE_SCHEMA => Ok(Self::V1(DesiredPeqProfileV1::from_json(bytes)?)),
            DESIRED_PROFILE_V2_SCHEMA => Ok(Self::V2(DesiredPeqBankProfileV2::from_json(bytes)?)),
            _ => Err(DesiredProfileError::UnsupportedSchema(peek.schema_version)),
        }
    }

    /// Canonical digest of the envelope.
    pub fn digest(&self) -> &str {
        match self {
            Self::V1(profile) => profile.digest(),
            Self::V2(profile) => profile.digest(),
        }
    }

    /// Ordered desired actions.
    pub fn actions(&self) -> &[DirectParameterAction] {
        match self {
            Self::V1(profile) => &profile.document().actions,
            Self::V2(profile) => &profile.document().actions,
        }
    }

    /// Declared schema string.
    pub const fn schema(&self) -> &'static str {
        match self {
            Self::V1(_) => DESIRED_PROFILE_SCHEMA,
            Self::V2(_) => DESIRED_PROFILE_V2_SCHEMA,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{EQ_COUNT_PARAMETER, EQ_ENABLED_PARAMETER};

    fn document(actions: Vec<DirectParameterAction>) -> PeqBankDocumentV2 {
        PeqBankDocumentV2 {
            target_output: 4,
            parameter_channel: 8,
            actions,
        }
    }

    fn notch_actions() -> Vec<DirectParameterAction> {
        let mut actions = BandCodes {
            frequency_code: 200,
            q_code: 40,
            gain_code: 90,
            kind_code: 1,
            slope_code: 0,
        }
        .actions(4, 1)
        .unwrap();
        actions.push(DirectParameterAction::new(8, EQ_COUNT_PARAMETER, 1).unwrap());
        actions.push(DirectParameterAction::new(8, EQ_ENABLED_PARAMETER, 1).unwrap());
        actions
    }

    #[test]
    fn code_grids_round_trip_every_code() {
        for code in 0..=MAX_FREQUENCY_CODE {
            assert_eq!(frequency_code(frequency_hz(code)), code);
        }
        for code in 0..=MAX_Q_CODE {
            assert_eq!(q_code(q_value(code)), code);
        }
        for code in 0..=MAX_GAIN_CODE {
            assert_eq!(gain_code(gain_db(code)), code);
        }
    }

    #[test]
    fn v2_profile_round_trips_and_detects_tampering() {
        let profile = DesiredPeqBankProfileV2::new(
            "o4-feedback".into(),
            "r1".into(),
            document(notch_actions()),
        )
        .unwrap();
        let json = profile.to_json().unwrap();
        assert_eq!(DesiredPeqBankProfileV2::from_json(&json).unwrap(), profile);
        assert_eq!(
            DesiredProfile::from_json(&json).unwrap(),
            DesiredProfile::V2(profile.clone())
        );

        let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        value["document"]["actions"][0]["value"] = 199.into();
        assert!(matches!(
            DesiredPeqBankProfileV2::from_json(&serde_json::to_vec(&value).unwrap()),
            Err(DesiredProfileError::DigestMismatch { .. })
        ));
        let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        value["schemaVersion"] = "dcx.desired-profile/v9".into();
        assert!(matches!(
            DesiredProfile::from_json(&serde_json::to_vec(&value).unwrap()),
            Err(DesiredProfileError::UnsupportedSchema(_))
        ));
    }

    #[test]
    fn v2_profile_rejects_boosts_mutes_foreign_channels_and_unreviewed_addresses() {
        let boost = DirectParameterAction::new(8, band_parameter(1, BandField::Gain), 151).unwrap();
        let mute = DirectParameterAction::new(8, 0x03, 0).unwrap();
        let foreign =
            DirectParameterAction::new(5, band_parameter(1, BandField::Gain), 100).unwrap();
        let crossover = DirectParameterAction::new(8, 0x42, 0).unwrap();
        let count = DirectParameterAction::new(8, EQ_COUNT_PARAMETER, 10).unwrap();
        for action in [boost, mute, foreign, crossover, count] {
            assert!(matches!(
                DesiredPeqBankProfileV2::new("p".into(), "r".into(), document(vec![action])),
                Err(DesiredProfileError::UnsupportedDocument(_))
            ));
        }
        let mut duplicate = notch_actions();
        duplicate.push(duplicate[0]);
        assert!(DesiredPeqBankProfileV2::new("p".into(), "r".into(), document(duplicate)).is_err());
        assert!(
            DesiredPeqBankProfileV2::new(
                "p".into(),
                "r".into(),
                PeqBankDocumentV2 {
                    target_output: 4,
                    parameter_channel: 5,
                    actions: notch_actions(),
                }
            )
            .is_err()
        );
    }
}
