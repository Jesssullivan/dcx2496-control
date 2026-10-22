//! Synthetic chain tests: validation refusals, the retune-from-0-dB order,
//! activation and deactivation, and a deterministic property sweep over
//! random O4 baselines and targets. Every frame here is synthetic; nothing is
//! device evidence.

use super::*;
use crate::{
    layout::{OUTPUT_COUNT, OutputField},
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

#[derive(Clone)]
struct Builder {
    dump0: Vec<u8>,
    dump1: Vec<u8>,
}

impl Builder {
    /// O1-O3 muted, O4 unmuted at 0 dB, LR24 high-pass at code 74 with the
    /// O3 low-pass type off at the same code (the bench's linked pair), PEQ
    /// off; O5/O6 muted; inputs at 0 dB.
    fn new() -> Self {
        let mut builder = Self {
            dump0: blank_dump(0),
            dump1: blank_dump(1),
        };
        for output in 1..=OUTPUT_COUNT {
            let channel = output_channel(output);
            builder.set(channel, MUTE_PARAMETER, u16::from(output != 4));
            for band in 1..=PEQ_BANDS {
                builder.set(channel, band_parameter(band, BandField::Frequency), 180);
                builder.set(channel, band_parameter(band, BandField::Q), 20);
                builder.set(channel, band_parameter(band, BandField::Gain), 150);
                builder.set(channel, band_parameter(band, BandField::Kind), 1);
                builder.set(channel, band_parameter(band, BandField::Slope), 1);
            }
            if output <= 4 {
                builder.set(channel, GAIN_PARAMETER, 150);
                builder.set(channel, LIMITER_THRESHOLD_PARAMETER, 240);
                builder.set(channel, LOW_PASS_FREQUENCY_PARAMETER, 319);
            }
        }
        builder.set(8, SOURCE_PARAMETER, 2);
        builder.set(8, HIGH_PASS_TYPE_PARAMETER, 8);
        builder.set(8, HIGH_PASS_FREQUENCY_PARAMETER, 74);
        builder.set(7, LOW_PASS_FREQUENCY_PARAMETER, 74);
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

    fn band(&mut self, output: u8, band: u8, codes: [u16; 5]) -> &mut Self {
        let channel = output_channel(output);
        for (field, code) in BandField::all().into_iter().zip(codes) {
            self.set(channel, band_parameter(band, field), code);
        }
        self
    }

    fn eq(&mut self, output: u8, enabled: bool, count: u16) -> &mut Self {
        let channel = output_channel(output);
        self.set(channel, EQ_ENABLED_PARAMETER, u16::from(enabled));
        self.set(channel, EQ_COUNT_PARAMETER, count)
    }

    fn snapshot(&self) -> SnapshotV1 {
        SnapshotV1::from_frames(&identity(), &self.dump0, &self.dump1).unwrap()
    }
}

fn code(snapshot: &SnapshotV1, output: u8, parameter: u8) -> u16 {
    snapshot.read_reviewed(reviewed_address(output_channel(output), parameter).unwrap())
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn milli(value: f64) -> u32 {
    (value * 1000.0).round() as u32
}

#[allow(clippy::cast_possible_truncation)]
fn tenths(gain_code: u16) -> i16 {
    (i32::from(gain_code) - 150) as i16
}

/// The profile of one output of a snapshot, as Legalab transcribes it from
/// `write show`: bands 1..count, codes exact, values rounded.
fn profile_of(snapshot: &SnapshotV1, output: u8) -> DcxOutputProfileV1 {
    let read = |parameter| code(snapshot, output, parameter);
    let count = read(EQ_COUNT_PARAMETER);
    let side = |type_parameter, frequency_parameter| {
        let frequency = read(frequency_parameter);
        ProfileCrossoverV1 {
            filter: CrossoverFilter::from_code(read(type_parameter)).unwrap(),
            frequency_milli_hz: milli(frequency_hz(frequency)),
            code: frequency,
        }
    };
    let bands = (1..=u8::try_from(count).unwrap())
        .map(|band| {
            let field = |field| read(band_parameter(band, field));
            let codes = ProfileBandCodesV1 {
                frequency: field(BandField::Frequency),
                gain: field(BandField::Gain),
                q: field(BandField::Q),
                kind: field(BandField::Kind),
                slope: field(BandField::Slope),
            };
            let kind = PeqKind::from_code(codes.kind).unwrap();
            ProfileBandV1 {
                band,
                kind,
                frequency_milli_hz: milli(frequency_hz(codes.frequency)),
                gain_db_tenths: tenths(codes.gain),
                q_thousandths: milli(q_value(codes.q)),
                shelf_slope_db: (kind != PeqKind::Bell).then_some(if codes.slope == 1 {
                    12
                } else {
                    6
                }),
                codes,
            }
        })
        .collect();
    let digest = snapshot.digest().to_owned();
    let hex8 = &digest["sha256/".len().."sha256/".len() + 8];
    DcxOutputProfileV1 {
        schema_version: OUTPUT_PROFILE_SCHEMA.to_owned(),
        contract_type: OUTPUT_PROFILE_CONTRACT.to_owned(),
        id: format!("dcx-o{output}-synthetic-{hex8}"),
        title: "SYNTHETIC".to_owned(),
        purpose: "SYNTHETIC".to_owned(),
        output,
        sink: "alto-pa".to_owned(),
        capture: ProfileCaptureV1 {
            typed_state_schema: "dcx.typed-state/v1".to_owned(),
            snapshot_digest: digest,
            evidence_class: EVIDENCE_UNVERIFIED.to_owned(),
            provenance: "observed-readback".to_owned(),
            observed_at: "2026-10-21T00:00Z".to_owned(),
            tool: "SYNTHETIC".to_owned(),
            lane_ref: "SYNTHETIC".to_owned(),
            lane_file_sha256: "0".repeat(64),
        },
        state: ProfileStateV1 {
            muted: read(MUTE_PARAMETER) == 1,
            source: OutputSource::from_code(read(SOURCE_PARAMETER)).unwrap(),
            gain_db_tenths: tenths(read(GAIN_PARAMETER)),
            polarity_inverted: read(POLARITY_PARAMETER) == 1,
            high_pass: side(HIGH_PASS_TYPE_PARAMETER, HIGH_PASS_FREQUENCY_PARAMETER),
            low_pass: side(LOW_PASS_TYPE_PARAMETER, LOW_PASS_FREQUENCY_PARAMETER),
            delay: ProfileDelayV1 {
                enabled: read(DELAY_ENABLED_PARAMETER) == 1,
                short_mm: u32::from(read(SHORT_DELAY_PARAMETER)) * SHORT_DELAY_STEP_MM,
                long_mm: u32::from(read(LONG_DELAY_PARAMETER)) * LONG_DELAY_STEP_MM,
            },
            limiter: ProfileLimiterV1 {
                enabled: read(LIMITER_ENABLED_PARAMETER) == 1,
                threshold_db_tenths: i16::try_from(
                    i32::from(read(LIMITER_THRESHOLD_PARAMETER)) - 240,
                )
                .unwrap(),
            },
            peq: ProfilePeqV1 {
                enabled: read(EQ_ENABLED_PARAMETER) == 1,
                count: u8::try_from(count).unwrap(),
                bands,
            },
        },
        limitations: "SYNTHETIC".to_owned(),
    }
}

fn chain(baseline: &SnapshotV1, target: &SnapshotV1) -> Result<ProfileChainV1, OutputProfileError> {
    let profile = profile_of(target, 4);
    profile.validate()?;
    plan_profile_chain(
        baseline,
        &profile,
        "sha256/synthetic",
        &WritePolicyV1::default(),
    )
}

/// Summed rise of one output between two states, counted independently of
/// the write policy: level rise, plus per band the effective-gain rise,
/// where moving an active band's shape releases its whole baseline cut.
fn independent_rise(before: &SnapshotV1, after: &SnapshotV1, output: u8) -> u16 {
    let read = |snapshot: &SnapshotV1, parameter| code(snapshot, output, parameter);
    let mut rise = read(after, GAIN_PARAMETER).saturating_sub(read(before, GAIN_PARAMETER));
    let is_active = |snapshot: &SnapshotV1, band: u8| {
        read(snapshot, EQ_ENABLED_PARAMETER) == 1
            && u16::from(band) <= read(snapshot, EQ_COUNT_PARAMETER)
    };
    for band in 1..=PEQ_BANDS {
        let effective = |snapshot: &SnapshotV1| {
            if is_active(snapshot, band) {
                read(snapshot, band_parameter(band, BandField::Gain))
            } else {
                UNITY_GAIN_CODE
            }
        };
        let moved = is_active(before, band)
            && [
                BandField::Frequency,
                BandField::Q,
                BandField::Kind,
                BandField::Slope,
            ]
            .into_iter()
            .any(|field| {
                read(before, band_parameter(band, field))
                    != read(after, band_parameter(band, field))
            });
        let after_gain = if moved {
            UNITY_GAIN_CODE
        } else {
            effective(after)
        };
        rise += after_gain.saturating_sub(effective(before));
    }
    rise
}

/// Every invariant one chain must hold, checked step by step.
fn check_chain(
    baseline: &SnapshotV1,
    profile: &DcxOutputProfileV1,
    chain: &ProfileChainV1,
    policy: &WritePolicyV1,
) {
    let mut previous = baseline.clone();
    for (index, step) in chain.steps.iter().enumerate() {
        assert_eq!(step.step, index + 1);
        let apply = &step.transaction.apply_plan;
        let rollback = &step.transaction.rollback_plan;
        assert_eq!(apply.baseline().digest(), previous.digest());
        assert_eq!(step.baseline_snapshot_digest, previous.digest());
        assert_eq!(rollback.baseline().digest(), previous.digest());
        assert_eq!(apply.desired().digest(), step.predicted_snapshot_digest);
        assert!(apply.action_count() >= 1);
        assert!(apply.action_count() <= DEFAULT_ACTIONS);
        assert_eq!(rollback.action_count(), apply.action_count());
        // The step re-plans to the same carrier, as `write apply` requires.
        let rebuilt = plan_writes(&previous, &step.write_plan, policy).unwrap();
        assert_eq!(&rebuilt.apply_plan, apply);
        let next = apply.desired().clone();
        // At most 3 dB of rise, counted independently.
        assert!(
            independent_rise(&previous, &next, 4) <= 30,
            "step {} rises too far",
            step.step
        );
        assert!(step.rise_db <= 3.0);
        assert!(
            (step.rise_db * 10.0 - f64::from(independent_rise(&previous, &next, 4))).abs() < 1e-9
        );
        assert!(
            (step.rollback_rise_db * 10.0 - f64::from(independent_rise(&next, &previous, 4))).abs()
                < 1e-9,
            "step {} rollback rise",
            step.step
        );
        // O4's verify-only fields never move.
        for parameter in [
            SOURCE_PARAMETER,
            POLARITY_PARAMETER,
            DELAY_ENABLED_PARAMETER,
            SHORT_DELAY_PARAMETER,
            LONG_DELAY_PARAMETER,
            LIMITER_ENABLED_PARAMETER,
            LIMITER_THRESHOLD_PARAMETER,
            crate::layout::LIMITER_RELEASE_PARAMETER,
        ] {
            assert_eq!(code(&previous, 4, parameter), code(&next, 4, parameter));
        }
        for band in 1..=PEQ_BANDS {
            let gain = code(&previous, 4, band_parameter(band, BandField::Gain));
            let was_active = code(&previous, 4, EQ_ENABLED_PARAMETER) == 1
                && u16::from(band) <= code(&previous, 4, EQ_COUNT_PARAMETER);
            let reshaped = [
                BandField::Frequency,
                BandField::Q,
                BandField::Kind,
                BandField::Slope,
            ]
            .into_iter()
            .any(|field| {
                code(&previous, 4, band_parameter(band, field))
                    != code(&next, 4, band_parameter(band, field))
            });
            // A cut band is retuned only from 0 dB.
            assert!(
                !(was_active && reshaped) || gain == UNITY_GAIN_CODE,
                "step {} retunes active band {band} from gain code {gain}",
                step.step
            );
        }
        assert!(code(&next, 4, EQ_COUNT_PARAMETER) <= u16::from(MAX_ACTIVE_PEQ_BANDS));
        // Nothing outside O4 moves, except the modelled O3 low-pass follower.
        for output in [1, 2, 3, 5, 6] {
            for parameter in 0..=0x7f_u8 {
                let Some(address) = reviewed_address(output_channel(output), parameter) else {
                    continue;
                };
                if output == 3 && address.field == OutputField::LowPassFrequency {
                    continue;
                }
                assert_eq!(
                    previous.read_reviewed(address),
                    next.read_reviewed(address),
                    "step {} moved O{output} {}",
                    step.step,
                    address.field.label()
                );
            }
        }
        for output in 1..=4 {
            assert_eq!(
                code(&previous, output, MUTE_PARAMETER),
                code(&next, output, MUTE_PARAMETER),
                "a chain never changes a mute"
            );
        }
        previous = next;
    }
    assert_eq!(chain.final_snapshot_digest, previous.digest());
    let verification = verify_profile(&previous, profile, "sha256/synthetic").unwrap();
    assert!(verification.matches(), "{:?}", verification.drift);
    assert_eq!(chain.budget.transactions, chain.steps.len());
    assert_eq!(
        chain.budget.apply_remote_mode_writes_total,
        5 * chain.steps.len()
    );
}

const DEFAULT_ACTIONS: usize = crate::typed_write::DEFAULT_MAX_ACTIONS;

fn gains(chain: &ProfileChainV1, band: u8) -> Vec<(u16, u16)> {
    chain
        .steps
        .iter()
        .map(|step| {
            let desired = step.transaction.apply_plan.desired();
            (
                code(desired, 4, band_parameter(band, BandField::Gain)),
                code(desired, 4, band_parameter(band, BandField::Frequency)),
            )
        })
        .collect()
}

#[test]
fn every_band_code_round_trips_through_the_typed_change_encoder() {
    for frequency in 0..=MAX_FREQUENCY_CODE {
        for (q, gain) in [
            (0, 0),
            (MAX_Q_CODE, UNITY_GAIN_CODE),
            (frequency % 41, frequency % 151),
        ] {
            let change = TypedChange::PeqBand {
                output: 4,
                band: 1,
                filter: PeqKind::Bell,
                frequency_hz: frequency_hz(frequency),
                gain_db: gain_db(gain),
                q: q_value(q),
                shelf_slope_db: Some(12),
            };
            let actions = crate::typed_write::encode_change(0, &change).unwrap();
            let values: Vec<u16> = actions.iter().map(|action| action.value()).collect();
            assert_eq!(values, vec![frequency, q, gain, 1, 1]);
        }
    }
    for gain in 0..=MAX_GAIN_CODE {
        let actions = crate::typed_write::encode_change(
            0,
            &TypedChange::OutputGain {
                output: 4,
                gain_db: gain_db(gain),
            },
        )
        .unwrap();
        assert_eq!(actions[0].value(), gain);
    }
}

#[test]
fn a_matching_snapshot_plans_no_transaction() {
    let snapshot = Builder::new()
        .eq(4, true, 1)
        .band(4, 1, [90, 20, 30, 0, 1])
        .snapshot();
    let chain = chain(&snapshot, &snapshot).unwrap();
    assert_eq!(chain.status, "already_matching");
    assert!(chain.steps.is_empty());
    assert_eq!(chain.final_snapshot_digest, snapshot.digest());
}

#[test]
fn a_minus_six_retune_goes_minus_three_then_zero_then_new() {
    let mut base = Builder::new();
    base.eq(4, true, 1).band(4, 1, [161, 38, 90, 1, 1]);
    let mut goal = base.clone();
    goal.band(4, 1, [141, 22, 100, 1, 1]);
    let (baseline, target) = (base.snapshot(), goal.snapshot());
    let chain = chain(&baseline, &target).unwrap();
    check_chain(
        &baseline,
        &profile_of(&target, 4),
        &chain,
        &WritePolicyV1::default(),
    );
    assert_eq!(gains(&chain, 1), vec![(120, 161), (150, 161), (100, 141)]);
    assert_eq!(
        chain
            .steps
            .iter()
            .map(|step| step.rise_db)
            .collect::<Vec<_>>(),
        vec![3.0, 3.0, 0.0]
    );
}

#[test]
fn a_minus_one_retune_still_steps_to_zero_first() {
    let mut base = Builder::new();
    base.eq(4, true, 1).band(4, 1, [251, 23, 140, 1, 1]);
    let mut goal = base.clone();
    goal.band(4, 1, [241, 23, 100, 1, 1]);
    let chain = chain(&base.snapshot(), &goal.snapshot()).unwrap();
    assert_eq!(gains(&chain, 1), vec![(150, 251), (100, 241)]);
}

#[test]
fn deeper_cuts_and_new_inactive_bands_are_free_and_go_together() {
    let mut base = Builder::new();
    base.eq(4, true, 2)
        .band(4, 1, [90, 20, 60, 0, 1])
        .band(4, 2, [101, 40, 90, 1, 1]);
    let mut goal = base.clone();
    goal.eq(4, true, 5)
        .band(4, 1, [90, 20, 30, 0, 1])
        .band(4, 3, [118, 40, 100, 1, 1])
        .band(4, 4, [251, 23, 110, 1, 1])
        .band(4, 5, [274, 20, 30, 2, 1]);
    let (baseline, target) = (base.snapshot(), goal.snapshot());
    let chain = chain(&baseline, &target).unwrap();
    check_chain(
        &baseline,
        &profile_of(&target, 4),
        &chain,
        &WritePolicyV1::default(),
    );
    assert_eq!(chain.steps.len(), 1);
    // Band fields go before the count rises, inside the one frame.
    let parameters: Vec<u8> = chain.steps[0]
        .transaction
        .writes
        .iter()
        .map(|write| write.parameter)
        .collect();
    assert_eq!(parameters.last(), Some(&EQ_COUNT_PARAMETER));
}

#[test]
fn a_band_leaving_the_active_set_is_released_before_the_count_drops() {
    let mut base = Builder::new();
    base.eq(4, true, 3)
        .band(4, 1, [90, 20, 30, 0, 1])
        .band(4, 2, [101, 40, 90, 1, 1])
        .band(4, 3, [118, 40, 100, 1, 1]);
    let mut goal = base.clone();
    goal.eq(4, true, 1);
    let (baseline, target) = (base.snapshot(), goal.snapshot());
    let chain = chain(&baseline, &target).unwrap();
    check_chain(
        &baseline,
        &profile_of(&target, 4),
        &chain,
        &WritePolicyV1::default(),
    );
    // 6 + 5 dB of release at 3 dB a step (3, 3, 3, 2), then the count.
    assert_eq!(chain.steps.len(), 5);
    let last = chain.steps.last().unwrap().transaction.apply_plan.desired();
    assert_eq!(code(last, 4, EQ_COUNT_PARAMETER), 1);
    let previous = chain.steps[3].transaction.apply_plan.desired();
    assert_eq!(code(previous, 4, EQ_COUNT_PARAMETER), 3);
    assert_eq!(code(previous, 4, band_parameter(2, BandField::Gain)), 150);
    assert_eq!(code(previous, 4, band_parameter(3, BandField::Gain)), 150);
}

#[test]
fn switching_peq_off_releases_every_active_cut_first() {
    let mut base = Builder::new();
    base.eq(4, true, 1).band(4, 1, [90, 20, 120, 0, 1]);
    let mut goal = base.clone();
    goal.eq(4, false, 1);
    let (baseline, target) = (base.snapshot(), goal.snapshot());
    let chain = chain(&baseline, &target).unwrap();
    check_chain(
        &baseline,
        &profile_of(&target, 4),
        &chain,
        &WritePolicyV1::default(),
    );
    // Release -3 dB, switch PEQ off, then restore the (now inactive) band's
    // profiled cut.
    assert_eq!(chain.steps.len(), 3);
    let off = chain.steps[1].transaction.apply_plan.desired();
    assert_eq!(code(off, 4, EQ_ENABLED_PARAMETER), 0);
    assert_eq!(code(off, 4, band_parameter(1, BandField::Gain)), 150);
}

#[test]
fn an_output_gain_rise_is_split_into_three_db_steps_and_capped_at_zero() {
    let mut base = Builder::new();
    base.set(8, GAIN_PARAMETER, 70);
    let goal = Builder::new();
    let (baseline, target) = (base.snapshot(), goal.snapshot());
    let chain = chain(&baseline, &target).unwrap();
    check_chain(
        &baseline,
        &profile_of(&target, 4),
        &chain,
        &WritePolicyV1::default(),
    );
    // -8 dB to 0 dB: 3 + 3 + 2.
    assert_eq!(chain.steps.len(), 3);
    let mut boost = goal.clone();
    boost.set(8, GAIN_PARAMETER, 160);
    let profile = profile_of(&boost.snapshot(), 4);
    assert!(matches!(
        plan_profile_chain(&baseline, &profile, "sha256/x", &WritePolicyV1::default()),
        Err(OutputProfileError::GainAboveCeiling { .. })
    ));
}

#[test]
fn verify_only_drift_is_refused_and_never_written() {
    let base = Builder::new();
    for (parameter, value) in [
        (MUTE_PARAMETER, 1),
        (SOURCE_PARAMETER, 3),
        (POLARITY_PARAMETER, 1),
        (DELAY_ENABLED_PARAMETER, 1),
        (LIMITER_ENABLED_PARAMETER, 1),
        (LIMITER_THRESHOLD_PARAMETER, 200),
    ] {
        let mut goal = base.clone();
        goal.set(8, parameter, value);
        let error = chain(&base.snapshot(), &goal.snapshot()).unwrap_err();
        assert!(
            matches!(error, OutputProfileError::VerifyOnlyDrift(ref fields) if fields.len() == 1),
            "{parameter:#04x}: {error}"
        );
    }
}

#[test]
fn a_muted_output_under_an_unmuted_profile_is_tuned_and_left_muted() {
    let mut base = Builder::new();
    base.set(8, MUTE_PARAMETER, 1);
    let mut goal = Builder::new();
    goal.eq(4, true, 1).band(4, 1, [161, 36, 90, 1, 1]);
    let (baseline, target) = (base.snapshot(), goal.snapshot());
    let profile = profile_of(&target, 4);
    assert!(!profile.state.muted);
    let chain = plan_profile_chain(&baseline, &profile, "x", &WritePolicyV1::default()).unwrap();
    assert!(chain.held_muted);
    assert_eq!(chain.steps.len(), 1);
    let last = chain.final_snapshot(&baseline);
    assert_eq!(code(last, 4, MUTE_PARAMETER), 1);
    assert_eq!(chain.final_verification.drift, vec!["mute".to_owned()]);
    assert!(chain.final_verification.matches_holding_mute(true));
    assert!(!chain.final_verification.matches_holding_mute(false));
    // The reverse is refused: a live output under a muted profile.
    let mut muted_goal = goal.clone();
    muted_goal.set(8, MUTE_PARAMETER, 1);
    let error = plan_profile_chain(
        &Builder::new().snapshot(),
        &profile_of(&muted_goal.snapshot(), 4),
        "x",
        &WritePolicyV1::default(),
    )
    .unwrap_err();
    assert!(
        matches!(error, OutputProfileError::VerifyOnlyDrift(ref fields) if fields == &["mute"])
    );
}

#[test]
fn loosening_an_active_low_pass_needs_protection_consent() {
    let mut base = Builder::new();
    base.set(8, LOW_PASS_TYPE_PARAMETER, 8)
        .set(8, LOW_PASS_FREQUENCY_PARAMETER, 250);
    for (parameter, value) in [
        (LOW_PASS_TYPE_PARAMETER, 0),
        (LOW_PASS_FREQUENCY_PARAMETER, 260),
    ] {
        let mut goal = base.clone();
        goal.set(8, parameter, value);
        let profile = profile_of(&goal.snapshot(), 4);
        assert!(matches!(
            plan_profile_chain(&base.snapshot(), &profile, "x", &WritePolicyV1::default()),
            Err(OutputProfileError::LowPassProtection(4))
        ));
        let consent = WritePolicyV1 {
            allow_protection_change: true,
            ..WritePolicyV1::default()
        };
        plan_profile_chain(&base.snapshot(), &profile, "x", &consent).unwrap();
    }
    // A lower low-pass corner tightens it and needs no consent.
    let mut goal = base.clone();
    goal.set(8, LOW_PASS_FREQUENCY_PARAMETER, 240);
    let profile = profile_of(&goal.snapshot(), 4);
    plan_profile_chain(&base.snapshot(), &profile, "x", &WritePolicyV1::default()).unwrap();
}

#[test]
fn a_stored_boost_is_lowered_to_zero_before_its_band_is_retuned() {
    let mut base = Builder::new();
    base.eq(4, true, 1).band(4, 1, [161, 36, 200, 1, 1]);
    let mut goal = base.clone();
    goal.band(4, 1, [141, 22, 100, 1, 1]);
    let (baseline, target) = (base.snapshot(), goal.snapshot());
    let chain = chain(&baseline, &target).unwrap();
    check_chain(
        &baseline,
        &profile_of(&target, 4),
        &chain,
        &WritePolicyV1::default(),
    );
    assert_eq!(gains(&chain, 1), vec![(150, 161), (100, 141)]);
    // Leaving the active set, it is lowered too.
    let mut fewer = base.clone();
    fewer.eq(4, true, 0);
    let chain = self::chain(&baseline, &fewer.snapshot()).unwrap();
    assert_eq!(chain.steps.len(), 2);
}

#[test]
fn a_step_that_adds_cuts_has_a_larger_rollback_rise() {
    let base = Builder::new();
    let mut goal = base.clone();
    goal.eq(4, true, 2)
        .band(4, 1, [90, 20, 30, 0, 1])
        .band(4, 2, [274, 20, 30, 2, 1]);
    let chain = chain(&base.snapshot(), &goal.snapshot()).unwrap();
    assert_eq!(chain.steps.len(), 1);
    assert!((chain.steps[0].rise_db).abs() < f64::EPSILON);
    assert!((chain.steps[0].rollback_rise_db - 24.0).abs() < 1e-9);
}

#[test]
fn reserved_outputs_unmuted_refuse_every_chain() {
    let mut base = Builder::new();
    base.set(output_channel(6), MUTE_PARAMETER, 0);
    let error = chain(&base.snapshot(), &Builder::new().snapshot()).unwrap_err();
    assert!(matches!(
        error,
        OutputProfileError::Write(TypedWriteError::ReservedOutputUnmuted { output: 6, .. })
    ));
}

#[test]
fn loosening_the_high_pass_needs_protection_consent() {
    let base = Builder::new();
    let mut goal = base.clone();
    goal.set(8, HIGH_PASS_FREQUENCY_PARAMETER, 64);
    let profile = profile_of(&goal.snapshot(), 4);
    let error =
        plan_profile_chain(&base.snapshot(), &profile, "x", &WritePolicyV1::default()).unwrap_err();
    assert!(matches!(
        error,
        OutputProfileError::Write(TypedWriteError::ProtectionChange { output: 4, .. })
    ));
    let consent = WritePolicyV1 {
        allow_protection_change: true,
        ..WritePolicyV1::default()
    };
    let chain = plan_profile_chain(&base.snapshot(), &profile, "x", &consent).unwrap();
    assert_eq!(chain.steps.len(), 1);
    assert!(chain.steps[0].write_plan.allow_protection_change);
    // The linked O3 low-pass (type off) follows the O4 high-pass move.
    let effects = &chain.steps[0].transaction.linked_effects;
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].follower_output, 3);
}

#[test]
fn profile_validation_refuses_nine_bands_boosts_and_inconsistent_values() {
    let snapshot = Builder::new()
        .eq(4, true, 2)
        .band(4, 1, [90, 20, 30, 0, 1])
        .band(4, 2, [101, 40, 90, 1, 1])
        .snapshot();
    let good = profile_of(&snapshot, 4);
    good.validate().unwrap();
    let round_trip = DcxOutputProfileV1::from_json(&serde_json::to_vec(&good).unwrap()).unwrap();
    assert_eq!(round_trip, good);

    let mut nine = good.clone();
    nine.state.peq.count = 9;
    assert!(matches!(
        nine.validate(),
        Err(OutputProfileError::DeviceBandCount(9))
    ));
    let mut boost = good.clone();
    boost.state.peq.bands[0].codes.gain = 160;
    boost.state.peq.bands[0].gain_db_tenths = 10;
    assert!(matches!(
        boost.validate(),
        Err(OutputProfileError::EqBoost { band: 1, .. })
    ));
    let mut value = good.clone();
    value.state.peq.bands[1].frequency_milli_hz += 5;
    assert!(matches!(
        value.validate(),
        Err(OutputProfileError::Invalid(_))
    ));
    let mut kind = good.clone();
    kind.state.peq.bands[1].kind = PeqKind::HighShelf;
    assert!(matches!(
        kind.validate(),
        Err(OutputProfileError::Invalid(_))
    ));
    let mut order = good.clone();
    order.state.peq.bands.swap(0, 1);
    assert!(matches!(
        order.validate(),
        Err(OutputProfileError::Invalid(_))
    ));
    let mut reserved = good.clone();
    reserved.output = 5;
    assert!(matches!(
        reserved.validate(),
        Err(OutputProfileError::ReservedOutput(5))
    ));
    let mut id = good.clone();
    id.id = "dcx-o4-synthetic-00000000".to_owned();
    assert!(matches!(id.validate(), Err(OutputProfileError::Invalid(_))));
    let mut unknown = serde_json::to_value(&good).unwrap();
    unknown["state"]["extra"] = serde_json::json!(1);
    assert!(DcxOutputProfileV1::from_json(&serde_json::to_vec(&unknown).unwrap()).is_err());
}

#[test]
fn typed_writes_refuse_a_band_count_of_nine() {
    let plan = WritePlanV1 {
        schema: WRITE_PLAN_SCHEMA.to_owned(),
        device_id: 0,
        allow_routing: false,
        allow_unmute: false,
        allow_protection_change: false,
        changes: vec![TypedChange::PeqBandCount {
            output: 4,
            count: 9,
        }],
    };
    let error =
        plan_writes(&Builder::new().snapshot(), &plan, &WritePolicyV1::default()).unwrap_err();
    assert!(matches!(
        error,
        TypedWriteError::DeviceBandCount { count: 9, .. }
    ));
}

#[test]
fn the_chain_id_binds_policy_and_carriers() {
    let mut base = Builder::new();
    base.eq(4, true, 1).band(4, 1, [161, 38, 90, 1, 1]);
    let mut goal = base.clone();
    goal.band(4, 1, [141, 22, 100, 1, 1]);
    let profile = profile_of(&goal.snapshot(), 4);
    let first =
        plan_profile_chain(&base.snapshot(), &profile, "x", &WritePolicyV1::default()).unwrap();
    let again =
        plan_profile_chain(&base.snapshot(), &profile, "x", &WritePolicyV1::default()).unwrap();
    assert_eq!(first.chain_id, again.chain_id);
    let tighter = WritePolicyV1 {
        max_gain_step_db: 2.0,
        ..WritePolicyV1::default()
    };
    let slower = plan_profile_chain(&base.snapshot(), &profile, "x", &tighter).unwrap();
    assert_ne!(slower.chain_id, first.chain_id);
    assert_eq!(slower.steps.len(), 4);
    let other =
        plan_profile_chain(&base.snapshot(), &profile, "y", &WritePolicyV1::default()).unwrap();
    assert_ne!(other.chain_id, first.chain_id);
}

#[test]
fn a_tight_action_limit_spreads_the_chain_without_breaking_it() {
    let base = Builder::new();
    let mut goal = base.clone();
    goal.eq(4, true, 8);
    for band in 1..=8 {
        goal.band(4, band, [100 + u16::from(band) * 10, 30, 100, 1, 1]);
    }
    let (baseline, target) = (base.snapshot(), goal.snapshot());
    let policy = WritePolicyV1 {
        max_actions: 4,
        ..WritePolicyV1::default()
    };
    let profile = profile_of(&target, 4);
    let chain = plan_profile_chain(&baseline, &profile, "x", &policy).unwrap();
    assert!(
        chain
            .steps
            .iter()
            .all(|step| step.transaction.apply_plan.action_count() <= 4)
    );
    let verification = verify_profile(chain.final_snapshot(&baseline), &profile, "x").unwrap();
    assert!(verification.matches());
}

/// Deterministic generator, as in the robustness sweeps.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: u16) -> u16 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        u16::try_from((self.0 >> 33) % u64::from(bound)).unwrap()
    }

    fn band(&mut self) -> [u16; 5] {
        let kind = self.next(3);
        // Cut-only, with a bias toward 0 dB and toward a few shared shapes so
        // the sweep hits same-shape gain moves as well as retunes.
        let gain = match self.next(4) {
            0 => UNITY_GAIN_CODE,
            1 => UNITY_GAIN_CODE - self.next(31),
            _ => self.next(UNITY_GAIN_CODE + 1),
        };
        let frequency = if self.next(2) == 0 {
            100 + self.next(4) * 20
        } else {
            self.next(MAX_FREQUENCY_CODE + 1)
        };
        [
            frequency,
            self.next(MAX_Q_CODE + 1),
            gain,
            kind,
            self.next(2),
        ]
    }

    /// A random O4 state. A baseline (`baseline`) may also hold stored
    /// boosts and an output gain above 0 dB; a target never does.
    fn state(&mut self, builder: &mut Builder, baseline: bool) {
        let count = self.next(9);
        builder.eq(4, self.next(4) != 0, count);
        for band in 1..=PEQ_BANDS {
            if self.next(3) != 0 {
                let mut codes = self.band();
                if baseline && self.next(6) == 0 {
                    codes[2] = UNITY_GAIN_CODE + 1 + self.next(150);
                }
                builder.band(4, band, codes);
            }
        }
        let gain_span = if baseline { 241 } else { 91 };
        builder.set(8, GAIN_PARAMETER, 60 + self.next(gain_span));
        if self.next(3) == 0 {
            builder.set(8, LOW_PASS_TYPE_PARAMETER, self.next(11));
            builder.set(8, LOW_PASS_FREQUENCY_PARAMETER, 200 + self.next(121));
        }
        if self.next(4) == 0 {
            builder.set(8, HIGH_PASS_FREQUENCY_PARAMETER, 74 + self.next(20));
        }
        if self.next(5) == 0 {
            builder.set(8, HIGH_PASS_TYPE_PARAMETER, 1 + self.next(10));
        }
        if !baseline {
            for band in 1..=PEQ_BANDS {
                let gain = band_parameter(band, BandField::Gain);
                let stored = code(&builder.snapshot(), 4, gain);
                builder.set(8, gain, stored.min(UNITY_GAIN_CODE));
            }
        }
    }
}

#[test]
fn property_random_chains_hold_every_invariant() {
    let mut random = Lcg(0x2496_dc04_2026_1021);
    let mut planned = 0;
    let mut refused = 0;
    for _ in 0..400 {
        let mut base = Builder::new();
        random.state(&mut base, true);
        let mut goal = base.clone();
        random.state(&mut goal, false);
        let (baseline, target) = (base.snapshot(), goal.snapshot());
        let profile = profile_of(&target, 4);
        let consent = random.next(2) == 0;
        let policy = WritePolicyV1 {
            allow_protection_change: consent,
            max_actions: usize::from(5 + random.next(12)),
            ..WritePolicyV1::default()
        };
        match plan_profile_chain(&baseline, &profile, "sha256/synthetic", &policy) {
            Ok(chain) => {
                check_chain(&baseline, &profile, &chain, &policy);
                assert!(chain.steps.iter().all(|step| {
                    step.transaction.apply_plan.action_count() <= policy.max_actions
                }));
                planned += 1;
            }
            // Without consent, a random crossover move may loosen an active
            // high-pass or low-pass.
            Err(
                OutputProfileError::Write(TypedWriteError::ProtectionChange { .. })
                | OutputProfileError::LowPassProtection(4),
            ) if !consent => refused += 1,
            Err(error) => panic!("unexpected refusal: {error}"),
        }
    }
    assert!(
        planned > 250,
        "only {planned} of 400 random chains planned ({refused} refused)"
    );
}
