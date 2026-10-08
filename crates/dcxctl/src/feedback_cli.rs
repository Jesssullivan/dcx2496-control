//! Offline static feedback-suppression commands: import, inspect, plan, and
//! bind a v2 desired profile. None of these opens a device.

use std::{error::Error, io, path::Path};

use dcx_core::{
    SnapshotV1,
    feedback::{
        FeedbackMeasurementV1, MAX_FEEDBACK_JSON_BYTES, MAX_FREQUENCY_LIST_BYTES, NotchPlanV1,
        NotchPolicyV1, plan_notches,
    },
    peq_bank::decode_peq_bank,
    rew::MAX_REW_BYTES,
};

use crate::read_bounded;

const MAX_SNAPSHOT_FILE_BYTES: usize = 256 * 1024;

/// Measurement source file kinds.
pub enum MeasurementInput<'a> {
    FrequencyList(&'a Path),
    Rew(&'a Path),
}

pub fn import(input: &MeasurementInput<'_>, target_output: u8) -> Result<(), Box<dyn Error>> {
    let measurement = match input {
        MeasurementInput::FrequencyList(path) => {
            let bytes = read_bounded(path, MAX_FREQUENCY_LIST_BYTES, "frequency list")?;
            FeedbackMeasurementV1::from_frequency_list(std::str::from_utf8(&bytes)?, target_output)?
        }
        MeasurementInput::Rew(path) => {
            let bytes = read_bounded(path, MAX_REW_BYTES, "REW export")?;
            FeedbackMeasurementV1::from_rew_generic_eq(std::str::from_utf8(&bytes)?, target_output)?
        }
    };
    println!("{}", serde_json::to_string_pretty(&measurement)?);
    Ok(())
}

pub fn inspect(snapshot: &Path, target_output: u8) -> Result<(), Box<dyn Error>> {
    let snapshot = load_snapshot(snapshot)?;
    let bank = decode_peq_bank(&snapshot, target_output)?;
    println!("{}", serde_json::to_string_pretty(&bank)?);
    Ok(())
}

pub struct PlanOptions<'a> {
    pub measurement: &'a Path,
    pub snapshot: &'a Path,
    pub prior_plan: Option<&'a Path>,
    pub max_notches: u8,
    pub max_cut_db: f64,
    pub allow_enable_operator_bands: bool,
}

pub fn plan(options: &PlanOptions<'_>) -> Result<(), Box<dyn Error>> {
    let measurement = FeedbackMeasurementV1::from_json(&read_bounded(
        options.measurement,
        MAX_FEEDBACK_JSON_BYTES,
        "feedback measurement",
    )?)?;
    let snapshot = load_snapshot(options.snapshot)?;
    let prior = options
        .prior_plan
        .map(|path| -> Result<NotchPlanV1, Box<dyn Error>> {
            Ok(NotchPlanV1::from_json(&read_bounded(
                path,
                MAX_FEEDBACK_JSON_BYTES,
                "prior notch plan",
            )?)?)
        })
        .transpose()?;
    if !options.max_cut_db.is_finite() || !(-15.0..=-1.0).contains(&options.max_cut_db) {
        return Err(invalid("--max-cut-db must be -15 through -1"));
    }
    #[allow(clippy::cast_possible_truncation)]
    let max_cut_decibels_tenths = (options.max_cut_db * 10.0).round() as i16;
    let defaults = NotchPolicyV1::default();
    let policy = NotchPolicyV1 {
        max_notches: options.max_notches,
        max_cut_decibels_tenths,
        initial_cut_decibels_tenths: defaults
            .initial_cut_decibels_tenths
            .max(max_cut_decibels_tenths),
        allow_enable_operator_bands: options.allow_enable_operator_bands,
        ..defaults
    };
    let plan = plan_notches(&measurement, &snapshot, policy, prior.as_ref())?;
    println!("{}", serde_json::to_string_pretty(&plan)?);
    Ok(())
}

pub fn desired_profile(
    plan: &Path,
    profile_id: String,
    revision: String,
) -> Result<(), Box<dyn Error>> {
    let plan = NotchPlanV1::from_json(&read_bounded(plan, MAX_FEEDBACK_JSON_BYTES, "notch plan")?)?;
    let profile = plan.desired_profile(profile_id, revision)?;
    println!("{}", serde_json::to_string_pretty(&profile)?);
    Ok(())
}

fn load_snapshot(path: &Path) -> Result<SnapshotV1, Box<dyn Error>> {
    Ok(SnapshotV1::from_json(&read_bounded(
        path,
        MAX_SNAPSHOT_FILE_BYTES,
        "control snapshot",
    )?)?)
}

fn invalid(message: &str) -> Box<dyn Error> {
    Box::new(io::Error::new(
        io::ErrorKind::InvalidInput,
        message.to_owned(),
    ))
}
