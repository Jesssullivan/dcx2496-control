//! Typed DCX2496 parameter writes: the `dcx.write-plan/v1` document, its
//! exact encoders, the write policy, and offline readback verification.
//!
//! A write plan is a list of typed changes (PEQ band, PEQ on/off and band
//! count, crossover, output gain, mute, polarity, delay, limiter, input gain,
//! and, behind two explicit flags, an O3/O4 source). [`plan_writes`] encodes
//! each change into the reviewed direct-parameter addresses of
//! [`crate::layout`], drops values that already equal the baseline, applies
//! the write policy, orders the remaining actions so every intermediate device
//! state is the safer one (mutes first, level cuts before boosts, unmutes
//! last), and binds the result through
//! [`ApplyTransactionV1::stage_projected`]. The apply and rollback carriers it
//! returns are the ordinary [`ApplyPlanV1`] and [`RollbackPlanV1`], so the
//! live `control apply` path re-captures a fresh baseline, refuses a stale
//! plan, writes exactly one direct-parameter frame, and reads back the
//! complete state. Rollback re-applies the baseline values of exactly the
//! touched addresses in reverse order; it is never a stored-preset recall.
//!
//! Value encodings follow the public `UltradrivePi` `protocol.md` notes (see
//! `NOTICE`) and are implementation hypotheses: frequency
//! `20 * 2^(code / 32)` Hz, Q `0.1 * 10^(code / 20)`, gain `code / 10 - 15` dB,
//! limiter threshold `code / 10 - 24` dB, long delay `code * 50` mm, short
//! delay `code * 2` mm, crossover types 0 (off) through 10 in the order
//! BW6, BW12, Bessel 12, LR12, BW18, BW24, Bessel 24, LR24, BW48, LR48. The
//! limiter release grid (20 through 4000 ms over codes 0 through 251) is
//! assumed logarithmic and labelled as such. No address added by this lane
//! has named-device readback; [`verify_written_values`] is the offline check a
//! readback must pass before any address is called observed.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ApplyPlanV1, ApplyTransactionError, ApplyTransactionV1, RollbackPlanV1, SnapshotDiffError,
    SnapshotV1,
    layout::{
        BandField, DELAY_ENABLED_PARAMETER, EQ_COUNT_PARAMETER, EQ_ENABLED_PARAMETER,
        GAIN_INPUT_COUNT, GAIN_PARAMETER, HIGH_PASS_FREQUENCY_PARAMETER, HIGH_PASS_TYPE_PARAMETER,
        LIMITER_ENABLED_PARAMETER, LIMITER_RELEASE_PARAMETER, LIMITER_THRESHOLD_PARAMETER,
        LONG_DELAY_PARAMETER, LOW_PASS_FREQUENCY_PARAMETER, LOW_PASS_TYPE_PARAMETER, LinkEvidence,
        MAX_FREQUENCY_CODE, MAX_LEVEL_STEP_CODES, MAX_LIMITER_RELEASE_CODE,
        MAX_LIMITER_THRESHOLD_CODE, MAX_LONG_DELAY_CODE, MAX_SHORT_DELAY_CODE, MUTE_PARAMETER,
        OUTPUT_COUNT, OutputField, PEQ_BANDS, POLARITY_PARAMETER, ReviewedAddress,
        SHORT_DELAY_PARAMETER, SOURCE_PARAMETER, TYPED_WRITE_OUTPUTS, UNITY_GAIN_CODE,
        band_parameter, output_channel, reviewed_address, routing_rank,
    },
    peq_bank::{self, PeqBankError, decode_peq_bank, frequency_hz, gain_db, q_value},
    protocol::{DeviceId, DirectParameterAction, DirectParameterCommand, ProtocolError},
    routing::OutputSource,
};

/// Versioned typed write-plan document.
pub const WRITE_PLAN_SCHEMA: &str = "dcx.write-plan/v1";
/// Versioned typed write transaction (dry-run) receipt.
pub const WRITE_TRANSACTION_SCHEMA: &str = "dcx.write-transaction/v1";
/// Versioned decoded typed state receipt.
pub const TYPED_STATE_SCHEMA: &str = "dcx.typed-state/v1";
/// Versioned offline written-value verification receipt.
pub const WRITE_VERIFICATION_SCHEMA: &str = "dcx.write-verification/v1";
/// Largest serialized write plan.
pub const MAX_WRITE_PLAN_BYTES: usize = 64 * 1024;
/// Largest number of typed changes in one plan.
pub const MAX_WRITE_CHANGES: usize = 32;
/// Default largest number of direct-parameter actions in one apply frame.
pub const DEFAULT_MAX_ACTIONS: usize = 16;
/// Hard largest number of direct-parameter actions in one apply frame.
pub const HARD_MAX_ACTIONS: usize = 32;
/// Default and largest per-plan level increase in dB.
pub const MAX_GAIN_STEP_DB: f64 = 3.0;
/// Default and largest absolute level ceiling in dB.
pub const MAX_GAIN_CEILING_DB: f64 = 0.0;
/// Long delay step in millimetres.
pub const LONG_DELAY_STEP_MM: u32 = 50;
/// Short delay step in millimetres.
pub const SHORT_DELAY_STEP_MM: u32 = 2;
/// Remote-mode writes one live apply transaction sends on success:
/// `Transmit` and `ReceiveDirect` around the baseline dumps,
/// `ReceiveAndTransmit` before the frame, and `Transmit` and `ReceiveDirect`
/// around the readback dumps.
pub const APPLY_REMOTE_MODE_WRITES: usize = 5;
/// Remote-mode writes one live rollback transaction sends on success:
/// `ReceiveAndTransmit` before the frame, then `Transmit` and `ReceiveDirect`
/// around the readback dumps.
pub const ROLLBACK_REMOTE_MODE_WRITES: usize = 3;
/// Direct-parameter frames one live apply or rollback transaction sends.
pub const DIRECT_FRAMES_PER_TRANSACTION: usize = 1;

const EVIDENCE_UNVERIFIED: &str = "transcribed_layout_unverified_on_named_device";
const FLOAT_TOLERANCE: f64 = 1e-6;

/// PEQ filter kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeqKind {
    /// Low shelf, code 0.
    LowShelf,
    /// Peaking bell, code 1.
    Bell,
    /// High shelf, code 2.
    HighShelf,
}

impl PeqKind {
    /// Device code.
    pub const fn code(self) -> u16 {
        match self {
            Self::LowShelf => 0,
            Self::Bell => 1,
            Self::HighShelf => 2,
        }
    }

    /// Kind of a device code, if it is one.
    pub const fn from_code(code: u16) -> Option<Self> {
        match code {
            0 => Some(Self::LowShelf),
            1 => Some(Self::Bell),
            2 => Some(Self::HighShelf),
            _ => None,
        }
    }
}

/// Crossover side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossoverSide {
    /// High-pass (`0x42` type, `0x43` frequency).
    HighPass,
    /// Low-pass (`0x44` type, `0x45` frequency).
    LowPass,
}

impl CrossoverSide {
    const fn parameters(self) -> (u8, u8) {
        match self {
            Self::HighPass => (HIGH_PASS_TYPE_PARAMETER, HIGH_PASS_FREQUENCY_PARAMETER),
            Self::LowPass => (LOW_PASS_TYPE_PARAMETER, LOW_PASS_FREQUENCY_PARAMETER),
        }
    }
}

/// Crossover filter type; the discriminant is the device code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrossoverFilter {
    /// Off, code 0.
    #[serde(rename = "off")]
    Off = 0,
    /// Butterworth 6 dB/oct, code 1.
    #[serde(rename = "bw6")]
    Butterworth6 = 1,
    /// Butterworth 12 dB/oct, code 2.
    #[serde(rename = "bw12")]
    Butterworth12 = 2,
    /// Bessel 12 dB/oct, code 3.
    #[serde(rename = "bes12")]
    Bessel12 = 3,
    /// Linkwitz-Riley 12 dB/oct, code 4.
    #[serde(rename = "lr12")]
    LinkwitzRiley12 = 4,
    /// Butterworth 18 dB/oct, code 5.
    #[serde(rename = "bw18")]
    Butterworth18 = 5,
    /// Butterworth 24 dB/oct, code 6.
    #[serde(rename = "bw24")]
    Butterworth24 = 6,
    /// Bessel 24 dB/oct, code 7.
    #[serde(rename = "bes24")]
    Bessel24 = 7,
    /// Linkwitz-Riley 24 dB/oct, code 8.
    #[serde(rename = "lr24")]
    LinkwitzRiley24 = 8,
    /// Butterworth 48 dB/oct, code 9.
    #[serde(rename = "bw48")]
    Butterworth48 = 9,
    /// Linkwitz-Riley 48 dB/oct, code 10.
    #[serde(rename = "lr48")]
    LinkwitzRiley48 = 10,
}

impl CrossoverFilter {
    const ALL: [Self; 11] = [
        Self::Off,
        Self::Butterworth6,
        Self::Butterworth12,
        Self::Bessel12,
        Self::LinkwitzRiley12,
        Self::Butterworth18,
        Self::Butterworth24,
        Self::Bessel24,
        Self::LinkwitzRiley24,
        Self::Butterworth48,
        Self::LinkwitzRiley48,
    ];

    /// Device code.
    pub const fn code(self) -> u16 {
        self as u16
    }

    /// Filter of a device code, if it is one.
    pub fn from_code(code: u16) -> Option<Self> {
        Self::ALL.get(usize::from(code)).copied()
    }

    /// Stable short name, as in the JSON document.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Butterworth6 => "bw6",
            Self::Butterworth12 => "bw12",
            Self::Bessel12 => "bes12",
            Self::LinkwitzRiley12 => "lr12",
            Self::Butterworth18 => "bw18",
            Self::Butterworth24 => "bw24",
            Self::Bessel24 => "bes24",
            Self::LinkwitzRiley24 => "lr24",
            Self::Butterworth48 => "bw48",
            Self::LinkwitzRiley48 => "lr48",
        }
    }
}

/// DCX input with a reviewed gain address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputName {
    /// Input A, channel 1.
    A,
    /// Input B, channel 2.
    B,
    /// Input C, channel 3.
    C,
}

impl InputName {
    /// Direct-parameter channel.
    pub const fn channel(self) -> u8 {
        match self {
            Self::A => 1,
            Self::B => 2,
            Self::C => 3,
        }
    }
}

/// One typed change.
///
/// Every field is written only when the change names it; a change whose
/// encoded values all equal the baseline produces no action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TypedChange {
    /// All of one PEQ band's response fields.
    PeqBand {
        /// Output, 1 through 4.
        output: u8,
        /// Band, 1 through 9.
        band: u8,
        /// Filter kind.
        filter: PeqKind,
        /// Centre or corner frequency, 20 through 20,480 Hz.
        frequency_hz: f64,
        /// Gain, -15 through 0 dB: the apply path is cut-only.
        gain_db: f64,
        /// Q, 0.1 through 10.
        q: f64,
        /// Shelf slope, 6 or 12 dB/oct; absent leaves it untouched.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shelf_slope_db: Option<u8>,
    },
    /// PEQ on/off of one output.
    PeqEnabled {
        /// Output, 1 through 4.
        output: u8,
        /// On.
        enabled: bool,
    },
    /// Active PEQ band count of one output.
    PeqBandCount {
        /// Output, 1 through 4.
        output: u8,
        /// Bands 1 through `count` are active, 0 through 9.
        count: u8,
    },
    /// One crossover side of one output.
    Crossover {
        /// Output, 1 through 4.
        output: u8,
        /// High-pass or low-pass.
        side: CrossoverSide,
        /// Filter type.
        filter: CrossoverFilter,
        /// Corner frequency, 20 through 20,480 Hz; required unless `off`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frequency_hz: Option<f64>,
    },
    /// Output level gain.
    OutputGain {
        /// Output, 1 through 4.
        output: u8,
        /// Gain, -15 through +15 dB; a rise is bound by the policy step and
        /// ceiling.
        gain_db: f64,
    },
    /// Output mute.
    OutputMute {
        /// Output, 1 through 4.
        output: u8,
        /// Muted. Unmuting needs `allow_unmute` in the plan and the policy.
        muted: bool,
    },
    /// Output polarity.
    Polarity {
        /// Output, 1 through 4.
        output: u8,
        /// Inverted.
        inverted: bool,
    },
    /// Output delay.
    Delay {
        /// Output, 1 through 4.
        output: u8,
        /// Delay switch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        /// Long delay, a multiple of 50 mm from 0 through 200,000 mm.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        long_mm: Option<u32>,
        /// Short delay, a multiple of 2 mm from 0 through 4,000 mm.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        short_mm: Option<u32>,
    },
    /// Output limiter.
    Limiter {
        /// Output, 1 through 4.
        output: u8,
        /// Limiter switch; switching an enabled limiter off is refused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        /// Threshold, -24 through 0 dB in 0.1 dB steps.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        threshold_db: Option<f64>,
        /// Release, 20 through 4,000 ms (logarithmic grid hypothesis).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        release_ms: Option<f64>,
    },
    /// Input level gain.
    InputGain {
        /// Input A, B, or C.
        input: InputName,
        /// Gain, -15 through +15 dB; a rise is bound by the policy step and
        /// ceiling.
        gain_db: f64,
    },
    /// Output source, O3 or O4 only. Needs `allow_routing` in the plan and
    /// the policy.
    OutputSource {
        /// Output, 3 or 4.
        output: u8,
        /// Source.
        source: OutputSource,
    },
}

/// One complete typed write plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WritePlanV1 {
    /// Always [`WRITE_PLAN_SCHEMA`].
    pub schema: String,
    /// Expected DCX device address.
    pub device_id: u8,
    /// Explicit plan-level consent to an O3/O4 source change.
    #[serde(default)]
    pub allow_routing: bool,
    /// Explicit plan-level consent to unmuting one of O1 through O4.
    #[serde(default)]
    pub allow_unmute: bool,
    /// Explicit plan-level consent to loosening an active high-pass.
    #[serde(default)]
    pub allow_protection_change: bool,
    /// Ordered typed changes, 1 through [`MAX_WRITE_CHANGES`].
    pub changes: Vec<TypedChange>,
}

impl WritePlanV1 {
    /// Parse a strict bounded write plan.
    ///
    /// # Errors
    ///
    /// Rejects oversized or malformed JSON, unknown fields, another schema,
    /// and an empty or oversized change list.
    pub fn from_json(bytes: &[u8]) -> Result<Self, TypedWriteError> {
        if bytes.len() > MAX_WRITE_PLAN_BYTES {
            return Err(TypedWriteError::PlanTooLarge(bytes.len()));
        }
        let plan: Self = serde_json::from_slice(bytes)?;
        if plan.schema != WRITE_PLAN_SCHEMA {
            return Err(TypedWriteError::Schema(plan.schema));
        }
        if plan.changes.is_empty() || plan.changes.len() > MAX_WRITE_CHANGES {
            return Err(TypedWriteError::ChangeCount(plan.changes.len()));
        }
        Ok(plan)
    }
}

/// Caller write policy. Every limit can only be tightened from its default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WritePolicyV1 {
    /// Largest level (and limiter threshold) increase per plan, 0 through 3 dB.
    pub max_gain_step_db: f64,
    /// Absolute output and input gain ceiling, -15 through 0 dB.
    pub gain_ceiling_db: f64,
    /// Largest number of direct-parameter actions, 1 through [`HARD_MAX_ACTIONS`].
    pub max_actions: usize,
    /// Caller consent to an O3/O4 source change.
    pub allow_routing: bool,
    /// Caller consent to unmuting one of O1 through O4.
    pub allow_unmute: bool,
    /// Caller consent to loosening an active high-pass (type change, off, or
    /// a lower corner).
    pub allow_protection_change: bool,
}

impl Default for WritePolicyV1 {
    fn default() -> Self {
        Self {
            max_gain_step_db: MAX_GAIN_STEP_DB,
            gain_ceiling_db: MAX_GAIN_CEILING_DB,
            max_actions: DEFAULT_MAX_ACTIONS,
            allow_routing: false,
            allow_unmute: false,
            allow_protection_change: false,
        }
    }
}

impl WritePolicyV1 {
    /// Validate that every limit lies inside its hard bound.
    ///
    /// # Errors
    ///
    /// Rejects a step above 3 dB or below 0, a ceiling above 0 dB or below
    /// -15 dB, a non-finite value, or an action limit outside 1 through 32.
    pub fn validate(&self) -> Result<(), TypedWriteError> {
        if !self.max_gain_step_db.is_finite()
            || !(0.0..=MAX_GAIN_STEP_DB).contains(&self.max_gain_step_db)
        {
            return Err(TypedWriteError::Policy(format!(
                "max_gain_step_db {} is outside 0 through {MAX_GAIN_STEP_DB}",
                self.max_gain_step_db
            )));
        }
        if !self.gain_ceiling_db.is_finite()
            || !(-15.0..=MAX_GAIN_CEILING_DB).contains(&self.gain_ceiling_db)
        {
            return Err(TypedWriteError::Policy(format!(
                "gain_ceiling_db {} is outside -15 through {MAX_GAIN_CEILING_DB}",
                self.gain_ceiling_db
            )));
        }
        if !(1..=HARD_MAX_ACTIONS).contains(&self.max_actions) {
            return Err(TypedWriteError::Policy(format!(
                "max_actions {} is outside 1 through {HARD_MAX_ACTIONS}",
                self.max_actions
            )));
        }
        Ok(())
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn step_codes(&self) -> u16 {
        ((self.max_gain_step_db * 10.0 + FLOAT_TOLERANCE).floor() as u16).min(MAX_LEVEL_STEP_CODES)
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn ceiling_code(&self) -> u16 {
        (((self.gain_ceiling_db + 15.0) * 10.0 + FLOAT_TOLERANCE).floor() as u16)
            .min(UNITY_GAIN_CODE)
    }
}

/// Encode a frequency in hertz onto the 0-320 grid.
///
/// # Errors
///
/// Rejects a non-finite frequency or one outside 20 through 20,480 Hz.
pub fn encode_frequency(label: &str, frequency: f64) -> Result<u16, TypedWriteError> {
    let maximum = frequency_hz(MAX_FREQUENCY_CODE);
    if !frequency.is_finite() || frequency < 20.0 - FLOAT_TOLERANCE || frequency > maximum + 1e-3 {
        return Err(TypedWriteError::OutOfRange {
            label: label.to_owned(),
            value: frequency,
            range: "20 through 20480 Hz",
        });
    }
    Ok(peq_bank::frequency_code(frequency))
}

/// Encode a PEQ Q onto the 0-40 grid.
///
/// # Errors
///
/// Rejects a non-finite Q or one outside 0.1 through 10.
pub fn encode_q(label: &str, q: f64) -> Result<u16, TypedWriteError> {
    if !q.is_finite() || q < 0.1 - FLOAT_TOLERANCE || q > 10.0 + FLOAT_TOLERANCE {
        return Err(TypedWriteError::OutOfRange {
            label: label.to_owned(),
            value: q,
            range: "0.1 through 10",
        });
    }
    Ok(peq_bank::q_code(q))
}

/// Encode a level or PEQ gain onto the 0-300 grid (0.1 dB steps).
///
/// # Errors
///
/// Rejects a non-finite gain or one outside -15 through `maximum_db`.
pub fn encode_gain(label: &str, gain: f64, maximum_db: f64) -> Result<u16, TypedWriteError> {
    if !gain.is_finite() || gain < -15.0 - FLOAT_TOLERANCE || gain > maximum_db + FLOAT_TOLERANCE {
        return Err(TypedWriteError::OutOfRange {
            label: label.to_owned(),
            value: gain,
            range: "-15 dB through the admitted maximum",
        });
    }
    Ok(peq_bank::gain_code(gain))
}

/// Encode a limiter threshold onto the 0-240 grid (0.1 dB steps).
///
/// # Errors
///
/// Rejects a non-finite threshold or one outside -24 through 0 dB.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn encode_limiter_threshold(label: &str, threshold: f64) -> Result<u16, TypedWriteError> {
    if !threshold.is_finite() || threshold < -24.0 - FLOAT_TOLERANCE || threshold > FLOAT_TOLERANCE
    {
        return Err(TypedWriteError::OutOfRange {
            label: label.to_owned(),
            value: threshold,
            range: "-24 through 0 dB",
        });
    }
    Ok(((threshold + 24.0) * 10.0)
        .round()
        .clamp(0.0, f64::from(MAX_LIMITER_THRESHOLD_CODE)) as u16)
}

/// Limiter threshold in dB represented by a code.
pub fn limiter_threshold_db(code: u16) -> f64 {
    f64::from(code) / 10.0 - 24.0
}

/// Encode a limiter release onto the 0-251 grid (logarithmic hypothesis).
///
/// # Errors
///
/// Rejects a non-finite release or one outside 20 through 4,000 ms.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn encode_limiter_release(label: &str, release: f64) -> Result<u16, TypedWriteError> {
    if !release.is_finite() || release < 20.0 - FLOAT_TOLERANCE || release > 4000.0 + 1e-3 {
        return Err(TypedWriteError::OutOfRange {
            label: label.to_owned(),
            value: release,
            range: "20 through 4000 ms",
        });
    }
    let code = f64::from(MAX_LIMITER_RELEASE_CODE) * (release / 20.0).ln() / 200.0_f64.ln();
    Ok(code.round().clamp(0.0, f64::from(MAX_LIMITER_RELEASE_CODE)) as u16)
}

/// Limiter release in ms represented by a code, under the logarithmic
/// hypothesis.
pub fn limiter_release_ms(code: u16) -> f64 {
    20.0 * 200.0_f64.powf(f64::from(code) / f64::from(MAX_LIMITER_RELEASE_CODE))
}

/// Encode a distance in millimetres onto an exact step grid.
///
/// # Errors
///
/// Rejects a distance that is not a multiple of `step` or exceeds the grid.
pub fn encode_distance(
    label: &str,
    millimetres: u32,
    step: u32,
    maximum_code: u16,
) -> Result<u16, TypedWriteError> {
    if !millimetres.is_multiple_of(step) {
        return Err(TypedWriteError::NotOnGrid {
            label: label.to_owned(),
            value: millimetres,
            step,
        });
    }
    let code = millimetres / step;
    u16::try_from(code)
        .ok()
        .filter(|code| *code <= maximum_code)
        .ok_or(TypedWriteError::OutOfRange {
            label: label.to_owned(),
            value: f64::from(millimetres),
            range: "the device delay range",
        })
}

/// One encoded field of one typed change, before baseline comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Encoded {
    change: usize,
    action: DirectParameterAction,
}

fn require_typed_output(change: usize, output: u8) -> Result<(), TypedWriteError> {
    if TYPED_WRITE_OUTPUTS.contains(&output) {
        Ok(())
    } else if (5..=OUTPUT_COUNT).contains(&output) {
        Err(TypedWriteError::ReservedOutput { change, output })
    } else {
        Err(TypedWriteError::InvalidOutput { change, output })
    }
}

fn action(
    channel: u8,
    parameter: u8,
    value: u16,
) -> Result<DirectParameterAction, TypedWriteError> {
    Ok(DirectParameterAction::new(channel, parameter, value)?)
}

/// Encode one typed change into its ordered direct-parameter actions.
///
/// Actions are listed in a natural field order; [`plan_writes`] reorders the
/// changed ones by safety phase.
///
/// # Errors
///
/// Rejects O5/O6 or another invalid target, values outside their device
/// range, PEQ boosts, missing crossover frequencies, and empty delay or
/// limiter changes.
#[allow(clippy::too_many_lines)]
pub fn encode_change(
    index: usize,
    change: &TypedChange,
) -> Result<Vec<DirectParameterAction>, TypedWriteError> {
    // Level targets may sit anywhere in the device range here; the policy
    // ceiling binds rises only, so a cut from above the ceiling is admitted.
    let level_maximum_db = gain_db(crate::layout::MAX_GAIN_CODE);
    match change {
        TypedChange::PeqBand {
            output,
            band,
            filter,
            frequency_hz,
            gain_db: gain,
            q,
            shelf_slope_db,
        } => {
            require_typed_output(index, *output)?;
            if !(1..=PEQ_BANDS).contains(band) {
                return Err(TypedWriteError::InvalidBand {
                    change: index,
                    band: *band,
                });
            }
            if *gain > FLOAT_TOLERANCE {
                return Err(TypedWriteError::EqBoost {
                    change: index,
                    gain_db: *gain,
                });
            }
            let label = format!("O{output} band{band}");
            let channel = output_channel(*output);
            let mut actions = vec![
                action(
                    channel,
                    band_parameter(*band, BandField::Frequency),
                    encode_frequency(&label, *frequency_hz)?,
                )?,
                action(
                    channel,
                    band_parameter(*band, BandField::Q),
                    encode_q(&label, *q)?,
                )?,
                action(
                    channel,
                    band_parameter(*band, BandField::Gain),
                    encode_gain(&label, *gain, 0.0)?,
                )?,
                action(
                    channel,
                    band_parameter(*band, BandField::Kind),
                    filter.code(),
                )?,
            ];
            if let Some(slope) = shelf_slope_db {
                let code = match slope {
                    6 => 0,
                    12 => 1,
                    other => {
                        return Err(TypedWriteError::OutOfRange {
                            label,
                            value: f64::from(*other),
                            range: "6 or 12 dB/oct",
                        });
                    }
                };
                actions.push(action(
                    channel,
                    band_parameter(*band, BandField::Slope),
                    code,
                )?);
            }
            Ok(actions)
        }
        TypedChange::PeqEnabled { output, enabled } => {
            require_typed_output(index, *output)?;
            Ok(vec![action(
                output_channel(*output),
                EQ_ENABLED_PARAMETER,
                u16::from(*enabled),
            )?])
        }
        TypedChange::PeqBandCount { output, count } => {
            require_typed_output(index, *output)?;
            if *count > PEQ_BANDS {
                return Err(TypedWriteError::InvalidBand {
                    change: index,
                    band: *count,
                });
            }
            Ok(vec![action(
                output_channel(*output),
                EQ_COUNT_PARAMETER,
                u16::from(*count),
            )?])
        }
        TypedChange::Crossover {
            output,
            side,
            filter,
            frequency_hz,
        } => {
            require_typed_output(index, *output)?;
            let (type_parameter, frequency_parameter) = side.parameters();
            let channel = output_channel(*output);
            let label = format!("O{output} {side:?}");
            let frequency = match (filter, frequency_hz) {
                (CrossoverFilter::Off, None) => None,
                (_, Some(frequency)) => Some(action(
                    channel,
                    frequency_parameter,
                    encode_frequency(&label, *frequency)?,
                )?),
                (_, None) => {
                    return Err(TypedWriteError::MissingField {
                        change: index,
                        field: "frequency_hz",
                    });
                }
            };
            let kind = action(channel, type_parameter, filter.code())?;
            // An active filter gets its corner before its type; a filter
            // being switched off goes off before its corner moves.
            Ok(if *filter == CrossoverFilter::Off {
                std::iter::once(kind).chain(frequency).collect()
            } else {
                frequency.into_iter().chain(std::iter::once(kind)).collect()
            })
        }
        TypedChange::OutputGain { output, gain_db } => {
            require_typed_output(index, *output)?;
            Ok(vec![action(
                output_channel(*output),
                GAIN_PARAMETER,
                encode_gain(&format!("O{output} gain"), *gain_db, level_maximum_db)?,
            )?])
        }
        TypedChange::OutputMute { output, muted } => {
            require_typed_output(index, *output)?;
            Ok(vec![action(
                output_channel(*output),
                MUTE_PARAMETER,
                u16::from(*muted),
            )?])
        }
        TypedChange::Polarity { output, inverted } => {
            require_typed_output(index, *output)?;
            Ok(vec![action(
                output_channel(*output),
                POLARITY_PARAMETER,
                u16::from(*inverted),
            )?])
        }
        TypedChange::Delay {
            output,
            enabled,
            long_mm,
            short_mm,
        } => {
            require_typed_output(index, *output)?;
            let channel = output_channel(*output);
            let label = format!("O{output} delay");
            let mut actions = Vec::with_capacity(3);
            if let Some(long) = long_mm {
                actions.push(action(
                    channel,
                    LONG_DELAY_PARAMETER,
                    encode_distance(&label, *long, LONG_DELAY_STEP_MM, MAX_LONG_DELAY_CODE)?,
                )?);
            }
            if let Some(short) = short_mm {
                actions.push(action(
                    channel,
                    SHORT_DELAY_PARAMETER,
                    encode_distance(&label, *short, SHORT_DELAY_STEP_MM, MAX_SHORT_DELAY_CODE)?,
                )?);
            }
            if let Some(enabled) = enabled {
                actions.push(action(
                    channel,
                    DELAY_ENABLED_PARAMETER,
                    u16::from(*enabled),
                )?);
            }
            if actions.is_empty() {
                return Err(TypedWriteError::MissingField {
                    change: index,
                    field: "enabled, long_mm or short_mm",
                });
            }
            Ok(actions)
        }
        TypedChange::Limiter {
            output,
            enabled,
            threshold_db,
            release_ms,
        } => {
            require_typed_output(index, *output)?;
            let channel = output_channel(*output);
            let label = format!("O{output} limiter");
            let mut actions = Vec::with_capacity(3);
            if let Some(threshold) = threshold_db {
                actions.push(action(
                    channel,
                    LIMITER_THRESHOLD_PARAMETER,
                    encode_limiter_threshold(&label, *threshold)?,
                )?);
            }
            if let Some(release) = release_ms {
                actions.push(action(
                    channel,
                    LIMITER_RELEASE_PARAMETER,
                    encode_limiter_release(&label, *release)?,
                )?);
            }
            if let Some(enabled) = enabled {
                actions.push(action(
                    channel,
                    LIMITER_ENABLED_PARAMETER,
                    u16::from(*enabled),
                )?);
            }
            if actions.is_empty() {
                return Err(TypedWriteError::MissingField {
                    change: index,
                    field: "enabled, threshold_db or release_ms",
                });
            }
            Ok(actions)
        }
        TypedChange::InputGain { input, gain_db } => Ok(vec![action(
            input.channel(),
            GAIN_PARAMETER,
            encode_gain(&format!("input {input:?} gain"), *gain_db, level_maximum_db)?,
        )?]),
        TypedChange::OutputSource { output, source } => {
            if !(3..=4).contains(output) {
                return Err(TypedWriteError::SourceNotReviewed {
                    change: index,
                    output: *output,
                });
            }
            Ok(vec![action(
                output_channel(*output),
                SOURCE_PARAMETER,
                source.code(),
            )?])
        }
    }
}

/// Safety phase of one changed action; lower phases are written first.
///
/// 0 mutes on; 1 level and limiter cuts, limiter on, release; 2 PEQ
/// deactivation (off, then fewer bands); 3 crossover, delay, polarity and
/// PEQ band fields (a band's gain first when its cut deepens, last when it
/// gets shallower, so the moving band is never louder than either end
/// state); 4 PEQ activation (more bands, then on); 5 level and threshold
/// rises; 6 sources in routing rank (O4 before O3); 7 unmutes.
fn phase(address: ReviewedAddress, before: u16, after: u16) -> (u8, u8) {
    let rising = after > before;
    match address.field {
        OutputField::Mute if after == 1 => (0, 0),
        OutputField::Mute => (7, 0),
        OutputField::Gain | OutputField::InputGain { .. } | OutputField::LimiterThreshold => {
            if rising {
                (5, 0)
            } else {
                (1, 0)
            }
        }
        OutputField::LimiterEnabled | OutputField::LimiterRelease => (1, 1),
        OutputField::EqEnabled if !rising => (2, 0),
        OutputField::EqCount if !rising => (2, 1),
        OutputField::EqCount => (4, 0),
        OutputField::EqEnabled => (4, 1),
        OutputField::Band {
            field: BandField::Gain,
            ..
        } => {
            if rising {
                (3, 2)
            } else {
                (3, 0)
            }
        }
        OutputField::Source | OutputField::InputSum => (6, routing_rank(address).unwrap_or(0)),
        _ => (3, 1),
    }
}

/// One changed direct-parameter action with its decoded meaning.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedWriteV1 {
    /// Index of the typed change this action encodes.
    pub change: usize,
    /// Physical output, 1 through 6, or 0 for an input gain.
    pub output: u8,
    /// Stable field label.
    pub field: String,
    /// Direct-parameter channel.
    pub channel: u8,
    /// Direct-parameter number.
    pub parameter: u8,
    /// Baseline device code.
    pub before_code: u16,
    /// Written device code.
    pub after_code: u16,
    /// Human value of the baseline code.
    pub before: serde_json::Value,
    /// Human value of the written code.
    pub after: serde_json::Value,
    /// Exact four wire bytes: channel, parameter, value bits 7-13, bits 0-6.
    pub wire: [u8; 4],
    /// Dump part and frame offsets the value lands at.
    pub dump_offsets: DumpOffsetsV1,
}

/// Where one written value must appear in a later dump.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DumpOffsetsV1 {
    /// 0 for Dump0, 1 for Dump1.
    pub part: u8,
    /// Frame offset of value bits 0-6.
    pub low: usize,
    /// 7-of-8 carrier offset and bit of value bit 7.
    pub carrier: Option<(usize, u8)>,
    /// Frame offset of value bits 8 and above.
    pub high: Option<usize>,
}

impl DumpOffsetsV1 {
    fn of(address: ReviewedAddress) -> Self {
        let location = address.location;
        Self {
            part: u8::from(location.part() == crate::protocol::DumpPart::Part1),
            low: location.low_offset(),
            carrier: location.carrier(),
            high: location.high_offset(),
        }
    }
}

/// One linked-crossover side effect a typed write plan predicts.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LinkedEffectV1 {
    /// `observed` or `inferred_unverified`.
    pub evidence: &'static str,
    /// Output whose written field drives the link.
    pub driver_output: u8,
    /// Driver field label.
    pub driver_field: String,
    /// Output whose field the device is predicted to move.
    pub follower_output: u8,
    /// Follower field label.
    pub follower_field: String,
    /// Follower baseline code.
    pub before_code: u16,
    /// Predicted follower code, equal to the written driver code.
    pub after_code: u16,
    /// Human value of the baseline code.
    pub before: serde_json::Value,
    /// Human value of the predicted code.
    pub after: serde_json::Value,
    /// Whether the desired snapshot, and so `write verify` and the live
    /// readback check, expect the change. Only observed directions do.
    pub in_desired_snapshot: bool,
    /// Dump part and frame offsets of the follower field.
    pub dump_offsets: DumpOffsetsV1,
}

/// Human value of one reviewed field code.
pub fn describe(field: OutputField, code: u16) -> serde_json::Value {
    use serde_json::json;
    match field {
        OutputField::Mute => json!({"muted": code == 1}),
        OutputField::EqEnabled | OutputField::DelayEnabled | OutputField::LimiterEnabled => {
            json!({"enabled": code == 1})
        }
        OutputField::Polarity => json!({"inverted": code == 1}),
        OutputField::EqCount => json!({"bands": code}),
        OutputField::Source => json!({"source": OutputSource::from_code(code)}),
        OutputField::InputSum => json!({"input_sum_code": code}),
        OutputField::Gain | OutputField::InputGain { .. } => json!({"gain_db": gain_db(code)}),
        OutputField::Band { field, .. } => match field {
            BandField::Frequency => json!({"frequency_hz": round3(frequency_hz(code))}),
            BandField::Q => json!({"q": round3(q_value(code))}),
            BandField::Gain => json!({"gain_db": gain_db(code)}),
            BandField::Kind => json!({"filter": PeqKind::from_code(code)}),
            BandField::Slope => json!({"shelf_slope_db": if code == 1 { 12 } else { 6 }}),
        },
        OutputField::HighPassType | OutputField::LowPassType => {
            json!({"filter": CrossoverFilter::from_code(code).map(CrossoverFilter::name)})
        }
        OutputField::HighPassFrequency | OutputField::LowPassFrequency => {
            json!({"frequency_hz": round3(frequency_hz(code))})
        }
        OutputField::LongDelay => {
            json!({"long_mm": u32::from(code) * LONG_DELAY_STEP_MM})
        }
        OutputField::ShortDelay => {
            json!({"short_mm": u32::from(code) * SHORT_DELAY_STEP_MM})
        }
        OutputField::LimiterThreshold => json!({"threshold_db": limiter_threshold_db(code)}),
        OutputField::LimiterRelease => {
            json!({"release_ms_log_hypothesis": round3(limiter_release_ms(code))})
        }
    }
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// Complete dry-run receipt of one typed write plan.
#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WriteTransactionV1 {
    /// Always [`WRITE_TRANSACTION_SCHEMA`].
    pub schema: &'static str,
    /// Always true: planning opens no device.
    pub dry_run: bool,
    /// Evidence class of every address this lane added.
    pub evidence_class: &'static str,
    /// Exact baseline snapshot digest.
    pub baseline_snapshot_digest: String,
    /// Exact desired snapshot digest the readback must equal.
    pub desired_snapshot_digest: String,
    /// Changed actions in apply order.
    pub writes: Vec<PlannedWriteV1>,
    /// Typed changes whose every encoded value already equals the baseline.
    pub unchanged_changes: Vec<usize>,
    /// Predicted linked-crossover side effects (O4 high-pass frequency and
    /// O3 low-pass frequency); no frame byte is added for them.
    pub linked_effects: Vec<LinkedEffectV1>,
    /// Exact apply frame, uppercase hexadecimal, or `None` for a no-op plan.
    pub apply_frame_hex: Option<String>,
    /// Exact rollback frame, uppercase hexadecimal, or `None`.
    pub rollback_frame_hex: Option<String>,
    /// Wire budget of the live transactions.
    pub wire_budget: WireBudgetV1,
    /// Bound apply carrier for `control apply` / `write apply`.
    pub apply_plan: ApplyPlanV1,
    /// Bound rollback carrier for `control rollback` / `write rollback`.
    pub rollback_plan: RollbackPlanV1,
}

/// Device traffic one live apply and rollback send for this plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WireBudgetV1 {
    /// Direct-parameter actions in the apply frame.
    pub apply_actions: usize,
    /// Apply frame length in bytes.
    pub apply_frame_bytes: usize,
    /// Direct-parameter frames per apply transaction.
    pub apply_direct_frames: usize,
    /// Remote-mode writes per successful apply transaction.
    pub apply_remote_mode_writes: usize,
    /// Direct-parameter frames per rollback transaction.
    pub rollback_direct_frames: usize,
    /// Remote-mode writes per successful rollback transaction.
    pub rollback_remote_mode_writes: usize,
}

/// Plan, check, order, and bind one typed write plan against a baseline.
///
/// The result is a dry run: nothing here opens a device. Its `apply_plan` and
/// `rollback_plan` are the only carriers a live command accepts.
///
/// # Errors
///
/// Rejects a policy outside its hard bounds, a plan for another device, an
/// unmuted O5 or O6 in the baseline, any O5/O6 target, out-of-range values,
/// EQ boosts, duplicate addresses, an unmute or source change without both
/// consents, an unmute together with a source change, a loosened active
/// high-pass without consent, a summed level rise past the policy step, a
/// level rise past the ceiling, a limiter switch-off, too many actions, a
/// baseline value outside its documented domain, and every core carrier
/// refusal.
#[allow(clippy::too_many_lines)]
pub fn plan_writes(
    baseline: &SnapshotV1,
    plan: &WritePlanV1,
    policy: &WritePolicyV1,
) -> Result<WriteTransactionV1, TypedWriteError> {
    policy.validate()?;
    if baseline.device().get() != plan.device_id {
        return Err(TypedWriteError::DeviceMismatch {
            plan: plan.device_id,
            snapshot: baseline.device().get(),
        });
    }
    let mut encoded = Vec::new();
    for (index, change) in plan.changes.iter().enumerate() {
        for action in encode_change(index, change)? {
            encoded.push(Encoded {
                change: index,
                action,
            });
        }
    }
    let mut seen = BTreeSet::new();
    for entry in &encoded {
        if !seen.insert((entry.action.channel(), entry.action.parameter())) {
            return Err(TypedWriteError::DuplicateAddress {
                channel: entry.action.channel(),
                parameter: entry.action.parameter(),
            });
        }
    }
    let effective = WritePolicyV1 {
        allow_routing: policy.allow_routing && plan.allow_routing,
        allow_unmute: policy.allow_unmute && plan.allow_unmute,
        allow_protection_change: policy.allow_protection_change && plan.allow_protection_change,
        ..*policy
    };
    let mut changed = Vec::new();
    for entry in encoded {
        let address = require_reviewed(entry.action)?;
        let before = baseline.read_reviewed(address);
        if !address.field.admits(before) {
            return Err(TypedWriteError::BaselineOutOfDomain {
                field: address.field.label(),
                value: before,
            });
        }
        if before != entry.action.value() {
            changed.push((entry, address, before));
        }
    }
    // Stable: equal phases keep plan order.
    changed.sort_by_key(|(entry, address, before)| phase(*address, *before, entry.action.value()));
    let actions: Vec<DirectParameterAction> =
        changed.iter().map(|(entry, _, _)| entry.action).collect();
    check_policy(baseline, &actions, &effective)?;
    let touched: BTreeSet<usize> = changed.iter().map(|(entry, _, _)| entry.change).collect();
    let unchanged_changes = (0..plan.changes.len())
        .filter(|index| !touched.contains(index))
        .collect();
    let transaction = if actions.is_empty() {
        ApplyTransactionV1::stage(baseline.clone(), baseline.clone(), Vec::new(), Vec::new())?
    } else {
        ApplyTransactionV1::stage_projected(baseline.clone(), actions.clone())?
    };
    let writes = changed
        .iter()
        .map(|(entry, address, before)| {
            Ok(PlannedWriteV1 {
                change: entry.change,
                output: address.output,
                field: address.field.label(),
                channel: entry.action.channel(),
                parameter: entry.action.parameter(),
                before_code: *before,
                after_code: entry.action.value(),
                before: describe(address.field, *before),
                after: describe(address.field, entry.action.value()),
                wire: wire_bytes(baseline.device(), entry.action)?,
                dump_offsets: DumpOffsetsV1::of(*address),
            })
        })
        .collect::<Result<Vec<_>, TypedWriteError>>()?;
    let linked_effects = baseline
        .linked_predictions(&actions)
        .into_iter()
        .map(|prediction| LinkedEffectV1 {
            evidence: prediction.link.evidence.label(),
            driver_output: prediction.driver.output,
            driver_field: prediction.driver.field.label(),
            follower_output: prediction.follower.output,
            follower_field: prediction.follower.field.label(),
            before_code: prediction.before,
            after_code: prediction.code,
            before: describe(prediction.follower.field, prediction.before),
            after: describe(prediction.follower.field, prediction.code),
            in_desired_snapshot: prediction.link.evidence == LinkEvidence::Observed,
            dump_offsets: DumpOffsetsV1::of(prediction.follower),
        })
        .collect();
    let apply_plan = transaction.apply_plan().clone();
    let rollback_plan = transaction.rollback_plan().clone();
    check_rollback(&rollback_plan)?;
    let apply_frame = apply_plan
        .command()
        .map(DirectParameterCommand::encode)
        .transpose()?;
    let rollback_frame = rollback_plan
        .command()
        .map(DirectParameterCommand::encode)
        .transpose()?;
    Ok(WriteTransactionV1 {
        schema: WRITE_TRANSACTION_SCHEMA,
        dry_run: true,
        evidence_class: EVIDENCE_UNVERIFIED,
        baseline_snapshot_digest: apply_plan.baseline_snapshot_digest().to_owned(),
        desired_snapshot_digest: apply_plan.desired_snapshot_digest().to_owned(),
        wire_budget: WireBudgetV1 {
            apply_actions: actions.len(),
            apply_frame_bytes: apply_frame.as_ref().map_or(0, Vec::len),
            apply_direct_frames: usize::from(apply_frame.is_some()) * DIRECT_FRAMES_PER_TRANSACTION,
            apply_remote_mode_writes: APPLY_REMOTE_MODE_WRITES,
            rollback_direct_frames: usize::from(rollback_frame.is_some())
                * DIRECT_FRAMES_PER_TRANSACTION,
            rollback_remote_mode_writes: if rollback_frame.is_some() {
                ROLLBACK_REMOTE_MODE_WRITES
            } else {
                0
            },
        },
        apply_frame_hex: apply_frame.as_deref().map(hex_upper),
        rollback_frame_hex: rollback_frame.as_deref().map(hex_upper),
        writes,
        unchanged_changes,
        linked_effects,
        apply_plan,
        rollback_plan,
    })
}

fn require_reviewed(action: DirectParameterAction) -> Result<ReviewedAddress, TypedWriteError> {
    reviewed_address(action.channel(), action.parameter()).ok_or(TypedWriteError::Unreviewed {
        channel: action.channel(),
        parameter: action.parameter(),
    })
}

/// The exact four action bytes of a one-action frame, taken from the same
/// encoder that builds the frame the device receives.
fn wire_bytes(device: DeviceId, action: DirectParameterAction) -> Result<[u8; 4], TypedWriteError> {
    let frame = DirectParameterCommand::new(device, vec![action])?.encode()?;
    // F0 00 20 32 <dev> 0E 20 01 <channel> <parameter> <hi> <lo> F7
    frame
        .get(8..12)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .ok_or(TypedWriteError::FrameLayout(frame.len()))
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        use std::fmt::Write as _;
        let _ = write!(text, "{byte:02X}");
        text
    })
}

/// Require that O5 and O6 read muted in a snapshot.
///
/// # Errors
///
/// Rejects any O5/O6 mute value other than 1.
pub fn require_reserved_muted(snapshot: &SnapshotV1) -> Result<(), TypedWriteError> {
    for output in 5..=OUTPUT_COUNT {
        let address = reviewed_address(output_channel(output), MUTE_PARAMETER).ok_or(
            TypedWriteError::Unreviewed {
                channel: output_channel(output),
                parameter: MUTE_PARAMETER,
            },
        )?;
        let value = snapshot.read_reviewed(address);
        if value != 1 {
            return Err(TypedWriteError::ReservedOutputUnmuted { output, value });
        }
    }
    Ok(())
}

/// Refuse loosening an active high-pass (a type change, or a lower corner)
/// without protection consent.
fn check_high_pass_protection(
    baseline: &SnapshotV1,
    address: ReviewedAddress,
    before: u16,
    after: u16,
    policy: &WritePolicyV1,
) -> Result<(), TypedWriteError> {
    let active = read_field(baseline, address.output, HIGH_PASS_TYPE_PARAMETER)? != 0;
    let loosens = address.field == OutputField::HighPassType || after < before;
    if active && loosens && !policy.allow_protection_change {
        return Err(TypedWriteError::ProtectionChange {
            output: address.output,
            field: address.field.label(),
        });
    }
    Ok(())
}

/// Check an ordered apply action list against the write policy.
///
/// This is the policy half of [`plan_writes`] and also re-validates a durable
/// apply carrier before `write apply` sends it:
///
/// - the baseline must hold O5 and O6 muted, and no action may address them;
/// - unmuting needs `allow_unmute`; a source write needs `allow_routing` and
///   may not share a plan with an unmute;
/// - changing the type of an active high-pass, switching it off, or lowering
///   its corner needs `allow_protection_change`;
/// - an enabled limiter stays on;
/// - the summed rise of every level gain, limiter threshold, and effective
///   PEQ band gain (a band's gain while active, unity while inactive) is at
///   most the policy step, so rises on one path cannot add up past it;
/// - a rising level gain stays at or below the policy ceiling;
/// - the actions follow the safe in-frame phase order [`plan_writes`] emits
///   (mutes on first, unmutes last, a band's gain first when its cut deepens),
///   so a frame cut short never leaves an output louder than both end states;
/// - the frame holds at most `max_actions` actions.
///
/// Moving or reshaping an active PEQ band (frequency, Q, kind or slope)
/// counts as releasing its whole baseline cut at the old position, then a
/// new cut: the released depth is spent from the step budget.
///
/// # Errors
///
/// Returns the first policy refusal.
pub fn check_policy(
    baseline: &SnapshotV1,
    actions: &[DirectParameterAction],
    policy: &WritePolicyV1,
) -> Result<(), TypedWriteError> {
    policy.validate()?;
    require_reserved_muted(baseline)?;
    if actions.len() > policy.max_actions {
        return Err(TypedWriteError::TooManyActions {
            actions: actions.len(),
            maximum: policy.max_actions,
        });
    }
    let mut unmutes = false;
    let mut sources = false;
    let mut rise: u32 = 0;
    let mut peq_outputs = BTreeSet::new();
    let mut previous_phase = None;
    for action in actions {
        let address = require_reviewed(*action)?;
        if (5..=OUTPUT_COUNT).contains(&address.output) {
            return Err(TypedWriteError::ReservedOutputAction {
                output: address.output,
            });
        }
        let before = baseline.read_reviewed(address);
        let after = action.value();
        let key = phase(address, before, after);
        if previous_phase.is_some_and(|previous| key < previous) {
            return Err(TypedWriteError::UnsafeOrder {
                channel: action.channel(),
                parameter: action.parameter(),
            });
        }
        previous_phase = Some(key);
        match address.field {
            OutputField::Mute if after == 0 => {
                if !policy.allow_unmute {
                    return Err(TypedWriteError::UnmuteNotAllowed {
                        output: address.output,
                    });
                }
                unmutes = true;
            }
            OutputField::Source | OutputField::InputSum => {
                if !policy.allow_routing {
                    return Err(TypedWriteError::RoutingNotAllowed {
                        channel: action.channel(),
                        parameter: action.parameter(),
                    });
                }
                sources = true;
            }
            OutputField::LimiterEnabled if before != 0 && after == 0 => {
                return Err(TypedWriteError::LimiterDisable {
                    output: address.output,
                });
            }
            OutputField::HighPassType | OutputField::HighPassFrequency => {
                check_high_pass_protection(baseline, address, before, after, policy)?;
            }
            field if field.is_peq() => {
                peq_outputs.insert(address.output);
            }
            _ => {}
        }
        let step_limited =
            address.field.is_level() || address.field == OutputField::LimiterThreshold;
        if step_limited && after > before {
            rise += u32::from(after - before);
        }
        if address.field.is_level() && after > before && after > policy.ceiling_code() {
            return Err(TypedWriteError::GainCeiling {
                field: address.field.label(),
                output: address.output,
                after_db: gain_db(after),
                ceiling_db: policy.gain_ceiling_db,
            });
        }
    }
    if unmutes && sources {
        return Err(TypedWriteError::UnmuteWithRouting);
    }
    check_linked_followers(baseline, actions, policy)?;
    if !peq_outputs.is_empty() {
        let desired = baseline.project_direct_actions(actions).map_err(|error| {
            TypedWriteError::Transaction(ApplyTransactionError::Plan(error.into()))
        })?;
        for output in peq_outputs {
            rise += peq_rise(baseline, &desired, output)?;
        }
    }
    if rise > u32::from(policy.step_codes()) {
        return Err(TypedWriteError::GainStep {
            rise_db: f64::from(rise) / 10.0,
            maximum_step_db: policy.max_gain_step_db,
        });
    }
    Ok(())
}

/// Policy for predicted linked-crossover follower moves (observed and
/// inferred): a follower whose filter is active in the baseline, or switched
/// on by the plan, needs both protection consents, as loosening an active
/// high-pass does for a direct write. The device would move it without a
/// frame byte of its own.
fn check_linked_followers(
    baseline: &SnapshotV1,
    actions: &[DirectParameterAction],
    policy: &WritePolicyV1,
) -> Result<(), TypedWriteError> {
    for prediction in baseline.linked_predictions(actions) {
        let type_parameter = match prediction.follower.field {
            OutputField::HighPassFrequency => HIGH_PASS_TYPE_PARAMETER,
            OutputField::LowPassFrequency => LOW_PASS_TYPE_PARAMETER,
            _ => continue,
        };
        let channel = prediction.link.follower.0;
        let planned_on = actions.iter().any(|action| {
            action.channel() == channel
                && action.parameter() == type_parameter
                && action.value() != 0
        });
        let active =
            read_field(baseline, prediction.follower.output, type_parameter)? != 0 || planned_on;
        if active && !policy.allow_protection_change {
            return Err(TypedWriteError::LinkedProtectionChange {
                driver_output: prediction.driver.output,
                follower_output: prediction.follower.output,
                field: prediction.follower.field.label(),
                evidence: prediction.link.evidence.label(),
            });
        }
    }
    Ok(())
}

fn read_field(snapshot: &SnapshotV1, output: u8, parameter: u8) -> Result<u16, TypedWriteError> {
    let channel = output_channel(output);
    let address = reviewed_address(channel, parameter)
        .ok_or(TypedWriteError::Unreviewed { channel, parameter })?;
    Ok(snapshot.read_reviewed(address))
}

/// Whether a PEQ band is active: PEQ on and the band inside the band count.
fn band_active(snapshot: &SnapshotV1, output: u8, band: u8) -> Result<bool, TypedWriteError> {
    let enabled = read_field(snapshot, output, EQ_ENABLED_PARAMETER)? != 0;
    let count = read_field(snapshot, output, EQ_COUNT_PARAMETER)?;
    Ok(enabled && u16::from(band) <= count)
}

/// Gain code a PEQ band applies: its gain while active, unity otherwise.
fn effective_band_gain(
    snapshot: &SnapshotV1,
    output: u8,
    band: u8,
) -> Result<u16, TypedWriteError> {
    if band_active(snapshot, output, band)? {
        read_field(snapshot, output, band_parameter(band, BandField::Gain))
    } else {
        Ok(UNITY_GAIN_CODE)
    }
}

/// Summed rise, in gain codes, of one output's effective PEQ band gains.
///
/// A band active in the baseline whose frequency, Q, kind or slope differs in
/// `desired` releases its whole baseline cut at the old position, whatever
/// the new cut is, so it counts as a release to unity (`UNITY - before`)
/// rather than `after - before`. Moving a -15 dB cut off a room mode is a
/// +15 dB rise at that mode.
fn peq_rise(
    baseline: &SnapshotV1,
    desired: &SnapshotV1,
    output: u8,
) -> Result<u32, TypedWriteError> {
    let mut rise = 0;
    for band in 1..=PEQ_BANDS {
        let before = effective_band_gain(baseline, output, band)?;
        let mut moved = false;
        if band_active(baseline, output, band)? {
            for field in BandField::all() {
                if field == BandField::Gain {
                    continue;
                }
                let parameter = band_parameter(band, field);
                if read_field(baseline, output, parameter)?
                    != read_field(desired, output, parameter)?
                {
                    moved = true;
                }
            }
        }
        let after = if moved {
            UNITY_GAIN_CODE
        } else {
            effective_band_gain(desired, output, band)?
        };
        rise += u32::from(after.saturating_sub(before));
    }
    Ok(rise)
}

/// Check a rollback carrier against the reserved-output and order rules.
///
/// A rollback re-applies the baseline values of exactly the apply's touched
/// addresses, in the exact reverse of the apply order, so from any prefix a
/// cut-short apply frame left behind it retraces the earlier apply states
/// (an unmute is restored only after the level it was raised from). It may
/// never address O5 or O6, so it can never unmute either (the stored-preset
/// rollback that unmutes O6 is the hazard this excludes; the core carrier
/// refuses any O5/O6 unmute on every lane as well).
///
/// A rollback carries no policy flags: it restores the baseline's mutes,
/// levels and high-pass without the consents an apply needs.
///
/// # Errors
///
/// Rejects a rollback whose baseline has O5 or O6 unmuted, which addresses
/// O5 or O6, or whose order is not the exact reverse of its apply.
pub fn check_rollback(rollback: &RollbackPlanV1) -> Result<(), TypedWriteError> {
    require_reserved_muted(rollback.baseline())?;
    let address = |action: &DirectParameterAction| (action.channel(), action.parameter());
    let apply = rollback
        .apply_plan()
        .command()
        .map_or(&[][..], DirectParameterCommand::actions);
    let inverse = rollback
        .command()
        .map_or(&[][..], DirectParameterCommand::actions);
    if !apply
        .iter()
        .rev()
        .map(address)
        .eq(inverse.iter().map(address))
    {
        return Err(TypedWriteError::RollbackOrder);
    }
    for action in rollback
        .command()
        .map_or(&[][..], DirectParameterCommand::actions)
    {
        let address = require_reviewed(*action)?;
        if (5..=OUTPUT_COUNT).contains(&address.output) {
            return Err(TypedWriteError::ReservedOutputAction {
                output: address.output,
            });
        }
    }
    Ok(())
}

/// Per-action comparison of one readback against an apply plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WrittenValueCheckV1 {
    /// Stable field label.
    pub field: String,
    /// Physical output, or 0 for an input gain.
    pub output: u8,
    /// Direct-parameter channel.
    pub channel: u8,
    /// Direct-parameter number.
    pub parameter: u8,
    /// Code the plan wrote.
    pub written_code: u16,
    /// Code decoded from the readback at the transcribed location.
    pub readback_code: u16,
    /// Whether they are equal.
    pub matches: bool,
}

/// Offline readback consistency receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WriteVerificationV1 {
    /// Always [`WRITE_VERIFICATION_SCHEMA`].
    pub schema: &'static str,
    /// `consistent` only when every value matches, nothing changed outside
    /// the plan's projected bits, and the readback equals the desired state.
    pub status: &'static str,
    /// Apply plan digest.
    pub transaction_id: String,
    /// Readback snapshot digest.
    pub readback_snapshot_digest: String,
    /// Whether the readback equals the plan's desired snapshot exactly.
    pub readback_matches_desired: bool,
    /// Per-action written values.
    pub values: Vec<WrittenValueCheckV1>,
    /// Bytes that changed outside the plan's projected bits and outside
    /// every matched linked-crossover follower field.
    pub uncontained_changes: usize,
    /// Matched linked-crossover side effects (see
    /// [`ApplyPlanV1::linked_changes`]); reported, never silently dropped.
    pub linked_changes: Vec<crate::LinkedChangeV1>,
}

/// Check that every value an apply plan wrote appears at its transcribed dump
/// location in a later readback, that nothing else changed, and that the
/// readback equals the desired snapshot.
///
/// # Errors
///
/// Rejects a readback from another device or an unreviewed carrier action.
pub fn verify_written_values(
    plan: &ApplyPlanV1,
    readback: &SnapshotV1,
) -> Result<WriteVerificationV1, TypedWriteError> {
    let uncontained = plan.uncontained_changes(readback)?;
    let linked_changes = plan.linked_changes(readback)?;
    let mut values = Vec::new();
    for action in plan
        .command()
        .map_or(&[][..], DirectParameterCommand::actions)
    {
        let address = require_reviewed(*action)?;
        let readback_code = readback.read_reviewed(address);
        values.push(WrittenValueCheckV1 {
            field: address.field.label(),
            output: address.output,
            channel: action.channel(),
            parameter: action.parameter(),
            written_code: action.value(),
            readback_code,
            matches: readback_code == action.value(),
        });
    }
    let readback_matches_desired = readback.digest() == plan.desired_snapshot_digest();
    let consistent = readback_matches_desired
        && uncontained.is_empty()
        && values.iter().all(|value| value.matches);
    Ok(WriteVerificationV1 {
        schema: WRITE_VERIFICATION_SCHEMA,
        status: if consistent {
            "consistent"
        } else {
            "inconsistent"
        },
        transaction_id: plan.digest().to_owned(),
        readback_snapshot_digest: readback.digest().to_owned(),
        readback_matches_desired,
        values,
        uncontained_changes: uncontained.len(),
        linked_changes,
    })
}

/// Decoded typed fields of one output.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputTypedStateV1 {
    /// Physical output, 1 through 6.
    pub output: u8,
    /// Field label to decoded value, or an `out_of_domain` record.
    pub fields: BTreeMap<String, serde_json::Value>,
}

/// Decoded typed state of one snapshot.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TypedStateV1 {
    /// Always [`TYPED_STATE_SCHEMA`].
    pub schema: &'static str,
    /// Evidence class of the decoded addresses.
    pub evidence_class: &'static str,
    /// Exact snapshot digest.
    pub snapshot_digest: String,
    /// O1 through O6.
    pub outputs: Vec<OutputTypedStateV1>,
    /// Input A through C gain.
    pub inputs: BTreeMap<String, serde_json::Value>,
}

/// Decode every reviewed field of a snapshot for `write show`.
///
/// A value outside its documented domain is reported with its raw code as
/// `out_of_domain` rather than interpreted.
///
/// # Errors
///
/// Rejects only an internal address-table inconsistency.
pub fn decode_typed_state(snapshot: &SnapshotV1) -> Result<TypedStateV1, TypedWriteError> {
    let entry = |address: ReviewedAddress| {
        let code = snapshot.read_reviewed(address);
        if address.field.admits(code) {
            let mut value = describe(address.field, code);
            if let serde_json::Value::Object(map) = &mut value {
                map.insert("code".to_owned(), code.into());
            }
            value
        } else {
            serde_json::json!({"out_of_domain": true, "code": code})
        }
    };
    let mut outputs = Vec::new();
    for output in 1..=OUTPUT_COUNT {
        let channel = output_channel(output);
        let mut fields = BTreeMap::new();
        for parameter in 0..=0x7f_u8 {
            let Some(address) = reviewed_address(channel, parameter) else {
                continue;
            };
            if matches!(address.field, OutputField::Band { .. }) {
                continue;
            }
            fields.insert(address.field.label(), entry(address));
        }
        match decode_peq_bank(snapshot, output) {
            Ok(bank) => {
                fields.insert("peq".to_owned(), serde_json::to_value(&bank.bands)?);
            }
            Err(PeqBankError::OutOfDomain { label, value, .. }) => {
                fields.insert(
                    "peq".to_owned(),
                    serde_json::json!({"out_of_domain": true, "field": label, "code": value}),
                );
            }
            Err(error) => return Err(TypedWriteError::Peq(error)),
        }
        outputs.push(OutputTypedStateV1 { output, fields });
    }
    let mut inputs = BTreeMap::new();
    for channel in 1..=GAIN_INPUT_COUNT {
        let address =
            reviewed_address(channel, GAIN_PARAMETER).ok_or(TypedWriteError::Unreviewed {
                channel,
                parameter: GAIN_PARAMETER,
            })?;
        inputs.insert(address.field.label(), entry(address));
    }
    Ok(TypedStateV1 {
        schema: TYPED_STATE_SCHEMA,
        evidence_class: EVIDENCE_UNVERIFIED,
        snapshot_digest: snapshot.digest().to_owned(),
        outputs,
        inputs,
    })
}

/// Device address check for a live carrier.
///
/// # Errors
///
/// Rejects a device address outside 0 through 15.
pub fn device(value: u8) -> Result<DeviceId, TypedWriteError> {
    Ok(DeviceId::new(value)?)
}

/// Typed write failures. Every variant is a refusal; nothing is clamped.
#[derive(Debug, Error)]
pub enum TypedWriteError {
    /// The plan JSON is too large.
    #[error("write plan has {0} bytes; maximum is 65536")]
    PlanTooLarge(usize),
    /// The plan names another schema.
    #[error("write plan schema {0:?} is not dcx.write-plan/v1")]
    Schema(String),
    /// The plan has no changes or too many.
    #[error("write plan has {0} changes; expected 1 through 32")]
    ChangeCount(usize),
    /// Malformed JSON or unknown field.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// The policy is outside its hard bounds.
    #[error("write policy refused: {0}")]
    Policy(String),
    /// The plan and snapshot name different devices.
    #[error("write plan device {plan} does not match snapshot device {snapshot}")]
    DeviceMismatch { plan: u8, snapshot: u8 },
    /// O5 or O6 targeted.
    #[error(
        "change {change}: O{output} is a reserved muted output; typed writes never address O5 or O6"
    )]
    ReservedOutput { change: usize, output: u8 },
    /// A carrier action addresses O5 or O6.
    #[error(
        "a carrier action addresses reserved output O{output}; typed writes never address O5 or O6"
    )]
    ReservedOutputAction { output: u8 },
    /// The baseline has O5 or O6 unmuted.
    #[error(
        "baseline O{output} mute code is {value}, not 1 (muted); typed writes refuse every plan until O5 and O6 read muted"
    )]
    ReservedOutputUnmuted { output: u8, value: u16 },
    /// Output outside 1 through 6.
    #[error("change {change}: invalid output {output}; expected 1 through 4")]
    InvalidOutput { change: usize, output: u8 },
    /// Band outside 1 through 9.
    #[error("change {change}: invalid PEQ band or count {band}")]
    InvalidBand { change: usize, band: u8 },
    /// PEQ boost requested.
    #[error("change {change}: PEQ gain {gain_db} dB is a boost; the apply path is cut-only")]
    EqBoost { change: usize, gain_db: f64 },
    /// A value is outside its range.
    #[error("{label}: value {value} is outside {range}")]
    OutOfRange {
        label: String,
        value: f64,
        range: &'static str,
    },
    /// A distance is not on its step grid.
    #[error("{label}: {value} mm is not a multiple of {step} mm")]
    NotOnGrid {
        label: String,
        value: u32,
        step: u32,
    },
    /// A required field is absent.
    #[error("change {change}: missing {field}")]
    MissingField { change: usize, field: &'static str },
    /// Source on an output without a reviewed source address.
    #[error("change {change}: O{output} source has no reviewed address; only O3 and O4")]
    SourceNotReviewed { change: usize, output: u8 },
    /// Two changes write the same address.
    #[error("channel {channel} parameter {parameter:#04x} is written by more than one change")]
    DuplicateAddress { channel: u8, parameter: u8 },
    /// An address escaped the reviewed allowlist.
    #[error("channel {channel} parameter {parameter:#04x} is not a reviewed address")]
    Unreviewed { channel: u8, parameter: u8 },
    /// A baseline value is outside its documented domain.
    #[error("baseline {field} holds code {value}, outside its documented domain")]
    BaselineOutOfDomain { field: String, value: u16 },
    /// Unmute without both consents.
    #[error(
        "unmuting O{output} needs allow_unmute in the plan and --allow-unmute on the command line"
    )]
    UnmuteNotAllowed { output: u8 },
    /// Routing write without both consents.
    #[error(
        "channel {channel} parameter {parameter:#04x} is routing; it needs allow_routing in the plan and --allow-routing"
    )]
    RoutingNotAllowed { channel: u8, parameter: u8 },
    /// Unmute and routing in one plan.
    #[error("a plan may not both unmute an output and change a source; split it")]
    UnmuteWithRouting,
    /// Limiter switch-off.
    #[error("O{output} limiter is enabled; typed writes never disable a limiter")]
    LimiterDisable { output: u8 },
    /// The plan's summed level rise exceeds the policy step.
    #[error(
        "plan raises levels by {rise_db} dB in total (level gains, limiter thresholds, effective PEQ band gains); the policy step is {maximum_step_db} dB"
    )]
    GainStep { rise_db: f64, maximum_step_db: f64 },
    /// An active high-pass would be loosened without consent.
    #[error(
        "O{output} {field}: loosening an active high-pass needs allow_protection_change in the plan and --allow-protection-change"
    )]
    ProtectionChange { output: u8, field: String },
    /// A predicted linked-crossover follower move on an active filter.
    #[error(
        "the O{driver_output} write is predicted ({evidence}) to move O{follower_output} {field} on an active filter (linked crossover); it needs allow_protection_change in the plan and --allow-protection-change"
    )]
    LinkedProtectionChange {
        driver_output: u8,
        follower_output: u8,
        field: String,
        evidence: &'static str,
    },
    /// The encoded one-action frame had an unexpected layout.
    #[error("one-action direct frame has unexpected length {0}")]
    FrameLayout(usize),
    /// Level rise past the policy ceiling.
    #[error("O{output} {field} {after_db} dB rises above the {ceiling_db} dB policy ceiling")]
    GainCeiling {
        field: String,
        output: u8,
        after_db: f64,
        ceiling_db: f64,
    },
    /// Too many actions for one frame.
    #[error("plan has {actions} changed actions; the policy admits {maximum} per frame")]
    TooManyActions { actions: usize, maximum: usize },
    /// A carrier's actions are not in the safe in-frame phase order.
    #[error(
        "channel {channel} parameter {parameter:#04x} is out of the safe in-frame order (mutes on, cuts, PEQ off, band fields, PEQ on, rises, sources, unmutes)"
    )]
    UnsafeOrder { channel: u8, parameter: u8 },
    /// A rollback carrier is not the exact reverse of its apply.
    #[error("rollback actions are not the exact reverse of the apply actions")]
    RollbackOrder,
    /// A typed action could not be represented.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// The core carrier refused the plan.
    #[error(transparent)]
    Transaction(#[from] ApplyTransactionError),
    /// The readback belongs to another device.
    #[error(transparent)]
    Diff(#[from] SnapshotDiffError),
    /// PEQ bank decode failed.
    #[error(transparent)]
    Peq(PeqBankError),
}

#[cfg(test)]
mod tests;
