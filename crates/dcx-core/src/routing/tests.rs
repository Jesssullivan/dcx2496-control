//! Synthetic projection, refusal, and seeded property tests for the closed
//! MVP routing lane. Every frame here is synthetic; nothing is device evidence.

use super::*;
use crate::{
    ApplyPlanError, ApplyPlanV1, ApplyTransactionV1, SnapshotProjectionError, SnapshotSection,
    layout::{EQ_ENABLED_PARAMETER, INPUT_SUM_LOCATION},
    peq_bank::DesiredProfile,
    protocol::{DUMP0_RESPONSE_LEN, DUMP1_RESPONSE_LEN},
};

const DUMP0_TRAILER: usize = DUMP0_RESPONSE_LEN - 2;
const DUMP1_TRAILER: usize = DUMP1_RESPONSE_LEN - 2;

/// Small deterministic xorshift generator; no external dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

fn identity() -> Vec<u8> {
    let mut frame = vec![0xf0, 0x00, 0x20, 0x32, 0x00, 0x0e, 0x00];
    frame.extend_from_slice(b"SYNTHETIC-IDENTITY");
    frame.push(0xf7);
    frame
}

fn blank_dump(part: u8) -> Vec<u8> {
    let length = if part == 0 {
        DUMP0_RESPONSE_LEN
    } else {
        DUMP1_RESPONSE_LEN
    };
    let mut frame = vec![0; length];
    frame[..7].copy_from_slice(&[0xf0, 0x00, 0x20, 0x32, 0x00, 0x0e, 0x10]);
    frame[12] = part;
    frame[length - 1] = 0xf7;
    frame
}

/// Synthetic baseline shaped like the routing the ruling starts from: input
/// sum off, O3/O4 sourced from B, O4/O5/O6 unmuted. Trailers are arbitrary.
fn synthetic_baseline() -> SnapshotV1 {
    let mut dump0 = blank_dump(0);
    let mut dump1 = blank_dump(1);
    dump0[DUMP0_TRAILER] = 7;
    dump1[195] = 1;
    dump1[365] = 1;
    dump1[DUMP1_TRAILER] = 78;
    SnapshotV1::from_frames(&identity(), &dump0, &dump1).unwrap()
}

fn random_baseline(rng: &mut Rng) -> SnapshotV1 {
    let mut dump0 = blank_dump(0);
    let mut dump1 = blank_dump(1);
    for frame in [&mut dump0, &mut dump1] {
        let length = frame.len();
        for byte in &mut frame[13..length - 1] {
            *byte = u8::try_from(rng.below(0x80)).unwrap();
        }
    }
    dump0[117] = [0, 4][usize::try_from(rng.below(2)).unwrap()];
    for offset in [223, 392, 561] {
        dump1[offset] = u8::try_from(rng.below(2)).unwrap();
    }
    for offset in [195, 365] {
        dump1[offset] = u8::try_from(rng.below(4)).unwrap();
    }
    SnapshotV1::from_frames(&identity(), &dump0, &dump1).unwrap()
}

fn action(channel: u8, parameter: u8, value: u16) -> DirectParameterAction {
    DirectParameterAction::new(channel, parameter, value).unwrap()
}

fn profile(
    actions: Vec<DirectParameterAction>,
) -> Result<DesiredRoutingProfileV1, DesiredProfileError> {
    DesiredRoutingProfileV1::new("mvp".into(), "r1".into(), RoutingDocumentV1 { actions })
}

fn changed_bytes(before: &SnapshotV1, after: &SnapshotV1) -> Vec<(SnapshotSection, usize)> {
    let mut changed = Vec::new();
    for section in [
        SnapshotSection::Identity,
        SnapshotSection::Dump0,
        SnapshotSection::Dump1,
    ] {
        for (index, (left, right)) in before
            .frame(section)
            .iter()
            .zip(after.frame(section))
            .enumerate()
        {
            if left != right {
                changed.push((section, index));
            }
        }
    }
    changed
}

#[test]
fn mvp_targets_produce_the_ruling_actions_in_apply_order() {
    let actions = RoutingTargetsV1::mvp().actions().unwrap();
    assert_eq!(
        actions,
        [
            action(9, 0x03, 1),
            action(10, 0x03, 1),
            action(0, 0x02, 4),
            action(8, 0x41, 2),
            action(7, 0x41, 3),
        ]
    );
    assert!(matches!(
        RoutingTargetsV1::default().actions(),
        Err(RoutingError::EmptyTargets)
    ));
}

#[test]
fn routing_profile_round_trips_and_detects_tampering() {
    let profile = DesiredRoutingProfileV1::from_targets(
        "mvp-routing".into(),
        "2026-10-07".into(),
        RoutingTargetsV1::mvp(),
    )
    .unwrap();
    let json = profile.to_json().unwrap();
    assert_eq!(DesiredRoutingProfileV1::from_json(&json).unwrap(), profile);
    let parsed = DesiredProfile::from_json(&json).unwrap();
    assert_eq!(parsed.schema(), DESIRED_ROUTING_SCHEMA);
    assert_eq!(parsed.actions(), profile.document().actions.as_slice());

    let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
    value["document"]["actions"][3]["value"] = 1.into();
    assert!(matches!(
        DesiredRoutingProfileV1::from_json(&serde_json::to_vec(&value).unwrap()),
        Err(DesiredProfileError::DigestMismatch { .. })
    ));
    let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
    value["document"]["extra"] = 1.into();
    assert!(DesiredRoutingProfileV1::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn synthetic_mvp_projection_changes_exactly_the_reviewed_bytes_and_trailers() {
    let baseline = synthetic_baseline();
    let actions = RoutingTargetsV1::mvp().actions().unwrap();
    let desired = baseline.project_direct_actions(&actions).unwrap();
    assert_eq!(
        changed_bytes(&baseline, &desired),
        [
            (SnapshotSection::Dump0, 117),
            (SnapshotSection::Dump0, DUMP0_TRAILER),
            (SnapshotSection::Dump1, 195),
            (SnapshotSection::Dump1, 365),
            (SnapshotSection::Dump1, 392),
            (SnapshotSection::Dump1, 561),
            (SnapshotSection::Dump1, DUMP1_TRAILER),
        ]
    );
    let dump0 = desired.frame(SnapshotSection::Dump0);
    let dump1 = desired.frame(SnapshotSection::Dump1);
    assert_eq!(dump0[117], 4);
    // Dump0 trailer balance: +4 payload lowers the trailer by 4 (7 -> 3).
    assert_eq!(dump0[DUMP0_TRAILER], 3);
    assert_eq!(
        (dump1[195], dump1[365], dump1[392], dump1[561]),
        (3, 2, 1, 1)
    );
    // Dump1: +2 +1 +1 +1 payload lowers the trailer by 5 (78 -> 73).
    assert_eq!(dump1[DUMP1_TRAILER], 73);
    // The forbidden Input C byte and its carrier never move.
    assert_eq!(dump0[121], baseline.frame(SnapshotSection::Dump0)[121]);
    assert_eq!(dump0[124], baseline.frame(SnapshotSection::Dump0)[124]);

    let state = decode_routing(&desired).unwrap();
    assert_eq!(
        (
            state.o4_muted,
            state.o5_muted,
            state.o6_muted,
            state.input_sum_code,
            state.o4_source,
            state.o3_source
        ),
        (false, true, true, 4, OutputSource::C, OutputSource::Sum)
    );
    let before = decode_routing(&baseline).unwrap();
    assert_eq!(
        (before.input_sum_code, before.o4_source, before.o3_source),
        (0, OutputSource::B, OutputSource::B)
    );
}

#[test]
fn staged_mvp_transaction_rolls_back_in_exact_reverse_order() {
    let baseline = synthetic_baseline();
    let actions = RoutingTargetsV1::mvp().actions().unwrap();
    let transaction =
        ApplyTransactionV1::stage_projected(baseline.clone(), actions.clone()).unwrap();
    let apply = transaction
        .apply_plan()
        .command()
        .unwrap()
        .actions()
        .to_vec();
    assert_eq!(apply, actions);
    let rollback = transaction
        .rollback_plan()
        .command()
        .unwrap()
        .actions()
        .to_vec();
    assert_eq!(
        rollback,
        [
            action(7, 0x41, 1),
            action(8, 0x41, 1),
            action(0, 0x02, 0),
            action(10, 0x03, 0),
            action(9, 0x03, 0),
        ]
    );
    let desired = transaction.apply_plan().desired().clone();
    assert_eq!(desired.project_direct_actions(&rollback).unwrap(), baseline);
    // Carriers round-trip through strict JSON.
    let json = transaction.apply_plan().to_json().unwrap();
    assert_eq!(
        &ApplyPlanV1::from_json(&json).unwrap(),
        transaction.apply_plan()
    );
}

#[test]
fn apply_plan_refuses_routing_out_of_order() {
    let baseline = synthetic_baseline();
    for actions in [
        // O3 SUM before the input sum it consumes.
        vec![action(7, 0x41, 3), action(0, 0x02, 4)],
        // A source before a mute.
        vec![action(8, 0x41, 2), action(9, 0x03, 1)],
        // O6 before O5.
        vec![action(10, 0x03, 1), action(9, 0x03, 1)],
        // O3 source before O4 source.
        vec![action(7, 0x41, 3), action(8, 0x41, 2)],
    ] {
        let desired = baseline.project_direct_actions(&actions).unwrap();
        assert!(matches!(
            ApplyPlanV1::new(&baseline, &desired, actions.clone()),
            Err(ApplyPlanError::RoutingOrder { .. })
        ));
        assert!(profile(actions).is_err());
    }
}

#[test]
fn every_unreviewed_routing_neighbor_is_refused() {
    let baseline = synthetic_baseline();
    let mut refused = vec![
        // O1/O2/O5/O6 sources.
        (5, 0x41, 0),
        (6, 0x41, 0),
        (9, 0x41, 2),
        (10, 0x41, 2),
        // O1/O2/O3 mutes.
        (5, 0x03, 1),
        (6, 0x03, 1),
        (7, 0x03, 1),
        // Input A..C and input-sum channel mutes and gains.
        (1, 0x03, 1),
        (4, 0x03, 1),
        (3, 0x02, 0),
    ];
    // Crossover on every output.
    for channel in 5..=10 {
        for parameter in 0x42..=0x45 {
            refused.push((channel, parameter, 0));
        }
    }
    // Every other setup parameter, including Input C gain (0x04, Dump0 121)
    // and Mute Outs (0x15).
    for parameter in (0..=0x7f).filter(|parameter| *parameter != 0x02) {
        refused.push((0, parameter, 0));
    }
    for (channel, parameter, value) in refused {
        let candidate = action(channel, parameter, value);
        assert!(
            matches!(
                baseline.project_direct_actions(&[candidate]),
                Err(SnapshotProjectionError::UnmappedAction { .. })
            ),
            "{channel}/{parameter:#04x}"
        );
        assert!(
            profile(vec![candidate]).is_err(),
            "{channel}/{parameter:#04x}"
        );
    }
    // PEQ fields are reviewed but not routing.
    assert!(profile(vec![action(8, EQ_ENABLED_PARAMETER, 1)]).is_err());
}

#[test]
fn routing_values_outside_the_reviewed_domain_are_refused() {
    let baseline = synthetic_baseline();
    for value in [1, 2, 3, 5, 6, 7, 0x7f] {
        let sum = action(0, 0x02, value);
        assert!(matches!(
            baseline.project_direct_actions(&[sum]),
            Err(SnapshotProjectionError::ValueNotAdmitted { .. })
        ));
        assert!(profile(vec![sum]).is_err());
    }
    // Source 4 is past SUM; mute 2 is not an on/off state.
    for candidate in [action(7, 0x41, 4), action(9, 0x03, 2)] {
        assert!(baseline.project_direct_actions(&[candidate]).is_err());
        assert!(profile(vec![candidate]).is_err());
    }
    // A baseline input sum outside off/A+B cannot be rolled back to: fail closed.
    let mut dump0 = baseline.frame(SnapshotSection::Dump0).to_vec();
    dump0[117] = 1;
    let foreign = SnapshotV1::from_frames(
        baseline.frame(SnapshotSection::Identity),
        &dump0,
        baseline.frame(SnapshotSection::Dump1),
    )
    .unwrap();
    assert!(foreign.inverse_actions_for(&[action(0, 0x02, 4)]).is_err());
    assert_eq!(INPUT_SUM_LOCATION.low_offset(), 117);
}

#[test]
fn random_routing_subsets_round_trip_and_touch_only_their_bytes() {
    let mut rng = Rng(0x0dc2_4960_5eed_0007);
    let mut exercised = 0;
    for case in 0..600 {
        let baseline = random_baseline(&mut rng);
        let pick_bool = |rng: &mut Rng| (rng.below(3) > 0).then(|| rng.below(2) == 1);
        let pick_source = |rng: &mut Rng| {
            (rng.below(3) > 0)
                .then(|| OutputSource::from_code(u16::try_from(rng.below(4)).unwrap()).unwrap())
        };
        let targets = RoutingTargetsV1 {
            o4_muted: pick_bool(&mut rng),
            o5_muted: pick_bool(&mut rng),
            o6_muted: pick_bool(&mut rng),
            input_sum: (rng.below(3) > 0).then(|| {
                if rng.below(2) == 1 {
                    InputSum::APlusB
                } else {
                    InputSum::Off
                }
            }),
            o4_source: pick_source(&mut rng),
            o3_source: pick_source(&mut rng),
        };
        let Ok(actions) = targets.actions() else {
            continue;
        };
        let bound = profile(actions.clone()).unwrap();
        assert_eq!(bound.document().actions, actions);
        let desired = baseline.project_direct_actions(&actions).unwrap();

        // Only the target low bytes and the two trailers may change.
        let mut allowed = vec![
            (SnapshotSection::Dump0, DUMP0_TRAILER),
            (SnapshotSection::Dump1, DUMP1_TRAILER),
        ];
        for candidate in &actions {
            let address = reviewed_address(candidate.channel(), candidate.parameter()).unwrap();
            let section = match address.location.part() {
                crate::protocol::DumpPart::Part0 => SnapshotSection::Dump0,
                crate::protocol::DumpPart::Part1 => SnapshotSection::Dump1,
            };
            assert_eq!(address.location.offsets(), [address.location.low_offset()]);
            allowed.push((section, address.location.low_offset()));
        }
        for changed in changed_bytes(&baseline, &desired) {
            assert!(
                allowed.contains(&changed),
                "case {case} changed {changed:?}"
            );
        }

        // Exact readback, exact reverse rollback, idempotence.
        assert_eq!(desired.inverse_actions_for(&actions).unwrap(), actions);
        let rollback = baseline.rollback_actions_for(&actions).unwrap();
        let reversed: Vec<_> = rollback
            .iter()
            .rev()
            .map(|item| (item.channel(), item.parameter()))
            .collect();
        let forward: Vec<_> = actions
            .iter()
            .map(|item| (item.channel(), item.parameter()))
            .collect();
        assert_eq!(reversed, forward);
        assert_eq!(desired.project_direct_actions(&rollback).unwrap(), baseline);
        assert_eq!(desired.project_direct_actions(&actions).unwrap(), desired);
        if desired != baseline {
            ApplyTransactionV1::stage_projected(baseline.clone(), actions.clone()).unwrap();
        }

        // Any non-identity permutation of two or more actions is refused.
        if actions.len() >= 2 {
            let mut shuffled = actions.clone();
            let length = u64::try_from(shuffled.len()).unwrap();
            let first = usize::try_from(rng.below(length)).unwrap();
            let mut second = usize::try_from(rng.below(length)).unwrap();
            if first == second {
                second = (second + 1) % shuffled.len();
            }
            shuffled.swap(first, second);
            assert!(profile(shuffled).is_err(), "case {case}");
        }
        exercised += 1;
    }
    assert!(exercised > 400, "only {exercised} cases exercised");
}

#[test]
fn mvp_command_encodes_the_semantic_frame_byte_exactly() {
    let actions = RoutingTargetsV1::mvp().actions().unwrap();
    let frame = crate::protocol::DirectParameterCommand::new(
        crate::protocol::DeviceId::new(0).unwrap(),
        actions,
    )
    .unwrap()
    .encode()
    .unwrap();
    assert_eq!(
        frame,
        [
            0xf0, 0x00, 0x20, 0x32, 0x00, 0x0e, 0x20, 0x05, // header, five actions
            0x09, 0x03, 0x00, 0x01, // O5 mute on
            0x0a, 0x03, 0x00, 0x01, // O6 mute on
            0x00, 0x02, 0x00, 0x04, // setup input sum A+B
            0x08, 0x41, 0x00, 0x02, // O4 source C
            0x07, 0x41, 0x00, 0x03, // O3 source SUM
            0xf7,
        ]
    );
}
