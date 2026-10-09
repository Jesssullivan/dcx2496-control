//! Static feedback suppression: measurement import and a pure notch planner.
//!
//! The live path uses static notches only. A measured list of feedback peaks
//! (a ring-out frequency list or a REW Generic EQ export) becomes a bounded
//! notch plan for the O4 PA output PEQ bank. The planner is a pure function of
//! the measurement, the baseline snapshot, the policy, and an optional prior
//! plan receipt. It never touches the operator's active bands: notches go only
//! into bands above the operator band count, every notch is a cut-only bell,
//! and the band count and PEQ enable are written after the band values.
//!
//! Nothing here establishes acoustic truth. A synthetic or imported list only
//! drives the exact device-state pipeline; real feedback frequencies require
//! an attended ring-out.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::{
    SnapshotV1,
    layout::{
        BELL_KIND_CODE, EQ_COUNT_PARAMETER, EQ_ENABLED_PARAMETER, MAX_Q_CODE, PEQ_BANDS,
        UNITY_GAIN_CODE, output_channel, reviewed_address,
    },
    peq_bank::{
        BandCodes, DesiredPeqBankProfileV2, PeqBankDocumentV2, PeqBankError, decode_peq_bank,
        frequency_code, frequency_hz, gain_db, q_value,
    },
    profile::FilterKind,
    protocol::{DirectParameterAction, ProtocolError},
    rew::{DesiredProfileError, RewParseError, import_rew},
};

/// Versioned feedback measurement carrier.
pub const MEASUREMENT_SCHEMA: &str = "dcx.feedback-measurement/v1";
/// Versioned notch plan carrier.
pub const NOTCH_PLAN_SCHEMA: &str = "dcx.notch-plan/v1";
/// The only output the feedback planner writes: O4, the PA feed.
pub const FEEDBACK_TARGET_OUTPUT: u8 = crate::peq_bank::DESIRED_PROFILE_V2_OUTPUT;
/// Maximum peaks in one measurement.
pub const MAX_MEASUREMENT_PEAKS: usize = 64;
/// Maximum frequency-list file size.
pub const MAX_FREQUENCY_LIST_BYTES: usize = 64 * 1024;
/// Maximum logical lines in one frequency list.
pub const MAX_FREQUENCY_LIST_LINES: usize = 512;
/// Maximum bytes in one frequency-list line.
pub const MAX_FREQUENCY_LIST_LINE_BYTES: usize = 256;
/// Maximum serialized measurement or plan carrier.
pub const MAX_FEEDBACK_JSON_BYTES: usize = 256 * 1024;

const MEASUREMENT_DOMAIN: &[u8] = b"dcx2496.feedback-measurement/v1\0";
const PLAN_DOMAIN: &[u8] = b"dcx2496.notch-plan/v1\0";

/// Where a measurement came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementSource {
    /// Plain or CSV ring-out frequency list.
    FrequencyList,
    /// REW Generic EQ export; enabled peaking cuts become peaks.
    RewGenericEq,
}

impl MeasurementSource {
    const fn tag(self) -> u8 {
        match self {
            Self::FrequencyList => 1,
            Self::RewGenericEq => 2,
        }
    }
}

/// One measured feedback peak.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackPeakV1 {
    /// Peak frequency, 20 through 20000 Hz.
    pub frequency_hz: f64,
    /// Optional relative severity; higher is worse.
    pub level_db: Option<f64>,
    /// Optional explicit cut, below 0 down to -15 dB.
    pub requested_cut_db: Option<f64>,
}

/// Digest-bound measured peak list for one output.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackMeasurementV1 {
    schema_version: String,
    source: MeasurementSource,
    source_digest: String,
    target_output: u8,
    peaks: Vec<FeedbackPeakV1>,
    digest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MeasurementWire {
    schema_version: String,
    source: MeasurementSource,
    source_digest: String,
    target_output: u8,
    peaks: Vec<FeedbackPeakV1>,
    digest: String,
}

impl FeedbackMeasurementV1 {
    /// Bind already-parsed peaks to their source.
    ///
    /// # Errors
    ///
    /// Rejects an invalid output, an empty or oversized peak list, or any
    /// non-finite or out-of-range peak value.
    pub fn new(
        source: MeasurementSource,
        source_bytes: &[u8],
        target_output: u8,
        peaks: &[FeedbackPeakV1],
    ) -> Result<Self, MeasurementError> {
        Self::bind(source, digest_hex(source_bytes), target_output, peaks)
    }

    fn bind(
        source: MeasurementSource,
        source_digest: String,
        target_output: u8,
        peaks: &[FeedbackPeakV1],
    ) -> Result<Self, MeasurementError> {
        if !(1..=6).contains(&target_output) {
            return Err(MeasurementError::InvalidOutput(target_output));
        }
        if peaks.is_empty() {
            return Err(MeasurementError::NoPeaks);
        }
        if peaks.len() > MAX_MEASUREMENT_PEAKS {
            return Err(MeasurementError::TooManyPeaks(peaks.len()));
        }
        let peaks: Vec<_> = peaks.iter().map(canonical_peak).collect();
        for (index, peak) in peaks.iter().enumerate() {
            validate_peak(index, peak)?;
        }
        if !is_digest(&source_digest) {
            return Err(MeasurementError::InvalidSourceDigest);
        }
        let digest = measurement_digest(source, &source_digest, target_output, &peaks);
        Ok(Self {
            schema_version: MEASUREMENT_SCHEMA.to_owned(),
            source,
            source_digest,
            target_output,
            peaks,
            digest,
        })
    }

    /// Import a ring-out frequency list.
    ///
    /// Grammar, one peak per line: `<frequency_hz>` or
    /// `<frequency_hz>,<level_db>` (comma, tab, or spaces). Blank lines and
    /// `#` comments are skipped. One optional header `frequency_hz` or
    /// `frequency_hz,level_db` may precede the data. Repeating a frequency
    /// records a recurrence.
    ///
    /// # Errors
    ///
    /// Rejects oversized input, malformed lines, and out-of-range values.
    pub fn from_frequency_list(text: &str, target_output: u8) -> Result<Self, MeasurementError> {
        if text.len() > MAX_FREQUENCY_LIST_BYTES {
            return Err(MeasurementError::InputTooLarge(text.len()));
        }
        let mut peaks = Vec::new();
        let mut saw_data = false;
        for (index, raw) in text.lines().enumerate() {
            let line_number = index + 1;
            if line_number > MAX_FREQUENCY_LIST_LINES {
                return Err(MeasurementError::TooManyLines(line_number));
            }
            if raw.len() > MAX_FREQUENCY_LIST_LINE_BYTES {
                return Err(MeasurementError::LineTooLong(line_number));
            }
            let line = if line_number == 1 {
                raw.strip_prefix('\u{feff}').unwrap_or(raw)
            } else {
                raw
            }
            .trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<_> = line
                .split(|character: char| character == ',' || character.is_ascii_whitespace())
                .filter(|field| !field.is_empty())
                .collect();
            if !saw_data && (fields == ["frequency_hz"] || fields == ["frequency_hz", "level_db"]) {
                saw_data = true;
                continue;
            }
            saw_data = true;
            let parse = |text: &str| -> Result<f64, MeasurementError> {
                let value = text
                    .parse::<f64>()
                    .map_err(|_| MeasurementError::MalformedLine(line_number))?;
                if value.is_finite() {
                    Ok(value)
                } else {
                    Err(MeasurementError::MalformedLine(line_number))
                }
            };
            let peak = match fields.as_slice() {
                [frequency] => FeedbackPeakV1 {
                    frequency_hz: parse(frequency)?,
                    level_db: None,
                    requested_cut_db: None,
                },
                [frequency, level] => FeedbackPeakV1 {
                    frequency_hz: parse(frequency)?,
                    level_db: Some(parse(level)?),
                    requested_cut_db: None,
                },
                _ => return Err(MeasurementError::MalformedLine(line_number)),
            };
            peaks.push(peak);
            if peaks.len() > MAX_MEASUREMENT_PEAKS {
                return Err(MeasurementError::TooManyPeaks(peaks.len()));
            }
        }
        Self::new(
            MeasurementSource::FrequencyList,
            text.as_bytes(),
            target_output,
            &peaks,
        )
    }

    /// Import a REW Generic EQ export through the strict REW parser.
    ///
    /// Every enabled filter must be a peaking cut. Its frequency becomes the
    /// peak, its cut becomes both the severity and the requested cut. REW's
    /// Q is not used: every feedback notch uses the policy Q.
    ///
    /// # Errors
    ///
    /// Rejects every REW parse error, shelving filters, and zero-gain filters.
    pub fn from_rew_generic_eq(text: &str, target_output: u8) -> Result<Self, MeasurementError> {
        let report = import_rew(text, target_output)?;
        let mut peaks = Vec::with_capacity(report.filters.len());
        for filter in &report.filters {
            let requested = &filter.requested;
            if requested.kind != FilterKind::Peak {
                return Err(MeasurementError::NotANotch(requested.index));
            }
            if requested.gain_db >= 0.0 {
                return Err(MeasurementError::ZeroCut(requested.index));
            }
            peaks.push(FeedbackPeakV1 {
                frequency_hz: requested.frequency_hz,
                level_db: Some(-requested.gain_db),
                requested_cut_db: Some(requested.gain_db),
            });
        }
        Self::new(
            MeasurementSource::RewGenericEq,
            text.as_bytes(),
            target_output,
            &peaks,
        )
    }

    /// Parse and fully revalidate a serialized measurement.
    ///
    /// # Errors
    ///
    /// Rejects oversized/malformed JSON, unknown fields, unsupported schema,
    /// invalid peaks, or digest mismatch.
    pub fn from_json(bytes: &[u8]) -> Result<Self, MeasurementError> {
        if bytes.len() > MAX_FEEDBACK_JSON_BYTES {
            return Err(MeasurementError::InputTooLarge(bytes.len()));
        }
        let wire: MeasurementWire = serde_json::from_slice(bytes)?;
        if wire.schema_version != MEASUREMENT_SCHEMA {
            return Err(MeasurementError::UnsupportedSchema(wire.schema_version));
        }
        let rebuilt = Self::bind(
            wire.source,
            wire.source_digest,
            wire.target_output,
            &wire.peaks,
        )?;
        if rebuilt.digest != wire.digest {
            return Err(MeasurementError::DigestMismatch);
        }
        Ok(rebuilt)
    }

    /// Serialize the strict carrier.
    ///
    /// # Errors
    ///
    /// Returns an encoding error only for an in-memory invariant failure.
    pub fn to_json(&self) -> Result<Vec<u8>, MeasurementError> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Measurement digest.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Bound output.
    pub const fn target_output(&self) -> u8 {
        self.target_output
    }

    /// Measured peaks in source order.
    pub fn peaks(&self) -> &[FeedbackPeakV1] {
        &self.peaks
    }

    /// Measurement source.
    pub const fn source(&self) -> MeasurementSource {
        self.source
    }
}

/// Round to a thousandth so every stored value has a short exact decimal
/// form that survives JSON serialization bit-for-bit.
fn milli(value: f64) -> f64 {
    if value.is_finite() {
        (value * 1000.0).round() / 1000.0
    } else {
        value
    }
}

#[allow(clippy::cast_possible_truncation)]
fn milli_units(value: f64) -> i64 {
    (value * 1000.0).round() as i64
}

fn canonical_peak(peak: &FeedbackPeakV1) -> FeedbackPeakV1 {
    FeedbackPeakV1 {
        frequency_hz: milli(peak.frequency_hz),
        level_db: peak.level_db.map(milli),
        requested_cut_db: peak.requested_cut_db.map(milli),
    }
}

fn validate_peak(index: usize, peak: &FeedbackPeakV1) -> Result<(), MeasurementError> {
    let in_range =
        |value: f64, low: f64, high: f64| value.is_finite() && value >= low && value <= high;
    if !in_range(peak.frequency_hz, 20.0, 20_000.0) {
        return Err(MeasurementError::PeakOutOfRange {
            index,
            field: "frequency_hz",
        });
    }
    if peak
        .level_db
        .is_some_and(|level| !in_range(level, -200.0, 200.0))
    {
        return Err(MeasurementError::PeakOutOfRange {
            index,
            field: "level_db",
        });
    }
    if peak
        .requested_cut_db
        .is_some_and(|cut| !in_range(cut, -15.0, -0.1))
    {
        return Err(MeasurementError::PeakOutOfRange {
            index,
            field: "requested_cut_db",
        });
    }
    Ok(())
}

fn measurement_digest(
    source: MeasurementSource,
    source_digest: &str,
    target_output: u8,
    peaks: &[FeedbackPeakV1],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(MEASUREMENT_DOMAIN);
    hasher.update([source.tag(), target_output]);
    hasher.update(source_digest.as_bytes());
    hasher.update(u32::try_from(peaks.len()).unwrap_or(u32::MAX).to_be_bytes());
    for peak in peaks {
        hasher.update(milli_units(peak.frequency_hz).to_be_bytes());
        for optional in [peak.level_db, peak.requested_cut_db] {
            hash_optional_milli(&mut hasher, optional);
        }
    }
    format!("sha256/{:x}", hasher.finalize())
}

fn hash_optional_milli(hasher: &mut Sha256, value: Option<f64>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update(milli_units(value).to_be_bytes());
        }
        None => hasher.update([0]),
    }
}

fn digest_hex(bytes: &[u8]) -> String {
    format!("sha256/{:x}", Sha256::digest(bytes))
}

fn is_digest(text: &str) -> bool {
    text.strip_prefix("sha256/").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// Measurement import failures.
#[derive(Debug, Error)]
pub enum MeasurementError {
    /// Input exceeded its byte bound.
    #[error("feedback input has {0} bytes; bound exceeded")]
    InputTooLarge(usize),
    /// Input exceeded its line bound.
    #[error("frequency list has at least {0} lines; maximum is {MAX_FREQUENCY_LIST_LINES}")]
    TooManyLines(usize),
    /// One line exceeded its byte bound.
    #[error("frequency list line {0} is too long")]
    LineTooLong(usize),
    /// A data line did not match the grammar.
    #[error("frequency list line {0} must be `<frequency_hz>[,<level_db>]`")]
    MalformedLine(usize),
    /// Output must be one through six.
    #[error("invalid DCX output {0}; expected 1 through 6")]
    InvalidOutput(u8),
    /// No peaks were present.
    #[error("feedback measurement contains no peaks")]
    NoPeaks,
    /// Too many peaks.
    #[error("feedback measurement has {0} peaks; maximum is {MAX_MEASUREMENT_PEAKS}")]
    TooManyPeaks(usize),
    /// A peak value was outside its range.
    #[error("peak {index} {field} is non-finite or out of range")]
    PeakOutOfRange { index: usize, field: &'static str },
    /// A REW filter was a shelf.
    #[error("REW filter {0} is a shelf; feedback notches are peaking cuts only")]
    NotANotch(u8),
    /// A REW filter had no cut.
    #[error("REW filter {0} has no cut")]
    ZeroCut(u8),
    /// A serialized source digest was malformed.
    #[error("source digest must be sha256/<64 lowercase hex>")]
    InvalidSourceDigest,
    /// Serialized schema was unsupported.
    #[error("unsupported feedback measurement schema {0}")]
    UnsupportedSchema(String),
    /// Serialized digest did not match.
    #[error("feedback measurement digest mismatch")]
    DigestMismatch,
    /// REW parse failure.
    #[error(transparent)]
    Rew(#[from] RewParseError),
    /// JSON failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Bounded static notch policy. Every field is an integer so the receipt
/// serializes exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotchPolicyV1 {
    /// Maximum notches written, 1 through 9, never more than the free bands.
    pub max_notches: u8,
    /// Q code of every notch; 40 is Q 10, the narrowest the device offers.
    pub q_code: u16,
    /// Cut of a peak seen once, in tenths of a dB.
    pub initial_cut_decibels_tenths: i16,
    /// Additional cut per recurrence, in tenths of a dB.
    pub recurrence_step_decibels_tenths: i16,
    /// Deepest policy cut, in tenths of a dB; the device limit is -150.
    pub max_cut_decibels_tenths: i16,
    /// Peaks closer than this many cents (1200 per octave) merge.
    pub merge_cents: u16,
    /// Maximum notches inside any one-octave span.
    pub max_per_octave: u8,
    /// Permit turning PEQ on when the operator's own bands would also enable.
    pub allow_enable_operator_bands: bool,
}

impl Default for NotchPolicyV1 {
    fn default() -> Self {
        Self {
            max_notches: 4,
            q_code: MAX_Q_CODE,
            initial_cut_decibels_tenths: -60,
            recurrence_step_decibels_tenths: -30,
            max_cut_decibels_tenths: -120,
            merge_cents: 200,
            max_per_octave: 2,
            allow_enable_operator_bands: false,
        }
    }
}

impl NotchPolicyV1 {
    fn validate(&self) -> Result<(), FeedbackPlanError> {
        let valid = (1..=PEQ_BANDS).contains(&self.max_notches)
            && self.q_code <= MAX_Q_CODE
            && (-150..=-10).contains(&self.max_cut_decibels_tenths)
            && self.initial_cut_decibels_tenths <= -1
            && self.initial_cut_decibels_tenths >= self.max_cut_decibels_tenths
            && (-60..=0).contains(&self.recurrence_step_decibels_tenths)
            && (75..=1200).contains(&self.merge_cents)
            && (1..=PEQ_BANDS).contains(&self.max_per_octave);
        if valid {
            Ok(())
        } else {
            Err(FeedbackPlanError::InvalidPolicy)
        }
    }
}

/// One planned notch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedNotchV1 {
    /// Destination band, above the operator band count.
    pub band: u8,
    /// Representative measured frequency.
    pub frequency_hz: f64,
    /// Number of measured peaks merged into this notch.
    pub occurrences: u8,
    /// Highest merged severity, if any.
    pub level_db: Option<f64>,
    /// Exact device codes written to the band.
    pub codes: BandCodes,
    /// Frequency represented by the code.
    pub encoded_frequency_hz: f64,
    /// Q represented by the code.
    pub encoded_q: f64,
    /// Cut represented by the code.
    pub encoded_gain_db: f64,
}

/// One measured peak that did not become a notch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DroppedPeakV1 {
    /// Representative frequency.
    pub frequency_hz: f64,
    /// Why it was dropped.
    pub reason: DropReason,
}

/// Why a merged peak was not planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropReason {
    /// The notch cap or free band count was reached.
    NotchCap,
    /// Adding it would exceed the per-octave limit.
    PerOctaveLimit,
    /// It quantized onto an already planned frequency code.
    DuplicateCode,
}

/// One ordered planned action with its field label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedActionV1 {
    /// Direct channel.
    pub channel: u8,
    /// Direct parameter.
    pub parameter: u8,
    /// Desired value.
    pub value: u16,
    /// Stable field label.
    pub field: String,
}

/// Pure, digest-bound static notch plan for O4.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotchPlanV1 {
    schema_version: String,
    target_output: u8,
    parameter_channel: u8,
    baseline_snapshot_digest: String,
    measurement_digest: String,
    policy: NotchPolicyV1,
    eq_enabled_before: bool,
    eq_count_before: u8,
    operator_band_count: u8,
    notches: Vec<PlannedNotchV1>,
    dropped: Vec<DroppedPeakV1>,
    actions: Vec<PlannedActionV1>,
    plan_digest: String,
}

#[derive(Debug, Clone)]
struct Cluster {
    frequency_hz: f64,
    occurrences: u8,
    level_db: Option<f64>,
    requested_cut_db: Option<f64>,
}

/// Plan static notches for one measurement against one baseline snapshot.
///
/// `prior` is the receipt of the plan most recently applied to this output.
/// When supplied, the baseline must still hold exactly that plan's notches
/// directly above its operator band count; those bands are then reused.
///
/// # Errors
///
/// Rejects any output but O4, an invalid policy, a baseline whose PEQ bank
/// fails to decode, a prior plan the baseline no longer matches, a full bank,
/// enabling PEQ over operator bands without the explicit policy flag, and
/// enabling PEQ over an operator band holding a boost under any policy.
pub fn plan_notches(
    measurement: &FeedbackMeasurementV1,
    baseline: &SnapshotV1,
    policy: NotchPolicyV1,
    prior: Option<&NotchPlanV1>,
) -> Result<NotchPlanV1, FeedbackPlanError> {
    policy.validate()?;
    if measurement.target_output != FEEDBACK_TARGET_OUTPUT {
        return Err(FeedbackPlanError::UnsupportedOutput(
            measurement.target_output,
        ));
    }
    let output = FEEDBACK_TARGET_OUTPUT;
    let bank = decode_peq_bank(baseline, output)?;
    let operator_band_count = match prior {
        None => bank.eq_count,
        Some(prior) => {
            prior.verify()?;
            if !prior.owns(&bank) {
                return Err(FeedbackPlanError::PriorPlanNotOwned);
            }
            prior.operator_band_count
        }
    };
    if !bank.eq_enabled && operator_band_count > 0 {
        if !policy.allow_enable_operator_bands {
            return Err(FeedbackPlanError::WouldEnableOperatorBands(
                operator_band_count,
            ));
        }
        // The flag admits enabling the operator's cuts, never a stored boost:
        // apply admission is cut-only and refuses the same plan again.
        if let Some(band) =
            (1..=operator_band_count).find(|&band| bank.band(band).gain_code > UNITY_GAIN_CODE)
        {
            return Err(FeedbackPlanError::WouldEnableStoredBoost {
                band,
                gain_code: bank.band(band).gain_code,
            });
        }
    }
    let free = PEQ_BANDS - operator_band_count;
    if free == 0 {
        return Err(FeedbackPlanError::NoFreeBands);
    }
    let cap = usize::from(policy.max_notches.min(free));

    let (selected, dropped) = select_notches(measurement.peaks(), &policy, cap);

    let channel = output_channel(output);
    let mut notches = Vec::with_capacity(selected.len());
    let mut actions = Vec::new();
    for (offset, (cluster, codes)) in selected.into_iter().enumerate() {
        let band_number = operator_band_count
            + u8::try_from(offset).map_err(|_| FeedbackPlanError::NoFreeBands)?
            + 1;
        for action in codes.actions(output, band_number)? {
            actions.push(labeled(action)?);
        }
        notches.push(PlannedNotchV1 {
            band: band_number,
            frequency_hz: cluster.frequency_hz,
            occurrences: cluster.occurrences,
            level_db: cluster.level_db,
            codes,
            encoded_frequency_hz: frequency_hz(codes.frequency_code),
            encoded_q: q_value(codes.q_code),
            encoded_gain_db: gain_db(codes.gain_code),
        });
    }
    let notch_count = u8::try_from(notches.len()).map_err(|_| FeedbackPlanError::NoFreeBands)?;
    actions.push(labeled(DirectParameterAction::new(
        channel,
        EQ_COUNT_PARAMETER,
        u16::from(operator_band_count + notch_count),
    )?)?);
    actions.push(labeled(DirectParameterAction::new(
        channel,
        EQ_ENABLED_PARAMETER,
        1,
    )?)?);

    let mut plan = NotchPlanV1 {
        schema_version: NOTCH_PLAN_SCHEMA.to_owned(),
        target_output: output,
        parameter_channel: channel,
        baseline_snapshot_digest: baseline.digest().to_owned(),
        measurement_digest: measurement.digest().to_owned(),
        policy,
        eq_enabled_before: bank.eq_enabled,
        eq_count_before: bank.eq_count,
        operator_band_count,
        notches,
        dropped,
        actions,
        plan_digest: String::new(),
    };
    plan.plan_digest = plan.compute_digest();
    Ok(plan)
}

/// Merge, rank, and select notches; returns them in frequency order.
fn select_notches(
    peaks: &[FeedbackPeakV1],
    policy: &NotchPolicyV1,
    cap: usize,
) -> (Vec<(Cluster, BandCodes)>, Vec<DroppedPeakV1>) {
    let mut clusters = merge_peaks(peaks, policy.merge_cents);
    clusters.sort_by(|left, right| {
        let severity = |cluster: &Cluster| cluster.level_db.unwrap_or(f64::NEG_INFINITY);
        severity(right)
            .total_cmp(&severity(left))
            .then(right.occurrences.cmp(&left.occurrences))
            .then(left.frequency_hz.total_cmp(&right.frequency_hz))
    });
    let mut selected: Vec<(Cluster, BandCodes)> = Vec::new();
    let mut dropped = Vec::new();
    for cluster in clusters {
        let codes = notch_codes(&cluster, policy);
        let reason = if selected.len() == cap {
            Some(DropReason::NotchCap)
        } else if selected
            .iter()
            .any(|(_, chosen)| chosen.frequency_code == codes.frequency_code)
        {
            Some(DropReason::DuplicateCode)
        } else if exceeds_octave_limit(&selected, codes.frequency_code, policy.max_per_octave) {
            Some(DropReason::PerOctaveLimit)
        } else {
            None
        };
        match reason {
            Some(reason) => dropped.push(DroppedPeakV1 {
                frequency_hz: cluster.frequency_hz,
                reason,
            }),
            None => selected.push((cluster, codes)),
        }
    }
    selected.sort_by_key(|(_, codes)| codes.frequency_code);
    (selected, dropped)
}

fn merge_peaks(peaks: &[FeedbackPeakV1], merge_cents: u16) -> Vec<Cluster> {
    let mut sorted = peaks.to_vec();
    sorted.sort_by(|left, right| left.frequency_hz.total_cmp(&right.frequency_hz));
    let mut clusters: Vec<(f64, Vec<FeedbackPeakV1>)> = Vec::new();
    for peak in sorted {
        match clusters.last_mut() {
            Some((anchor, members))
                if 1200.0 * (peak.frequency_hz / *anchor).log2() < f64::from(merge_cents) =>
            {
                members.push(peak);
            }
            _ => clusters.push((peak.frequency_hz, vec![peak])),
        }
    }
    clusters
        .into_iter()
        .map(|(_, members)| {
            let representative = members
                .iter()
                .copied()
                .reduce(|best, candidate| {
                    let best_level = best.level_db.unwrap_or(f64::NEG_INFINITY);
                    let level = candidate.level_db.unwrap_or(f64::NEG_INFINITY);
                    if level > best_level { candidate } else { best }
                })
                .expect("a cluster has at least one member");
            Cluster {
                frequency_hz: representative.frequency_hz,
                occurrences: u8::try_from(members.len()).unwrap_or(u8::MAX),
                level_db: members
                    .iter()
                    .filter_map(|member| member.level_db)
                    .reduce(f64::max),
                requested_cut_db: members
                    .iter()
                    .filter_map(|member| member.requested_cut_db)
                    .reduce(f64::min),
            }
        })
        .collect()
}

fn notch_codes(cluster: &Cluster, policy: &NotchPolicyV1) -> BandCodes {
    // Requested cuts are canonical thousandths inside -15..=-0.1 dB.
    #[allow(clippy::cast_possible_truncation)]
    let base = cluster
        .requested_cut_db
        .map_or(policy.initial_cut_decibels_tenths, |cut| {
            (cut * 10.0).round() as i16
        });
    let recurrences = i16::from(cluster.occurrences.saturating_sub(1).min(20));
    let cut = base
        .saturating_add(
            policy
                .recurrence_step_decibels_tenths
                .saturating_mul(recurrences),
        )
        .max(policy.max_cut_decibels_tenths)
        .min(-1);
    // Gain code is 150 + tenths of a dB; the cut keeps it in 0..=149.
    let gain = u16::try_from(i16::try_from(UNITY_GAIN_CODE).unwrap_or(150) + cut).unwrap_or(0);
    BandCodes {
        frequency_code: frequency_code(cluster.frequency_hz),
        q_code: policy.q_code,
        gain_code: gain,
        kind_code: BELL_KIND_CODE,
        slope_code: 0,
    }
}

/// Whether adding `candidate` puts more than `limit` codes in some octave.
///
/// One octave is 32 frequency codes; a window `[f, f + 32)` anchored at each
/// planned or candidate code is checked, which covers every one-octave span.
fn exceeds_octave_limit(selected: &[(Cluster, BandCodes)], candidate: u16, limit: u8) -> bool {
    let mut codes: Vec<u16> = selected
        .iter()
        .map(|(_, codes)| codes.frequency_code)
        .collect();
    codes.push(candidate);
    codes.iter().any(|&start| {
        codes
            .iter()
            .filter(|&&code| code >= start && code < start + 32)
            .count()
            > usize::from(limit)
    })
}

fn labeled(action: DirectParameterAction) -> Result<PlannedActionV1, FeedbackPlanError> {
    let address = reviewed_address(action.channel(), action.parameter()).ok_or(
        FeedbackPlanError::Peq(PeqBankError::Unreviewed(action.parameter())),
    )?;
    Ok(PlannedActionV1 {
        channel: action.channel(),
        parameter: action.parameter(),
        value: action.value(),
        field: address.field.label(),
    })
}

impl NotchPlanV1 {
    /// Parse and fully revalidate a plan receipt.
    ///
    /// # Errors
    ///
    /// Rejects oversized/malformed JSON, unknown fields, and any plan whose
    /// actions or digest do not match exact reconstruction.
    pub fn from_json(bytes: &[u8]) -> Result<Self, FeedbackPlanError> {
        if bytes.len() > MAX_FEEDBACK_JSON_BYTES {
            return Err(FeedbackPlanError::JsonTooLarge(bytes.len()));
        }
        let mut plan: Self = serde_json::from_slice(bytes)?;
        for notch in &mut plan.notches {
            let derived = [
                frequency_hz(notch.codes.frequency_code),
                q_value(notch.codes.q_code),
                gain_db(notch.codes.gain_code),
            ];
            let carried = [
                notch.encoded_frequency_hz,
                notch.encoded_q,
                notch.encoded_gain_db,
            ];
            if derived
                .iter()
                .zip(carried)
                .any(|(derived, carried)| (derived - carried).abs() > 1e-9 * derived.abs().max(1.0))
            {
                return Err(FeedbackPlanError::InvalidPlan("encoded values"));
            }
            [
                notch.encoded_frequency_hz,
                notch.encoded_q,
                notch.encoded_gain_db,
            ] = derived;
        }
        plan.verify()?;
        Ok(plan)
    }

    /// Serialize the strict plan receipt.
    ///
    /// # Errors
    ///
    /// Returns an encoding error only for an in-memory invariant failure.
    pub fn to_json(&self) -> Result<Vec<u8>, FeedbackPlanError> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Recheck schema, notch placement, actions, and digest.
    ///
    /// # Errors
    ///
    /// Returns [`FeedbackPlanError::InvalidPlan`] or a digest mismatch.
    pub fn verify(&self) -> Result<(), FeedbackPlanError> {
        if self.schema_version != NOTCH_PLAN_SCHEMA
            || self.target_output != FEEDBACK_TARGET_OUTPUT
            || self.parameter_channel != output_channel(FEEDBACK_TARGET_OUTPUT)
            || self.operator_band_count >= PEQ_BANDS
            || self.notches.is_empty()
            || usize::from(self.operator_band_count) + self.notches.len() > usize::from(PEQ_BANDS)
        {
            return Err(FeedbackPlanError::InvalidPlan("plan header"));
        }
        self.policy.validate()?;
        let mut expected = Vec::new();
        for (offset, notch) in self.notches.iter().enumerate() {
            let band = usize::from(self.operator_band_count) + offset + 1;
            if usize::from(notch.band) != band
                || notch.codes.kind_code != BELL_KIND_CODE
                || notch.codes.slope_code != 0
                || notch.codes.q_code != self.policy.q_code
                || notch.codes.gain_code >= UNITY_GAIN_CODE
            {
                return Err(FeedbackPlanError::InvalidPlan("notch placement or shape"));
            }
            for action in notch.codes.actions(self.target_output, notch.band)? {
                expected.push(labeled(action)?);
            }
        }
        let count = u16::from(self.operator_band_count)
            + u16::try_from(self.notches.len())
                .map_err(|_| FeedbackPlanError::InvalidPlan("count"))?;
        expected.push(labeled(DirectParameterAction::new(
            self.parameter_channel,
            EQ_COUNT_PARAMETER,
            count,
        )?)?);
        expected.push(labeled(DirectParameterAction::new(
            self.parameter_channel,
            EQ_ENABLED_PARAMETER,
            1,
        )?)?);
        if expected != self.actions {
            return Err(FeedbackPlanError::InvalidPlan("actions"));
        }
        if self.compute_digest() != self.plan_digest {
            return Err(FeedbackPlanError::DigestMismatch);
        }
        Ok(())
    }

    fn owns(&self, bank: &crate::peq_bank::PeqBankStateV1) -> bool {
        let count = usize::from(self.operator_band_count) + self.notches.len();
        usize::from(bank.eq_count) == count
            && self
                .notches
                .iter()
                .all(|notch| bank.band(notch.band) == notch.codes)
    }

    fn compute_digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(PLAN_DOMAIN);
        hasher.update([self.target_output, self.parameter_channel]);
        hasher.update(self.baseline_snapshot_digest.as_bytes());
        hasher.update([0]);
        hasher.update(self.measurement_digest.as_bytes());
        hasher.update([0]);
        let policy = &self.policy;
        hasher.update([policy.max_notches, policy.max_per_octave]);
        hasher.update([u8::from(policy.allow_enable_operator_bands)]);
        hasher.update(policy.q_code.to_be_bytes());
        hasher.update(policy.initial_cut_decibels_tenths.to_be_bytes());
        hasher.update(policy.recurrence_step_decibels_tenths.to_be_bytes());
        hasher.update(policy.max_cut_decibels_tenths.to_be_bytes());
        hasher.update(policy.merge_cents.to_be_bytes());
        hasher.update([
            u8::from(self.eq_enabled_before),
            self.eq_count_before,
            self.operator_band_count,
        ]);
        hasher.update(
            u32::try_from(self.notches.len())
                .unwrap_or(u32::MAX)
                .to_be_bytes(),
        );
        for notch in &self.notches {
            hasher.update([notch.band, notch.occurrences]);
            hasher.update(milli_units(notch.frequency_hz).to_be_bytes());
            hash_optional_milli(&mut hasher, notch.level_db);
            for field in crate::layout::BandField::all() {
                hasher.update(notch.codes.get(field).to_be_bytes());
            }
        }
        hasher.update(
            u32::try_from(self.dropped.len())
                .unwrap_or(u32::MAX)
                .to_be_bytes(),
        );
        for dropped in &self.dropped {
            hasher.update(milli_units(dropped.frequency_hz).to_be_bytes());
            hasher.update([match dropped.reason {
                DropReason::NotchCap => 1,
                DropReason::PerOctaveLimit => 2,
                DropReason::DuplicateCode => 3,
            }]);
        }
        for action in &self.actions {
            hasher.update([action.channel, action.parameter]);
            hasher.update(action.value.to_be_bytes());
        }
        format!("sha256/{:x}", hasher.finalize())
    }

    /// Ordered desired actions: band fields, then band count, then PEQ on.
    ///
    /// # Errors
    ///
    /// Returns a protocol error only for an in-memory invariant failure.
    pub fn direct_actions(&self) -> Result<Vec<DirectParameterAction>, ProtocolError> {
        self.actions
            .iter()
            .map(|action| {
                DirectParameterAction::new(action.channel, action.parameter, action.value)
            })
            .collect()
    }

    /// Bind the plan's desired actions into a staged v2 desired profile.
    ///
    /// # Errors
    ///
    /// Rejects invalid identity strings and any v2 validation failure.
    pub fn desired_profile(
        &self,
        profile_id: String,
        revision: String,
    ) -> Result<DesiredPeqBankProfileV2, FeedbackPlanError> {
        Ok(DesiredPeqBankProfileV2::new(
            profile_id,
            revision,
            PeqBankDocumentV2 {
                target_output: self.target_output,
                parameter_channel: self.parameter_channel,
                actions: self.direct_actions()?,
            },
        )?)
    }

    /// Planned notches in band order.
    pub fn notches(&self) -> &[PlannedNotchV1] {
        &self.notches
    }

    /// Peaks that were not planned.
    pub fn dropped(&self) -> &[DroppedPeakV1] {
        &self.dropped
    }

    /// Bands below or at this count are never written.
    pub const fn operator_band_count(&self) -> u8 {
        self.operator_band_count
    }

    /// Plan digest.
    pub fn digest(&self) -> &str {
        &self.plan_digest
    }
}

/// Notch planning failures.
#[derive(Debug, Error)]
pub enum FeedbackPlanError {
    /// Only O4 is a feedback-suppression target.
    #[error("feedback notches target O4 only; O{0} is refused")]
    UnsupportedOutput(u8),
    /// Policy values were out of bounds.
    #[error("notch policy is out of bounds")]
    InvalidPolicy,
    /// PEQ is off, so enabling it would also enable operator bands.
    #[error(
        "O4 PEQ is off with {0} operator bands; enabling it needs --allow-enable-operator-bands"
    )]
    WouldEnableOperatorBands(u8),
    /// Enabling PEQ would activate an operator band that holds a boost.
    #[error(
        "O4 operator band {band} holds stored boost gain code {gain_code}; enabling PEQ is refused even with --allow-enable-operator-bands"
    )]
    WouldEnableStoredBoost {
        /// Operator band holding the boost.
        band: u8,
        /// Stored gain code above unity (150).
        gain_code: u16,
    },
    /// Every band is in operator use.
    #[error("O4 has no free PEQ band above the operator bands")]
    NoFreeBands,
    /// The baseline no longer holds the prior plan's notches.
    #[error(
        "baseline does not hold the prior plan's notches exactly; refusing to guess band ownership"
    )]
    PriorPlanNotOwned,
    /// A carried plan failed reconstruction.
    #[error("invalid notch plan: {0}")]
    InvalidPlan(&'static str),
    /// A carried plan digest did not match.
    #[error("notch plan digest mismatch")]
    DigestMismatch,
    /// A carried plan exceeded its bound.
    #[error("notch plan JSON has {0} bytes; bound exceeded")]
    JsonTooLarge(usize),
    /// PEQ bank decoding failed.
    #[error(transparent)]
    Peq(#[from] PeqBankError),
    /// A typed action could not be built.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// Desired-profile binding failed.
    #[error(transparent)]
    Profile(#[from] DesiredProfileError),
    /// JSON failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests;
