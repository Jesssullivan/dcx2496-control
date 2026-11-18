//! Linked crossover corners, replayed on a fake DCX.
//!
//! The plans are in `docs/plans/linked-corners-fa3b05b4`. The fake device
//! writes each direct-parameter action in frame order. A link it honours
//! moves the follower to the written code when the follower held the driver's
//! code before the write, as the named device did on 2026-10-20 (O4 high-pass
//! to O3 low-pass) and on 2026-11-16 (O2 high-pass to O1 low-pass).
//!
//! The always-run tests rebuild synthetic snapshots whose typed codes equal
//! the decode of the device images `fa3b05b4` and `b31a7e38` at the fields
//! the plans read. The raw captures stay outside Git. The two `#[ignore]`
//! tests replay the same plans from the captured bytes when
//! `DCX_REPLAY_VISIT1_DIR` and `DCX_REPLAY_LIMITER_DIR` name the private
//! snapshot folders.

use std::path::PathBuf;

use dcx_core::{
    ApplyTransactionState, ApplyTransactionV1, DirectParameterAction, SnapshotV1,
    layout::{
        GAIN_PARAMETER, HIGH_PASS_FREQUENCY_PARAMETER, HIGH_PASS_TYPE_PARAMETER,
        LIMITER_ENABLED_PARAMETER, LIMITER_THRESHOLD_PARAMETER, LOW_PASS_FREQUENCY_PARAMETER,
        LOW_PASS_TYPE_PARAMETER, MUTE_PARAMETER, output_channel, reviewed_address,
    },
    protocol::{DUMP0_RESPONSE_LEN, DUMP1_RESPONSE_LEN, DumpPart},
    typed_write::{
        TypedWriteError, WritePlanV1, WritePolicyV1, WriteTransactionV1, plan_writes,
        verify_written_values,
    },
};

const JBL_CORNER: &[u8] = include_bytes!(
    "../../../docs/plans/linked-corners-fa3b05b4/jbl-1-high-pass-corner.write-plan.json"
);
const JBL_ON: &[u8] = include_bytes!(
    "../../../docs/plans/linked-corners-fa3b05b4/jbl-2-high-pass-on.write-plan.json"
);
const SUB_ALONE: &[u8] = include_bytes!(
    "../../../docs/plans/linked-corners-fa3b05b4/sub-o3-low-pass-80hz-alone.write-plan.json"
);
const SUB_80: &[u8] = include_bytes!(
    "../../../docs/plans/linked-corners-fa3b05b4/sub-o3-low-pass-80hz-with-o4.write-plan.json"
);
const SUB_120: &[u8] = include_bytes!(
    "../../../docs/plans/linked-corners-fa3b05b4/sub-o3-low-pass-120hz-with-o4.write-plan.json"
);

/// Attended write rule: 5 remote-mode writes per apply, 3 per rollback. A
/// reading that counts parameter actions also stays inside 3 per frame.
const MAX_ACTIONS: usize = 3;

type Field = (u8, u8);

/// Links the named device showed: a high-pass write moves the low-pass of
/// the output below.
const OBSERVED_LINKS: [(Field, Field); 2] = [
    (
        (6, HIGH_PASS_FREQUENCY_PARAMETER),
        (5, LOW_PASS_FREQUENCY_PARAMETER),
    ),
    (
        (8, HIGH_PASS_FREQUENCY_PARAMETER),
        (7, LOW_PASS_FREQUENCY_PARAMETER),
    ),
];

/// The two reverse directions, never seen on the device.
const REVERSE_LINKS: [(Field, Field); 2] = [
    (
        (5, LOW_PASS_FREQUENCY_PARAMETER),
        (6, HIGH_PASS_FREQUENCY_PARAMETER),
    ),
    (
        (7, LOW_PASS_FREQUENCY_PARAMETER),
        (8, HIGH_PASS_FREQUENCY_PARAMETER),
    ),
];

fn read(snapshot: &SnapshotV1, (channel, parameter): Field) -> u16 {
    snapshot.read_reviewed(reviewed_address(channel, parameter).unwrap())
}

/// A DCX that applies direct-parameter frames in order and honours the links
/// it is given. It keeps each dump's trailer balance as the device does.
struct FakeDcx {
    state: SnapshotV1,
    links: Vec<(Field, Field)>,
}

impl FakeDcx {
    fn new(state: SnapshotV1, reverse_linked: bool) -> Self {
        let mut links = OBSERVED_LINKS.to_vec();
        if reverse_linked {
            links.extend(REVERSE_LINKS);
        }
        Self { state, links }
    }

    fn send(&mut self, actions: &[DirectParameterAction]) {
        for action in actions {
            let field = (action.channel(), action.parameter());
            let follows: Vec<Field> = self
                .links
                .iter()
                .filter(|(driver, follower)| {
                    *driver == field && read(&self.state, *follower) == read(&self.state, *driver)
                })
                .map(|(_, follower)| *follower)
                .collect();
            self.state = self.state.project_direct_actions(&[*action]).unwrap();
            for (channel, parameter) in follows {
                let moved = DirectParameterAction::new(channel, parameter, action.value()).unwrap();
                self.state = self.state.project_direct_actions(&[moved]).unwrap();
            }
        }
    }
}

/// Plan, apply on the fake, verify, roll back on the fake and verify the
/// exact baseline. Returns the transaction and the applied readback.
fn replay(
    base: &SnapshotV1,
    plan: &[u8],
    policy: &WritePolicyV1,
    reverse_linked: bool,
) -> (WriteTransactionV1, SnapshotV1) {
    let plan = WritePlanV1::from_json(plan).unwrap();
    let tx = plan_writes(base, &plan, policy).unwrap();
    assert!(
        tx.writes.len() <= MAX_ACTIONS,
        "{} actions",
        tx.writes.len()
    );
    assert_eq!(tx.wire_budget.apply_remote_mode_writes, 5);
    assert_eq!(tx.wire_budget.rollback_remote_mode_writes, 3);
    assert_eq!(tx.wire_budget.apply_direct_frames, 1);
    assert!(tx.writes.iter().all(|write| write.output <= 4));

    let mut device = FakeDcx::new(base.clone(), reverse_linked);
    device.send(tx.apply_plan.command().unwrap().actions());
    let readback = device.state.clone();
    let verification = verify_written_values(&tx.apply_plan, &readback).unwrap();
    assert_eq!(verification.status, "consistent");
    assert!(verification.readback_matches_desired);
    assert_eq!(verification.uncontained_changes, 0);
    let mut apply = ApplyTransactionV1::from_apply_plan(&tx.apply_plan).unwrap();
    apply.begin_apply().unwrap();
    assert!(apply.verify_apply_readback(&readback).unwrap().is_exact());
    assert_eq!(apply.state(), ApplyTransactionState::Verified);

    let mut rollback = ApplyTransactionV1::resume_rollback(&tx.rollback_plan).unwrap();
    rollback.begin_rollback().unwrap();
    device.send(tx.rollback_plan.command().unwrap().actions());
    assert!(
        rollback
            .verify_rollback_readback(&device.state)
            .unwrap()
            .is_exact()
    );
    assert_eq!(rollback.state(), ApplyTransactionState::RolledBack);
    assert_eq!(&device.state, base);
    (tx, readback)
}

fn consent() -> WritePolicyV1 {
    WritePolicyV1 {
        allow_protection_change: true,
        ..WritePolicyV1::default()
    }
}

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

/// Synthetic image with the typed codes of the device image `fa3b05b4` at
/// every field the plans read: O1 and O2 limiters off, O1 low-pass and O2
/// high-pass linked at code 180, O3 muted with its low-pass off at code 74,
/// and O4's LR24 high-pass at code 74, 99.35 Hz, the Alto protection. With
/// `limiters_on`, O1 and O2 hold the limiter on, as in `b31a7e38`.
fn rack(limiters_on: bool) -> SnapshotV1 {
    let mut dump0 = blank_dump(DUMP0_RESPONSE_LEN, 0);
    let mut dump1 = blank_dump(DUMP1_RESPONSE_LEN, 1);
    let mut set = |channel: u8, parameter: u8, value: u16| {
        let location = reviewed_address(channel, parameter).unwrap().location;
        let frame = match location.part() {
            DumpPart::Part0 => &mut dump0,
            DumpPart::Part1 => &mut dump1,
        };
        location.write(frame, value).unwrap();
    };
    for output in 1..=6 {
        set(
            output_channel(output),
            MUTE_PARAMETER,
            u16::from(matches!(output, 3 | 5 | 6)),
        );
    }
    // [gain, hp type, hp freq, lp type, lp freq, limiter on, threshold].
    let on = u16::from(limiters_on);
    let rows: [(u8, [u16; 7]); 4] = [
        (1, [150, 0, 0, 0, 180, on, 102]),
        (2, [150, 0, 180, 0, 319, on, 102]),
        (3, [130, 0, 0, 0, 74, 0, 240]),
        (4, [150, 8, 74, 0, 319, 0, 240]),
    ];
    for (output, codes) in rows {
        let channel = output_channel(output);
        for (parameter, code) in [
            GAIN_PARAMETER,
            HIGH_PASS_TYPE_PARAMETER,
            HIGH_PASS_FREQUENCY_PARAMETER,
            LOW_PASS_TYPE_PARAMETER,
            LOW_PASS_FREQUENCY_PARAMETER,
            LIMITER_ENABLED_PARAMETER,
            LIMITER_THRESHOLD_PARAMETER,
        ]
        .into_iter()
        .zip(codes)
        {
            set(channel, parameter, code);
        }
    }
    SnapshotV1::from_frames(&identity(), &dump0, &dump1).unwrap()
}

/// O1 high-pass, O1 low-pass, O2 high-pass, O3 low-pass and O4 high-pass
/// frequency codes, then the O4 high-pass type code.
fn corners(snapshot: &SnapshotV1) -> [u16; 6] {
    [
        read(snapshot, (5, HIGH_PASS_FREQUENCY_PARAMETER)),
        read(snapshot, (5, LOW_PASS_FREQUENCY_PARAMETER)),
        read(snapshot, (6, HIGH_PASS_FREQUENCY_PARAMETER)),
        read(snapshot, (7, LOW_PASS_FREQUENCY_PARAMETER)),
        read(snapshot, (8, HIGH_PASS_FREQUENCY_PARAMETER)),
        read(snapshot, (8, HIGH_PASS_TYPE_PARAMETER)),
    ]
}

/// The JBL high-pass of ruling 54 on O1 and O2: the corner, then the type.
fn jbl_high_pass(base: &SnapshotV1) -> SnapshotV1 {
    for reverse_linked in [false, true] {
        let (tx, _) = replay(base, JBL_CORNER, &WritePolicyV1::default(), reverse_linked);
        let [effect] = tx.linked_effects.as_slice() else {
            panic!("the O2 corner moves the linked O1 low-pass")
        };
        assert_eq!(effect.evidence, "observed");
        assert!(effect.in_desired_snapshot);
        assert_eq!((effect.driver_output, effect.follower_output), (2, 1));
        assert_eq!((effect.before_code, effect.after_code), (180, 37));
        assert_eq!(
            (effect.dump_offsets.low, effect.dump_offsets.carrier),
            (866, Some((868, 5)))
        );
        assert!(tx.linked_writes.is_empty());
    }
    let (_, cornered) = replay(base, JBL_CORNER, &WritePolicyV1::default(), false);
    // O1 high-pass 37, O1 low-pass follows to 37 with its type still off.
    assert_eq!(corners(&cornered), [37, 37, 37, 74, 74, 8]);
    assert_eq!(read(&cornered, (5, LOW_PASS_TYPE_PARAMETER)), 0);

    let (tx, on) = replay(&cornered, JBL_ON, &WritePolicyV1::default(), false);
    assert!(tx.linked_effects.is_empty() && tx.linked_writes.is_empty());
    assert_eq!(
        tx.writes
            .iter()
            .map(|write| (write.output, write.field.as_str(), write.after_code))
            .collect::<Vec<_>>(),
        [(1, "high_pass.type", 8), (2, "high_pass.type", 8)]
    );
    assert_eq!(read(&on, (5, HIGH_PASS_TYPE_PARAMETER)), 8);
    assert_eq!(read(&on, (6, HIGH_PASS_TYPE_PARAMETER)), 8);
    on
}

/// The sub low-pass on O3 at an operator corner, with the Alto high-pass.
fn sub_corner(base: &SnapshotV1) {
    // Alone, the O3 corner would drag the active O4 high-pass with it.
    let alone = WritePlanV1::from_json(SUB_ALONE).unwrap();
    let error = plan_writes(base, &alone, &WritePolicyV1::default()).unwrap_err();
    let TypedWriteError::LinkedProtectionChange {
        driver_output: 3,
        follower_output: 4,
        evidence: "inferred_unverified",
        follower_hz,
        ..
    } = error
    else {
        panic!("{error}")
    };
    assert!((follower_hz - 99.35).abs() < 0.01, "{follower_hz}");
    let text = error.to_string();
    assert!(
        text.contains("Add an O4 change at the same corner"),
        "{text}"
    );

    // Named on both sides at 80 Hz: lowering the Alto high-pass still needs
    // the protection consent of a direct write.
    let with_o4 = WritePlanV1::from_json(SUB_80).unwrap();
    assert!(matches!(
        plan_writes(base, &with_o4, &WritePolicyV1::default()),
        Err(TypedWriteError::ProtectionChange { output: 4, .. })
    ));
    for reverse_linked in [false, true] {
        let (tx, applied) = replay(base, SUB_80, &consent(), reverse_linked);
        assert_eq!(
            tx.writes
                .iter()
                .map(|write| (write.output, write.field.as_str(), write.after_code))
                .collect::<Vec<_>>(),
            [
                (3, "low_pass.frequency", 64),
                (3, "low_pass.type", 8),
                (4, "high_pass.frequency", 64)
            ]
        );
        assert!(tx.linked_effects.is_empty());
        let [pair] = tx.linked_writes.as_slice() else {
            panic!("one linked pair written on both sides")
        };
        assert_eq!((pair.high_pass_output, pair.low_pass_output), (4, 3));
        assert_eq!((pair.before_code, pair.after_code), (74, 64));
        assert_eq!(pair.evidence, "observed");
        assert_eq!(corners(&applied)[3..], [64, 64, 8]);
    }

    // Above the Alto corner, both sides move up with no consent: the Alto
    // high-pass only tightens.
    for reverse_linked in [false, true] {
        let (tx, applied) = replay(base, SUB_120, &WritePolicyV1::default(), reverse_linked);
        assert_eq!(tx.linked_writes.len(), 1);
        assert_eq!(corners(&applied)[3..], [83, 83, 8]);
    }
}

#[test]
fn jbl_high_pass_on_o1_and_o2_replays_on_a_linked_device() {
    let on = jbl_high_pass(&rack(false));
    // The Alto and the sub are untouched.
    assert_eq!(corners(&on)[3..], [74, 74, 8]);
}

#[test]
fn sub_low_pass_moves_the_alto_high_pass_only_when_the_plan_names_it() {
    sub_corner(&rack(false));
}

#[test]
fn both_sides_at_two_corners_are_refused() {
    let mut plan = WritePlanV1::from_json(SUB_80).unwrap();
    let other = WritePlanV1::from_json(SUB_120).unwrap();
    plan.changes[1] = other.changes[1].clone();
    let error = plan_writes(&rack(false), &plan, &consent()).unwrap_err();
    assert!(
        error.to_string().contains("write both at one corner"),
        "{error}"
    );
}

#[test]
fn the_november_16_apply_4_now_predicts_the_o1_low_pass_move() {
    // From b31a7e38 the device moved O1 low-pass 180 -> 37. The planner now
    // expects it, so the readback the device gave verifies.
    let base = rack(true);
    let (_, readback) = replay(&base, JBL_CORNER, &WritePolicyV1::default(), false);
    assert_eq!(corners(&readback)[..3], [37, 37, 37]);
}

fn private(var: &str, name: &str) -> SnapshotV1 {
    let dir = std::env::var(var).unwrap_or_else(|_| panic!("set {var}"));
    let path = PathBuf::from(dir).join(name);
    SnapshotV1::from_json(&std::fs::read(&path).unwrap())
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
#[ignore = "needs DCX_REPLAY_VISIT1_DIR, the private visit 1 snapshots"]
fn private_visit_1_apply_4_readback_is_the_desired_state() {
    let base = private("DCX_REPLAY_VISIT1_DIR", "base-J.json");
    let device = private("DCX_REPLAY_VISIT1_DIR", "after-4-read-3.json");
    assert!(base.digest().starts_with("sha256/b31a7e38"));
    assert!(device.digest().starts_with("sha256/e2088871"));
    let (tx, fake) = replay(&base, JBL_CORNER, &WritePolicyV1::default(), false);
    assert_eq!(fake, device, "the fake device matches the real readback");
    assert_eq!(tx.desired_snapshot_digest, device.digest());
    let verification = verify_written_values(&tx.apply_plan, &device).unwrap();
    assert_eq!(verification.status, "consistent");
    assert_eq!(verification.linked_changes.len(), 1);
    assert!(verification.linked_changes[0].contained);
    // The rollback the device ran restored b31a7e38 (after-rb4.json).
    let restored = private("DCX_REPLAY_VISIT1_DIR", "after-rb4.json");
    assert_eq!(restored, base);
}

#[test]
#[ignore = "needs DCX_REPLAY_LIMITER_DIR, the private November 18 snapshots"]
fn private_fa3b05b4_plans_replay_on_the_fake_device() {
    let base = private("DCX_REPLAY_LIMITER_DIR", "L2.json");
    assert!(base.digest().starts_with("sha256/fa3b05b4"));
    let on = jbl_high_pass(&base);
    assert_eq!(corners(&on)[3..], [74, 74, 8]);
    sub_corner(&base);
}
