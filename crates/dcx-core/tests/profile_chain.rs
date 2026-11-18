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
    // The planned end state is the night's replayed end state, except band
    // 9's stored values, which the refused count-9 write left behind.
    let (_, night) = replay().pop().unwrap();
    let planned = chain.final_snapshot(&baseline);
    for channel in 1..=10_u8 {
        for parameter in 0..=0x7f_u8 {
            let Some(address) = reviewed_address(channel, parameter) else {
                continue;
            };
            let band9 = channel == 8
                && (band_parameter(9, BandField::Frequency)..=band_parameter(9, BandField::Slope))
                    .contains(&parameter);
            if !band9 {
                assert_eq!(
                    planned.read_reviewed(address),
                    night.read_reviewed(address),
                    "channel {channel} {}",
                    address.field.label()
                );
            }
        }
    }
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
    let baseline = builder.snapshot();
    let chain = plan_profile_chain(&baseline, &profile(), "x", &WritePolicyV1::default()).unwrap();
    assert!(chain.held_muted);
    assert_eq!(chain.steps.len(), 6);
    for step in &chain.steps {
        assert!(
            step.transaction
                .writes
                .iter()
                .all(|write| write.field != "mute")
        );
    }
    assert_eq!(code(chain.final_snapshot(&baseline), MUTE_PARAMETER), 1);
}

fn room_expectation(snapshot: &SnapshotV1) -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": "dcx.room-expectation/v1",
        "sceneId": "practice-ready",
        "sceneRevision": 1,
        "compiledDigest": format!("sha256/{}", "a".repeat(64)),
        "presetDigest": format!("sha256/{}", "b".repeat(64)),
        "approvedSnapshotDigest": snapshot.digest(),
        "outputMutes": [true, true, true, false, true, true],
        "profiles": [profile()],
    })
}

#[test]
fn room_match_requires_approved_image_and_reviewed_profiles() {
    use dcx_core::room_verification::{RoomVerification, verify_room};
    let states = replay();
    let snapshot = &states.last().unwrap().1;
    let expected = room_expectation(snapshot);
    let bytes = serde_json::to_vec(&expected).unwrap();
    let RoomVerification::V1(comparison) = verify_room(snapshot, &bytes).unwrap() else {
        panic!("a v1 expectation yields a v1 comparison");
    };
    assert!(comparison.matches);
    assert!(comparison.readable);
    assert_eq!(comparison.expectation_digest, profile_digest(&bytes));
    assert!(comparison.compared_fields > 7);

    // A self-consistent but different image never inherits the approved baseline.
    assert!(!verify_room(&states[0].1, &bytes).unwrap().matches());
    // A contradictory typed profile cannot approve even the exact same bytes.
    let mut contradicted = expected;
    contradicted["profiles"][0]["state"]["gainDbTenths"] = serde_json::json!(-10);
    assert!(
        !verify_room(snapshot, &serde_json::to_vec(&contradicted).unwrap())
            .unwrap()
            .matches()
    );
}

#[test]
fn every_unreviewed_jbl_source_change_fails_exact_image_admission() {
    use dcx_core::{SnapshotSection, layout::output_location, room_verification::verify_room};
    let states = replay();
    let baseline = &states.last().unwrap().1;
    let bytes = serde_json::to_vec(&room_expectation(baseline)).unwrap();
    for output in 1..=2 {
        assert!(reviewed_address(output_channel(output), SOURCE_PARAMETER).is_none());
        let location = output_location(output, SOURCE_PARAMETER).unwrap();
        for source in 0..=3 {
            let mut dump0 = baseline.frame(SnapshotSection::Dump0).to_vec();
            let mut dump1 = baseline.frame(SnapshotSection::Dump1).to_vec();
            let frame = if location.part() == DumpPart::Part0 {
                &mut dump0
            } else {
                &mut dump1
            };
            let before = location.read(frame);
            location.write(frame, source).unwrap();
            let changed =
                SnapshotV1::from_frames(baseline.frame(SnapshotSection::Identity), &dump0, &dump1)
                    .unwrap();
            let comparison = verify_room(&changed, &bytes).unwrap();
            assert_eq!(
                comparison.matches(),
                source == before,
                "output {output}, source {source}"
            );
        }
    }
}

#[test]
fn room_expectation_refuses_unsafe_or_ambiguous_bindings() {
    use dcx_core::room_verification::RoomExpectationV1;
    let states = replay();
    let baseline = &states.last().unwrap().1;
    for field in ["compiledDigest", "presetDigest", "approvedSnapshotDigest"] {
        let mut value = room_expectation(baseline);
        value[field] = serde_json::json!("sha256/not-a-digest");
        assert!(RoomExpectationV1::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    for output in 4..=5 {
        let mut value = room_expectation(baseline);
        value["outputMutes"][output] = serde_json::json!(false);
        assert!(RoomExpectationV1::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let mut value = room_expectation(baseline);
    value["approveCurrentReadAutomatically"] = serde_json::json!(true);
    assert!(RoomExpectationV1::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn approved_refresh_requires_exact_independent_expectation_bytes() {
    use dcx_core::{
        output_profile::profile_digest, protocol::DeviceId,
        room_verification::ApprovedRoomRefreshAdmission,
    };
    let states = replay();
    let snapshot = &states.last().unwrap().1;
    let expected = room_expectation(snapshot);
    let bytes = serde_json::to_vec(&expected).unwrap();
    let digest = profile_digest(&bytes);
    let device = DeviceId::new(0).unwrap();
    let admission = ApprovedRoomRefreshAdmission::from_json(&bytes, &digest, device).unwrap();
    assert_eq!(admission.expectation(), bytes);
    assert_eq!(admission.expected_device(), device);
    let mut whitespace = bytes.clone();
    whitespace.push(b' ');
    assert!(ApprovedRoomRefreshAdmission::from_json(&whitespace, &digest, device).is_err());
    for index in 7..digest.len() {
        let mut changed = digest.clone().into_bytes();
        changed[index] = if changed[index] == b'0' { b'1' } else { b'0' };
        assert!(
            ApprovedRoomRefreshAdmission::from_json(
                &bytes,
                std::str::from_utf8(&changed).unwrap(),
                device
            )
            .is_err()
        );
    }
    for key in [
        "approvedSnapshotDigest",
        "profiles",
        "outputMutes",
        "sceneRevision",
    ] {
        let mut malformed = expected.clone();
        malformed.as_object_mut().unwrap().remove(key);
        let raw = serde_json::to_vec(&malformed).unwrap();
        assert!(
            ApprovedRoomRefreshAdmission::from_json(&raw, &profile_digest(&raw), device).is_err()
        );
    }
    let mut unsafe_reserve = expected;
    unsafe_reserve["outputMutes"][5] = serde_json::json!(false);
    let raw = serde_json::to_vec(&unsafe_reserve).unwrap();
    assert!(ApprovedRoomRefreshAdmission::from_json(&raw, &profile_digest(&raw), device).is_err());
}

// TIN-5385 step 6: a preset-bound expectation binds the DCX preset and the
// scene's `dcx` block, never a scene revision.
fn preset_expectation(snapshot: &SnapshotV1) -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": "dcx.room-expectation/v2",
        "presetId": "practice-room",
        "presetDigest": format!("sha256/{}", "b".repeat(64)),
        "dcxBlockDigest": format!("sha256/{}", "c".repeat(64)),
        "snapshotDigest": snapshot.digest(),
        "approved": true,
        "basis": "SYNTHETIC approval for tests",
        "outputMutes": [true, true, true, false, true, true],
        "profiles": [profile()],
    })
}

fn object_keys(value: &serde_json::Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

#[test]
fn preset_expectation_compares_like_v1_and_names_no_scene() {
    use dcx_core::room_verification::{RoomVerification, verify_room};
    let states = replay();
    let snapshot = &states.last().unwrap().1;
    let v1 = serde_json::to_vec(&room_expectation(snapshot)).unwrap();
    let v2 = serde_json::to_vec(&preset_expectation(snapshot)).unwrap();
    let old = verify_room(snapshot, &v1).unwrap();
    let new = verify_room(snapshot, &v2).unwrap();
    assert!(old.matches() && new.matches());
    let RoomVerification::V2(comparison) = &new else {
        panic!("a v2 expectation yields a v2 comparison");
    };
    assert!(comparison.approved);
    assert_eq!(comparison.expectation_digest, profile_digest(&v2));
    assert_eq!(comparison.preset_id, "practice-room");
    // Same image checks: identity, six mutes and every profile field.
    let RoomVerification::V1(reference) = &old else {
        panic!("v1 stays v1")
    };
    assert_eq!(comparison.compared_fields, reference.compared_fields);
    assert!(!verify_room(&states[0].1, &v2).unwrap().matches());

    // Wire forms: v1 output keeps its exact key set for installed helpers.
    let old_json = serde_json::to_value(&old).unwrap();
    assert_eq!(old_json["schemaVersion"], "dcx.room-comparison/v1");
    assert_eq!(
        object_keys(&old_json),
        [
            "comparedFields",
            "compiledDigest",
            "expectationDigest",
            "expectedSnapshotDigest",
            "matches",
            "presetDigest",
            "profiles",
            "readable",
            "sceneId",
            "sceneRevision",
            "snapshotDigest",
            "schemaVersion",
        ]
        .iter()
        .map(ToString::to_string)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
    );
    let new_json = serde_json::to_value(&new).unwrap();
    assert_eq!(new_json["schemaVersion"], "dcx.room-comparison/v2");
    for key in ["sceneId", "sceneRevision", "compiledDigest"] {
        assert!(new_json.get(key).is_none(), "{key}");
    }
    assert_eq!(new_json["presetId"], "practice-room");
    assert_eq!(
        new_json["dcxBlockDigest"],
        format!("sha256/{}", "c".repeat(64))
    );
    assert_eq!(new_json["approved"], true);
}

#[test]
fn preset_expectation_refuses_malformed_or_partial_approval() {
    use dcx_core::room_verification::{RoomExpectation, RoomExpectationV2};
    let states = replay();
    let baseline = &states.last().unwrap().1;
    let decode = |value: &serde_json::Value| {
        RoomExpectationV2::from_json(&serde_json::to_vec(value).unwrap())
    };
    assert!(decode(&preset_expectation(baseline)).unwrap().approved);
    // An approved file must carry both Legalab digests.
    for key in ["presetDigest", "dcxBlockDigest"] {
        let mut value = preset_expectation(baseline);
        value[key] = serde_json::Value::Null;
        assert!(decode(&value).is_err(), "{key} null");
        value.as_object_mut().unwrap().remove(key);
        assert!(decode(&value).is_err(), "{key} absent");
        // The same gap is admitted while unapproved.
        value["approved"] = serde_json::json!(false);
        assert!(decode(&value).is_ok(), "{key} unapproved");
    }
    for (key, bad) in [
        ("presetDigest", serde_json::json!("sha256/not-a-digest")),
        ("dcxBlockDigest", serde_json::json!("sha256/ABC")),
        ("snapshotDigest", serde_json::json!("sha256/")),
        ("presetId", serde_json::json!("Practice Room")),
        ("presetId", serde_json::json!("")),
        ("basis", serde_json::json!("")),
        ("basis", serde_json::json!("line\nbreak")),
        (
            "schemaVersion",
            serde_json::json!("dcx.room-expectation/v3"),
        ),
    ] {
        let mut value = preset_expectation(baseline);
        value[key] = bad;
        assert!(decode(&value).is_err(), "{key}");
    }
    for key in [
        "sceneId",
        "sceneRevision",
        "compiledDigest",
        "approvedSnapshotDigest",
    ] {
        let mut value = preset_expectation(baseline);
        value[key] = serde_json::json!("x");
        assert!(decode(&value).is_err(), "scene key {key} is unknown in v2");
    }
    let mut value = preset_expectation(baseline);
    value["outputMutes"][5] = serde_json::json!(false);
    assert!(decode(&value).is_err());
    // The dispatcher refuses an unknown schema rather than guessing a form.
    let mut value = preset_expectation(baseline);
    value["schemaVersion"] = serde_json::json!("dcx.room-expectation/v9");
    assert!(RoomExpectation::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn unapproved_preset_compares_offline_but_never_admits_a_refresh() {
    use dcx_core::{
        protocol::DeviceId,
        room_verification::{ApprovedRoomRefreshAdmission, RoomVerification, verify_room},
    };
    let states = replay();
    let snapshot = &states.last().unwrap().1;
    let device = DeviceId::new(0).unwrap();
    let approved = serde_json::to_vec(&preset_expectation(snapshot)).unwrap();
    assert!(
        ApprovedRoomRefreshAdmission::from_json(&approved, &profile_digest(&approved), device)
            .is_ok()
    );
    let mut pending = preset_expectation(snapshot);
    pending["approved"] = serde_json::json!(false);
    pending["presetDigest"] = serde_json::Value::Null;
    let raw = serde_json::to_vec(&pending).unwrap();
    // A matching readback is the evidence for approval, not approval itself.
    let RoomVerification::V2(comparison) = verify_room(snapshot, &raw).unwrap() else {
        panic!("v2")
    };
    assert!(comparison.matches && !comparison.approved);
    assert!(ApprovedRoomRefreshAdmission::from_json(&raw, &profile_digest(&raw), device).is_err());
}

const PRACTICE_ROOM: &[u8] =
    include_bytes!("../../../fixtures/room-expectations/LEGALAB-practice-room.json");
const JBL_PROTECTED: &[u8] =
    include_bytes!("../../../fixtures/room-expectations/LEGALAB-jbl-protected.json");
const JBL_PLAN: &[u8] = include_bytes!("../../../docs/plans/jbl-protection-20a5b485/expected.json");

#[test]
fn declared_preset_files_bind_the_approved_and_planned_images() {
    use dcx_core::{
        protocol::DeviceId,
        room_verification::{ApprovedRoomRefreshAdmission, RoomExpectationV2},
    };
    let device = DeviceId::new(0).unwrap();
    let room = RoomExpectationV2::from_json(PRACTICE_ROOM).unwrap();
    assert_eq!(room.preset_id, "practice-room");
    assert!(room.approved);
    assert_eq!(
        room.snapshot_digest,
        "sha256/20a5b485a4b4053b4b7026b9fb53517d210cbb3b456aa7f135b578c9b728b5e7"
    );
    // The shared preset digest the LS56 agent pins today, and the canonical
    // digest of the practice-room `dcx` block in the scene catalog.
    assert_eq!(
        room.preset_digest.as_deref(),
        Some("sha256/a361aecd15c91f173bc475361f455cc6e0772acf92a84efc18092ba0a35a6038")
    );
    assert_eq!(
        room.dcx_block_digest.as_deref(),
        Some("sha256/ce403b60c9ae9cf40bc2d36358de9aba82deaf49ac0234caf9c2c63e1a9fae88")
    );
    assert!(
        ApprovedRoomRefreshAdmission::from_json(
            PRACTICE_ROOM,
            &profile_digest(PRACTICE_ROOM),
            device
        )
        .is_ok()
    );

    let jbl = RoomExpectationV2::from_json(JBL_PROTECTED).unwrap();
    assert_eq!(jbl.preset_id, "jbl-protected");
    assert!(!jbl.approved);
    let plan: serde_json::Value = serde_json::from_slice(JBL_PLAN).unwrap();
    let applies = plan["applies"].as_array().unwrap();
    assert_eq!(
        plan["baseline_snapshot_digest"].as_str(),
        Some(room.snapshot_digest.as_str())
    );
    assert_eq!(
        applies.last().unwrap()["desired_snapshot_digest"].as_str(),
        Some(jbl.snapshot_digest.as_str())
    );
    assert_eq!(
        jbl.snapshot_digest,
        "sha256/dffe9c614bcbf363613a76b91baf17366717b36311c53e58d43d9461466d3ddf"
    );
    // The plan writes no mute, so the planned image keeps the baseline mutes.
    assert_eq!(jbl.output_mutes, room.output_mutes);
    assert!(
        ApprovedRoomRefreshAdmission::from_json(
            JBL_PROTECTED,
            &profile_digest(JBL_PROTECTED),
            device
        )
        .is_err()
    );
}
