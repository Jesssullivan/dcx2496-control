//! Golden encoder vectors, refusal, ordering, rollback, and dump-consistency
//! tests for typed writes. Every frame here is synthetic; nothing is device
//! evidence. The one external vector is the public `UltradrivePi`
//! `protocol.md` example "set in A gain of device #1 to +6dB".

use super::*;
use crate::{
    ApplyPlanError,
    layout::{DumpLocation, LIMITER_ENABLED_PARAMETER},
    protocol::{DUMP0_RESPONSE_LEN, DUMP1_RESPONSE_LEN, DumpPart},
};

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

/// Synthetic state shaped like a plausible bench baseline: O1-O4 unmuted at
/// 0 dB with PEQ off, crossovers off, limiters off at 0 dB threshold, no
/// delay; O5/O6 muted; inputs at 0 dB. PEQ bands sit at code 160, Q code 20,
/// flat bells.
struct Builder {
    dump0: Vec<u8>,
    dump1: Vec<u8>,
}

impl Builder {
    fn new() -> Self {
        let mut builder = Self {
            dump0: blank_dump(0),
            dump1: blank_dump(1),
        };
        for output in 1..=6 {
            let channel = output_channel(output);
            builder.set(channel, MUTE_PARAMETER, u16::from(output >= 5));
            for band in 1..=9 {
                builder.set(channel, band_parameter(band, BandField::Frequency), 160);
                builder.set(channel, band_parameter(band, BandField::Q), 20);
                builder.set(channel, band_parameter(band, BandField::Gain), 150);
                builder.set(channel, band_parameter(band, BandField::Kind), 1);
            }
            if output <= 4 {
                builder.set(channel, GAIN_PARAMETER, 150);
                builder.set(channel, LIMITER_THRESHOLD_PARAMETER, 240);
                builder.set(channel, HIGH_PASS_FREQUENCY_PARAMETER, 0);
                builder.set(channel, LOW_PASS_FREQUENCY_PARAMETER, 320);
            }
        }
        for channel in 1..=3 {
            builder.set(channel, GAIN_PARAMETER, 150);
        }
        builder
    }

    fn set(&mut self, channel: u8, parameter: u8, value: u16) -> &mut Self {
        let location = reviewed_address(channel, parameter).unwrap().location;
        let frame = match location.part() {
            DumpPart::Part0 => &mut self.dump0,
            DumpPart::Part1 => &mut self.dump1,
        };
        location.write(frame, value).unwrap();
        self
    }

    fn snapshot(&self) -> SnapshotV1 {
        SnapshotV1::from_frames(&identity(), &self.dump0, &self.dump1).unwrap()
    }
}

fn baseline() -> SnapshotV1 {
    Builder::new().snapshot()
}

fn plan(changes: Vec<TypedChange>) -> WritePlanV1 {
    WritePlanV1 {
        schema: WRITE_PLAN_SCHEMA.to_owned(),
        device_id: 0,
        allow_routing: false,
        allow_unmute: false,
        allow_protection_change: false,
        changes,
    }
}

fn run(changes: Vec<TypedChange>) -> Result<WriteTransactionV1, TypedWriteError> {
    plan_writes(&baseline(), &plan(changes), &WritePolicyV1::default())
}

fn frame(actions: &[[u8; 4]]) -> String {
    let mut bytes = vec![0xf0, 0x00, 0x20, 0x32, 0x00, 0x0e, 0x20];
    bytes.push(u8::try_from(actions.len()).unwrap());
    for action in actions {
        bytes.extend_from_slice(action);
    }
    bytes.push(0xf7);
    hex_upper(&bytes)
}

fn wires(transaction: &WriteTransactionV1) -> Vec<[u8; 4]> {
    transaction.writes.iter().map(|write| write.wire).collect()
}

#[test]
fn protocol_notes_input_a_gain_example_is_reproduced_exactly() {
    // UltradrivePi protocol.md: "Set in A gain of device #1 to +6dB"
    // F0 00 20 32 00 0E 20 01 01 02 01 52 F7. The policy never admits it;
    // the encoder and frame builder must still reproduce the vector.
    let code = encode_gain("input A", 6.0, 15.0).unwrap();
    assert_eq!(code, 210);
    let action = DirectParameterAction::new(InputName::A.channel(), GAIN_PARAMETER, code).unwrap();
    assert_eq!(
        wire_bytes(DeviceId::new(0).unwrap(), action).unwrap(),
        [0x01, 0x02, 0x01, 0x52]
    );
    let command = DirectParameterCommand::new(DeviceId::new(0).unwrap(), vec![action]).unwrap();
    assert_eq!(
        hex_upper(&command.encode().unwrap()),
        "F0002032000E200101020152F7"
    );
    // The same change as a plan is refused by the 0 dB ceiling.
    assert!(matches!(
        run(vec![TypedChange::InputGain {
            input: InputName::A,
            gain_db: 6.0,
        }]),
        Err(TypedWriteError::GainCeiling { .. })
    ));
}

#[test]
fn peq_band_golden_vector_and_exact_rollback() {
    let transaction = run(vec![TypedChange::PeqBand {
        output: 1,
        band: 1,
        filter: PeqKind::Bell,
        frequency_hz: 63.0,
        gain_db: -6.0,
        q: 4.0,
        shelf_slope_db: None,
    }])
    .unwrap();
    // Frequency 53 (63.1 Hz), Q 32 (3.98), gain 90 (-6 dB). The kind is
    // already a bell and is not written. The deepening cut is written first.
    let expected = [
        [0x05, 0x15, 0x00, 0x5a],
        [0x05, 0x13, 0x00, 0x35],
        [0x05, 0x14, 0x00, 0x20],
    ];
    assert_eq!(wires(&transaction), expected);
    assert_eq!(
        transaction.apply_frame_hex.as_deref(),
        Some(frame(&expected).as_str())
    );
    // Rollback is the exact baseline values of exactly those addresses,
    // reversed: Q 20, frequency 160, then gain 150 last.
    let rollback = [
        [0x05, 0x14, 0x00, 0x14],
        [0x05, 0x13, 0x01, 0x20],
        [0x05, 0x15, 0x01, 0x16],
    ];
    assert_eq!(
        transaction.rollback_frame_hex.as_deref(),
        Some(frame(&rollback).as_str())
    );
    assert_eq!(transaction.wire_budget.apply_actions, 3);
    assert_eq!(transaction.wire_budget.apply_frame_bytes, 9 + 4 * 3);
    assert_eq!(transaction.wire_budget.apply_direct_frames, 1);
    assert_eq!(transaction.wire_budget.apply_remote_mode_writes, 5);
    assert_eq!(transaction.wire_budget.rollback_remote_mode_writes, 3);
    assert!(transaction.dry_run);
    // O1 band 1 lives in Dump0 at the transcribed offsets.
    assert_eq!(transaction.writes[0].dump_offsets.part, 0);
    assert_eq!(transaction.writes[1].dump_offsets.low, 752);
}

#[test]
fn shallower_cut_writes_gain_first_and_shelves_carry_slope() {
    let mut builder = Builder::new();
    builder.set(5, band_parameter(2, BandField::Gain), 60);
    let baseline = builder.snapshot();
    let transaction = plan_writes(
        &baseline,
        &plan(vec![TypedChange::PeqBand {
            output: 1,
            band: 2,
            filter: PeqKind::LowShelf,
            frequency_hz: 100.0,
            gain_db: -3.0,
            q: 1.0,
            shelf_slope_db: Some(12),
        }]),
        &WritePolicyV1::default(),
    )
    .unwrap();
    // Gain 60 -> 120 gets shallower: written last, after frequency 74,
    // kind 0 and slope 1, so the moving band is never louder than its end.
    assert_eq!(
        wires(&transaction),
        [
            [0x05, 0x18, 0x00, 0x4a],
            [0x05, 0x1b, 0x00, 0x00],
            [0x05, 0x1c, 0x00, 0x01],
            [0x05, 0x1a, 0x00, 0x78],
        ]
    );
}

#[test]
fn crossover_golden_vectors_for_the_accepted_d1_and_d4_settings() {
    // D1: O1 high-pass LR24 at 80 Hz (code 64, exact). D4: O4 high-pass
    // LR24 at 100 Hz (code 74, 99.4 Hz). Frequency is written before type.
    let transaction = run(vec![
        TypedChange::Crossover {
            output: 1,
            side: CrossoverSide::HighPass,
            filter: CrossoverFilter::LinkwitzRiley24,
            frequency_hz: Some(80.0),
        },
        TypedChange::Crossover {
            output: 4,
            side: CrossoverSide::HighPass,
            filter: CrossoverFilter::LinkwitzRiley24,
            frequency_hz: Some(100.0),
        },
    ])
    .unwrap();
    assert_eq!(
        wires(&transaction),
        [
            [0x05, 0x43, 0x00, 0x40],
            [0x05, 0x42, 0x00, 0x08],
            [0x08, 0x43, 0x00, 0x4a],
            [0x08, 0x42, 0x00, 0x08],
        ]
    );
    assert_eq!(
        transaction.writes[1].after,
        serde_json::json!({"filter": "lr24"})
    );
    // O4 crossover lives in Dump1.
    assert_eq!(transaction.writes[3].dump_offsets.part, 1);
    assert_eq!(transaction.writes[3].dump_offsets.low, 367);
    // Every crossover type code round-trips through its name.
    for code in 0..=10 {
        assert_eq!(CrossoverFilter::from_code(code).unwrap().code(), code);
    }
    assert_eq!(CrossoverFilter::from_code(11), None);
    // Switching a filter off writes only its type.
    let mut builder = Builder::new();
    builder.set(7, LOW_PASS_TYPE_PARAMETER, 8);
    let off = plan_writes(
        &builder.snapshot(),
        &plan(vec![TypedChange::Crossover {
            output: 3,
            side: CrossoverSide::LowPass,
            filter: CrossoverFilter::Off,
            frequency_hz: None,
        }]),
        &WritePolicyV1::default(),
    )
    .unwrap();
    assert_eq!(wires(&off), [[0x07, 0x44, 0x00, 0x00]]);
    // Switching off while moving the corner writes the type first.
    let moved = plan_writes(
        &builder.snapshot(),
        &plan(vec![TypedChange::Crossover {
            output: 3,
            side: CrossoverSide::LowPass,
            filter: CrossoverFilter::Off,
            frequency_hz: Some(80.0),
        }]),
        &WritePolicyV1::default(),
    )
    .unwrap();
    assert_eq!(
        wires(&moved),
        [[0x07, 0x44, 0x00, 0x00], [0x07, 0x45, 0x00, 0x40]]
    );
}

#[test]
fn level_mute_polarity_delay_and_limiter_golden_vectors() {
    let transaction = run(vec![
        TypedChange::OutputGain {
            output: 3,
            gain_db: -6.0,
        },
        TypedChange::OutputMute {
            output: 3,
            muted: true,
        },
        TypedChange::Polarity {
            output: 3,
            inverted: true,
        },
        TypedChange::Delay {
            output: 1,
            enabled: Some(true),
            long_mm: Some(1000),
            short_mm: Some(1716),
        },
        TypedChange::Limiter {
            output: 1,
            enabled: Some(true),
            threshold_db: Some(-12.0),
            release_ms: Some(100.0),
        },
        TypedChange::InputGain {
            input: InputName::C,
            gain_db: -3.0,
        },
    ])
    .unwrap();
    // Phase order: mute on; level/threshold cuts; limiter on and release;
    // then delay and polarity.
    assert_eq!(
        wires(&transaction),
        [
            [0x07, 0x03, 0x00, 0x01],
            [0x07, 0x02, 0x00, 0x5a],
            [0x05, 0x47, 0x00, 0x78],
            [0x03, 0x02, 0x00, 0x78],
            [0x05, 0x48, 0x00, 0x4c],
            [0x05, 0x46, 0x00, 0x01],
            [0x07, 0x49, 0x00, 0x01],
            [0x05, 0x05, 0x00, 0x14],
            [0x05, 0x4b, 0x06, 0x5a],
            [0x05, 0x04, 0x00, 0x01],
        ]
    );
    let input = transaction
        .writes
        .iter()
        .find(|write| write.channel == 3)
        .unwrap();
    assert_eq!(input.output, 0);
    assert_eq!(input.field, "input_c.gain");
    assert_eq!(input.dump_offsets.low, 430);
    assert!((limiter_threshold_db(120) + 12.0).abs() < 1e-9);
    assert!((limiter_release_ms(76) - 100.0).abs() < 2.5);
    assert!((limiter_release_ms(0) - 20.0).abs() < 1e-9);
    assert!((limiter_release_ms(251) - 4000.0).abs() < 1e-6);
}

#[test]
fn every_written_value_lands_at_its_transcribed_dump_offsets() {
    // Offline consistency: for every reviewed address on O1-O4 and inputs
    // A-C, a projected value decodes back from the frame bytes the plan
    // names, and `verify_written_values` passes the exact desired snapshot.
    let base = baseline();
    let mut addresses = Vec::new();
    for channel in (1..=3).chain(5..=8) {
        for parameter in 0..=0x7f_u8 {
            if let Some(address) = reviewed_address(channel, parameter)
                && (address.field.is_typed_write() || address.field == OutputField::Mute)
            {
                addresses.push((channel, parameter, address));
            }
        }
    }
    assert_eq!(addresses.len(), 3 + 4 * 13);
    for (channel, parameter, address) in addresses {
        let current = base.read_reviewed(address);
        // Level and threshold fields are cut; every other field moves one
        // code inside its domain.
        let value = if address.field.is_level() || address.field == OutputField::LimiterThreshold {
            current - 7
        } else if current < address.field.device_max() {
            current + 1
        } else {
            current - 1
        };
        let action = DirectParameterAction::new(channel, parameter, value).unwrap();
        let transaction = ApplyTransactionV1::stage_projected(base.clone(), vec![action]).unwrap();
        let desired = transaction.apply_plan().desired();
        let offsets = DumpOffsetsV1::of(address);
        let frame = desired.frame(if offsets.part == 0 {
            crate::SnapshotSection::Dump0
        } else {
            crate::SnapshotSection::Dump1
        });
        let mut decoded = u16::from(frame[offsets.low] & 0x7f);
        if let Some((carrier, bit)) = offsets.carrier {
            decoded |= u16::from((frame[carrier] >> bit) & 1) << 7;
        }
        if let Some(high) = offsets.high {
            decoded |= u16::from(frame[high] & 0x7f) << 8;
        }
        assert_eq!(decoded, value, "{channel}/{parameter:#04x}");
        let verification = verify_written_values(transaction.apply_plan(), desired).unwrap();
        assert_eq!(verification.status, "consistent");
        assert!(verification.values.iter().all(|value| value.matches));
        // The untouched baseline is not consistent with the plan.
        let stale = verify_written_values(transaction.apply_plan(), &base).unwrap();
        assert_eq!(stale.status, "inconsistent");
        assert!(!stale.values[0].matches);
    }
}

#[test]
fn readback_with_a_foreign_byte_change_is_inconsistent() {
    let transaction = run(vec![TypedChange::OutputGain {
        output: 1,
        gain_db: -1.0,
    }])
    .unwrap();
    let mut builder = Builder::new();
    builder.set(5, GAIN_PARAMETER, 140);
    builder.set(6, GAIN_PARAMETER, 149);
    let mut readback = builder.snapshot();
    // Keep the readback's trailer arbitrary; the foreign O2 gain change is
    // the finding.
    let verification = verify_written_values(&transaction.apply_plan, &readback).unwrap();
    assert_eq!(verification.status, "inconsistent");
    assert!(verification.values[0].matches);
    assert!(verification.uncontained_changes > 0);
    readback = transaction.apply_plan.desired().clone();
    assert_eq!(
        verify_written_values(&transaction.apply_plan, &readback)
            .unwrap()
            .status,
        "consistent"
    );
}

#[test]
fn reserved_outputs_are_never_addressed_and_must_read_muted() {
    for output in [5, 6] {
        assert!(matches!(
            run(vec![TypedChange::OutputGain {
                output,
                gain_db: -3.0
            }]),
            Err(TypedWriteError::ReservedOutput { output: o, .. }) if o == output
        ));
        assert!(matches!(
            run(vec![TypedChange::OutputMute {
                output,
                muted: true
            }]),
            Err(TypedWriteError::ReservedOutput { .. })
        ));
    }
    assert!(matches!(
        run(vec![TypedChange::OutputMute {
            output: 7,
            muted: true
        }]),
        Err(TypedWriteError::InvalidOutput { .. })
    ));
    // A baseline with O6 unmuted refuses every plan, even a harmless cut.
    let mut builder = Builder::new();
    builder.set(10, MUTE_PARAMETER, 0);
    let error = plan_writes(
        &builder.snapshot(),
        &plan(vec![TypedChange::OutputGain {
            output: 1,
            gain_db: -3.0,
        }]),
        &WritePolicyV1::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        TypedWriteError::ReservedOutputUnmuted {
            output: 6,
            value: 0
        }
    ));
}

#[test]
fn rollback_never_addresses_o5_or_o6_and_restores_only_touched_addresses() {
    let transaction = run(vec![
        TypedChange::OutputMute {
            output: 3,
            muted: true,
        },
        TypedChange::Crossover {
            output: 2,
            side: CrossoverSide::HighPass,
            filter: CrossoverFilter::LinkwitzRiley24,
            frequency_hz: Some(80.0),
        },
    ])
    .unwrap();
    let apply: Vec<_> = transaction
        .apply_plan
        .command()
        .unwrap()
        .actions()
        .iter()
        .map(|action| (action.channel(), action.parameter()))
        .collect();
    let rollback: Vec<_> = transaction
        .rollback_plan
        .command()
        .unwrap()
        .actions()
        .iter()
        .map(|action| (action.channel(), action.parameter(), action.value()))
        .collect();
    assert_eq!(apply, [(7, 0x03), (6, 0x43), (6, 0x42)]);
    // Exact reverse, with baseline values: O2 type off, O2 frequency 0, O3
    // unmuted (its baseline), never O5/O6.
    assert_eq!(rollback, [(6, 0x42, 0), (6, 0x43, 0), (7, 0x03, 0)]);
    check_rollback(&transaction.rollback_plan).unwrap();
    assert_eq!(
        transaction
            .apply_plan
            .desired()
            .project_direct_actions(transaction.rollback_plan.command().unwrap().actions())
            .unwrap(),
        baseline()
    );
}

#[test]
fn eq_boosts_are_refused_and_stored_boosts_stay_inactive() {
    assert!(matches!(
        run(vec![TypedChange::PeqBand {
            output: 2,
            band: 1,
            filter: PeqKind::Bell,
            frequency_hz: 1000.0,
            gain_db: 1.0,
            q: 1.0,
            shelf_slope_db: None,
        }]),
        Err(TypedWriteError::EqBoost { .. })
    ));
    // Raising the band count over a stored +3 dB band is a core refusal.
    let mut builder = Builder::new();
    builder.set(6, band_parameter(2, BandField::Gain), 180);
    builder.set(6, EQ_ENABLED_PARAMETER, 1);
    builder.set(6, EQ_COUNT_PARAMETER, 1);
    let error = plan_writes(
        &builder.snapshot(),
        &plan(vec![TypedChange::PeqBandCount {
            output: 2,
            count: 2,
        }]),
        &WritePolicyV1::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        TypedWriteError::Transaction(ApplyTransactionError::Plan(
            ApplyPlanError::StoredBoostActivation {
                output: 2,
                band: 2,
                ..
            }
        ))
    ));
}

#[test]
fn peq_activation_writes_fields_then_count_then_enable() {
    let transaction = run(vec![
        TypedChange::PeqEnabled {
            output: 1,
            enabled: true,
        },
        TypedChange::PeqBandCount {
            output: 1,
            count: 1,
        },
        TypedChange::PeqBand {
            output: 1,
            band: 1,
            filter: PeqKind::Bell,
            frequency_hz: 63.0,
            gain_db: -4.0,
            q: 4.0,
            shelf_slope_db: None,
        },
    ])
    .unwrap();
    let order: Vec<_> = transaction
        .writes
        .iter()
        .map(|write| write.parameter)
        .collect();
    assert_eq!(
        order,
        [0x15, 0x13, 0x14, EQ_COUNT_PARAMETER, EQ_ENABLED_PARAMETER]
    );
    // Rollback deactivates first: enable off, count 0, then the fields.
    let rollback: Vec<_> = transaction
        .rollback_plan
        .command()
        .unwrap()
        .actions()
        .iter()
        .map(|action| action.parameter())
        .collect();
    assert_eq!(
        rollback,
        [EQ_ENABLED_PARAMETER, EQ_COUNT_PARAMETER, 0x14, 0x13, 0x15]
    );
}

#[test]
fn unmute_needs_both_consents_and_is_written_last() {
    let mut builder = Builder::new();
    builder.set(7, MUTE_PARAMETER, 1);
    let base = builder.snapshot();
    let changes = vec![
        TypedChange::OutputMute {
            output: 3,
            muted: false,
        },
        TypedChange::OutputGain {
            output: 3,
            gain_db: -10.0,
        },
    ];
    let mut document = plan(changes);
    let mut policy = WritePolicyV1::default();
    for (plan_flag, cli_flag) in [(false, false), (true, false), (false, true)] {
        document.allow_unmute = plan_flag;
        policy.allow_unmute = cli_flag;
        assert!(matches!(
            plan_writes(&base, &document, &policy),
            Err(TypedWriteError::UnmuteNotAllowed { output: 3 })
        ));
    }
    document.allow_unmute = true;
    policy.allow_unmute = true;
    let transaction = plan_writes(&base, &document, &policy).unwrap();
    assert_eq!(
        wires(&transaction),
        [[0x07, 0x02, 0x00, 0x32], [0x07, 0x03, 0x00, 0x00]]
    );
    // Rollback re-mutes first, then restores the gain.
    assert_eq!(
        transaction.rollback_frame_hex.as_deref(),
        Some(frame(&[[0x07, 0x03, 0x00, 0x01], [0x07, 0x02, 0x01, 0x16]]).as_str())
    );
}

#[test]
fn routing_needs_both_consents_is_o3_o4_only_and_never_joins_an_unmute() {
    let source = TypedChange::OutputSource {
        output: 3,
        source: OutputSource::B,
    };
    assert!(matches!(
        run(vec![source.clone()]),
        Err(TypedWriteError::RoutingNotAllowed { .. })
    ));
    let mut document = plan(vec![source.clone()]);
    document.allow_routing = true;
    let policy = WritePolicyV1 {
        allow_routing: true,
        ..WritePolicyV1::default()
    };
    let transaction = plan_writes(&baseline(), &document, &policy).unwrap();
    assert_eq!(wires(&transaction), [[0x07, 0x41, 0x00, 0x01]]);
    assert!(matches!(
        run(vec![TypedChange::OutputSource {
            output: 1,
            source: OutputSource::B,
        }]),
        Err(TypedWriteError::SourceNotReviewed { output: 1, .. })
    ));
    let mut builder = Builder::new();
    builder.set(8, MUTE_PARAMETER, 1);
    builder.set(8, SOURCE_PARAMETER, 1);
    let both = WritePlanV1 {
        allow_routing: true,
        allow_unmute: true,
        ..plan(vec![
            TypedChange::OutputMute {
                output: 4,
                muted: false,
            },
            TypedChange::OutputSource {
                output: 4,
                source: OutputSource::C,
            },
        ])
    };
    let consent = WritePolicyV1 {
        allow_routing: true,
        allow_unmute: true,
        ..WritePolicyV1::default()
    };
    assert!(matches!(
        plan_writes(&builder.snapshot(), &both, &consent),
        Err(TypedWriteError::UnmuteWithRouting)
    ));
}

#[test]
fn level_rises_are_bounded_by_step_and_ceiling() {
    let mut builder = Builder::new();
    builder.set(5, GAIN_PARAMETER, 100);
    builder.set(5, LIMITER_THRESHOLD_PARAMETER, 100);
    let base = builder.snapshot();
    let attempt = |changes, policy: WritePolicyV1| plan_writes(&base, &plan(changes), &policy);
    // -5 dB -> -1.9 dB is +3.1 dB: refused. -5 -> -2 is exactly +3 dB.
    assert!(matches!(
        attempt(
            vec![TypedChange::OutputGain {
                output: 1,
                gain_db: -1.9
            }],
            WritePolicyV1::default()
        ),
        Err(TypedWriteError::GainStep { .. })
    ));
    assert!(
        attempt(
            vec![TypedChange::OutputGain {
                output: 1,
                gain_db: -2.0
            }],
            WritePolicyV1::default()
        )
        .is_ok()
    );
    // A tighter step and ceiling bind.
    let tight = WritePolicyV1 {
        max_gain_step_db: 1.0,
        gain_ceiling_db: -4.5,
        ..WritePolicyV1::default()
    };
    assert!(matches!(
        attempt(
            vec![TypedChange::OutputGain {
                output: 1,
                gain_db: -3.5
            }],
            tight
        ),
        Err(TypedWriteError::GainCeiling { .. })
    ));
    assert!(
        attempt(
            vec![TypedChange::OutputGain {
                output: 1,
                gain_db: -4.5
            }],
            tight
        )
        .is_ok()
    );
    // Limiter threshold rises obey the step too: -14 dB -> -10 dB refused.
    assert!(matches!(
        attempt(
            vec![TypedChange::Limiter {
                output: 1,
                enabled: None,
                threshold_db: Some(-10.0),
                release_ms: None
            }],
            WritePolicyV1::default()
        ),
        Err(TypedWriteError::GainStep { .. })
    ));
    // A ceiling above 0 dB or a step above 3 dB is not a policy.
    for policy in [
        WritePolicyV1 {
            gain_ceiling_db: 0.5,
            ..WritePolicyV1::default()
        },
        WritePolicyV1 {
            max_gain_step_db: 6.0,
            ..WritePolicyV1::default()
        },
        WritePolicyV1 {
            max_actions: 33,
            ..WritePolicyV1::default()
        },
    ] {
        assert!(matches!(
            attempt(
                vec![TypedChange::OutputGain {
                    output: 1,
                    gain_db: -6.0
                }],
                policy
            ),
            Err(TypedWriteError::Policy(_))
        ));
    }
}

#[test]
fn limiter_switch_off_is_refused_by_policy_and_core() {
    let mut builder = Builder::new();
    builder.set(8, LIMITER_ENABLED_PARAMETER, 1);
    let base = builder.snapshot();
    assert!(matches!(
        plan_writes(
            &base,
            &plan(vec![TypedChange::Limiter {
                output: 4,
                enabled: Some(false),
                threshold_db: None,
                release_ms: None,
            }]),
            &WritePolicyV1::default(),
        ),
        Err(TypedWriteError::LimiterDisable { output: 4 })
    ));
    let off = DirectParameterAction::new(8, LIMITER_ENABLED_PARAMETER, 0).unwrap();
    assert!(matches!(
        ApplyTransactionV1::stage_projected(base, vec![off]),
        Err(ApplyTransactionError::Plan(
            ApplyPlanError::LimiterDisable { channel: 8 }
        ))
    ));
}

#[test]
fn core_carriers_refuse_level_rises_past_unity_or_three_db() {
    let mut builder = Builder::new();
    builder.set(5, GAIN_PARAMETER, 100);
    let base = builder.snapshot();
    let gain = |value| DirectParameterAction::new(5, GAIN_PARAMETER, value).unwrap();
    assert!(matches!(
        ApplyTransactionV1::stage_projected(base.clone(), vec![gain(131)]),
        Err(ApplyTransactionError::Plan(
            ApplyPlanError::LevelStepTooLarge { .. }
        ))
    ));
    assert!(ApplyTransactionV1::stage_projected(base.clone(), vec![gain(130)]).is_ok());
    let mut builder = Builder::new();
    builder.set(5, GAIN_PARAMETER, 140);
    assert!(matches!(
        ApplyTransactionV1::stage_projected(builder.snapshot(), vec![gain(151)]),
        Err(ApplyTransactionError::Plan(
            ApplyPlanError::LevelAboveUnity { .. }
        ))
    ));
    // A baseline already above unity may still be cut, and its rollback
    // restores it.
    let mut builder = Builder::new();
    builder.set(5, GAIN_PARAMETER, 170);
    let transaction =
        ApplyTransactionV1::stage_projected(builder.snapshot(), vec![gain(160)]).unwrap();
    assert_eq!(
        transaction.rollback_plan().command().unwrap().actions()[0].value(),
        170
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn malformed_values_and_documents_are_refused() {
    let refusals = [
        TypedChange::PeqBand {
            output: 1,
            band: 10,
            filter: PeqKind::Bell,
            frequency_hz: 100.0,
            gain_db: -1.0,
            q: 1.0,
            shelf_slope_db: None,
        },
        TypedChange::PeqBand {
            output: 1,
            band: 1,
            filter: PeqKind::Bell,
            frequency_hz: 19.0,
            gain_db: -1.0,
            q: 1.0,
            shelf_slope_db: None,
        },
        TypedChange::PeqBand {
            output: 1,
            band: 1,
            filter: PeqKind::Bell,
            frequency_hz: 100.0,
            gain_db: -1.0,
            q: 11.0,
            shelf_slope_db: None,
        },
        TypedChange::PeqBand {
            output: 1,
            band: 1,
            filter: PeqKind::HighShelf,
            frequency_hz: 100.0,
            gain_db: -1.0,
            q: 1.0,
            shelf_slope_db: Some(9),
        },
        TypedChange::PeqBand {
            output: 1,
            band: 1,
            filter: PeqKind::Bell,
            frequency_hz: f64::NAN,
            gain_db: -1.0,
            q: 1.0,
            shelf_slope_db: None,
        },
        TypedChange::PeqBandCount {
            output: 1,
            count: 10,
        },
        TypedChange::Crossover {
            output: 1,
            side: CrossoverSide::LowPass,
            filter: CrossoverFilter::Butterworth12,
            frequency_hz: None,
        },
        TypedChange::Delay {
            output: 1,
            enabled: None,
            long_mm: Some(1001),
            short_mm: None,
        },
        TypedChange::Delay {
            output: 1,
            enabled: None,
            long_mm: None,
            short_mm: Some(4002),
        },
        TypedChange::Delay {
            output: 1,
            enabled: None,
            long_mm: None,
            short_mm: None,
        },
        TypedChange::Limiter {
            output: 1,
            enabled: None,
            threshold_db: Some(0.5),
            release_ms: None,
        },
        TypedChange::Limiter {
            output: 1,
            enabled: None,
            threshold_db: None,
            release_ms: Some(5000.0),
        },
        TypedChange::OutputGain {
            output: 1,
            gain_db: -15.5,
        },
    ];
    for change in refusals {
        assert!(run(vec![change.clone()]).is_err(), "{change:?}");
    }
    // Two changes may not write one address.
    assert!(matches!(
        run(vec![
            TypedChange::OutputGain {
                output: 1,
                gain_db: -1.0
            },
            TypedChange::OutputGain {
                output: 1,
                gain_db: -2.0
            },
        ]),
        Err(TypedWriteError::DuplicateAddress { .. })
    ));
    // Unknown fields, unknown kinds, another schema, and an empty list fail.
    for document in [
        r#"{"schema":"dcx.write-plan/v1","device_id":0,"changes":[{"kind":"output_gain","output":1,"gain_db":-1,"extra":1}]}"#,
        r#"{"schema":"dcx.write-plan/v1","device_id":0,"changes":[{"kind":"phase","output":1}]}"#,
        r#"{"schema":"dcx.write-plan/v2","device_id":0,"changes":[{"kind":"output_mute","output":1,"muted":true}]}"#,
        r#"{"schema":"dcx.write-plan/v1","device_id":0,"changes":[]}"#,
        r#"{"schema":"dcx.write-plan/v1","device_id":0,"changes":[],"bytes":"F0"}"#,
    ] {
        assert!(
            WritePlanV1::from_json(document.as_bytes()).is_err(),
            "{document}"
        );
    }
    let parsed = WritePlanV1::from_json(
        br#"{"schema":"dcx.write-plan/v1","device_id":0,"changes":[{"kind":"crossover","output":1,"side":"high_pass","filter":"lr24","frequency_hz":80}]}"#,
    )
    .unwrap();
    assert_eq!(
        parsed.changes[0],
        TypedChange::Crossover {
            output: 1,
            side: CrossoverSide::HighPass,
            filter: CrossoverFilter::LinkwitzRiley24,
            frequency_hz: Some(80.0),
        }
    );
    // Another device is refused.
    let mut other = plan(vec![TypedChange::OutputMute {
        output: 1,
        muted: true,
    }]);
    other.device_id = 3;
    assert!(matches!(
        plan_writes(&baseline(), &other, &WritePolicyV1::default()),
        Err(TypedWriteError::DeviceMismatch { .. })
    ));
}

#[test]
fn action_limit_binds_and_no_op_plans_send_nothing() {
    let changes: Vec<_> = (1..=9)
        .map(|band| TypedChange::PeqBand {
            output: 2,
            band,
            filter: PeqKind::HighShelf,
            frequency_hz: 1000.0,
            gain_db: -1.0,
            q: 2.0,
            shelf_slope_db: None,
        })
        .collect();
    // Nine bands with four changed fields each is 36 actions: over 16.
    assert!(matches!(
        run(changes),
        Err(TypedWriteError::TooManyActions {
            actions: 36,
            maximum: 16
        })
    ));
    let no_op = run(vec![TypedChange::OutputGain {
        output: 1,
        gain_db: 0.0,
    }])
    .unwrap();
    assert!(no_op.writes.is_empty());
    assert_eq!(no_op.unchanged_changes, [0]);
    assert_eq!(no_op.apply_frame_hex, None);
    assert_eq!(no_op.wire_budget.apply_direct_frames, 0);
    assert_eq!(no_op.wire_budget.rollback_remote_mode_writes, 0);
}

#[test]
fn carrier_policy_revalidation_matches_planning() {
    let mut builder = Builder::new();
    builder.set(7, MUTE_PARAMETER, 1);
    let base = builder.snapshot();
    let unmute = DirectParameterAction::new(7, MUTE_PARAMETER, 0).unwrap();
    assert!(matches!(
        check_policy(&base, &[unmute], &WritePolicyV1::default()),
        Err(TypedWriteError::UnmuteNotAllowed { output: 3 })
    ));
    let consent = WritePolicyV1 {
        allow_unmute: true,
        ..WritePolicyV1::default()
    };
    check_policy(&base, &[unmute], &consent).unwrap();
    // An O5 mute write in a carrier (as the routing lane can make) is not a
    // typed-write carrier.
    let o5 = DirectParameterAction::new(9, MUTE_PARAMETER, 1).unwrap();
    assert!(matches!(
        check_policy(&base, &[o5], &consent),
        Err(TypedWriteError::ReservedOutputAction { output: 5 })
    ));
}

#[test]
fn typed_state_decodes_every_reviewed_field_and_flags_out_of_domain_codes() {
    let mut builder = Builder::new();
    builder.set(5, HIGH_PASS_TYPE_PARAMETER, 8);
    builder.set(5, HIGH_PASS_FREQUENCY_PARAMETER, 64);
    builder.set(6, POLARITY_PARAMETER, 1);
    // A crossover type code of 11 is outside the documented domain.
    let location: DumpLocation = reviewed_address(8, LOW_PASS_TYPE_PARAMETER)
        .unwrap()
        .location;
    location.write(&mut builder.dump1, 11).unwrap();
    let state = decode_typed_state(&builder.snapshot()).unwrap();
    assert_eq!(state.outputs.len(), 6);
    let o1 = &state.outputs[0].fields;
    assert_eq!(o1["high_pass.type"]["filter"], "lr24");
    assert_eq!(o1["high_pass.frequency"]["frequency_hz"], 80.0);
    assert_eq!(state.outputs[1].fields["polarity"]["inverted"], true);
    assert_eq!(
        state.outputs[3].fields["low_pass.type"]["out_of_domain"],
        true
    );
    assert_eq!(state.outputs[4].fields["mute"]["muted"], true);
    assert!(!state.outputs[4].fields.contains_key("gain"));
    assert_eq!(state.inputs["input_a.gain"]["gain_db"], 0.0);
    assert_eq!(state.inputs.len(), 3);
}

#[test]
fn first_bench_write_frames_match_the_documented_procedure() {
    // docs/agent-notes/2026-10-15-typed-writes.md step 3: O1 PEQ on, count 0,
    // band 1 an inactive low shelf at code 58, Q 20, gain 95, slope 0. These
    // are synthetic codes chosen to mirror the decoded 10-09 state; they are
    // not a capture.
    let mut builder = Builder::new();
    builder.set(5, EQ_ENABLED_PARAMETER, 1);
    builder.set(5, band_parameter(1, BandField::Frequency), 58);
    builder.set(5, band_parameter(1, BandField::Gain), 95);
    builder.set(5, band_parameter(1, BandField::Kind), 0);
    let transaction = plan_writes(
        &builder.snapshot(),
        &plan(vec![TypedChange::PeqBand {
            output: 1,
            band: 1,
            filter: PeqKind::Bell,
            frequency_hz: 63.0,
            gain_db: -3.0,
            q: 4.0,
            shelf_slope_db: None,
        }]),
        &WritePolicyV1 {
            max_actions: 4,
            ..WritePolicyV1::default()
        },
    )
    .unwrap();
    assert_eq!(
        transaction.apply_frame_hex.as_deref(),
        Some("F0002032000E200405130035051400200516000105150078F7")
    );
    assert_eq!(
        transaction.rollback_frame_hex.as_deref(),
        Some("F0002032000E20040515005F05160000051400140513003AF7")
    );
    assert_eq!(transaction.wire_budget.apply_frame_bytes, 25);
}

#[test]
fn loosening_an_active_high_pass_needs_both_consents() {
    let mut builder = Builder::new();
    builder.set(8, HIGH_PASS_TYPE_PARAMETER, 6);
    builder.set(8, HIGH_PASS_FREQUENCY_PARAMETER, 180);
    let base = builder.snapshot();
    let d4 = TypedChange::Crossover {
        output: 4,
        side: CrossoverSide::HighPass,
        filter: CrossoverFilter::LinkwitzRiley24,
        frequency_hz: Some(100.0),
    };
    let tighten = TypedChange::Crossover {
        output: 4,
        side: CrossoverSide::HighPass,
        filter: CrossoverFilter::Butterworth24,
        frequency_hz: Some(2000.0),
    };
    // Raising the corner of the active filter, same type, is admitted.
    assert!(plan_writes(&base, &plan(vec![tighten]), &WritePolicyV1::default()).is_ok());
    let mut document = plan(vec![d4]);
    let mut policy = WritePolicyV1::default();
    for (plan_flag, cli_flag) in [(false, false), (true, false), (false, true)] {
        document.allow_protection_change = plan_flag;
        policy.allow_protection_change = cli_flag;
        assert!(matches!(
            plan_writes(&base, &document, &policy),
            Err(TypedWriteError::ProtectionChange { output: 4, .. })
        ));
    }
    document.allow_protection_change = true;
    policy.allow_protection_change = true;
    let transaction = plan_writes(&base, &document, &policy).unwrap();
    assert_eq!(
        wires(&transaction),
        [[0x08, 0x43, 0x00, 0x4a], [0x08, 0x42, 0x00, 0x08]]
    );
    // Switching it off is the same consent.
    let off = plan(vec![TypedChange::Crossover {
        output: 4,
        side: CrossoverSide::HighPass,
        filter: CrossoverFilter::Off,
        frequency_hz: None,
    }]);
    assert!(matches!(
        plan_writes(&base, &off, &WritePolicyV1::default()),
        Err(TypedWriteError::ProtectionChange { .. })
    ));
}

#[test]
fn level_rises_are_summed_across_gains_and_peq_bands() {
    let mut builder = Builder::new();
    builder.set(1, GAIN_PARAMETER, 120);
    builder.set(5, GAIN_PARAMETER, 120);
    builder.set(5, EQ_ENABLED_PARAMETER, 1);
    builder.set(5, EQ_COUNT_PARAMETER, 1);
    builder.set(5, band_parameter(1, BandField::Gain), 100);
    let base = builder.snapshot();
    let attempt = |changes| plan_writes(&base, &plan(changes), &WritePolicyV1::default());
    // +2 dB input A and +2 dB O1 gain are each inside the step, not together.
    assert!(matches!(
        attempt(vec![
            TypedChange::InputGain {
                input: InputName::A,
                gain_db: -1.0,
            },
            TypedChange::OutputGain {
                output: 1,
                gain_db: -1.0,
            },
        ]),
        Err(TypedWriteError::GainStep { .. })
    ));
    // A shallower active cut counts: -5 dB to -1 dB is +4 dB. The band stays
    // at its baseline 640 Hz (code 160), Q 1 (code 20), so only its gain moves.
    let band = |gain_db| TypedChange::PeqBand {
        output: 1,
        band: 1,
        filter: PeqKind::Bell,
        frequency_hz: 640.0,
        gain_db,
        q: 1.0,
        shelf_slope_db: None,
    };
    assert!(matches!(
        attempt(vec![band(-1.0)]),
        Err(TypedWriteError::GainStep { .. })
    ));
    assert!(attempt(vec![band(-2.0)]).is_ok());
    // Switching PEQ off releases the -5 dB cut: also +5 dB.
    assert!(matches!(
        attempt(vec![TypedChange::PeqEnabled {
            output: 1,
            enabled: false,
        }]),
        Err(TypedWriteError::GainStep { .. })
    ));
    // Moving an active cut releases its whole depth at the old frequency,
    // whatever the new cut is: moving the -5 dB cut is +5 dB at 640 Hz.
    let moved = |gain_db| TypedChange::PeqBand {
        output: 1,
        band: 1,
        filter: PeqKind::Bell,
        frequency_hz: 63.0,
        gain_db,
        q: 4.0,
        shelf_slope_db: None,
    };
    assert!(matches!(
        attempt(vec![moved(-5.0)]),
        Err(TypedWriteError::GainStep { .. })
    ));
    // A cut no deeper than the step may still be moved: -2 dB releases +2 dB.
    builder.set(5, band_parameter(1, BandField::Gain), 130);
    let shallow = builder.snapshot();
    assert!(
        plan_writes(
            &shallow,
            &plan(vec![moved(-2.0)]),
            &WritePolicyV1::default()
        )
        .is_ok()
    );
    // Reshaping alone (a new Q at the same frequency) also releases it.
    let reshaped = TypedChange::PeqBand {
        output: 1,
        band: 1,
        filter: PeqKind::Bell,
        frequency_hz: 640.0,
        gain_db: -5.0,
        q: 4.0,
        shelf_slope_db: None,
    };
    assert!(matches!(
        attempt(vec![reshaped]),
        Err(TypedWriteError::GainStep { .. })
    ));
}

#[test]
fn sources_follow_routing_rank_and_cuts_from_above_the_ceiling_pass() {
    let consent = WritePolicyV1 {
        allow_routing: true,
        ..WritePolicyV1::default()
    };
    let document = WritePlanV1 {
        allow_routing: true,
        ..plan(vec![
            TypedChange::OutputSource {
                output: 3,
                source: OutputSource::B,
            },
            TypedChange::OutputSource {
                output: 4,
                source: OutputSource::B,
            },
        ])
    };
    let transaction = plan_writes(&baseline(), &document, &consent).unwrap();
    assert_eq!(
        wires(&transaction),
        [[0x08, 0x41, 0x00, 0x01], [0x07, 0x41, 0x00, 0x01]]
    );
    // A front-panel +5 dB may be cut to +2 dB; it may not be raised.
    let mut builder = Builder::new();
    builder.set(5, GAIN_PARAMETER, 200);
    let hot = builder.snapshot();
    let to = |gain_db| {
        plan_writes(
            &hot,
            &plan(vec![TypedChange::OutputGain { output: 1, gain_db }]),
            &WritePolicyV1::default(),
        )
    };
    assert!(to(2.0).is_ok());
    assert!(to(5.0).unwrap().writes.is_empty());
    assert!(matches!(to(6.0), Err(TypedWriteError::GainCeiling { .. })));
}

#[test]
fn core_refuses_typed_lane_carriers_while_a_reserved_output_is_live() {
    let mut builder = Builder::new();
    builder.set(9, MUTE_PARAMETER, 0);
    let live = builder.snapshot();
    let cut = DirectParameterAction::new(5, GAIN_PARAMETER, 140).unwrap();
    assert!(matches!(
        ApplyTransactionV1::stage_projected(live.clone(), vec![cut]),
        Err(ApplyTransactionError::Plan(
            ApplyPlanError::TypedLaneReservedUnmuted {
                output: 5,
                value: 0
            }
        ))
    ));
    // The older lanes are unchanged: a PEQ cut still plans on that baseline.
    let peq = DirectParameterAction::new(5, band_parameter(1, BandField::Gain), 140).unwrap();
    assert!(ApplyTransactionV1::stage_projected(live, vec![peq]).is_ok());
}

// Regression tests for the PR #58 adversarial review (2026-10-17). Each one
// started as a proof of concept that passed against the defect at b543b1a;
// the assertions are inverted to pin the fix.

#[test]
fn core_carriers_refuse_every_o5_o6_unmute_on_every_lane() {
    // Review finding 1 (R4): a routing-lane mute of a live O6 has an exact
    // rollback that unmutes it, so it can no longer be staged at all.
    let mut builder = Builder::new();
    builder.set(10, MUTE_PARAMETER, 0);
    let live = builder.snapshot();
    let mute = DirectParameterAction::new(10, MUTE_PARAMETER, 1).unwrap();
    assert!(
        !reviewed_address(10, MUTE_PARAMETER)
            .unwrap()
            .is_typed_lane()
    );
    assert!(matches!(
        ApplyTransactionV1::stage_projected(live, vec![mute]),
        Err(ApplyTransactionError::Plan(
            ApplyPlanError::ReservedUnmute {
                output: 6,
                value: 0
            }
        ))
    ));
    // And an apply that unmutes O6 from a muted baseline is refused before
    // any carrier exists.
    let unmute = DirectParameterAction::new(10, MUTE_PARAMETER, 0).unwrap();
    assert!(matches!(
        ApplyTransactionV1::stage_projected(baseline(), vec![unmute]),
        Err(ApplyTransactionError::Plan(
            ApplyPlanError::ReservedUnmute {
                output: 6,
                value: 0
            }
        ))
    ));
}

#[test]
fn write_apply_recheck_refuses_an_unsafe_in_frame_order() {
    // Review finding 3: [raise O1 to 0 dB, then mute] passes the core, but a
    // frame cut short after action 1 leaves O1 live at 0 dB, louder than
    // either end state. The `write apply` policy re-check refuses it.
    let mut builder = Builder::new();
    builder.set(5, GAIN_PARAMETER, 120); // O1 live at -3 dB
    let base = builder.snapshot();
    let rise = DirectParameterAction::new(5, GAIN_PARAMETER, 150).unwrap();
    let mute = DirectParameterAction::new(5, MUTE_PARAMETER, 1).unwrap();
    let transaction = ApplyTransactionV1::stage_projected(base.clone(), vec![rise, mute]).unwrap();
    let carrier = ApplyPlanV1::from_json(&transaction.apply_plan().to_json().unwrap()).unwrap();
    assert!(matches!(
        check_policy(
            carrier.baseline(),
            carrier.command().unwrap().actions(),
            &WritePolicyV1::default(),
        ),
        Err(TypedWriteError::UnsafeOrder {
            channel: 5,
            parameter: MUTE_PARAMETER
        })
    ));
    // Planning the same intent orders the mute first and passes the check.
    let planned = plan_writes(
        &base,
        &plan(vec![
            TypedChange::OutputGain {
                output: 1,
                gain_db: 0.0,
            },
            TypedChange::OutputMute {
                output: 1,
                muted: true,
            },
        ]),
        &WritePolicyV1::default(),
    )
    .unwrap();
    let actions = planned.apply_plan.command().unwrap().actions();
    let order: Vec<_> = actions.iter().map(|action| action.parameter()).collect();
    assert_eq!(order, [MUTE_PARAMETER, GAIN_PARAMETER]);
    check_policy(&base, actions, &WritePolicyV1::default()).unwrap();
}

#[test]
fn write_rollback_refuses_a_non_reverse_order() {
    // Review finding 3, rollback side: restoring [unmute, then gain] leaves
    // O1 live at 0 dB between the two writes.
    let mut builder = Builder::new();
    builder.set(5, GAIN_PARAMETER, 120);
    let base = builder.snapshot();
    let apply = vec![
        DirectParameterAction::new(5, MUTE_PARAMETER, 1).unwrap(),
        DirectParameterAction::new(5, GAIN_PARAMETER, 150).unwrap(),
    ];
    let desired = base.project_direct_actions(&apply).unwrap();
    let rollback = vec![
        DirectParameterAction::new(5, MUTE_PARAMETER, 0).unwrap(),
        DirectParameterAction::new(5, GAIN_PARAMETER, 120).unwrap(),
    ];
    let transaction = ApplyTransactionV1::stage(base, desired, apply, rollback).unwrap();
    let parsed =
        RollbackPlanV1::from_json(&transaction.rollback_plan().to_json().unwrap()).unwrap();
    assert!(matches!(
        check_rollback(&parsed),
        Err(TypedWriteError::RollbackOrder)
    ));
}

#[test]
fn moving_a_deep_active_cut_spends_the_rise_budget() {
    // Review finding 4: moving an active -15 dB bell off the 63 Hz front
    // mode releases all 15 dB at 63 Hz, exactly as releasing it by gain.
    let mut builder = Builder::new();
    builder.set(5, EQ_ENABLED_PARAMETER, 1);
    builder.set(5, EQ_COUNT_PARAMETER, 1);
    builder.set(5, band_parameter(1, BandField::Frequency), 53); // ~63 Hz
    builder.set(5, band_parameter(1, BandField::Q), 32); // ~4
    builder.set(5, band_parameter(1, BandField::Gain), 0); // -15 dB
    let base = builder.snapshot();
    let band = |frequency_hz, gain_db, q| TypedChange::PeqBand {
        output: 1,
        band: 1,
        filter: PeqKind::Bell,
        frequency_hz,
        gain_db,
        q,
        shelf_slope_db: None,
    };
    for change in [band(63.0, 0.0, 4.0), band(10_000.0, -15.0, 10.0)] {
        assert!(matches!(
            plan_writes(&base, &plan(vec![change]), &WritePolicyV1::default()),
            Err(TypedWriteError::GainStep { .. })
        ));
    }
}

#[test]
fn peq_only_carriers_stay_under_core_rules_only() {
    // Review finding 5 (scope, documented): a PEQ-only carrier on O1-O4 is
    // not a typed-write-lane carrier. The write policy would refuse this
    // PEQ-off over a -15 dB cut with O6 live, but the core admits it, as it
    // did before the typed-write lane existed, and `control apply` does not
    // route it through the write policy. docs/typed-writes.md says so.
    let mut builder = Builder::new();
    builder.set(5, EQ_ENABLED_PARAMETER, 1);
    builder.set(5, EQ_COUNT_PARAMETER, 1);
    builder.set(5, band_parameter(1, BandField::Gain), 0);
    builder.set(10, MUTE_PARAMETER, 0);
    let base = builder.snapshot();
    let off = DirectParameterAction::new(5, EQ_ENABLED_PARAMETER, 0).unwrap();
    assert!(
        !reviewed_address(5, EQ_ENABLED_PARAMETER)
            .unwrap()
            .is_typed_lane()
    );
    let transaction = ApplyTransactionV1::stage_projected(base.clone(), vec![off]).unwrap();
    assert!(
        check_policy(
            &base,
            transaction.apply_plan().command().unwrap().actions(),
            &WritePolicyV1::default()
        )
        .is_err()
    );
}
