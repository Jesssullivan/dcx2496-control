//! Offline diagnostic interpretation of a validated DCX snapshot.
//!
//! The byte locations come from DuinoDCX `Ultradrive.cpp` at
//! `00b9d70d6192e993f31ec94ff0bc20e6e2265b2c` (MIT). Indices address the
//! complete Dump0/Dump1 response frame, including its protocol header. The
//! meanings of the source and mode values are described by the public DCX2496
//! serial parameter reference. These candidates have no paired named-device
//! fixture and MUST NOT be used as a safety gate or an apply plan.

use serde::Serialize;
use thiserror::Error;

use crate::snapshot::{SnapshotSection, SnapshotV1};

const INPUT_C_MODE: (SnapshotSection, usize) = (SnapshotSection::Dump0, 121);
const O4_SOURCE: (SnapshotSection, usize) = (SnapshotSection::Dump1, 365);
const OUTPUT_MUTES: [(SnapshotSection, usize); 6] = [
    (SnapshotSection::Dump0, 715),
    (SnapshotSection::Dump0, 885),
    (SnapshotSection::Dump1, 54),
    (SnapshotSection::Dump1, 223),
    (SnapshotSection::Dump1, 392),
    (SnapshotSection::Dump1, 561),
];

/// Diagnostic candidates only; the output deliberately has no `safe` verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PracticeRouteCandidateV1 {
    pub evidence_class: &'static str,
    pub mapping_source: &'static str,
    pub snapshot_digest: String,
    pub device_address: u8,
    pub input_c_mode_raw: u8,
    pub input_c_mode_candidate: &'static str,
    pub o4_source_raw: u8,
    pub o4_source_candidate: &'static str,
    pub output_mutes: [OutputMuteCandidate; 6],
    pub auto_align: &'static str,
    pub phantom_15v: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OutputMuteCandidate {
    pub output: u8,
    pub raw: u8,
    pub muted_candidate: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PracticeInspectError {
    #[error("candidate field {field} has unsupported raw value {value}")]
    UnsupportedValue { field: &'static str, value: u8 },
}

/// Inspect only an already validated [`SnapshotV1`].
///
/// The candidate leaves Auto Align and +15 V explicitly unverified because
/// neither has a separately established mapping in the inspected sources.
///
/// # Errors
///
/// Rejects any candidate byte outside its documented value range.
pub fn inspect_practice_route(
    snapshot: &SnapshotV1,
) -> Result<PracticeRouteCandidateV1, PracticeInspectError> {
    let input_c_mode_raw = raw(snapshot, INPUT_C_MODE);
    let input_c_mode_candidate = match input_c_mode_raw {
        0 => "line",
        1 => "mic",
        value => {
            return Err(PracticeInspectError::UnsupportedValue {
                field: "input_c_mode",
                value,
            });
        }
    };

    let o4_source_raw = raw(snapshot, O4_SOURCE);
    let o4_source_candidate = match o4_source_raw {
        0 => "A",
        1 => "B",
        2 => "C",
        3 => "SUM",
        value => {
            return Err(PracticeInspectError::UnsupportedValue {
                field: "o4_source",
                value,
            });
        }
    };

    let mut output_mutes = [OutputMuteCandidate {
        output: 0,
        raw: 0,
        muted_candidate: false,
    }; 6];
    for (index, (section, offset)) in OUTPUT_MUTES.iter().copied().enumerate() {
        let value = raw(snapshot, (section, offset));
        if value > 1 {
            return Err(PracticeInspectError::UnsupportedValue {
                field: "output_mute",
                value,
            });
        }
        output_mutes[index] = OutputMuteCandidate {
            output: u8::try_from(index + 1).expect("fixed six-output table"),
            raw: value,
            muted_candidate: value == 1,
        };
    }

    Ok(PracticeRouteCandidateV1 {
        evidence_class: "unverified_candidate",
        mapping_source: "DuinoDCX 00b9d70d6192e993f31ec94ff0bc20e6e2265b2c",
        snapshot_digest: snapshot.digest().to_owned(),
        device_address: snapshot.device().get(),
        input_c_mode_raw,
        input_c_mode_candidate,
        o4_source_raw,
        o4_source_candidate,
        output_mutes,
        auto_align: "unverified_not_decoded",
        phantom_15v: "unverified_not_decoded",
    })
}

fn raw(snapshot: &SnapshotV1, (section, offset): (SnapshotSection, usize)) -> u8 {
    // The fixed offsets are all inside their validated exact-length frames.
    snapshot.frame(section)[offset]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{DUMP0_RESPONSE_LEN, DUMP1_RESPONSE_LEN};

    fn identity() -> Vec<u8> {
        let mut frame = vec![0xf0, 0, 0x20, 0x32, 0, 0x0e, 0];
        frame.extend_from_slice(b"SYNTHETIC-IDENTITY");
        frame.push(0xf7);
        frame
    }

    fn dump(part: u8) -> Vec<u8> {
        let len = if part == 0 {
            DUMP0_RESPONSE_LEN
        } else {
            DUMP1_RESPONSE_LEN
        };
        let mut frame = vec![0; len];
        frame[..7].copy_from_slice(&[0xf0, 0, 0x20, 0x32, 0, 0x0e, 0x10]);
        frame[12] = part;
        frame[len - 1] = 0xf7;
        frame
    }

    fn sample() -> (Vec<u8>, Vec<u8>) {
        let mut d0 = dump(0);
        let mut d1 = dump(1);
        d0[INPUT_C_MODE.1] = 0;
        d1[O4_SOURCE.1] = 2;
        for (section, offset) in OUTPUT_MUTES {
            match section {
                SnapshotSection::Dump0 => d0[offset] = 1,
                SnapshotSection::Dump1 => d1[offset] = 1,
                SnapshotSection::Identity => unreachable!(),
            }
        }
        (d0, d1)
    }

    #[test]
    fn known_values_report_candidates_without_a_safety_verdict() {
        let (d0, mut d1) = sample();
        d1[OUTPUT_MUTES[3].1] = 0;
        let snapshot = SnapshotV1::from_frames(&identity(), &d0, &d1).unwrap();
        let report = inspect_practice_route(&snapshot).unwrap();
        assert_eq!(report.evidence_class, "unverified_candidate");
        assert_eq!(report.input_c_mode_candidate, "line");
        assert_eq!(report.o4_source_candidate, "C");
        assert!(!report.output_mutes[3].muted_candidate);
        assert!(report.output_mutes[4].muted_candidate);
        assert!(report.output_mutes[5].muted_candidate);
        assert_eq!(report.phantom_15v, "unverified_not_decoded");
        assert_eq!(report.auto_align, "unverified_not_decoded");
        assert!(!serde_json::to_string(&report).unwrap().contains("safe"));
    }

    #[test]
    fn unsupported_values_fail_closed_for_every_field() {
        for (section, offset, value) in [
            (INPUT_C_MODE.0, INPUT_C_MODE.1, 2),
            (O4_SOURCE.0, O4_SOURCE.1, 4),
            (OUTPUT_MUTES[0].0, OUTPUT_MUTES[0].1, 2),
            (OUTPUT_MUTES[1].0, OUTPUT_MUTES[1].1, 2),
            (OUTPUT_MUTES[2].0, OUTPUT_MUTES[2].1, 2),
            (OUTPUT_MUTES[3].0, OUTPUT_MUTES[3].1, 2),
            (OUTPUT_MUTES[4].0, OUTPUT_MUTES[4].1, 2),
            (OUTPUT_MUTES[5].0, OUTPUT_MUTES[5].1, 2),
        ] {
            let (mut d0, mut d1) = sample();
            match section {
                SnapshotSection::Dump0 => d0[offset] = value,
                SnapshotSection::Dump1 => d1[offset] = value,
                SnapshotSection::Identity => unreachable!(),
            }
            let snapshot = SnapshotV1::from_frames(&identity(), &d0, &d1).unwrap();
            assert!(matches!(
                inspect_practice_route(&snapshot),
                Err(PracticeInspectError::UnsupportedValue { value: 2 | 4, .. })
            ));
        }
    }

    #[test]
    fn malformed_or_short_snapshot_never_reaches_inspection() {
        let (d0, d1) = sample();
        assert!(SnapshotV1::from_frames(&identity(), &d0[..d0.len() - 1], &d1).is_err());
        let snapshot = SnapshotV1::from_frames(&identity(), &d0, &d1).unwrap();
        let mut json: serde_json::Value =
            serde_json::from_slice(&snapshot.to_json().unwrap()).unwrap();
        json["dump1"]["frame"][O4_SOURCE.1] = serde_json::Value::from(0);
        assert!(SnapshotV1::from_json(&serde_json::to_vec(&json).unwrap()).is_err());
    }
}
