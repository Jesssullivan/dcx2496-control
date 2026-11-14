//! The committed JBL protection plan in `docs/plans/jbl-protection-20a5b485`.
//!
//! The plan's baseline is the approved device snapshot `20a5b485`. That raw
//! capture stays outside Git, so this test rebuilds a synthetic snapshot whose
//! typed O1-O6 codes equal the Legalab decode of `20a5b485`
//! (`docs/agent-notes/2026-11-13-jbl-level/show-20a5b485.json`). Every other
//! byte is synthetic, so the snapshot digests here are not the device digests.
//! The frames, the codes each write moves and the final typed state depend
//! only on those codes, so they must equal `expected.json` exactly. The
//! device digests are checked by `scripts/jbl-protection-plan-dry-run.sh`
//! against the private snapshot.

use dcx_core::{
    SnapshotV1,
    layout::{
        BandField, EQ_COUNT_PARAMETER, EQ_ENABLED_PARAMETER, GAIN_PARAMETER,
        HIGH_PASS_FREQUENCY_PARAMETER, HIGH_PASS_TYPE_PARAMETER, LIMITER_ENABLED_PARAMETER,
        LIMITER_RELEASE_PARAMETER, LIMITER_THRESHOLD_PARAMETER, LOW_PASS_FREQUENCY_PARAMETER,
        MUTE_PARAMETER, PEQ_BANDS, SOURCE_PARAMETER, band_parameter, output_channel,
        reviewed_address,
    },
    protocol::{DUMP0_RESPONSE_LEN, DUMP1_RESPONSE_LEN, DumpPart},
    typed_write::{WritePlanV1, WritePolicyV1, decode_typed_state, plan_writes},
};
use serde_json::Value;

const PLANS: [(&str, &[u8]); 5] = [
    (
        "apply-1-trim.write-plan.json",
        include_bytes!("../../../docs/plans/jbl-protection-20a5b485/apply-1-trim.write-plan.json"),
    ),
    (
        "apply-2-limiter-threshold.write-plan.json",
        include_bytes!(
            "../../../docs/plans/jbl-protection-20a5b485/apply-2-limiter-threshold.write-plan.json"
        ),
    ),
    (
        "apply-3-limiter-on.write-plan.json",
        include_bytes!(
            "../../../docs/plans/jbl-protection-20a5b485/apply-3-limiter-on.write-plan.json"
        ),
    ),
    (
        "apply-4-high-pass-corner.write-plan.json",
        include_bytes!(
            "../../../docs/plans/jbl-protection-20a5b485/apply-4-high-pass-corner.write-plan.json"
        ),
    ),
    (
        "apply-5-high-pass-on.write-plan.json",
        include_bytes!(
            "../../../docs/plans/jbl-protection-20a5b485/apply-5-high-pass-on.write-plan.json"
        ),
    ),
];

const EXPECTED: &[u8] = include_bytes!("../../../docs/plans/jbl-protection-20a5b485/expected.json");

/// Largest action count in one apply, so its exact rollback also holds at
/// most 3 actions.
const MAX_ACTIONS_PER_APPLY: usize = 3;

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
}

/// Typed codes of `20a5b485`, from the Legalab decode. PEQ bands keep the
/// flat default, which the plan never touches.
fn synthetic_20a5b485() -> SnapshotV1 {
    let mut b = Builder {
        dump0: blank_dump(DUMP0_RESPONSE_LEN, 0),
        dump1: blank_dump(DUMP1_RESPONSE_LEN, 1),
    };
    for output in 1..=6 {
        let channel = output_channel(output);
        b.set(
            channel,
            MUTE_PARAMETER,
            u16::from(matches!(output, 3 | 5 | 6)),
        );
        for band in 1..=PEQ_BANDS {
            for (field, code) in BandField::all().into_iter().zip([180, 20, 150, 1, 1]) {
                b.set(channel, band_parameter(band, field), code);
            }
        }
    }
    for channel in 1..=3 {
        b.set(channel, GAIN_PARAMETER, 150);
    }
    // O1 (channel 5) through O4 (channel 8):
    // [gain, hp type, hp freq, lp freq, limiter on, threshold, release].
    let rows: [(u8, [u16; 7]); 4] = [
        (5, [150, 0, 0, 180, 0, 235, 142]),
        (6, [150, 0, 180, 319, 0, 240, 141]),
        (7, [130, 0, 0, 74, 0, 240, 141]),
        (8, [150, 8, 74, 319, 0, 240, 141]),
    ];
    for (channel, codes) in rows {
        b.set(channel, GAIN_PARAMETER, codes[0])
            .set(channel, HIGH_PASS_TYPE_PARAMETER, codes[1])
            .set(channel, HIGH_PASS_FREQUENCY_PARAMETER, codes[2])
            .set(channel, LOW_PASS_FREQUENCY_PARAMETER, codes[3])
            .set(channel, LIMITER_ENABLED_PARAMETER, codes[4])
            .set(channel, LIMITER_THRESHOLD_PARAMETER, codes[5])
            .set(channel, LIMITER_RELEASE_PARAMETER, codes[6]);
    }
    b.set(5, EQ_ENABLED_PARAMETER, 1)
        .set(7, SOURCE_PARAMETER, 3)
        .set(8, SOURCE_PARAMETER, 2)
        .set(8, EQ_ENABLED_PARAMETER, 1)
        .set(8, EQ_COUNT_PARAMETER, 8);
    SnapshotV1::from_frames(&identity(), &b.dump0, &b.dump1).unwrap()
}

fn frame_actions(hex: &str) -> usize {
    // 9 framing bytes plus 4 bytes per action.
    (hex.len() / 2 - 9) / 4
}

#[test]
fn committed_plan_chains_to_the_declared_jbl_protection() {
    let expected: Value = serde_json::from_slice(EXPECTED).unwrap();
    let applies = expected["applies"].as_array().unwrap();
    assert_eq!(applies.len(), PLANS.len());
    assert_eq!(
        expected["max_actions_per_apply"].as_u64(),
        Some(MAX_ACTIONS_PER_APPLY as u64)
    );

    let mut snapshot = synthetic_20a5b485();
    for ((name, bytes), want) in PLANS.iter().zip(applies) {
        assert_eq!(want["plan"].as_str(), Some(*name));
        let plan = WritePlanV1::from_json(bytes).unwrap();
        let tx = plan_writes(&snapshot, &plan, &WritePolicyV1::default())
            .unwrap_or_else(|error| panic!("{name}: {error}"));

        let writes: Vec<Value> = tx
            .writes
            .iter()
            .map(|w| {
                assert!(w.output <= 2, "{name} touches O{}", w.output);
                serde_json::json!({
                    "output": w.output, "field": w.field,
                    "before_code": w.before_code, "after_code": w.after_code,
                    "after": w.after,
                })
            })
            .collect();
        assert_eq!(Value::Array(writes), want["writes"], "{name} writes");
        assert!(
            tx.linked_effects.is_empty(),
            "{name} predicts a linked move"
        );
        assert!(
            tx.unchanged_changes.is_empty(),
            "{name} sends a no-op change"
        );

        let apply_hex = tx.apply_frame_hex.clone().unwrap();
        let rollback_hex = tx.rollback_frame_hex.clone().unwrap();
        assert_eq!(Some(apply_hex.as_str()), want["apply_frame_hex"].as_str());
        assert_eq!(
            Some(rollback_hex.as_str()),
            want["rollback_frame_hex"].as_str()
        );
        assert!(frame_actions(&apply_hex) <= MAX_ACTIONS_PER_APPLY);
        assert!(frame_actions(&rollback_hex) <= MAX_ACTIONS_PER_APPLY);
        assert_eq!(tx.wire_budget.apply_remote_mode_writes, 5);
        assert_eq!(tx.wire_budget.rollback_remote_mode_writes, 3);
        assert_eq!(tx.wire_budget.apply_direct_frames, 1);

        snapshot = tx.apply_plan.desired().clone();
    }

    let state = serde_json::to_value(decode_typed_state(&snapshot).unwrap()).unwrap();
    let outputs = state["outputs"].as_array().unwrap();
    for output in outputs {
        let number = output["output"].as_u64().unwrap();
        let fields = &output["fields"];
        match number {
            1 | 2 => {
                let want = &expected["final"]["outputs"][number.to_string()];
                for (field, key) in [
                    ("gain", "gain_code"),
                    ("high_pass.type", "high_pass_type_code"),
                    ("high_pass.frequency", "high_pass_frequency_code"),
                    ("limiter.enabled", "limiter_enabled_code"),
                    ("limiter.threshold", "limiter_threshold_code"),
                    ("mute", "mute_code"),
                ] {
                    assert_eq!(fields[field]["code"], want[key], "O{number} {field}");
                }
            }
            3 | 5 | 6 => assert_eq!(fields["mute"]["muted"], true, "O{number} stays muted"),
            4 => {
                assert_eq!(fields["mute"]["muted"], false, "O4 is left as it was");
                assert_eq!(fields["limiter.enabled"]["enabled"], false);
            }
            _ => unreachable!(),
        }
    }
}
