//! Legalab profile fixture and the 2026-10-21 O4 tuning chains.
//!
//! `fixtures/profiles/LEGALAB-dcx-o4-vocal-1dc18140.json` is a verbatim copy
//! of Legalab `profiles/dcx-o4-vocal-1dc18140.json` (legalab main
//! `33c13655`, sha256 `942c8864...`): a sanitized typed state, codes only,
//! with no snapshot, serial or port.
//!
//! The snapshots here are synthetic. Their O4 (and O3 low-pass) codes are
//! transcribed from the Legalab records of that night: the `143d32b5` typed
//! readback, the band table in legalab
//! `docs/agent-notes/2026-10-21-lab-day-vocal-pa-tuning.md`, and Linear
//! TIN-6827 comment `20aafcf2`. Where a record gives a value but not its
//! code (the intermediate bands 6-8 of `24296ae3`, band 8 of `32846545`),
//! the code is the device step nearest the recorded value, an inference.
//! The synthetic digests are not the device digests.

use dcx_core::{
    SnapshotV1,
    layout::{
        BandField, EQ_COUNT_PARAMETER, EQ_ENABLED_PARAMETER, GAIN_PARAMETER,
        HIGH_PASS_FREQUENCY_PARAMETER, HIGH_PASS_TYPE_PARAMETER, LIMITER_RELEASE_PARAMETER,
        LIMITER_THRESHOLD_PARAMETER, LOW_PASS_FREQUENCY_PARAMETER, MUTE_PARAMETER, PEQ_BANDS,
        SOURCE_PARAMETER, UNITY_GAIN_CODE, band_parameter, output_channel, reviewed_address,
    },
    output_profile::{
        DcxOutputProfileV1, OutputProfileError, ProfileBandCodesV1, ProfileBandV1,
        plan_profile_chain, profile_digest, verify_profile,
    },
    peq_bank::{frequency_hz, gain_db, q_value},
    protocol::{DUMP0_RESPONSE_LEN, DUMP1_RESPONSE_LEN, DumpPart},
    typed_write::{
        PeqKind, TypedChange, TypedWriteError, WRITE_PLAN_SCHEMA, WritePlanV1, WritePolicyV1,
        plan_writes,
    },
};

const PROFILE: &[u8] =
    include_bytes!("../../../fixtures/profiles/LEGALAB-dcx-o4-vocal-1dc18140.json");

/// Frequency, Q, gain, kind, slope codes.
type Codes = [u16; 5];

const FLAT: Codes = [180, 20, 150, 1, 1];

fn identity() -> Vec<u8> {
    let mut frame = vec![0xf0, 0x00, 0x20, 0x32, 0x00, 0x0e, 0x00];
    frame.extend_from_slice(b"SYNTHETIC-IDENTITY");
    frame.push(0xf7);
    frame
}

fn blank_dump(length: usize, part: u8) -> Vec<u8> {
    let mut frame = vec![0; length];
    frame[..7].copy_from_slice(&[0xf0, 0x00, 0x20, 0x32, 0x00, 0x0e, 0x10]);
    frame[12] = part;
    frame[length - 1] = 0xf7;
    frame
}

#[derive(Clone)]
struct Builder {
    dump0: Vec<u8>,
    dump1: Vec<u8>,
}

impl Builder {
    fn set(&mut self, channel: u8, parameter: u8, value: u16) -> &mut Self {
        let location = reviewed_address(channel, parameter).unwrap().location;
        let frame = match location.part() {
            DumpPart::Part0 => &mut self.dump0,
            DumpPart::Part1 => &mut self.dump1,
        };
        location.write(frame, value).unwrap();
        self
    }

    fn band(&mut self, band: u8, codes: Codes) -> &mut Self {
        for (field, code) in BandField::all().into_iter().zip(codes) {
            self.set(8, band_parameter(band, field), code);
        }
        self
    }

    fn snapshot(&self) -> SnapshotV1 {
        SnapshotV1::from_frames(&identity(), &self.dump0, &self.dump1).unwrap()
    }
}

/// The `143d32b5` state (the #241 plan applied): O1-O3 muted, O3 source sum
/// at -2 dB with its low-pass off at code 74, O4 unmuted from input C at
/// 0 dB, LR24 high-pass at code 74, low-pass off at 319, limiter off at
/// 0 dB, PEQ on with 5 bands; O5/O6 muted.
fn state_143d32b5() -> Builder {
    let mut builder = Builder {
        dump0: blank_dump(DUMP0_RESPONSE_LEN, 0),
        dump1: blank_dump(DUMP1_RESPONSE_LEN, 1),
    };
    for output in 1..=6 {
        let channel = output_channel(output);
        builder.set(channel, MUTE_PARAMETER, u16::from(output != 4));
        for band in 1..=PEQ_BANDS {
            for (field, code) in BandField::all().into_iter().zip(FLAT) {
                builder.set(channel, band_parameter(band, field), code);
            }
        }
        if output <= 4 {
            builder.set(channel, GAIN_PARAMETER, 150);
            builder.set(channel, LIMITER_THRESHOLD_PARAMETER, 240);
            builder.set(channel, LIMITER_RELEASE_PARAMETER, 141);
            builder.set(channel, LOW_PASS_FREQUENCY_PARAMETER, 319);
        }
    }
    for channel in 1..=3 {
        builder.set(channel, GAIN_PARAMETER, 150);
    }
    builder
        .set(7, SOURCE_PARAMETER, 3)
        .set(7, GAIN_PARAMETER, 130)
        .set(7, LOW_PASS_FREQUENCY_PARAMETER, 74)
        .set(8, SOURCE_PARAMETER, 2)
        .set(8, HIGH_PASS_TYPE_PARAMETER, 8)
        .set(8, HIGH_PASS_FREQUENCY_PARAMETER, 74)
        .set(8, EQ_ENABLED_PARAMETER, 1)
        .set(8, EQ_COUNT_PARAMETER, 5)
        .band(1, [90, 20, 30, 0, 1])
        .band(2, [101, 40, 90, 1, 1])
        .band(3, [118, 40, 100, 1, 1])
        .band(4, [251, 23, 110, 1, 1])
        .band(5, [274, 20, 30, 2, 1]);
    builder
}

/// One recorded O4 write: the band moves and an optional new count.
struct Recorded {
    to: &'static str,
    bands: &'static [(u8, Codes)],
    count: Option<u8>,
}

/// The verified (or readback-confirmed) 2026-10-21 O4 chain after
/// `143d32b5`, in order. The refused count-9 apply is tested separately; the
/// device stored band 9's values (`d3989341`) and kept count 8.
const TONIGHT: &[Recorded] = &[
    Recorded {
        to: "24296ae3 bands 6-8 at -6 dB, count 8",
        bands: &[
            (6, [213, 26, 90, 1, 1]),
            (7, [161, 38, 90, 1, 1]),
            (8, [256, 38, 90, 1, 1]),
        ],
        count: Some(8),
    },
    Recorded {
        to: "d3989341 band 9 stored, inactive",
        bands: &[(9, [138, 23, 100, 1, 1])],
        count: None,
    },
    Recorded {
        to: "aaeba8e5 band 7 -3",
        bands: &[(7, [161, 38, 120, 1, 1])],
        count: None,
    },
    Recorded {
        to: "bb2ae4f7 band 7 0",
        bands: &[(7, [161, 38, 150, 1, 1])],
        count: None,
    },
    Recorded {
        to: "7dff30d7 band 7 424 Hz Q1.26 -5",
        bands: &[(7, [141, 22, 100, 1, 1])],
        count: None,
    },
    Recorded {
        to: "e8e96d59 band 4 -1",
        bands: &[(4, [251, 23, 140, 1, 1])],
        count: None,
    },
    Recorded {
        to: "56c0087e band 4 3.7k Q1.41 -5",
        bands: &[(4, [241, 23, 100, 1, 1])],
        count: None,
    },
    Recorded {
        to: "ca116475 band 8 -3",
        bands: &[(8, [256, 38, 120, 1, 1])],
        count: None,
    },
    Recorded {
        to: "fb51ff27 band 8 0",
        bands: &[(8, [256, 38, 150, 1, 1])],
        count: None,
    },
    Recorded {
        to: "5a93489a band 8 1.1k Q2 -4",
        bands: &[(8, [185, 26, 110, 1, 1])],
        count: None,
    },
    Recorded {
        to: "2420aba4 band 6 -3",
        bands: &[(6, [213, 26, 120, 1, 1])],
        count: None,
    },
    Recorded {
        to: "b665cf11 band 4 -2",
        bands: &[(4, [241, 23, 130, 1, 1])],
        count: None,
    },
    Recorded {
        to: "987f6726 band 4 3.4k Q0.89 -9",
        bands: &[(4, [237, 19, 60, 1, 1])],
        count: None,
    },
    Recorded {
        to: "32846545 band 8 -1",
        bands: &[(8, [185, 26, 140, 1, 1])],
        count: None,
    },
    Recorded {
        to: "924fb384 band 8 2792 Hz Q7.9 -6",
        bands: &[(8, [228, 38, 90, 1, 1])],
        count: None,
    },
    Recorded {
        to: "ae4fafab band 2 -3",
        bands: &[(2, [101, 40, 120, 1, 1])],
        count: None,
    },
    Recorded {
        to: "5861ff9f band 2 0",
        bands: &[(2, [101, 40, 150, 1, 1])],
        count: None,
    },
    Recorded {
        to: "6f5c4148 band 2 199 Hz low shelf -15",
        bands: &[(2, [106, 20, 0, 0, 1])],
        count: None,
    },
    Recorded {
        to: "332576d7 band 6 1053 Hz Q2 -5",
        bands: &[(6, [183, 26, 100, 1, 1])],
        count: None,
    },
    Recorded {
        to: "3dde1805 band 3 -2",
        bands: &[(3, [118, 40, 130, 1, 1])],
        count: None,
    },
    Recorded {
        to: "1dc18140 band 3 654 Hz Q6.3 -6",
        bands: &[(3, [161, 36, 90, 1, 1])],
        count: None,
    },
];

fn band_change(band: u8, codes: Codes) -> TypedChange {
    TypedChange::PeqBand {
        output: 4,
        band,
        filter: PeqKind::from_code(codes[3]).unwrap(),
        frequency_hz: frequency_hz(codes[0]),
        gain_db: gain_db(codes[2]),
        q: q_value(codes[1]),
        shelf_slope_db: Some(if codes[4] == 1 { 12 } else { 6 }),
    }
}

fn write_plan(changes: Vec<TypedChange>) -> WritePlanV1 {
    WritePlanV1 {
        schema: WRITE_PLAN_SCHEMA.to_owned(),
        device_id: 0,
        allow_routing: false,
        allow_unmute: false,
        allow_protection_change: false,
        changes,
    }
}

fn code(snapshot: &SnapshotV1, parameter: u8) -> u16 {
    snapshot.read_reviewed(reviewed_address(8, parameter).unwrap())
}

fn profile() -> DcxOutputProfileV1 {
    DcxOutputProfileV1::from_json(PROFILE).unwrap()
}

/// Replay the recorded chain through the write policy; return every state.
fn replay() -> Vec<(&'static str, SnapshotV1)> {
    let mut state = state_143d32b5().snapshot();
    let mut states = vec![("143d32b5", state.clone())];
    for step in TONIGHT {
        let mut changes: Vec<_> = step
            .bands
            .iter()
            .map(|(band, codes)| band_change(*band, *codes))
            .collect();
        if let Some(count) = step.count {
            changes.push(TypedChange::PeqBandCount { output: 4, count });
        }
        let transaction = plan_writes(&state, &write_plan(changes), &WritePolicyV1::default())
            .unwrap_or_else(|error| panic!("{}: {error}", step.to));
        state = transaction.apply_plan.desired().clone();
        states.push((step.to, state.clone()));
    }
    states
}

/// Profile of a replayed state, built from the fixture with the state's
/// bands, so the identity and capture stay the fixture's.
fn profile_at(snapshot: &SnapshotV1) -> DcxOutputProfileV1 {
    let mut profile = profile();
    for band in &mut profile.state.peq.bands {
        let read = |field| code(snapshot, band_parameter(band.band, field));
        let codes = ProfileBandCodesV1 {
            frequency: read(BandField::Frequency),
            gain: read(BandField::Gain),
            q: read(BandField::Q),
            kind: read(BandField::Kind),
            slope: read(BandField::Slope),
        };
        let kind = PeqKind::from_code(codes.kind).unwrap();
        *band = ProfileBandV1 {
            band: band.band,
            kind,
            frequency_milli_hz: milli(frequency_hz(codes.frequency)),
            gain_db_tenths: i16::try_from(i32::from(codes.gain) - 150).unwrap(),
            q_thousandths: milli(q_value(codes.q)),
            shelf_slope_db: (kind != PeqKind::Bell).then_some(12),
            codes,
        };
    }
    profile.validate().unwrap();
    profile
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn milli(value: f64) -> u32 {
    (value * 1000.0).round() as u32
}

#[test]
fn the_legalab_fixture_parses_and_validates() {
    let profile = profile();
    assert_eq!(profile.id, "dcx-o4-vocal-1dc18140");
    assert_eq!(profile.output, 4);
    assert_eq!(profile.state.peq.count, 8);
    assert_eq!(
        profile_digest(PROFILE),
        "sha256/942c886415b3a8d749271a9b9485a6e7f1f756a5117f10f975aea5b041297555"
    );
}

#[test]
fn every_recorded_step_is_admitted_and_ends_at_the_profile() {
    let states = replay();
    let (_, last) = states.last().unwrap();
    let verification = verify_profile(last, &profile(), "x").unwrap();
    assert!(verification.matches(), "{:?}", verification.drift);
    // The starting state does not match.
    let start = verify_profile(&states[0].1, &profile(), "x").unwrap();
    assert_eq!(start.status, "drift");
}

#[test]
fn the_refused_count_of_nine_is_refused_offline() {
    let states = replay();
    let (_, after_bands_6_8) = &states[1];
    let error = plan_writes(
        after_bands_6_8,
        &write_plan(vec![
            band_change(9, [138, 23, 100, 1, 1]),
            TypedChange::PeqBandCount {
                output: 4,
                count: 9,
            },
        ]),
        &WritePolicyV1::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        TypedWriteError::DeviceBandCount { count: 9, .. }
    ));
}

fn chain_gains(baseline: &SnapshotV1, profile: &DcxOutputProfileV1) -> Vec<Vec<(u8, u16, u16)>> {
    let chain = plan_profile_chain(baseline, profile, "x", &WritePolicyV1::default()).unwrap();
    let mut previous = baseline.clone();
    let mut summary = Vec::new();
    for step in &chain.steps {
        let next = step.transaction.apply_plan.desired().clone();
        let moved = (1..=PEQ_BANDS)
            .filter_map(|band| {
                let gain = band_parameter(band, BandField::Gain);
                let frequency = band_parameter(band, BandField::Frequency);
                (code(&previous, gain) != code(&next, gain)
                    || code(&previous, frequency) != code(&next, frequency))
                .then(|| (band, code(&next, gain), code(&next, frequency)))
            })
            .collect();
        summary.push(moved);
        previous = next;
    }
    let verification = verify_profile(&previous, profile, "x").unwrap();
    assert!(verification.matches(), "{:?}", verification.drift);
    summary
}

#[test]
fn the_planner_reproduces_tonights_band_7_retune_exactly() {
    let states = replay();
    // d3989341 -> aaeba8e5 -> bb2ae4f7 -> 7dff30d7 (states 2 through 5).
    assert_eq!(
        chain_gains(&states[2].1, &profile_at(&states[5].1)),
        vec![
            vec![(7, 120, 161)],
            vec![(7, 150, 161)],
            vec![(7, 100, 141)]
        ]
    );
}

#[test]
fn the_planner_steps_tonights_band_2_low_shelf_exactly() {
    let states = replay();
    // 924fb384 -> ae4fafab -> 5861ff9f -> 6f5c4148 (states 15 through 18).
    let target = profile_at(&states[18].1);
    assert_eq!(
        chain_gains(&states[15].1, &target),
        vec![vec![(2, 120, 101)], vec![(2, 150, 101)], vec![(2, 0, 106)]]
    );
}

#[test]
fn the_planner_retunes_only_from_zero_where_tonight_combined_the_last_release() {
    let states = replay();
    // 6f5c4148 -> 332576d7 was one transaction: band 6 from -3 dB straight
    // to 1053 Hz -5 dB (a 3 dB release the policy admits). The planner raises
    // it to 0 dB first and retunes from there: one more transaction.
    assert_eq!(
        chain_gains(&states[18].1, &profile_at(&states[19].1)),
        vec![vec![(6, 150, 213)], vec![(6, 100, 183)]]
    );
}

#[test]
fn the_planner_chain_from_143d32b5_to_1dc18140() {
    let baseline = state_143d32b5().snapshot();
    let profile = profile();
    let chain = plan_profile_chain(&baseline, &profile, "x", &WritePolicyV1::default()).unwrap();
    assert_eq!(chain.status, "planned");
    // Free moves first (bands 6-8 written and switched on), then the
    // releases of bands 2, 3 and 4 in 3 dB steps, each retuned from 0 dB.
    assert_eq!(
        chain_gains(&baseline, &profile),
        vec![
            vec![(2, 120, 101), (6, 100, 183), (7, 100, 141), (8, 90, 228)],
            vec![(2, 150, 101)],
            vec![(2, 0, 106), (3, 130, 118)],
            vec![(3, 150, 118), (4, 120, 251)],
            vec![(3, 90, 161), (4, 150, 251)],
            vec![(4, 60, 237)],
        ]
    );
    assert!(chain.steps.iter().all(|step| step.rise_db <= 3.0));
    assert_eq!(chain.budget.transactions, 6);
    assert_eq!(chain.budget.apply_remote_mode_writes_total, 30);
    let first = &chain.steps[0].transaction;
    assert_eq!(code(first.apply_plan.desired(), EQ_COUNT_PARAMETER), 8);
    assert!(first.apply_plan.action_count() <= 16);
}

#[test]
fn every_recorded_state_plans_a_chain_to_the_profile() {
    let profile = profile();
    for (label, state) in replay() {
        let chain = plan_profile_chain(&state, &profile, "x", &WritePolicyV1::default())
            .unwrap_or_else(|error| panic!("{label}: {error}"));
        let verification = verify_profile(chain.final_snapshot(&state), &profile, "x").unwrap();
        assert!(verification.matches(), "{label}");
        // Band 9 is never active and never written.
        let last = chain.final_snapshot(&state);
        assert!(code(last, EQ_COUNT_PARAMETER) <= 8);
        assert_eq!(
            code(last, band_parameter(9, BandField::Gain)),
            code(&state, band_parameter(9, BandField::Gain)),
            "{label}"
        );
        assert!(code(last, band_parameter(9, BandField::Gain)) <= UNITY_GAIN_CODE);
    }
}

#[test]
fn a_profile_naming_nine_bands_is_refused() {
    let mut value: serde_json::Value = serde_json::from_slice(PROFILE).unwrap();
    value["state"]["peq"]["count"] = serde_json::json!(9);
    let bytes = serde_json::to_vec(&value).unwrap();
    assert!(matches!(
        DcxOutputProfileV1::from_json(&bytes),
        Err(OutputProfileError::DeviceBandCount(9))
    ));
}

#[test]
fn an_unmuted_profile_never_unmutes_a_muted_output() {
    let mut builder = state_143d32b5();
    builder.set(8, MUTE_PARAMETER, 1);
    let error = plan_profile_chain(
        &builder.snapshot(),
        &profile(),
        "x",
        &WritePolicyV1::default(),
    )
    .unwrap_err();
    assert!(
        matches!(error, OutputProfileError::VerifyOnlyDrift(ref fields) if fields == &["mute"]),
        "{error}"
    );
}
