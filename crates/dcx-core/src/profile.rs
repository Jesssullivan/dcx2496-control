//! Strict versioned profiles and semantic profile differences.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Current profile schema version.
pub const PROFILE_SCHEMA_VERSION: u16 = 1;
/// Only profile family accepted during the Legalab MVP.
pub const LEGALAB_PROFILE_ID: &str = "tinyland-legalab-pzm-dcx2496";
/// Current reviewed profile contract revision.
pub const LEGALAB_PROFILE_REVISION: u32 = 1;
/// Exact supported manufacturer identity.
pub const DEVICE_MANUFACTURER: &str = "Behringer";
/// Exact supported model identity.
pub const DEVICE_MODEL: &str = "DCX2496";

/// Complete desired state for one identified DCX2496.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabProfileV1 {
    /// Must equal [`PROFILE_SCHEMA_VERSION`].
    pub schema_version: u16,
    /// Must equal [`LEGALAB_PROFILE_ID`] for this bounded MVP.
    pub profile_id: String,
    /// Must equal [`LEGALAB_PROFILE_REVISION`].
    pub profile_revision: u32,
    /// Exact expected manufacturer.
    pub device_manufacturer: String,
    /// Exact expected product model.
    pub device_model: String,
    /// Expected device address.
    pub device_id: u8,
    /// Safety-critical Input C mode and feature state.
    pub input_c: InputCProfile,
    /// Every output 1 through 6, exactly once.
    pub outputs: Vec<OutputProfile>,
}

/// Safety-critical Input C configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputCProfile {
    /// The MVP only accepts ordinary line mode.
    pub mode: InputCMode,
    /// Auto Align microphone measurement must remain off.
    pub auto_align_enabled: bool,
    /// Auto EQ microphone measurement must remain off.
    pub auto_eq_enabled: bool,
    /// The DCX Input C +15 V microphone supply must remain off.
    pub phantom_15v_enabled: bool,
}

/// Input C modes representable by the MVP schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputCMode {
    /// Ordinary line-level input mode.
    Line,
}

/// One completely managed physical output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputProfile {
    /// Physical output number, 1 through 6.
    pub output: u8,
    /// Exact lab role for this output number.
    pub role: OutputRole,
    /// Human-readable endpoint name.
    pub label: String,
    /// Sink manufacturer, absent only when the sink is unverified or unused.
    pub sink_manufacturer: Option<String>,
    /// Exact sink model, absent only when the sink is unverified or unused.
    pub sink_model: Option<String>,
    /// Whether the physical sink identity has been independently verified.
    pub model_verified: bool,
    /// Input source; absent is allowed only while muted.
    pub source: Option<InputSource>,
    /// Desired safe state. Activation is intentionally a separate operation.
    pub muted: bool,
    /// Output gain from -15 through +15 dB.
    pub gain_db: f64,
    /// At most nine static filters.
    pub filters: Vec<PeqFilter>,
    /// Limiter required before an output can be unmuted.
    pub limiter: Option<Limiter>,
}

/// Fixed Legalab sink role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputRole {
    /// JBL left monitor on O1.
    MonitorLeft,
    /// JBL right monitor on O2.
    MonitorRight,
    /// JBL subwoofer on O3.
    Subwoofer,
    /// Alto PA on O4.
    Pa,
    /// Deliberately unused output on O5/O6.
    Unused,
}

/// DCX input selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    /// Input A.
    A,
    /// Input B.
    B,
    /// Input C.
    C,
    /// Configured input sum.
    Sum,
}

/// Static filter type accepted by the profile schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterKind {
    /// Peaking equalizer.
    Peak,
    /// Low shelf.
    LowShelf,
    /// High shelf.
    HighShelf,
}

/// One static EQ filter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeqFilter {
    /// Stable slot, 1 through 9.
    pub slot: u8,
    /// Filter response type.
    pub kind: FilterKind,
    /// Center/corner frequency, 20 through 20,000 Hz.
    pub frequency_hz: f64,
    /// Gain, -15 through 0 dB for the cut-only MVP.
    pub gain_db: f64,
    /// Q, 0.1 through 10.
    pub q: f64,
}

/// Output limiter configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limiter {
    /// Limiter must be enabled before activation.
    pub enabled: bool,
    /// Threshold, -24 through 0 dB.
    pub threshold_db: f64,
    /// Release time, 20 through 4,000 ms.
    pub release_ms: f64,
}

/// Validated profile identity bound into a staged plan.
///
/// Fields are private and this type is not deserializable: callers can obtain
/// it only from [`LabProfileV1::binding_for_apply`], which enforces exact device,
/// route, Input C, reserved-output, and sink-model gates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileBinding {
    device_id: u8,
    profile_id: String,
    profile_revision: u32,
    profile_digest: String,
}

impl ProfileBinding {
    /// Bound device address.
    pub const fn device_id(&self) -> u8 {
        self.device_id
    }

    /// Bound profile family.
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Bound contract revision.
    pub const fn profile_revision(&self) -> u32 {
        self.profile_revision
    }

    /// Bound digest of the exact profile bytes/canonical form supplied by the caller.
    pub fn profile_digest(&self) -> &str {
        &self.profile_digest
    }
}

impl LabProfileV1 {
    /// Parse strict JSON and validate all structural and fail-closed invariants.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileError`] for malformed JSON, unknown fields, an unknown
    /// schema version, or any failed safety invariant.
    pub fn from_json(bytes: &[u8]) -> Result<Self, ProfileError> {
        let profile: Self = serde_json::from_slice(bytes)?;
        profile.validate()?;
        Ok(profile)
    }

    /// Validate exact identity, complete routing, Input C, and output safety.
    ///
    /// Unverified muted sinks may be represented for inventory/diff work. They
    /// cannot produce an apply binding.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileError`] when the profile is incomplete or unsafe.
    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.schema_version != PROFILE_SCHEMA_VERSION {
            return Err(ProfileError::UnsupportedSchema(self.schema_version));
        }
        if self.profile_id != LEGALAB_PROFILE_ID {
            return Err(ProfileError::WrongProfileId(self.profile_id.clone()));
        }
        if self.profile_revision != LEGALAB_PROFILE_REVISION {
            return Err(ProfileError::WrongProfileRevision(self.profile_revision));
        }
        if self.device_manufacturer != DEVICE_MANUFACTURER || self.device_model != DEVICE_MODEL {
            return Err(ProfileError::WrongDeviceIdentity {
                manufacturer: self.device_manufacturer.clone(),
                model: self.device_model.clone(),
            });
        }
        if self.device_id > 15 {
            return Err(ProfileError::InvalidDeviceId(self.device_id));
        }
        self.input_c.validate()?;
        if self.outputs.len() != 6 {
            return Err(ProfileError::IncompleteOutputs(self.outputs.len()));
        }

        let mut outputs = BTreeSet::new();
        for output in &self.outputs {
            if !(1..=6).contains(&output.output) {
                return Err(ProfileError::InvalidOutput(output.output));
            }
            if !outputs.insert(output.output) {
                return Err(ProfileError::DuplicateOutput(output.output));
            }
            output.validate()?;
        }
        if outputs != BTreeSet::from([1, 2, 3, 4, 5, 6]) {
            return Err(ProfileError::IncompleteOutputs(outputs.len()));
        }
        Ok(())
    }

    /// Validate the additional named-sink gate required before any apply plan.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileError::UnverifiedSink`] until every used lab sink has
    /// an exact, verified manufacturer and model.
    pub fn validate_for_apply(&self) -> Result<(), ProfileError> {
        self.validate()?;
        for output in &self.outputs {
            if output.role != OutputRole::Unused && !output.has_verified_model() {
                return Err(ProfileError::UnverifiedSink(output.output));
            }
        }
        Ok(())
    }

    /// Bind an independently calculated SHA-256 digest to this apply-ready profile.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileError`] when the profile is not apply-ready or the digest
    /// is not a lowercase 64-character hexadecimal SHA-256 representation.
    pub fn binding_for_apply(&self, profile_digest: &str) -> Result<ProfileBinding, ProfileError> {
        self.validate_for_apply()?;
        if !is_sha256(profile_digest) {
            return Err(ProfileError::InvalidProfileDigest);
        }
        Ok(ProfileBinding {
            device_id: self.device_id,
            profile_id: self.profile_id.clone(),
            profile_revision: self.profile_revision,
            profile_digest: profile_digest.to_owned(),
        })
    }

    /// Produce a stable, path-oriented semantic diff.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileError`] if either input profile is invalid or cannot
    /// be represented as JSON.
    pub fn diff(&self, desired: &Self) -> Result<ProfileDiff, ProfileError> {
        self.validate()?;
        desired.validate()?;
        let before = serde_json::to_value(normalized(self.clone()))?;
        let after = serde_json::to_value(normalized(desired.clone()))?;
        let mut changes = Vec::new();
        diff_value("$", &before, &after, &mut changes);
        Ok(ProfileDiff { changes })
    }
}

fn normalized(mut profile: LabProfileV1) -> LabProfileV1 {
    profile.outputs.sort_by_key(|output| output.output);
    for output in &mut profile.outputs {
        output.filters.sort_by_key(|filter| filter.slot);
    }
    profile
}

impl InputCProfile {
    fn validate(self) -> Result<(), ProfileError> {
        if self.mode != InputCMode::Line
            || self.auto_align_enabled
            || self.auto_eq_enabled
            || self.phantom_15v_enabled
        {
            return Err(ProfileError::UnsafeInputC);
        }
        Ok(())
    }
}

impl OutputProfile {
    fn validate(&self) -> Result<(), ProfileError> {
        let expected_role = expected_role(self.output);
        if self.role != expected_role {
            return Err(ProfileError::WrongOutputRole {
                output: self.output,
                expected: expected_role,
                actual: self.role,
            });
        }
        if self.label.is_empty() || self.label.len() > 80 {
            return Err(ProfileError::InvalidLabel(self.output));
        }
        validate_optional_identity(
            self.output,
            "sink_manufacturer",
            self.sink_manufacturer.as_deref(),
        )?;
        validate_optional_identity(self.output, "sink_model", self.sink_model.as_deref())?;
        if self.model_verified && !self.has_named_model() {
            return Err(ProfileError::IncompleteVerifiedSink(self.output));
        }
        if !self.muted && !self.has_verified_model() {
            return Err(ProfileError::UnverifiedSink(self.output));
        }
        finite_range("gain_db", self.output, self.gain_db, -15.0, 15.0)?;
        if !self.muted {
            if self.source.is_none() {
                return Err(ProfileError::UnmutedWithoutSource(self.output));
            }
            match &self.limiter {
                Some(limiter) if limiter.enabled => {}
                _ => return Err(ProfileError::UnmutedWithoutLimiter(self.output)),
            }
        }
        if self.role == OutputRole::Unused {
            self.validate_unused()?;
        }
        if self.filters.len() > 9 {
            return Err(ProfileError::TooManyFilters {
                output: self.output,
                count: self.filters.len(),
            });
        }
        let mut slots = BTreeSet::new();
        for filter in &self.filters {
            if !(1..=9).contains(&filter.slot) {
                return Err(ProfileError::InvalidFilterSlot {
                    output: self.output,
                    slot: filter.slot,
                });
            }
            if !slots.insert(filter.slot) {
                return Err(ProfileError::DuplicateFilterSlot {
                    output: self.output,
                    slot: filter.slot,
                });
            }
            finite_range(
                "frequency_hz",
                self.output,
                filter.frequency_hz,
                20.0,
                20_000.0,
            )?;
            finite_range("filter_gain_db", self.output, filter.gain_db, -15.0, 0.0)?;
            finite_range("q", self.output, filter.q, 0.1, 10.0)?;
        }
        if let Some(limiter) = &self.limiter {
            finite_range(
                "limiter_threshold_db",
                self.output,
                limiter.threshold_db,
                -24.0,
                0.0,
            )?;
            finite_range(
                "limiter_release_ms",
                self.output,
                limiter.release_ms,
                20.0,
                4_000.0,
            )?;
        }
        Ok(())
    }

    fn validate_unused(&self) -> Result<(), ProfileError> {
        if !self.muted
            || self.source.is_some()
            || self.gain_db.to_bits() != (-15.0_f64).to_bits()
            || !self.filters.is_empty()
            || self.limiter.is_some()
            || self.sink_manufacturer.is_some()
            || self.sink_model.is_some()
            || self.model_verified
        {
            return Err(ProfileError::UnsafeReservedOutput(self.output));
        }
        Ok(())
    }

    fn has_named_model(&self) -> bool {
        self.sink_manufacturer
            .as_deref()
            .is_some_and(|v| !v.is_empty())
            && self.sink_model.as_deref().is_some_and(|v| !v.is_empty())
    }

    fn has_verified_model(&self) -> bool {
        self.model_verified && self.has_named_model()
    }
}

const fn expected_role(output: u8) -> OutputRole {
    match output {
        1 => OutputRole::MonitorLeft,
        2 => OutputRole::MonitorRight,
        3 => OutputRole::Subwoofer,
        4 => OutputRole::Pa,
        _ => OutputRole::Unused,
    }
}

fn validate_optional_identity(
    output: u8,
    field: &'static str,
    value: Option<&str>,
) -> Result<(), ProfileError> {
    if value.is_some_and(|item| item.is_empty() || item.len() > 80) {
        Err(ProfileError::InvalidSinkIdentity { output, field })
    } else {
        Ok(())
    }
}

fn finite_range(
    field: &'static str,
    output: u8,
    value: f64,
    min: f64,
    max: f64,
) -> Result<(), ProfileError> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(ProfileError::OutOfRange {
            field,
            output,
            value,
            min,
            max,
        })
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// One semantic difference between two profiles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldChange {
    /// JSON-style stable path.
    pub path: String,
    /// Baseline value, or null when absent.
    pub before: Value,
    /// Desired value, or null when absent.
    pub after: Value,
}

/// Stable semantic profile difference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileDiff {
    /// Differences ordered lexicographically by object key and by array index.
    pub changes: Vec<FieldChange>,
}

fn diff_value(path: &str, before: &Value, after: &Value, changes: &mut Vec<FieldChange>) {
    match (before, after) {
        (Value::Object(left), Value::Object(right)) => {
            let keys: BTreeSet<_> = left.keys().chain(right.keys()).collect();
            for key in keys {
                let next_path = format!("{path}.{key}");
                match (left.get(key), right.get(key)) {
                    (Some(a), Some(b)) => diff_value(&next_path, a, b, changes),
                    (a, b) => changes.push(FieldChange {
                        path: next_path,
                        before: a.cloned().unwrap_or(Value::Null),
                        after: b.cloned().unwrap_or(Value::Null),
                    }),
                }
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            let limit = left.len().max(right.len());
            for index in 0..limit {
                let next_path = format!("{path}[{index}]");
                match (left.get(index), right.get(index)) {
                    (Some(a), Some(b)) => diff_value(&next_path, a, b, changes),
                    (a, b) => changes.push(FieldChange {
                        path: next_path,
                        before: a.cloned().unwrap_or(Value::Null),
                        after: b.cloned().unwrap_or(Value::Null),
                    }),
                }
            }
        }
        _ if before != after => changes.push(FieldChange {
            path: path.to_owned(),
            before: before.clone(),
            after: after.clone(),
        }),
        _ => {}
    }
}

/// Profile parse or safety validation failure.
#[derive(Debug, Error)]
pub enum ProfileError {
    /// JSON syntax, unknown field, or type error.
    #[error("invalid strict JSON profile: {0}")]
    Json(#[from] serde_json::Error),
    /// Unknown schema versions fail closed.
    #[error("unsupported schema version: {0}")]
    UnsupportedSchema(u16),
    /// Profile family does not match the reviewed MVP contract.
    #[error("profile_id must be {LEGALAB_PROFILE_ID:?}, found {0:?}")]
    WrongProfileId(String),
    /// Profile revision does not match the reviewed MVP contract.
    #[error("profile_revision must be {LEGALAB_PROFILE_REVISION}, found {0}")]
    WrongProfileRevision(u32),
    /// Device manufacturer/model did not identify a DCX2496 exactly.
    #[error("expected {DEVICE_MANUFACTURER} {DEVICE_MODEL}, found {manufacturer} {model}")]
    WrongDeviceIdentity { manufacturer: String, model: String },
    /// Device address was not 0 through 15.
    #[error("device_id must be 0 through 15: {0}")]
    InvalidDeviceId(u8),
    /// Input C was not in the required fail-closed line configuration.
    #[error("Input C must be line mode with Auto Align, Auto EQ, and +15 V disabled")]
    UnsafeInputC,
    /// Profiles must describe all six sinks.
    #[error("profile must contain exactly outputs 1 through 6; found {0}")]
    IncompleteOutputs(usize),
    /// Output number was invalid.
    #[error("invalid output number: {0}")]
    InvalidOutput(u8),
    /// Output occurred more than once.
    #[error("duplicate output number: {0}")]
    DuplicateOutput(u8),
    /// Output role did not match the fixed Legalab route.
    #[error("output {output} role must be {expected:?}, found {actual:?}")]
    WrongOutputRole {
        output: u8,
        expected: OutputRole,
        actual: OutputRole,
    },
    /// Output label was invalid.
    #[error("output {0} label must contain 1 through 80 bytes")]
    InvalidLabel(u8),
    /// Sink identity field was empty or unreasonably long.
    #[error("output {output} {field} must contain 1 through 80 bytes when present")]
    InvalidSinkIdentity { output: u8, field: &'static str },
    /// A sink marked verified omitted its exact manufacturer or model.
    #[error("output {0} is marked model_verified but lacks exact manufacturer/model")]
    IncompleteVerifiedSink(u8),
    /// A used sink lacks independent exact-model verification.
    #[error("output {0} sink model is unverified; apply remains blocked")]
    UnverifiedSink(u8),
    /// O5/O6 did not preserve every reserved-output invariant.
    #[error("reserved output {0} must be unused, muted, unrouted, identity-free, and at -15 dB")]
    UnsafeReservedOutput(u8),
    /// An unmuted output had no explicit input source.
    #[error("output {0} cannot be unmuted without an explicit source")]
    UnmutedWithoutSource(u8),
    /// An unmuted output had no enabled limiter.
    #[error("output {0} cannot be unmuted without an enabled limiter")]
    UnmutedWithoutLimiter(u8),
    /// A numeric field was NaN, infinite, or outside its device range.
    #[error("output {output} field {field}={value} is outside {min}..={max}")]
    OutOfRange {
        field: &'static str,
        output: u8,
        value: f64,
        min: f64,
        max: f64,
    },
    /// More than nine static filters were supplied.
    #[error("output {output} has {count} filters; maximum is 9")]
    TooManyFilters { output: u8, count: usize },
    /// Filter slot was outside 1 through 9.
    #[error("output {output} has invalid filter slot {slot}")]
    InvalidFilterSlot { output: u8, slot: u8 },
    /// Filter slot was repeated.
    #[error("output {output} repeats filter slot {slot}")]
    DuplicateFilterSlot { output: u8, slot: u8 },
    /// Profile digest was not a canonical lowercase SHA-256 string.
    #[error("profile digest must be 64 lowercase hexadecimal characters")]
    InvalidProfileDigest,
}

/// Convert a sequence of outputs into a deterministic lookup map.
pub fn outputs_by_number(profile: &LabProfileV1) -> BTreeMap<u8, &OutputProfile> {
    profile
        .outputs
        .iter()
        .map(|output| (output.output, output))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn role(output: u8) -> OutputRole {
        expected_role(output)
    }

    fn safe_profile() -> LabProfileV1 {
        LabProfileV1 {
            schema_version: PROFILE_SCHEMA_VERSION,
            profile_id: LEGALAB_PROFILE_ID.into(),
            profile_revision: LEGALAB_PROFILE_REVISION,
            device_manufacturer: DEVICE_MANUFACTURER.into(),
            device_model: DEVICE_MODEL.into(),
            device_id: 0,
            input_c: InputCProfile {
                mode: InputCMode::Line,
                auto_align_enabled: false,
                auto_eq_enabled: false,
                phantom_15v_enabled: false,
            },
            outputs: (1..=6)
                .map(|output| OutputProfile {
                    output,
                    role: role(output),
                    label: format!("output-{output}"),
                    sink_manufacturer: (output <= 2).then(|| "JBL".into()),
                    sink_model: (output <= 2).then(|| "306P MkII".into()),
                    model_verified: output <= 2,
                    source: None,
                    muted: true,
                    gain_db: -15.0,
                    filters: Vec::new(),
                    limiter: None,
                })
                .collect(),
        }
    }

    fn apply_ready_profile() -> LabProfileV1 {
        let mut profile = safe_profile();
        for output in &mut profile.outputs[..4] {
            output.sink_manufacturer = Some(if output.output == 4 { "Alto" } else { "JBL" }.into());
            output.sink_model = Some(format!("verified-model-{}", output.output));
            output.model_verified = true;
        }
        profile
    }

    #[test]
    fn complete_muted_inventory_profile_is_valid_but_not_apply_ready() {
        let profile = safe_profile();
        profile.validate().unwrap();
        assert!(matches!(
            profile.validate_for_apply(),
            Err(ProfileError::UnverifiedSink(3))
        ));
    }

    #[test]
    fn apply_binding_requires_exact_identity_models_and_digest() {
        let profile = apply_ready_profile();
        let binding = profile.binding_for_apply(DIGEST).unwrap();
        assert_eq!(binding.device_id(), 0);
        assert_eq!(binding.profile_id(), LEGALAB_PROFILE_ID);
        assert_eq!(binding.profile_revision(), LEGALAB_PROFILE_REVISION);
        assert_eq!(binding.profile_digest(), DIGEST);
    }

    #[test]
    fn input_c_features_fail_closed() {
        for feature in 0..3 {
            let mut profile = safe_profile();
            match feature {
                0 => profile.input_c.auto_align_enabled = true,
                1 => profile.input_c.auto_eq_enabled = true,
                _ => profile.input_c.phantom_15v_enabled = true,
            }
            assert!(matches!(
                profile.validate(),
                Err(ProfileError::UnsafeInputC)
            ));
        }
    }

    #[test]
    fn exact_output_roles_and_reserved_outputs_are_enforced() {
        for rotation in 1..6 {
            let mut profile = safe_profile();
            let mut roles: Vec<_> = profile.outputs.iter().map(|item| item.role).collect();
            roles.rotate_left(rotation);
            for (output, role) in profile.outputs.iter_mut().zip(roles) {
                output.role = role;
            }
            assert!(matches!(
                profile.validate(),
                Err(ProfileError::WrongOutputRole { .. })
            ));
        }

        for index in [4, 5] {
            let mut profile = safe_profile();
            profile.outputs[index].source = Some(InputSource::A);
            assert!(matches!(
                profile.validate(),
                Err(ProfileError::UnsafeReservedOutput(_))
            ));
        }
    }

    #[test]
    fn unmute_requires_verified_source_and_limiter() {
        let mut profile = safe_profile();
        profile.outputs[0].muted = false;
        profile.outputs[0].model_verified = false;
        assert!(matches!(
            profile.validate(),
            Err(ProfileError::UnverifiedSink(1))
        ));
        profile.outputs[0].model_verified = true;
        assert!(matches!(
            profile.validate(),
            Err(ProfileError::UnmutedWithoutSource(1))
        ));
        profile.outputs[0].source = Some(InputSource::A);
        assert!(matches!(
            profile.validate(),
            Err(ProfileError::UnmutedWithoutLimiter(1))
        ));
    }

    #[test]
    fn boosts_are_not_representable_in_profiles() {
        let mut profile = safe_profile();
        profile.outputs[0].filters.push(PeqFilter {
            slot: 1,
            kind: FilterKind::Peak,
            frequency_hz: 100.0,
            gain_db: 0.1,
            q: 1.0,
        });
        assert!(matches!(
            profile.validate(),
            Err(ProfileError::OutOfRange {
                field: "filter_gain_db",
                ..
            })
        ));
    }

    #[test]
    fn strict_json_rejects_unknown_fields() {
        let mut value = serde_json::to_value(safe_profile()).unwrap();
        value["unexpected"] = Value::Bool(true);
        assert!(LabProfileV1::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn diff_is_path_oriented_and_stable() {
        let left = safe_profile();
        let mut right = left.clone();
        right.outputs[1].label = "right".into();
        let diff = left.diff(&right).unwrap();
        assert_eq!(diff.changes.len(), 1);
        assert_eq!(diff.changes[0].path, "$.outputs[1].label");
    }

    #[test]
    fn output_order_is_not_a_semantic_difference() {
        let left = safe_profile();
        let mut right = left.clone();
        right.outputs.rotate_left(3);
        assert!(left.diff(&right).unwrap().changes.is_empty());
    }
}
