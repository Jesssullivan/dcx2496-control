//! Legalab `DcxOutputProfileV1` targets: strict parsing, offline
//! verification, and a budgeted chain of typed write transactions that moves
//! one output from a snapshot to the profile.
//!
//! Legalab owns the profile contract (`legalab.dcx-output-profile/v1`, one
//! output's typed state with the exact device codes) and the operator
//! ceremony. This module only reads it. Legalab ruling
//! `dec-scene-dcx-apply-20261021` (operator interview, 2026-10-21) lets the
//! scene engine apply a profile's PEQ, gain and crossover diffs through this
//! chain; every other field (mute, source, polarity, delay, limiter) is
//! verify-only, and a plan refuses while one of them differs.
//!
//! [`plan_profile_chain`] is offline. Each step is one ordinary
//! `dcx.write-plan/v1` document planned by [`plan_writes`] against the
//! previous step's predicted snapshot, so each step is exactly one `write
//! apply` transaction (5 remote-mode writes, one direct frame) with its exact
//! touched-address rollback (3 remote-mode writes), and every write-policy
//! rule holds per step: at most a 3 dB summed rise, no level above 0 dB,
//! cut-only EQ, O5/O6 muted and never addressed, no unmute and no source
//! change. The planner sequences what one step may not do:
//!
//! - a cut band whose shape (frequency, Q, kind or slope) changes is first
//!   raised to 0 dB in moves of at most the step budget, and is retuned only
//!   from 0 dB, in a later step (2026-10-21: band 7 went -6, -3, 0, then
//!   420 Hz);
//! - a band leaving the active set is raised to 0 dB before the count drops;
//! - a band entering the active set is written before the count rises (the
//!   same step may do both, because the frame writes band fields first);
//! - deeper cuts, lower gains and crossover moves cost nothing and go first.
//!
//! The device refuses a band count of 9 ([`MAX_ACTIVE_PEQ_BANDS`]), so a
//! profile with more than 8 active bands is refused.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    SnapshotV1,
    layout::{
        BandField, DELAY_ENABLED_PARAMETER, EQ_COUNT_PARAMETER, EQ_ENABLED_PARAMETER,
        GAIN_PARAMETER, HIGH_PASS_FREQUENCY_PARAMETER, HIGH_PASS_TYPE_PARAMETER,
        LIMITER_ENABLED_PARAMETER, LIMITER_THRESHOLD_PARAMETER, LONG_DELAY_PARAMETER,
        LOW_PASS_FREQUENCY_PARAMETER, LOW_PASS_TYPE_PARAMETER, MAX_ACTIVE_PEQ_BANDS,
        MAX_FREQUENCY_CODE, MAX_GAIN_CODE, MAX_LIMITER_THRESHOLD_CODE, MAX_Q_CODE, MUTE_PARAMETER,
        PEQ_BANDS, POLARITY_PARAMETER, SHORT_DELAY_PARAMETER, SOURCE_PARAMETER, UNITY_GAIN_CODE,
        band_parameter, output_channel, reviewed_address,
    },
    peq_bank::{frequency_hz, gain_db, q_value},
    routing::OutputSource,
    typed_write::{
        APPLY_REMOTE_MODE_WRITES, CrossoverFilter, CrossoverSide, LONG_DELAY_STEP_MM,
        MAX_GAIN_STEP_DB, PeqKind, ROLLBACK_REMOTE_MODE_WRITES, SHORT_DELAY_STEP_MM, TypedChange,
        TypedWriteError, WRITE_PLAN_SCHEMA, WritePlanV1, WritePolicyV1, WriteTransactionV1,
        plan_writes, require_reserved_muted,
    },
};

/// Legalab profile schema this module reads.
pub const OUTPUT_PROFILE_SCHEMA: &str = "legalab.dcx-output-profile/v1";
/// Legalab profile contract type.
pub const OUTPUT_PROFILE_CONTRACT: &str = "DcxOutputProfileV1";
/// Versioned chain (dry-run) receipt.
pub const PROFILE_CHAIN_SCHEMA: &str = "dcx.profile-chain/v1";
/// Versioned offline profile verification receipt.
pub const PROFILE_VERIFICATION_SCHEMA: &str = "dcx.profile-verification/v1";
/// Largest serialized profile.
pub const MAX_OUTPUT_PROFILE_BYTES: usize = 64 * 1024;
/// Largest number of transactions in one chain.
pub const MAX_CHAIN_STEPS: usize = 64;

const EVIDENCE_UNVERIFIED: &str = "transcribed_layout_unverified_on_named_device";
const FLOAT_TOLERANCE: f64 = 1e-6;

/// The profile's capture provenance; read, never interpreted beyond its
/// digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileCaptureV1 {
    /// Always `dcx.typed-state/v1`.
    pub typed_state_schema: String,
    /// Digest of the snapshot the profile was transcribed from.
    pub snapshot_digest: String,
    /// The show's evidence class, verbatim.
    pub evidence_class: String,
    /// Always `observed-readback`.
    pub provenance: String,
    /// When the durable copy was made.
    pub observed_at: String,
    /// Tool that produced the typed show.
    pub tool: String,
    /// Lane file reference (outside Git).
    pub lane_ref: String,
    /// SHA-256 of the lane file.
    pub lane_file_sha256: String,
}

/// One crossover side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileCrossoverV1 {
    /// Filter type.
    #[serde(rename = "type")]
    pub filter: CrossoverFilter,
    /// Corner, milli-hertz, rounded from the code.
    pub frequency_milli_hz: u32,
    /// Exact frequency code.
    pub code: u16,
}

/// Output delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileDelayV1 {
    /// Delay switch.
    pub enabled: bool,
    /// Short delay, millimetres.
    pub short_mm: u32,
    /// Long delay, millimetres.
    pub long_mm: u32,
}

/// Output limiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileLimiterV1 {
    /// Limiter switch.
    pub enabled: bool,
    /// Threshold, tenths of a dB.
    pub threshold_db_tenths: i16,
}

/// Exact device codes of one band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileBandCodesV1 {
    /// Frequency code, 0 through 320.
    pub frequency: u16,
    /// Gain code, 0 through 300 (150 is 0 dB).
    pub gain: u16,
    /// Q code, 0 through 40.
    pub q: u16,
    /// Kind code, 0 through 2.
    pub kind: u16,
    /// Shelf slope code, 0 (6 dB/oct) or 1 (12 dB/oct).
    pub slope: u16,
}

/// One active band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileBandV1 {
    /// Band, 1 through 8.
    pub band: u8,
    /// Filter kind.
    pub kind: PeqKind,
    /// Frequency, milli-hertz, rounded from the code.
    pub frequency_milli_hz: u32,
    /// Gain, tenths of a dB.
    pub gain_db_tenths: i16,
    /// Q, thousandths, rounded from the code.
    pub q_thousandths: u32,
    /// Shelf slope in dB/oct, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shelf_slope_db: Option<u8>,
    /// Exact codes.
    pub codes: ProfileBandCodesV1,
}

/// PEQ section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePeqV1 {
    /// PEQ switch.
    pub enabled: bool,
    /// Active band count, 0 through 8.
    pub count: u8,
    /// Bands 1 through `count`, in order.
    pub bands: Vec<ProfileBandV1>,
}

/// One output's typed state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileStateV1 {
    /// Output mute (verify-only).
    pub muted: bool,
    /// Output source (verify-only; reviewed on O3/O4 only).
    pub source: OutputSource,
    /// Output gain, tenths of a dB.
    pub gain_db_tenths: i16,
    /// Polarity (verify-only).
    pub polarity_inverted: bool,
    /// High-pass side.
    pub high_pass: ProfileCrossoverV1,
    /// Low-pass side.
    pub low_pass: ProfileCrossoverV1,
    /// Delay (verify-only).
    pub delay: ProfileDelayV1,
    /// Limiter (verify-only).
    pub limiter: ProfileLimiterV1,
    /// PEQ.
    pub peq: ProfilePeqV1,
}

/// Legalab `DcxOutputProfileV1` (`legalab.dcx-output-profile/v1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DcxOutputProfileV1 {
    /// Always [`OUTPUT_PROFILE_SCHEMA`].
    pub schema_version: String,
    /// Always [`OUTPUT_PROFILE_CONTRACT`].
    pub contract_type: String,
    /// Profile identity, `dcx-o<output>-<name>-<8 hex>`.
    pub id: String,
    /// Title.
    pub title: String,
    /// Purpose.
    pub purpose: String,
    /// Output, 1 through 4.
    pub output: u8,
    /// Sink label.
    pub sink: String,
    /// Capture provenance.
    pub capture: ProfileCaptureV1,
    /// Typed state.
    pub state: ProfileStateV1,
    /// Limitations.
    pub limitations: String,
}

/// SHA-256 of the exact profile bytes, `sha256/<hex>`.
pub fn profile_digest(bytes: &[u8]) -> String {
    format!("sha256/{}", hex_lower(&Sha256::digest(bytes)))
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        use std::fmt::Write as _;
        let _ = write!(text, "{byte:02x}");
        text
    })
}

fn within_one(recorded: u32, exact: f64) -> bool {
    (f64::from(recorded) - exact.round()).abs() <= 1.0
}

impl DcxOutputProfileV1 {
    /// Parse and validate a strict bounded profile.
    ///
    /// # Errors
    ///
    /// Rejects oversized or malformed JSON, unknown fields, and every
    /// [`Self::validate`] refusal.
    pub fn from_json(bytes: &[u8]) -> Result<Self, OutputProfileError> {
        if bytes.len() > MAX_OUTPUT_PROFILE_BYTES {
            return Err(OutputProfileError::TooLarge(bytes.len()));
        }
        let profile: Self = serde_json::from_slice(bytes)?;
        profile.validate()?;
        Ok(profile)
    }

    /// Check the contract constants and enums, the output, the 8-band device
    /// limit, the band order, every code's domain, and that each recorded
    /// value is the value of its code. Codes are authoritative; values are
    /// checked to within one unit of rounding. This is narrower than the
    /// Legalab schema in one place: a band boost (gain code above 150) is
    /// refused, because the apply path is cut-only.
    ///
    /// # Errors
    ///
    /// Returns the first refusal.
    #[allow(clippy::too_many_lines)]
    pub fn validate(&self) -> Result<(), OutputProfileError> {
        let invalid = |reason: String| Err(OutputProfileError::Invalid(reason));
        if self.schema_version != OUTPUT_PROFILE_SCHEMA {
            return invalid(format!("schemaVersion {:?}", self.schema_version));
        }
        if self.contract_type != OUTPUT_PROFILE_CONTRACT {
            return invalid(format!("contractType {:?}", self.contract_type));
        }
        if (5..=6).contains(&self.output) {
            return Err(OutputProfileError::ReservedOutput(self.output));
        }
        if !(1..=4).contains(&self.output) {
            return invalid(format!("output {}", self.output));
        }
        let lower_hex = |text: &str| {
            text.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        let capture = &self.capture;
        let observed = capture.observed_at.as_bytes();
        let observed_ok = observed.len() == 17
            && observed.starts_with(b"20")
            && observed
                .iter()
                .enumerate()
                .all(|(index, byte)| match index {
                    4 | 7 => *byte == b'-',
                    10 => *byte == b'T',
                    13 => *byte == b':',
                    16 => *byte == b'Z',
                    // The schema's ranges: month [01]x, day [0-3]x, hour
                    // [0-2]x, minute [0-5]x.
                    5 => (b'0'..=b'1').contains(byte),
                    8 => (b'0'..=b'3').contains(byte),
                    11 => (b'0'..=b'2').contains(byte),
                    14 => (b'0'..=b'5').contains(byte),
                    _ => byte.is_ascii_digit(),
                });
        let lengths = [
            ("title", self.title.chars().count(), 80),
            ("purpose", self.purpose.chars().count(), 600),
            ("limitations", self.limitations.chars().count(), 1600),
            ("capture.tool", capture.tool.chars().count(), 200),
            ("capture.laneRef", capture.lane_ref.chars().count(), 200),
        ];
        if let Some((label, length, maximum)) = lengths
            .into_iter()
            .find(|(_, length, maximum)| *length == 0 || length > maximum)
        {
            return invalid(format!(
                "{label} has {length} characters; 1 through {maximum}"
            ));
        }
        if !["jbl-left", "jbl-right", "jbl-sub", "alto-pa"].contains(&self.sink.as_str())
            || capture.typed_state_schema != "dcx.typed-state/v1"
            || ![
                "transcribed_layout_unverified_on_named_device",
                "transcribed_layout_o1_peq9_device_observed_other_bands_unverified",
            ]
            .contains(&capture.evidence_class.as_str())
            || capture.provenance != "observed-readback"
            || !observed_ok
            || capture.lane_file_sha256.len() != 64
            || !lower_hex(&capture.lane_file_sha256)
        {
            return invalid(
                "sink, capture schema, evidence class, provenance, observedAt or laneFileSha256 is outside the contract".to_owned(),
            );
        }
        let digest_hex = self
            .capture
            .snapshot_digest
            .strip_prefix("sha256/")
            .filter(|hex| {
                hex.len() == 64
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            });
        let Some(digest_hex) = digest_hex else {
            return invalid("capture.snapshotDigest is not sha256/<64 hex>".to_owned());
        };
        let name = self
            .id
            .strip_prefix(&format!("dcx-o{}-", self.output))
            .and_then(|rest| rest.strip_suffix(&format!("-{}", &digest_hex[..8])));
        let name_ok = name.is_some_and(|name| {
            (1..=24).contains(&name.len())
                && name.starts_with(|c: char| c.is_ascii_lowercase())
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        });
        if !name_ok {
            return invalid(format!(
                "id {:?} does not name output {} and the capture digest",
                self.id, self.output
            ));
        }
        let state = &self.state;
        if !(-150..=150).contains(&state.gain_db_tenths) {
            return invalid(format!("gainDbTenths {}", state.gain_db_tenths));
        }
        for (label, side) in [("highPass", state.high_pass), ("lowPass", state.low_pass)] {
            if side.code > MAX_FREQUENCY_CODE
                || !within_one(side.frequency_milli_hz, frequency_hz(side.code) * 1000.0)
            {
                return invalid(format!(
                    "{label}: frequencyMilliHz {} is not the value of code {}",
                    side.frequency_milli_hz, side.code
                ));
            }
        }
        if !state.delay.short_mm.is_multiple_of(SHORT_DELAY_STEP_MM) || state.delay.short_mm > 4000
        {
            return invalid(format!("delay.shortMm {}", state.delay.short_mm));
        }
        if !state.delay.long_mm.is_multiple_of(LONG_DELAY_STEP_MM) || state.delay.long_mm > 200_000
        {
            return invalid(format!("delay.longMm {}", state.delay.long_mm));
        }
        if !(-240..=0).contains(&state.limiter.threshold_db_tenths) {
            return invalid(format!(
                "limiter.thresholdDbTenths {}",
                state.limiter.threshold_db_tenths
            ));
        }
        let peq = &state.peq;
        if peq.count > MAX_ACTIVE_PEQ_BANDS {
            return Err(OutputProfileError::DeviceBandCount(peq.count));
        }
        if peq.bands.len() != usize::from(peq.count) {
            return invalid(format!(
                "peq.bands holds {} bands; peq.count is {}",
                peq.bands.len(),
                peq.count
            ));
        }
        for (index, band) in peq.bands.iter().enumerate() {
            let number = index + 1;
            if usize::from(band.band) != number {
                return invalid(format!("peq.bands[{index}] is band {}", band.band));
            }
            let codes = band.codes;
            if codes.frequency > MAX_FREQUENCY_CODE
                || codes.gain > MAX_GAIN_CODE
                || codes.q > MAX_Q_CODE
                || codes.slope > 1
            {
                return invalid(format!("band {number}: a code is outside its domain"));
            }
            if PeqKind::from_code(codes.kind) != Some(band.kind) {
                return invalid(format!(
                    "band {number}: kind {:?} is not kind code {}",
                    band.kind, codes.kind
                ));
            }
            if codes.gain > UNITY_GAIN_CODE {
                return Err(OutputProfileError::EqBoost {
                    band: band.band,
                    gain_db: gain_db(codes.gain),
                });
            }
            if i32::from(band.gain_db_tenths) != i32::from(codes.gain) - i32::from(UNITY_GAIN_CODE)
            {
                return invalid(format!(
                    "band {number}: gainDbTenths {} is not gain code {}",
                    band.gain_db_tenths, codes.gain
                ));
            }
            if !within_one(
                band.frequency_milli_hz,
                frequency_hz(codes.frequency) * 1000.0,
            ) || !within_one(band.q_thousandths, q_value(codes.q) * 1000.0)
            {
                return invalid(format!(
                    "band {number}: frequency or Q is not the value of its code"
                ));
            }
            if let Some(slope) = band.shelf_slope_db {
                let code = match slope {
                    6 => 0,
                    12 => 1,
                    _ => return invalid(format!("band {number}: shelfSlopeDb {slope}")),
                };
                if code != codes.slope {
                    return invalid(format!(
                        "band {number}: shelfSlopeDb {slope} is not slope code {}",
                        codes.slope
                    ));
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn gain_code(&self) -> u16 {
        (i32::from(self.state.gain_db_tenths) + i32::from(UNITY_GAIN_CODE)) as u16
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn limiter_threshold_code(&self) -> u16 {
        (i32::from(self.state.limiter.threshold_db_tenths) + i32::from(MAX_LIMITER_THRESHOLD_CODE))
            as u16
    }

    fn target(&self) -> Target {
        let mut bands = [None; PEQ_BANDS as usize];
        for band in &self.state.peq.bands {
            let codes = band.codes;
            bands[usize::from(band.band - 1)] = Some(Band {
                frequency: codes.frequency,
                q: codes.q,
                gain: codes.gain,
                kind: codes.kind,
                slope: codes.slope,
            });
        }
        Target {
            gain: self.gain_code(),
            high_pass: (
                self.state.high_pass.filter.code(),
                self.state.high_pass.code,
            ),
            low_pass: (self.state.low_pass.filter.code(), self.state.low_pass.code),
            eq_enabled: u16::from(self.state.peq.enabled),
            eq_count: u16::from(self.state.peq.count),
            bands,
        }
    }

    /// Every profiled field this repository has a reviewed address for, with
    /// its scope: `apply` fields may be written by a chain, `verify_only`
    /// fields never are.
    fn fields(&self) -> Vec<(u8, u16, FieldScope)> {
        use FieldScope::{Apply, VerifyOnly};
        let state = &self.state;
        let mut fields = vec![(MUTE_PARAMETER, u16::from(state.muted), VerifyOnly)];
        if source_reviewed(self.output) {
            fields.push((SOURCE_PARAMETER, state.source.code(), VerifyOnly));
        }
        fields.extend([
            (GAIN_PARAMETER, self.gain_code(), Apply),
            (
                POLARITY_PARAMETER,
                u16::from(state.polarity_inverted),
                VerifyOnly,
            ),
            (
                DELAY_ENABLED_PARAMETER,
                u16::from(state.delay.enabled),
                VerifyOnly,
            ),
            (
                SHORT_DELAY_PARAMETER,
                u16::try_from(state.delay.short_mm / SHORT_DELAY_STEP_MM).unwrap_or(u16::MAX),
                VerifyOnly,
            ),
            (
                LONG_DELAY_PARAMETER,
                u16::try_from(state.delay.long_mm / LONG_DELAY_STEP_MM).unwrap_or(u16::MAX),
                VerifyOnly,
            ),
            (
                LIMITER_ENABLED_PARAMETER,
                u16::from(state.limiter.enabled),
                VerifyOnly,
            ),
            (
                LIMITER_THRESHOLD_PARAMETER,
                self.limiter_threshold_code(),
                VerifyOnly,
            ),
            (
                HIGH_PASS_TYPE_PARAMETER,
                state.high_pass.filter.code(),
                Apply,
            ),
            (HIGH_PASS_FREQUENCY_PARAMETER, state.high_pass.code, Apply),
            (LOW_PASS_TYPE_PARAMETER, state.low_pass.filter.code(), Apply),
            (LOW_PASS_FREQUENCY_PARAMETER, state.low_pass.code, Apply),
            (EQ_ENABLED_PARAMETER, u16::from(state.peq.enabled), Apply),
            (EQ_COUNT_PARAMETER, u16::from(state.peq.count), Apply),
        ]);
        for band in &state.peq.bands {
            let codes = band.codes;
            for (field, code) in [
                (BandField::Frequency, codes.frequency),
                (BandField::Q, codes.q),
                (BandField::Gain, codes.gain),
                (BandField::Kind, codes.kind),
                (BandField::Slope, codes.slope),
            ] {
                fields.push((band_parameter(band.band, field), code, Apply));
            }
        }
        fields
    }
}

const fn source_reviewed(output: u8) -> bool {
    output == 3 || output == 4
}

/// Whether a chain may write a profiled field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldScope {
    /// PEQ, output gain or crossover: a chain may write it.
    Apply,
    /// Mute, source, polarity, delay or limiter: verified, never written.
    VerifyOnly,
}

/// One profiled field compared with a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileFieldCheckV1 {
    /// Stable field label.
    pub field: String,
    /// Whether a chain may write it.
    pub scope: FieldScope,
    /// Profile code.
    pub expected_code: u16,
    /// Snapshot code.
    pub observed_code: u16,
    /// Whether they are equal.
    pub matches: bool,
}

/// Offline comparison of one snapshot with one profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileVerificationV1 {
    /// Always [`PROFILE_VERIFICATION_SCHEMA`].
    pub schema: &'static str,
    /// `match` only when every reviewed profiled field matches; else `drift`.
    pub status: &'static str,
    /// Evidence class of the decoded addresses.
    pub evidence_class: &'static str,
    /// Profile identity.
    pub profile_id: String,
    /// SHA-256 of the exact profile bytes.
    pub profile_digest: String,
    /// Output.
    pub output: u8,
    /// Snapshot digest.
    pub snapshot_digest: String,
    /// Labels of the fields that differ.
    pub drift: Vec<String>,
    /// Every compared field, in profile order.
    pub fields: Vec<ProfileFieldCheckV1>,
    /// Profiled fields without a reviewed address, never compared (the O1/O2
    /// source). Inactive bands' stored values are not profiled.
    pub unreviewed_fields: Vec<&'static str>,
}

impl ProfileVerificationV1 {
    /// Whether every reviewed profiled field matches.
    pub fn matches(&self) -> bool {
        self.drift.is_empty()
    }

    /// Whether every reviewed profiled field matches, except, when
    /// `held_muted`, the mute of an output a chain left muted.
    pub fn matches_holding_mute(&self, held_muted: bool) -> bool {
        self.drift.iter().all(|field| held_muted && field == "mute")
    }

    fn drift_in(&self, scope: FieldScope) -> Vec<String> {
        self.fields
            .iter()
            .filter(|field| field.scope == scope && !field.matches)
            .map(|field| field.field.clone())
            .collect()
    }
}

fn read(
    snapshot: &SnapshotV1,
    channel: u8,
    parameter: u8,
) -> Result<(String, u16), OutputProfileError> {
    let address = reviewed_address(channel, parameter)
        .ok_or(OutputProfileError::Unreviewed { channel, parameter })?;
    Ok((address.field.label(), snapshot.read_reviewed(address)))
}

/// Compare every reviewed profiled field of `profile` with `snapshot`.
///
/// # Errors
///
/// Rejects only an internal address-table inconsistency.
pub fn verify_profile(
    snapshot: &SnapshotV1,
    profile: &DcxOutputProfileV1,
    profile_digest: &str,
) -> Result<ProfileVerificationV1, OutputProfileError> {
    let channel = output_channel(profile.output);
    let mut fields = Vec::new();
    for (parameter, expected_code, scope) in profile.fields() {
        let (field, observed_code) = read(snapshot, channel, parameter)?;
        fields.push(ProfileFieldCheckV1 {
            field,
            scope,
            expected_code,
            observed_code,
            matches: expected_code == observed_code,
        });
    }
    let drift = fields
        .iter()
        .filter(|field| !field.matches)
        .map(|field| field.field.clone())
        .collect::<Vec<_>>();
    Ok(ProfileVerificationV1 {
        schema: PROFILE_VERIFICATION_SCHEMA,
        status: if drift.is_empty() { "match" } else { "drift" },
        evidence_class: EVIDENCE_UNVERIFIED,
        profile_id: profile.id.clone(),
        profile_digest: profile_digest.to_owned(),
        output: profile.output,
        snapshot_digest: snapshot.digest().to_owned(),
        drift,
        fields,
        unreviewed_fields: if source_reviewed(profile.output) {
            Vec::new()
        } else {
            vec!["source"]
        },
    })
}

/// One band's codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Band {
    frequency: u16,
    q: u16,
    gain: u16,
    kind: u16,
    slope: u16,
}

impl Band {
    fn same_shape(self, other: Self) -> bool {
        self.frequency == other.frequency
            && self.q == other.q
            && self.kind == other.kind
            && self.slope == other.slope
    }

    fn differing_fields(self, other: Self) -> usize {
        [
            self.frequency != other.frequency,
            self.q != other.q,
            self.gain != other.gain,
            self.kind != other.kind,
            self.slope != other.slope,
        ]
        .into_iter()
        .filter(|differs| *differs)
        .count()
    }
}

/// The apply-scope codes of one output.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    gain: u16,
    high_pass: (u16, u16),
    low_pass: (u16, u16),
    eq_enabled: u16,
    eq_count: u16,
    bands: [Option<Band>; PEQ_BANDS as usize],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OutputState {
    gain: u16,
    high_pass: (u16, u16),
    low_pass: (u16, u16),
    eq_enabled: u16,
    eq_count: u16,
    bands: [Band; PEQ_BANDS as usize],
}

impl OutputState {
    fn read(snapshot: &SnapshotV1, output: u8) -> Result<Self, OutputProfileError> {
        let channel = output_channel(output);
        let code = |parameter| read(snapshot, channel, parameter).map(|(_, code)| code);
        let mut bands = [Band {
            frequency: 0,
            q: 0,
            gain: 0,
            kind: 0,
            slope: 0,
        }; PEQ_BANDS as usize];
        for (index, band) in bands.iter_mut().enumerate() {
            let number = u8::try_from(index + 1).unwrap_or(PEQ_BANDS);
            *band = Band {
                frequency: code(band_parameter(number, BandField::Frequency))?,
                q: code(band_parameter(number, BandField::Q))?,
                gain: code(band_parameter(number, BandField::Gain))?,
                kind: code(band_parameter(number, BandField::Kind))?,
                slope: code(band_parameter(number, BandField::Slope))?,
            };
        }
        Ok(Self {
            gain: code(GAIN_PARAMETER)?,
            high_pass: (
                code(HIGH_PASS_TYPE_PARAMETER)?,
                code(HIGH_PASS_FREQUENCY_PARAMETER)?,
            ),
            low_pass: (
                code(LOW_PASS_TYPE_PARAMETER)?,
                code(LOW_PASS_FREQUENCY_PARAMETER)?,
            ),
            eq_enabled: code(EQ_ENABLED_PARAMETER)?,
            eq_count: code(EQ_COUNT_PARAMETER)?,
            bands,
        })
    }

    fn band(&self, number: u8) -> Band {
        self.bands[usize::from(number - 1)]
    }

    fn reached(&self, target: &Target) -> bool {
        self.gain == target.gain
            && self.high_pass == target.high_pass
            && self.low_pass == target.low_pass
            && self.eq_enabled == target.eq_enabled
            && self.eq_count == target.eq_count
            && target
                .bands
                .iter()
                .zip(&self.bands)
                .all(|(goal, band)| goal.is_none_or(|goal| goal == *band))
    }
}

fn active(enabled: u16, count: u16, band: u8) -> bool {
    enabled != 0 && u16::from(band) <= count
}

/// Summed rise, in gain codes, from one output state to another, as the write
/// policy counts it: the output gain rise plus each band's effective-gain
/// rise, where an active band whose shape changes releases its whole cut.
fn rise_codes(before: &OutputState, after: &OutputState) -> u16 {
    let effective = |state: &OutputState, number: u8| {
        if active(state.eq_enabled, state.eq_count, number) {
            state.band(number).gain
        } else {
            UNITY_GAIN_CODE
        }
    };
    let mut rise = after.gain.saturating_sub(before.gain);
    for number in 1..=PEQ_BANDS {
        let moved = active(before.eq_enabled, before.eq_count, number)
            && !before.band(number).same_shape(after.band(number));
        let after_gain = if moved {
            UNITY_GAIN_CODE
        } else {
            effective(after, number)
        };
        rise += after_gain.saturating_sub(effective(before, number));
    }
    rise
}

/// The changes one step selects, bounded by the frame's action limit and the
/// step's rise budget (in gain codes).
struct Selection {
    output: u8,
    max_actions: usize,
    budget: u16,
    actions: usize,
    rise: u16,
    changes: Vec<TypedChange>,
}

impl Selection {
    fn add(&mut self, change: TypedChange, actions: usize, rise: u16) -> bool {
        if actions == 0
            || self.actions + actions > self.max_actions
            || self.rise + rise > self.budget
        {
            return false;
        }
        self.actions += actions;
        self.rise += rise;
        self.changes.push(change);
        true
    }

    fn remaining_rise(&self) -> u16 {
        self.budget - self.rise
    }

    fn band(&mut self, number: u8, current: Band, goal: Band, rise: u16) -> bool {
        let change = TypedChange::PeqBand {
            output: self.output,
            band: number,
            filter: PeqKind::from_code(goal.kind).unwrap_or(PeqKind::Bell),
            frequency_hz: frequency_hz(goal.frequency),
            gain_db: gain_db(goal.gain),
            q: q_value(goal.q),
            shelf_slope_db: Some(if goal.slope == 1 { 12 } else { 6 }),
        };
        self.add(change, current.differing_fields(goal), rise)
    }

    fn crossover(&mut self, side: CrossoverSide, current: (u16, u16), goal: (u16, u16)) {
        let actions = usize::from(current.0 != goal.0) + usize::from(current.1 != goal.1);
        if let Some(filter) = CrossoverFilter::from_code(goal.0) {
            self.add(
                TypedChange::Crossover {
                    output: self.output,
                    side,
                    filter,
                    frequency_hz: Some(frequency_hz(goal.1)),
                },
                actions,
                0,
            );
        }
    }

    fn gain(&mut self, current: u16, goal: u16) {
        let rise = goal.saturating_sub(current);
        self.add(
            TypedChange::OutputGain {
                output: self.output,
                gain_db: gain_db(goal),
            },
            usize::from(current != goal),
            rise,
        );
    }
}

/// Select one step's changes. See the module documentation for the order.
#[allow(clippy::too_many_lines)]
fn select_step(state: &OutputState, target: &Target, selection: &mut Selection) {
    // An active band holding a boost that is retuned or leaves the active
    // set is first lowered to 0 dB: a cut, so free.
    for number in 1..=PEQ_BANDS {
        let current = state.band(number);
        if current.gain <= UNITY_GAIN_CODE || !active(state.eq_enabled, state.eq_count, number) {
            continue;
        }
        let keeps = active(target.eq_enabled, target.eq_count, number)
            && target.bands[usize::from(number - 1)].is_some_and(|goal| current.same_shape(goal));
        if !keeps {
            let unity = Band {
                gain: UNITY_GAIN_CODE,
                ..current
            };
            selection.band(number, current, unity, 0);
        }
    }
    // Free moves: a lower gain, crossover moves, deeper cuts, inactive bands,
    // and retunes of a band already at 0 dB.
    if state.gain > target.gain {
        selection.gain(state.gain, target.gain);
    }
    if state.high_pass != target.high_pass {
        selection.crossover(CrossoverSide::HighPass, state.high_pass, target.high_pass);
    }
    if state.low_pass != target.low_pass {
        selection.crossover(CrossoverSide::LowPass, state.low_pass, target.low_pass);
    }
    let mut written = BTreeSet::new();
    for number in 1..=PEQ_BANDS {
        let Some(goal) = target.bands[usize::from(number - 1)] else {
            continue;
        };
        let current = state.band(number);
        if current == goal {
            continue;
        }
        let was_active = active(state.eq_enabled, state.eq_count, number);
        let will_be_active = active(target.eq_enabled, target.eq_count, number);
        if !was_active {
            if selection.band(number, current, goal, 0) {
                written.insert(number);
            }
        } else if will_be_active {
            if current.same_shape(goal) {
                if goal.gain < current.gain {
                    selection.band(number, current, goal, 0);
                }
            } else if current.gain == UNITY_GAIN_CODE {
                selection.band(number, current, goal, 0);
            }
        }
    }
    // PEQ switch and count, once every band leaving the active set sits at
    // 0 dB and every band entering it holds its target (or is written now).
    if state.eq_enabled != target.eq_enabled || state.eq_count != target.eq_count {
        let ready = (1..=PEQ_BANDS).all(|number| {
            let was_active = active(state.eq_enabled, state.eq_count, number);
            let will_be_active = active(target.eq_enabled, target.eq_count, number);
            let current = state.band(number);
            if was_active && !will_be_active {
                current.gain == UNITY_GAIN_CODE
            } else if !was_active && will_be_active {
                written.contains(&number)
                    || target.bands[usize::from(number - 1)].is_none_or(|goal| goal == current)
            } else {
                true
            }
        });
        if ready {
            let actions = usize::from(state.eq_enabled != target.eq_enabled)
                + usize::from(state.eq_count != target.eq_count);
            let mut changes = Vec::new();
            if state.eq_count != target.eq_count {
                changes.push(TypedChange::PeqBandCount {
                    output: selection.output,
                    count: u8::try_from(target.eq_count).unwrap_or(u8::MAX),
                });
            }
            if state.eq_enabled != target.eq_enabled {
                changes.push(TypedChange::PeqEnabled {
                    output: selection.output,
                    enabled: target.eq_enabled != 0,
                });
            }
            if selection.actions + actions <= selection.max_actions {
                selection.actions += actions;
                selection.changes.extend(changes);
            }
        }
    }
    // Rises, within the step budget, band by band, then the output gain.
    for number in 1..=PEQ_BANDS {
        if !active(state.eq_enabled, state.eq_count, number) {
            continue;
        }
        let current = state.band(number);
        let goal = target.bands[usize::from(number - 1)];
        let will_be_active = active(target.eq_enabled, target.eq_count, number);
        let goal_gain = match goal {
            Some(goal) if will_be_active && current.same_shape(goal) => goal.gain,
            _ => UNITY_GAIN_CODE,
        };
        if goal_gain > current.gain {
            let take = (goal_gain - current.gain).min(selection.remaining_rise());
            if take > 0 {
                let raised = Band {
                    gain: current.gain + take,
                    ..current
                };
                selection.band(number, current, raised, take);
            }
        }
    }
    if target.gain > state.gain {
        let take = (target.gain - state.gain).min(selection.remaining_rise());
        if take > 0 {
            selection.gain(state.gain, state.gain + take);
        }
    }
}

/// Policy echo in a chain receipt.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChainPolicyV1 {
    /// Largest summed rise per step in dB.
    pub max_gain_step_db: f64,
    /// Output gain ceiling in dB.
    pub gain_ceiling_db: f64,
    /// Largest number of actions per frame.
    pub max_actions: usize,
    /// Consent to loosening an active high-pass.
    pub allow_protection_change: bool,
}

/// Device traffic of the whole chain on success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChainBudgetV1 {
    /// Apply transactions.
    pub transactions: usize,
    /// Remote-mode writes per successful apply transaction.
    pub apply_remote_mode_writes_per_transaction: usize,
    /// Remote-mode writes per successful rollback transaction.
    pub rollback_remote_mode_writes_per_transaction: usize,
    /// Remote-mode writes of the whole chain on success.
    pub apply_remote_mode_writes_total: usize,
    /// Direct-parameter frames of the whole chain on success.
    pub direct_frames_total: usize,
}

/// One transaction of a chain.
#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileChainStepV1 {
    /// Step, from 1.
    pub step: usize,
    /// Snapshot the step is bound to (the previous step's prediction).
    pub baseline_snapshot_digest: String,
    /// Snapshot the readback must equal.
    pub predicted_snapshot_digest: String,
    /// Summed level rise of this step in dB, as the write policy counts it.
    pub rise_db: f64,
    /// Summed level rise of this step's exact rollback in dB, counted the
    /// same way. A rollback carries no policy, so this can exceed 3 dB: a
    /// step that adds several new cuts releases all of them on rollback.
    /// Like `rise_db` it counts gains only: crossover moves (and so the
    /// rollback of a low-pass tightening) are not in it.
    pub rollback_rise_db: f64,
    /// The `dcx.write-plan/v1` document this step plans.
    pub write_plan: WritePlanV1,
    /// The dry-run transaction, with its apply and rollback carriers.
    pub transaction: WriteTransactionV1,
}

/// A complete dry-run chain from a snapshot to a profile.
#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileChainV1 {
    /// Always [`PROFILE_CHAIN_SCHEMA`].
    pub schema: &'static str,
    /// Always true: planning opens no device.
    pub dry_run: bool,
    /// Evidence class of the addresses.
    pub evidence_class: &'static str,
    /// `already_matching` or `planned`.
    pub status: &'static str,
    /// Digest binding the profile, baseline, policy and every carrier; `profile
    /// apply` refuses unless its rebuilt chain has the same id.
    pub chain_id: String,
    /// Profile identity.
    pub profile_id: String,
    /// SHA-256 of the exact profile bytes.
    pub profile_digest: String,
    /// Output.
    pub output: u8,
    /// Snapshot the chain starts from.
    pub baseline_snapshot_digest: String,
    /// Predicted snapshot after the last step.
    pub final_snapshot_digest: String,
    /// The output reads muted and the profile is unmuted. A chain never
    /// unmutes: it runs on the muted output and leaves it muted, and the only
    /// drift it leaves is that mute. Unmuting stays with the caller's own
    /// reviewed mute write, after the chain.
    pub held_muted: bool,
    /// Policy the chain was planned under.
    pub policy: ChainPolicyV1,
    /// Device traffic on success.
    pub budget: ChainBudgetV1,
    /// Ordered transactions.
    pub steps: Vec<ProfileChainStepV1>,
    /// The profile compared with the predicted final snapshot.
    pub final_verification: ProfileVerificationV1,
}

impl ProfileChainV1 {
    /// Predicted snapshot after the last step (the baseline when no step).
    pub fn final_snapshot<'a>(&'a self, baseline: &'a SnapshotV1) -> &'a SnapshotV1 {
        self.steps
            .last()
            .map_or(baseline, |step| step.transaction.apply_plan.desired())
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn step_codes(policy: &WritePolicyV1) -> u16 {
    ((policy.max_gain_step_db * 10.0 + FLOAT_TOLERANCE).floor() as u16)
        .min((MAX_GAIN_STEP_DB * 10.0) as u16)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn ceiling_code(policy: &WritePolicyV1) -> u16 {
    (((policy.gain_ceiling_db + 15.0) * 10.0 + FLOAT_TOLERANCE).floor() as u16).min(UNITY_GAIN_CODE)
}

fn chain_id(
    profile_digest: &str,
    baseline: &SnapshotV1,
    policy: &ChainPolicyV1,
    steps: &[ProfileChainStepV1],
) -> String {
    let mut text = format!(
        "{PROFILE_CHAIN_SCHEMA}\nprofile {profile_digest}\nbaseline {}\npolicy {} {} {} {}\n",
        baseline.digest(),
        policy.max_gain_step_db,
        policy.gain_ceiling_db,
        policy.max_actions,
        policy.allow_protection_change
    );
    for step in steps {
        use std::fmt::Write as _;
        let _ = writeln!(
            text,
            "step {} {} {}",
            step.step,
            step.transaction.apply_plan.digest(),
            step.transaction.rollback_plan.digest()
        );
    }
    format!("sha256/{}", hex_lower(&Sha256::digest(text.as_bytes())))
}

/// Plan the ordered chain of typed write transactions that moves the
/// profile's output from `baseline` to the profile.
///
/// `policy` is the ordinary write policy; its routing and unmute consents are
/// ignored (a chain never unmutes or routes), and `allow_protection_change`
/// is both the plan field and the flag of every step.
///
/// # Errors
///
/// Rejects an out-of-bounds policy, O5 or O6 unmuted, a verify-only field
/// (mute, source, polarity, delay, limiter) that differs from the profile
/// (except a muted output under an unmuted profile, see
/// [`ProfileChainV1::held_muted`]), an
/// output gain the profile raises above the ceiling, a loosened active
/// low-pass without protection consent, a chain longer than
/// [`MAX_CHAIN_STEPS`], a state no admitted step can advance, and every
/// [`plan_writes`] refusal of a step.
#[allow(clippy::too_many_lines)]
pub fn plan_profile_chain(
    baseline: &SnapshotV1,
    profile: &DcxOutputProfileV1,
    profile_digest: &str,
    policy: &WritePolicyV1,
) -> Result<ProfileChainV1, OutputProfileError> {
    profile.validate()?;
    policy.validate()?;
    require_reserved_muted(baseline)?;
    let policy = WritePolicyV1 {
        allow_routing: false,
        allow_unmute: false,
        ..*policy
    };
    let echo = ChainPolicyV1 {
        max_gain_step_db: policy.max_gain_step_db,
        gain_ceiling_db: policy.gain_ceiling_db,
        max_actions: policy.max_actions,
        allow_protection_change: policy.allow_protection_change,
    };
    let start = verify_profile(baseline, profile, profile_digest)?;
    // A muted output under an unmuted profile is the one verify-only drift a
    // chain admits: it is the quieter side, and the chain keeps it.
    let held_muted = !profile.state.muted
        && read(baseline, output_channel(profile.output), MUTE_PARAMETER)?.1 == 1;
    let mut verify_only = start.drift_in(FieldScope::VerifyOnly);
    verify_only.retain(|field| !(held_muted && field == "mute"));
    if !verify_only.is_empty() {
        return Err(OutputProfileError::VerifyOnlyDrift(verify_only));
    }
    let output = profile.output;
    let target = profile.target();
    let initial = OutputState::read(baseline, output)?;
    if target.gain > initial.gain && target.gain > ceiling_code(&policy) {
        return Err(OutputProfileError::GainAboveCeiling {
            gain_db: gain_db(target.gain),
            ceiling_db: policy.gain_ceiling_db,
        });
    }
    // Crossover moves are not counted in the rise budget, so loosening an
    // active low-pass needs the same consent as loosening the high-pass.
    if initial.low_pass.0 != 0
        && (target.low_pass.0 != initial.low_pass.0 || target.low_pass.1 > initial.low_pass.1)
        && !policy.allow_protection_change
    {
        return Err(OutputProfileError::LowPassProtection(output));
    }
    let budget = step_codes(&policy);
    let mut steps: Vec<ProfileChainStepV1> = Vec::new();
    let mut state_snapshot = baseline.clone();
    loop {
        let state = OutputState::read(&state_snapshot, output)?;
        if state.reached(&target) {
            break;
        }
        if steps.len() == MAX_CHAIN_STEPS {
            return Err(OutputProfileError::TooManySteps(MAX_CHAIN_STEPS));
        }
        let mut selection = Selection {
            output,
            max_actions: policy.max_actions,
            budget,
            actions: 0,
            rise: 0,
            changes: Vec::new(),
        };
        select_step(&state, &target, &mut selection);
        if selection.changes.is_empty() {
            return Err(OutputProfileError::Stuck {
                step: steps.len() + 1,
                max_actions: policy.max_actions,
            });
        }
        let write_plan = WritePlanV1 {
            schema: WRITE_PLAN_SCHEMA.to_owned(),
            device_id: baseline.device().get(),
            allow_routing: false,
            allow_unmute: false,
            allow_protection_change: policy.allow_protection_change,
            changes: selection.changes,
        };
        let transaction = plan_writes(&state_snapshot, &write_plan, &policy)?;
        if transaction.writes.is_empty() {
            return Err(OutputProfileError::Stuck {
                step: steps.len() + 1,
                max_actions: policy.max_actions,
            });
        }
        let next = transaction.apply_plan.desired().clone();
        let after = OutputState::read(&next, output)?;
        steps.push(ProfileChainStepV1 {
            step: steps.len() + 1,
            baseline_snapshot_digest: state_snapshot.digest().to_owned(),
            predicted_snapshot_digest: next.digest().to_owned(),
            rise_db: f64::from(rise_codes(&state, &after)) / 10.0,
            rollback_rise_db: f64::from(rise_codes(&after, &state)) / 10.0,
            write_plan,
            transaction,
        });
        state_snapshot = next;
    }
    let final_verification = verify_profile(&state_snapshot, profile, profile_digest)?;
    if !final_verification.matches_holding_mute(held_muted) {
        return Err(OutputProfileError::FinalDrift(final_verification.drift));
    }
    let transactions = steps.len();
    Ok(ProfileChainV1 {
        schema: PROFILE_CHAIN_SCHEMA,
        dry_run: true,
        evidence_class: EVIDENCE_UNVERIFIED,
        status: if steps.is_empty() {
            "already_matching"
        } else {
            "planned"
        },
        chain_id: chain_id(profile_digest, baseline, &echo, &steps),
        profile_id: profile.id.clone(),
        profile_digest: profile_digest.to_owned(),
        output,
        baseline_snapshot_digest: baseline.digest().to_owned(),
        final_snapshot_digest: state_snapshot.digest().to_owned(),
        held_muted,
        policy: echo,
        budget: ChainBudgetV1 {
            transactions,
            apply_remote_mode_writes_per_transaction: APPLY_REMOTE_MODE_WRITES,
            rollback_remote_mode_writes_per_transaction: ROLLBACK_REMOTE_MODE_WRITES,
            apply_remote_mode_writes_total: transactions * APPLY_REMOTE_MODE_WRITES,
            direct_frames_total: transactions,
        },
        steps,
        final_verification,
    })
}

/// Profile and chain refusals. Nothing is clamped.
#[derive(Debug, Error)]
pub enum OutputProfileError {
    /// The profile JSON is too large.
    #[error("output profile has {0} bytes; maximum is 65536")]
    TooLarge(usize),
    /// Malformed JSON or unknown field.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// A contract rule failed.
    #[error("output profile refused: {0}")]
    Invalid(String),
    /// The profile names O5 or O6.
    #[error("output profile names reserved output O{0}; profiles cover O1-O4 only")]
    ReservedOutput(u8),
    /// More than 8 active bands.
    #[error(
        "output profile has {0} active PEQ bands; the DCX2496 refused a count of 9 on 2026-10-21, so at most 8"
    )]
    DeviceBandCount(u8),
    /// A band boost.
    #[error("output profile band {band} gain {gain_db} dB is a boost; the apply path is cut-only")]
    EqBoost { band: u8, gain_db: f64 },
    /// The output gain target is above the ceiling.
    #[error("output profile gain {gain_db} dB is above the {ceiling_db} dB ceiling")]
    GainAboveCeiling { gain_db: f64, ceiling_db: f64 },
    /// A verify-only field differs; a chain never writes it.
    #[error(
        "verify-only fields differ from the profile and a chain never writes them (mute, source, polarity, delay, limiter): {0:?}"
    )]
    VerifyOnlyDrift(Vec<String>),
    /// No admitted step advances the state.
    #[error(
        "step {step}: no admitted change advances the output: the next change does not fit {max_actions} actions (a band needs up to 5, the PEQ switch and count 2) or a 0 dB step budget"
    )]
    Stuck { step: usize, max_actions: usize },
    /// An active low-pass would be loosened without consent.
    #[error(
        "O{0} low-pass is active: changing its type, switching it off or raising its corner needs --allow-protection-change"
    )]
    LowPassProtection(u8),
    /// The chain would be too long.
    #[error("the chain needs more than {0} transactions")]
    TooManySteps(usize),
    /// The planned end state does not match the profile.
    #[error("the predicted final snapshot still differs from the profile: {0:?}")]
    FinalDrift(Vec<String>),
    /// An address escaped the reviewed table.
    #[error("channel {channel} parameter {parameter:#04x} is not a reviewed address")]
    Unreviewed { channel: u8, parameter: u8 },
    /// A step was refused by the write policy or the core carrier.
    #[error(transparent)]
    Write(#[from] TypedWriteError),
}

#[cfg(test)]
mod tests;
