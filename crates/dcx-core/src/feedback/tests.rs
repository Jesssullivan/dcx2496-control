//! Seeded, dependency-free property tests for the notch planner and the
//! generalized PEQ projection.

use super::*;
use crate::peq_bank::gain_code;
use crate::{
    ApplyTransactionV1, SnapshotSection,
    layout::{BandField, OUTPUT_COUNT, band_parameter, dump_len, output_location},
    protocol::{DUMP0_RESPONSE_LEN, DUMP1_RESPONSE_LEN, DumpPart},
};

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

    fn unit(&mut self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let value = (self.next() >> 11) as f64 / (1_u64 << 53) as f64;
        value
    }

    fn chance(&mut self, numerator: u64, denominator: u64) -> bool {
        self.below(denominator) < numerator
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

fn random_dump(rng: &mut Rng, part: u8) -> Vec<u8> {
    let mut frame = blank_dump(part);
    let length = frame.len();
    for byte in &mut frame[13..length - 1] {
        *byte = u8::try_from(rng.below(0x80)).unwrap();
    }
    frame
}

#[derive(Debug, Clone, Copy)]
struct Bank {
    enabled: bool,
    count: u8,
    bands: [BandCodes; 9],
}

fn random_codes(rng: &mut Rng) -> BandCodes {
    let pick = |rng: &mut Rng, field: BandField| {
        u16::try_from(rng.below(u64::from(field.device_max()) + 1)).unwrap()
    };
    BandCodes {
        frequency_code: pick(rng, BandField::Frequency),
        q_code: pick(rng, BandField::Q),
        gain_code: pick(rng, BandField::Gain),
        kind_code: pick(rng, BandField::Kind),
        slope_code: pick(rng, BandField::Slope),
    }
}

fn random_bank(rng: &mut Rng) -> Bank {
    let mut bands = [BandCodes {
        frequency_code: 0,
        q_code: 0,
        gain_code: 0,
        kind_code: 0,
        slope_code: 0,
    }; 9];
    for band in &mut bands {
        *band = random_codes(rng);
    }
    Bank {
        enabled: rng.chance(1, 2),
        count: u8::try_from(rng.below(10)).unwrap(),
        bands,
    }
}

fn write(dump0: &mut [u8], dump1: &mut [u8], output: u8, parameter: u8, value: u16) {
    let location = output_location(output, parameter).unwrap();
    let frame = match location.part() {
        DumpPart::Part0 => dump0,
        DumpPart::Part1 => dump1,
    };
    location.write(frame, value).unwrap();
}

/// A random but domain-valid snapshot: every output's PEQ bank and the O4
/// mute hold device-domain values; every other payload byte is random.
fn random_snapshot(rng: &mut Rng, o4: Bank) -> SnapshotV1 {
    let mut dump0 = random_dump(rng, 0);
    let mut dump1 = random_dump(rng, 1);
    for output in 1..=OUTPUT_COUNT {
        let bank = if output == 4 { o4 } else { random_bank(rng) };
        write(
            &mut dump0,
            &mut dump1,
            output,
            0x06,
            u16::from(bank.enabled),
        );
        write(&mut dump0, &mut dump1, output, 0x07, u16::from(bank.count));
        for band in 1..=9 {
            let codes = bank.bands[usize::from(band - 1)];
            for field in BandField::all() {
                write(
                    &mut dump0,
                    &mut dump1,
                    output,
                    band_parameter(band, field),
                    codes.get(field),
                );
            }
        }
    }
    write(&mut dump0, &mut dump1, 4, 0x03, 1);
    SnapshotV1::from_frames(&identity(), &dump0, &dump1).unwrap()
}

fn random_measurement(rng: &mut Rng) -> FeedbackMeasurementV1 {
    let count = 1 + rng.below(20);
    let mut peaks = Vec::new();
    for _ in 0..count {
        // Log-uniform over 20 Hz .. 20 kHz.
        let frequency_hz = 20.0 * 1000.0_f64.powf(rng.unit());
        let peak = FeedbackPeakV1 {
            frequency_hz: frequency_hz.clamp(20.0, 20_000.0),
            level_db: rng.chance(2, 3).then(|| -40.0 + 60.0 * rng.unit()),
            requested_cut_db: rng.chance(1, 4).then(|| -15.0 + 14.9 * rng.unit()),
        };
        peaks.push(peak);
        // Recurrences: sometimes repeat the same peak.
        if rng.chance(1, 5) {
            peaks.push(peak);
        }
    }
    FeedbackMeasurementV1::new(MeasurementSource::FrequencyList, b"synthetic", 4, &peaks).unwrap()
}

fn changed_actions(
    baseline: &SnapshotV1,
    desired: &[DirectParameterAction],
) -> Vec<DirectParameterAction> {
    let observed = baseline.inverse_actions_for(desired).unwrap();
    desired
        .iter()
        .zip(&observed)
        .filter(|(want, have)| want.value() != have.value())
        .map(|(want, _)| *want)
        .collect()
}

#[test]
#[allow(clippy::too_many_lines)]
fn planner_properties_hold_over_seeded_random_banks_and_measurements() {
    let mut rng = Rng(0x00d0_c0de_2496_0007);
    let policy = NotchPolicyV1::default();
    let mut planned = 0;
    for case in 0..600 {
        let bank = random_bank(&mut rng);
        let baseline = random_snapshot(&mut rng, bank);
        let measurement = random_measurement(&mut rng);
        let result = plan_notches(&measurement, &baseline, policy, None);
        let n = bank.count;
        if !bank.enabled && n > 0 {
            assert!(
                matches!(result, Err(FeedbackPlanError::WouldEnableOperatorBands(count)) if count == n),
                "case {case}"
            );
            continue;
        }
        if n == 9 {
            assert!(
                matches!(result, Err(FeedbackPlanError::NoFreeBands)),
                "case {case}"
            );
            continue;
        }
        let plan = result.unwrap();
        planned += 1;
        let k = plan.notches().len();
        assert!(
            k >= 1 && k <= usize::from(policy.max_notches.min(9 - n)),
            "case {case}"
        );
        assert_eq!(plan.operator_band_count(), n);

        // Notch shape, placement, distinct codes, and the per-octave limit.
        let mut codes = Vec::new();
        for (offset, notch) in plan.notches().iter().enumerate() {
            assert_eq!(usize::from(notch.band), usize::from(n) + offset + 1);
            assert_eq!(notch.codes.q_code, 40);
            assert_eq!(notch.codes.kind_code, 1);
            assert!(notch.codes.gain_code < 150);
            assert!(
                i32::from(notch.codes.gain_code) >= 150 + i32::from(policy.max_cut_decibels_tenths)
            );
            assert!(notch.codes.frequency_code <= 320);
            codes.push(notch.codes.frequency_code);
        }
        assert!(
            codes.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case}"
        );
        for &start in &codes {
            let in_octave = codes
                .iter()
                .filter(|&&code| code >= start && code < start + 32)
                .count();
            assert!(in_octave <= 2, "case {case}");
        }
        assert_eq!(
            k + plan.dropped().len(),
            merge_peaks(measurement.peaks(), policy.merge_cents).len()
        );

        // No write lands on an operator band; the last two actions are count then enable.
        let actions = plan.direct_actions().unwrap();
        for action in &actions[..actions.len() - 2] {
            let address = reviewed_address(action.channel(), action.parameter()).unwrap();
            let crate::layout::OutputField::Band { band, .. } = address.field else {
                panic!("non-band action before count/enable");
            };
            assert!(band > n, "case {case} wrote operator band {band}");
            assert_eq!(address.output, 4);
        }
        assert_eq!(actions[actions.len() - 2].parameter(), 0x07);
        assert_eq!(
            usize::from(actions[actions.len() - 2].value()),
            usize::from(n) + k
        );
        assert_eq!(actions[actions.len() - 1].parameter(), 0x06);
        assert_eq!(actions[actions.len() - 1].value(), 1);

        // The receipt round-trips and revalidates.
        assert_eq!(
            NotchPlanV1::from_json(&plan.to_json().unwrap()).unwrap(),
            plan
        );
        let profile = plan
            .desired_profile("o4-feedback".into(), "r1".into())
            .unwrap();
        assert_eq!(profile.document().actions, actions);

        // Apply then rollback restores every byte, both trailers included.
        let changed = changed_actions(&baseline, &actions);
        let transaction =
            ApplyTransactionV1::stage_projected(baseline.clone(), changed.clone()).unwrap();
        let desired = transaction.apply_plan().desired().clone();
        let rollback = transaction
            .rollback_plan()
            .command()
            .unwrap()
            .actions()
            .to_vec();
        assert_eq!(desired.project_direct_actions(&rollback).unwrap(), baseline);
        // Rollback restores the enable and count before band values.
        let mut reversed = changed.clone();
        reversed.reverse();
        assert_eq!(
            rollback
                .iter()
                .map(|action| (action.channel(), action.parameter()))
                .collect::<Vec<_>>(),
            reversed
                .iter()
                .map(|action| (action.channel(), action.parameter()))
                .collect::<Vec<_>>()
        );

        // Only the O4 notch bands, count, enable, and the Dump1 trailer move.
        let mut allowed = std::collections::BTreeSet::new();
        for action in &changed {
            let location = output_location(4, action.parameter()).unwrap();
            assert_eq!(location.part(), DumpPart::Part1);
            allowed.extend(location.offsets());
        }
        allowed.insert(DUMP1_RESPONSE_LEN - 2);
        assert_eq!(
            desired.frame(SnapshotSection::Dump0),
            baseline.frame(SnapshotSection::Dump0)
        );
        let before = baseline.frame(SnapshotSection::Dump1);
        let after = desired.frame(SnapshotSection::Dump1);
        for index in 0..dump_len(DumpPart::Part1) {
            if before[index] != after[index] {
                assert!(
                    allowed.contains(&index),
                    "case {case} moved Dump1 byte {index}"
                );
            }
        }
        let decoded_before = decode_peq_bank(&baseline, 4).unwrap();
        let decoded_after = decode_peq_bank(&desired, 4).unwrap();
        for band in 1..=n {
            assert_eq!(decoded_after.band(band), decoded_before.band(band));
        }
        assert!(decoded_after.eq_enabled);
        assert_eq!(usize::from(decoded_after.eq_count), usize::from(n) + k);

        // Replanning the same measurement against the applied state, with
        // this plan as the prior receipt, is idempotent and changes nothing.
        let replanned = plan_notches(&measurement, &desired, policy, Some(&plan)).unwrap();
        assert_eq!(replanned.notches(), plan.notches());
        assert_eq!(replanned.direct_actions().unwrap(), actions);
        assert!(
            changed_actions(&desired, &actions).is_empty(),
            "case {case}"
        );

        // Planning is a pure function of its inputs.
        assert_eq!(
            plan_notches(&measurement, &baseline, policy, None).unwrap(),
            plan
        );
    }
    assert!(planned > 100, "only {planned} seeded cases planned");
}

#[test]
fn every_reviewed_address_round_trips_apply_then_rollback_over_random_baselines() {
    let mut rng = Rng(0x5eed_0f00_dcab_1e00);
    for case in 0..400 {
        let bank = random_bank(&mut rng);
        let baseline = random_snapshot(&mut rng, bank);
        let output = u8::try_from(1 + rng.below(6)).unwrap();
        let channel = output_channel(output);
        let mut actions = Vec::new();
        for parameter in 0..=0x7f_u8 {
            let Some(address) = reviewed_address(channel, parameter) else {
                continue;
            };
            if !rng.chance(1, 3) {
                continue;
            }
            let value =
                u16::try_from(rng.below(u64::from(address.field.device_max()) + 1)).unwrap();
            actions.push(DirectParameterAction::new(channel, parameter, value).unwrap());
        }
        if actions.is_empty() {
            continue;
        }
        let desired = baseline.project_direct_actions(&actions).unwrap();
        // Exact readback of every written value.
        let observed = desired.inverse_actions_for(&actions).unwrap();
        assert_eq!(observed, actions, "case {case}");
        // Exact restoration, including both trailers.
        let rollback = baseline.rollback_actions_for(&actions).unwrap();
        assert_eq!(
            desired.project_direct_actions(&rollback).unwrap(),
            baseline,
            "case {case}"
        );
        // Projection is idempotent.
        assert_eq!(desired.project_direct_actions(&actions).unwrap(), desired);
    }
}

#[test]
fn prior_plan_ownership_is_exact_and_refuses_drift() {
    let mut rng = Rng(7);
    let mut bank = random_bank(&mut rng);
    bank.enabled = true;
    bank.count = 2;
    let baseline = random_snapshot(&mut rng, bank);
    let measurement = FeedbackMeasurementV1::from_frequency_list("2500\n630\n", 4).unwrap();
    let policy = NotchPolicyV1::default();
    let plan = plan_notches(&measurement, &baseline, policy, None).unwrap();
    assert_eq!(
        plan.notches()
            .iter()
            .map(|notch| notch.band)
            .collect::<Vec<_>>(),
        [3, 4]
    );
    // The prior plan does not match the untouched baseline.
    assert!(matches!(
        plan_notches(&measurement, &baseline, policy, Some(&plan)),
        Err(FeedbackPlanError::PriorPlanNotOwned)
    ));
    let desired = baseline
        .project_direct_actions(&plan.direct_actions().unwrap())
        .unwrap();
    // Without a prior receipt the notches look like operator bands: stack above.
    let stacked = plan_notches(&measurement, &desired, policy, None).unwrap();
    assert_eq!(stacked.operator_band_count(), 4);
    // A cumulative ring-out that repeats 2500 Hz deepens only that notch.
    let deeper = FeedbackMeasurementV1::from_frequency_list("2500\n630\n2500\n", 4).unwrap();
    let replanned = plan_notches(&deeper, &desired, policy, Some(&plan)).unwrap();
    assert_eq!(replanned.operator_band_count(), 2);
    let gain = |plan: &NotchPlanV1, band: u8| {
        plan.notches()
            .iter()
            .find(|notch| notch.band == band)
            .unwrap()
            .codes
            .gain_code
    };
    assert_eq!(gain(&plan, 4), gain_code(-6.0));
    assert_eq!(gain(&replanned, 4), gain_code(-9.0));
    assert_eq!(gain(&replanned, 3), gain(&plan, 3));
    // A tampered receipt fails closed.
    let mut value: serde_json::Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
    value["operator_band_count"] = 1.into();
    assert!(NotchPlanV1::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn recurrence_deepens_to_the_policy_cap_and_merges_within_a_sixth_octave() {
    let mut rng = Rng(11);
    let mut bank = random_bank(&mut rng);
    bank.count = 0;
    let baseline = random_snapshot(&mut rng, bank);
    let list = "frequency_hz,level_db\n1000,3\n1050,9\n1000,1\n1000\n1000\n# comment\n";
    let measurement = FeedbackMeasurementV1::from_frequency_list(list, 4).unwrap();
    let plan = plan_notches(&measurement, &baseline, NotchPolicyV1::default(), None).unwrap();
    assert_eq!(plan.notches().len(), 1);
    let notch = &plan.notches()[0];
    assert_eq!(notch.occurrences, 5);
    // The most severe member is the representative frequency.
    assert!((notch.frequency_hz - 1050.0).abs() < f64::EPSILON);
    assert_eq!(notch.level_db, Some(9.0));
    // -6 - 3 * 4 = -18, capped at the -12 dB policy floor.
    assert_eq!(notch.codes.gain_code, gain_code(-12.0));
}

#[test]
fn caps_and_octave_limit_drop_the_least_severe_peaks() {
    let mut rng = Rng(13);
    let mut bank = random_bank(&mut rng);
    bank.count = 0;
    let baseline = random_snapshot(&mut rng, bank);
    // Three peaks inside one octave, plus three spread peaks.
    let list = "1000,10\n1300,9\n1700,8\n100,7\n5000,6\n12000,5\n";
    let measurement = FeedbackMeasurementV1::from_frequency_list(list, 4).unwrap();
    let plan = plan_notches(&measurement, &baseline, NotchPolicyV1::default(), None).unwrap();
    let planned: Vec<_> = plan
        .notches()
        .iter()
        .map(|notch| notch.frequency_hz)
        .collect();
    assert_eq!(planned, [100.0, 1000.0, 1300.0, 5000.0]);
    let dropped: Vec<_> = plan
        .dropped()
        .iter()
        .map(|peak| (peak.frequency_hz, peak.reason))
        .collect();
    assert_eq!(
        dropped,
        [
            (1700.0, DropReason::PerOctaveLimit),
            (12000.0, DropReason::NotchCap)
        ]
    );
}

#[test]
fn only_o4_is_a_feedback_target_and_policy_is_bounded() {
    let baseline = SnapshotV1::from_frames(&identity(), &blank_dump(0), &blank_dump(1)).unwrap();
    for output in [1, 2, 3, 5, 6] {
        let measurement = FeedbackMeasurementV1::from_frequency_list("1000\n", output).unwrap();
        assert!(matches!(
            plan_notches(&measurement, &baseline, NotchPolicyV1::default(), None),
            Err(FeedbackPlanError::UnsupportedOutput(found)) if found == output
        ));
    }
    let measurement = FeedbackMeasurementV1::from_frequency_list("1000\n", 4).unwrap();
    for policy in [
        NotchPolicyV1 {
            max_notches: 0,
            ..NotchPolicyV1::default()
        },
        NotchPolicyV1 {
            max_cut_decibels_tenths: -160,
            ..NotchPolicyV1::default()
        },
        NotchPolicyV1 {
            initial_cut_decibels_tenths: 10,
            ..NotchPolicyV1::default()
        },
        NotchPolicyV1 {
            q_code: 41,
            ..NotchPolicyV1::default()
        },
        NotchPolicyV1 {
            merge_cents: 0,
            ..NotchPolicyV1::default()
        },
    ] {
        assert!(matches!(
            plan_notches(&measurement, &baseline, policy, None),
            Err(FeedbackPlanError::InvalidPolicy)
        ));
    }
    // An all-zero synthetic bank: PEQ off with no operator bands is allowed.
    let plan = plan_notches(&measurement, &baseline, NotchPolicyV1::default(), None).unwrap();
    assert_eq!(plan.notches()[0].band, 1);
}

#[test]
fn frequency_list_grammar_is_strict_and_bounded() {
    for bad in [
        "",
        "# only a comment\n",
        "1000,2,3\n",
        "abc\n",
        "10\n",
        "20001\n",
        "1000,nan\n",
        "1000,inf\n",
        "frequency_hz\nfrequency_hz\n",
    ] {
        assert!(
            FeedbackMeasurementV1::from_frequency_list(bad, 4).is_err(),
            "{bad:?}"
        );
    }
    let measurement =
        FeedbackMeasurementV1::from_frequency_list("\u{feff}frequency_hz\n 250 \n2500\t-3\n", 4)
            .unwrap();
    assert_eq!(measurement.peaks().len(), 2);
    assert_eq!(measurement.peaks()[1].level_db, Some(-3.0));
    let too_many = "1000\n".repeat(MAX_MEASUREMENT_PEAKS + 1);
    assert!(matches!(
        FeedbackMeasurementV1::from_frequency_list(&too_many, 4),
        Err(MeasurementError::TooManyPeaks(_))
    ));
    let round_trip = FeedbackMeasurementV1::from_json(&measurement.to_json().unwrap()).unwrap();
    assert_eq!(round_trip, measurement);
    let mut value: serde_json::Value =
        serde_json::from_slice(&measurement.to_json().unwrap()).unwrap();
    value["peaks"][0]["frequency_hz"] = 260.0.into();
    assert!(matches!(
        FeedbackMeasurementV1::from_json(&serde_json::to_vec(&value).unwrap()),
        Err(MeasurementError::DigestMismatch)
    ));
}

#[test]
fn rew_export_becomes_requested_cuts_and_rejects_shelves() {
    let rew = "Equaliser: Generic\n\
               Filter 1: ON PK Fc 630 Hz Gain -4.5 dB Q 8.0\n\
               Filter 2: ON PK Fc 2500 Hz Gain -9.0 dB Q 10.0\n\
               Filter 3: OFF PK Fc 4000 Hz Gain -3.0 dB Q 5.0\n";
    let measurement = FeedbackMeasurementV1::from_rew_generic_eq(rew, 4).unwrap();
    assert_eq!(measurement.source(), MeasurementSource::RewGenericEq);
    assert_eq!(measurement.peaks().len(), 2);
    let baseline = SnapshotV1::from_frames(&identity(), &blank_dump(0), &blank_dump(1)).unwrap();
    let plan = plan_notches(&measurement, &baseline, NotchPolicyV1::default(), None).unwrap();
    let gains: Vec<_> = plan
        .notches()
        .iter()
        .map(|notch| notch.codes.gain_code)
        .collect();
    assert_eq!(gains, [gain_code(-4.5), gain_code(-9.0)]);
    let shelf = "Equaliser: Generic\nFilter 1: ON LS Fc 100 Hz Gain -3 dB Q 0.7\n";
    assert!(matches!(
        FeedbackMeasurementV1::from_rew_generic_eq(shelf, 4),
        Err(MeasurementError::NotANotch(1))
    ));
    let zero = "Equaliser: Generic\nFilter 1: ON PK Fc 100 Hz Gain 0 dB Q 5\n";
    assert!(matches!(
        FeedbackMeasurementV1::from_rew_generic_eq(zero, 4),
        Err(MeasurementError::ZeroCut(1))
    ));
}

#[test]
fn boosts_are_refused_at_apply_admission_even_when_projectable() {
    let baseline = SnapshotV1::from_frames(&identity(), &blank_dump(0), &blank_dump(1)).unwrap();
    let boost = DirectParameterAction::new(8, band_parameter(1, BandField::Gain), 200).unwrap();
    // Projection represents the full device domain so rollback can restore a
    // stored operator boost exactly, but the apply path refuses to write one.
    assert!(baseline.project_direct_actions(&[boost]).is_ok());
    assert!(matches!(
        ApplyTransactionV1::stage_projected(baseline, vec![boost]),
        Err(crate::ApplyTransactionError::Plan(
            crate::ApplyPlanError::BoostNotAdmitted { value: 200, .. }
        ))
    ));
}

/// A blank (all-cut, PEQ off) snapshot with one O4 bank overridden.
fn o4_snapshot(enabled: bool, count: u8, gains: &[(u8, u16)]) -> SnapshotV1 {
    let mut dump0 = blank_dump(0);
    let mut dump1 = blank_dump(1);
    write(&mut dump0, &mut dump1, 4, 0x06, u16::from(enabled));
    write(&mut dump0, &mut dump1, 4, 0x07, u16::from(count));
    for &(band, gain) in gains {
        write(
            &mut dump0,
            &mut dump1,
            4,
            band_parameter(band, BandField::Gain),
            gain,
        );
    }
    SnapshotV1::from_frames(&identity(), &dump0, &dump1).unwrap()
}

fn o4_action(parameter: u8, value: u16) -> DirectParameterAction {
    DirectParameterAction::new(8, parameter, value).unwrap()
}

fn stored_boost_refusal(
    result: Result<ApplyTransactionV1, crate::ApplyTransactionError>,
) -> Option<(u8, u8, u16)> {
    match result {
        Err(crate::ApplyTransactionError::Plan(crate::ApplyPlanError::StoredBoostActivation {
            output,
            band,
            gain_code,
        })) => Some((output, band, gain_code)),
        Ok(_) => None,
        Err(error) => panic!("unexpected staging error: {error}"),
    }
}

#[test]
fn enable_peq_over_stored_boost_is_refused() {
    // PEQ off, two operator bands, band 2 stores +5 dB.
    let baseline = o4_snapshot(false, 2, &[(1, 120), (2, 200)]);
    let enable = o4_action(0x06, 1);
    assert_eq!(
        stored_boost_refusal(ApplyTransactionV1::stage_projected(
            baseline.clone(),
            vec![enable]
        )),
        Some((4, 2, 200))
    );
    // The same refusal reaches a v2 desired profile bound through `control diff`.
    let profile = DesiredPeqBankProfileV2::new(
        "o4-enable".into(),
        "r1".into(),
        PeqBankDocumentV2 {
            target_output: 4,
            parameter_channel: 8,
            actions: vec![enable],
        },
    )
    .unwrap();
    let changed = changed_actions(&baseline, &profile.document().actions);
    assert_eq!(
        stored_boost_refusal(ApplyTransactionV1::stage_projected(
            baseline.clone(),
            changed
        )),
        Some((4, 2, 200))
    );
    // Writing a cut over the stored boost in the same plan is admitted.
    let cut_then_enable = vec![o4_action(band_parameter(2, BandField::Gain), 140), enable];
    assert_eq!(
        stored_boost_refusal(ApplyTransactionV1::stage_projected(
            baseline.clone(),
            cut_then_enable
        )),
        None
    );
    // Unity gain is not a boost.
    let unity = o4_snapshot(false, 2, &[(1, 120), (2, 150)]);
    assert_eq!(
        stored_boost_refusal(ApplyTransactionV1::stage_projected(unity, vec![enable])),
        None
    );
    // The planner refuses it even with --allow-enable-operator-bands.
    let measurement = FeedbackMeasurementV1::from_frequency_list("2500\n", 4).unwrap();
    let permissive = NotchPolicyV1 {
        allow_enable_operator_bands: true,
        ..NotchPolicyV1::default()
    };
    assert!(matches!(
        plan_notches(&measurement, &baseline, permissive, None),
        Err(FeedbackPlanError::WouldEnableStoredBoost {
            band: 2,
            gain_code: 200
        })
    ));
    // Operator cuts only: the flag admits the plan and it stages.
    let cuts = o4_snapshot(false, 2, &[(1, 120), (2, 100)]);
    let plan = plan_notches(&measurement, &cuts, permissive, None).unwrap();
    let changed = changed_actions(&cuts, &plan.direct_actions().unwrap());
    ApplyTransactionV1::stage_projected(cuts, changed).unwrap();
}

#[test]
fn raise_band_count_over_stored_boost_is_refused() {
    // PEQ on, one active band; band 2 stores +0.1 dB (code 151).
    let baseline = o4_snapshot(true, 1, &[(1, 120), (2, 151)]);
    assert_eq!(
        stored_boost_refusal(ApplyTransactionV1::stage_projected(
            baseline.clone(),
            vec![o4_action(0x07, 2)]
        )),
        Some((4, 2, 151))
    );
    // Raising past it is refused at the first boosted band.
    let deep = o4_snapshot(true, 1, &[(2, 100), (3, 300)]);
    assert_eq!(
        stored_boost_refusal(ApplyTransactionV1::stage_projected(
            deep,
            vec![o4_action(0x07, 5)]
        )),
        Some((4, 3, 300))
    );
    // An already active stored boost is not newly activated: raising the
    // count over a cut band is admitted, and so is lowering the count.
    let active = o4_snapshot(true, 1, &[(1, 250), (2, 100)]);
    assert_eq!(
        stored_boost_refusal(ApplyTransactionV1::stage_projected(
            active.clone(),
            vec![o4_action(0x07, 2)]
        )),
        None
    );
    assert_eq!(
        stored_boost_refusal(ApplyTransactionV1::stage_projected(
            baseline,
            vec![o4_action(0x07, 0)]
        )),
        None
    );
    // Turning PEQ off and back on in separate plans: re-enabling is refused.
    let off = o4_snapshot(false, 1, &[(1, 250)]);
    assert_eq!(
        stored_boost_refusal(ApplyTransactionV1::stage_projected(
            off,
            vec![o4_action(0x06, 1)]
        )),
        Some((4, 1, 250))
    );
    // An admitted carrier revalidates through the same check.
    let plan = ApplyTransactionV1::stage_projected(active, vec![o4_action(0x07, 2)])
        .unwrap()
        .apply_plan()
        .clone();
    assert!(crate::ApplyPlanV1::from_json(&plan.to_json().unwrap()).is_ok());
}

/// Independent oracle: the first band that becomes active and holds a boost.
fn newly_active_boost(baseline: &SnapshotV1, desired: &SnapshotV1) -> Option<(u8, u8, u16)> {
    for output in 1..=OUTPUT_COUNT {
        let before = decode_peq_bank(baseline, output).unwrap();
        let after = decode_peq_bank(desired, output).unwrap();
        for state in &after.bands {
            let was_active = before.eq_enabled && state.band <= before.eq_count;
            let is_active = after.eq_enabled && state.band <= after.eq_count;
            if is_active && !was_active && state.codes.gain_code > 150 {
                return Some((output, state.band, state.codes.gain_code));
            }
        }
    }
    None
}

#[test]
fn stored_boost_activation_property_over_seeded_random_banks() {
    let mut rng = Rng(0xb005_7ed0_dcab_2496);
    let (mut refused, mut admitted) = (0, 0);
    for case in 0..400 {
        let bank = random_bank(&mut rng);
        let baseline = random_snapshot(&mut rng, bank);
        let output = u8::try_from(1 + rng.below(6)).unwrap();
        let channel = output_channel(output);
        let mut actions = Vec::new();
        if rng.chance(1, 2) {
            actions.push(
                DirectParameterAction::new(channel, 0x06, u16::from(rng.chance(3, 4))).unwrap(),
            );
        }
        if rng.chance(2, 3) {
            let count = u16::try_from(rng.below(10)).unwrap();
            actions.push(DirectParameterAction::new(channel, 0x07, count).unwrap());
        }
        for band in 1..=9 {
            if rng.chance(1, 6) {
                let gain = u16::try_from(rng.below(151)).unwrap();
                actions.push(
                    DirectParameterAction::new(
                        channel,
                        band_parameter(band, BandField::Gain),
                        gain,
                    )
                    .unwrap(),
                );
            }
        }
        let changed = if actions.is_empty() {
            Vec::new()
        } else {
            changed_actions(&baseline, &actions)
        };
        if changed.is_empty() {
            continue;
        }
        let desired = baseline.project_direct_actions(&changed).unwrap();
        let expected = newly_active_boost(&baseline, &desired);
        let result = ApplyTransactionV1::stage_projected(baseline.clone(), changed);
        match (expected, result) {
            (Some(found), result) => {
                assert_eq!(stored_boost_refusal(result), Some(found), "case {case}");
                refused += 1;
            }
            (None, Ok(transaction)) => {
                let plan = transaction.apply_plan();
                // Durable carrier revalidation is costly in debug; sample it.
                if admitted % 25 == 0 {
                    assert_eq!(
                        &crate::ApplyPlanV1::from_json(&plan.to_json().unwrap()).unwrap(),
                        plan,
                        "case {case}"
                    );
                }
                // The desired readback is contained by the plan's projection.
                assert!(
                    plan.uncontained_changes(&desired).unwrap().is_empty(),
                    "case {case}"
                );
                admitted += 1;
            }
            (None, Err(error)) => panic!("case {case}: unexpected refusal {error}"),
        }
    }
    assert!(
        refused > 50 && admitted > 50,
        "{refused} refused, {admitted} admitted"
    );

    // Under --allow-enable-operator-bands, every plan the planner emits stages.
    let permissive = NotchPolicyV1 {
        allow_enable_operator_bands: true,
        ..NotchPolicyV1::default()
    };
    let mut planner_refused = 0;
    for case in 0..200 {
        let bank = random_bank(&mut rng);
        let baseline = random_snapshot(&mut rng, bank);
        let measurement = random_measurement(&mut rng);
        match plan_notches(&measurement, &baseline, permissive, None) {
            Ok(plan) => {
                let changed = changed_actions(&baseline, &plan.direct_actions().unwrap());
                ApplyTransactionV1::stage_projected(baseline, changed)
                    .unwrap_or_else(|error| panic!("case {case}: {error}"));
            }
            Err(FeedbackPlanError::WouldEnableStoredBoost { band, gain_code }) => {
                assert!(!bank.enabled && band <= bank.count, "case {case}");
                assert!(gain_code > 150, "case {case}");
                planner_refused += 1;
            }
            Err(FeedbackPlanError::NoFreeBands) => assert_eq!(bank.count, 9),
            Err(error) => panic!("case {case}: {error}"),
        }
    }
    assert!(
        planner_refused > 20,
        "only {planner_refused} planner refusals"
    );
}

#[test]
fn readback_bytes_outside_the_projection_are_uncontained() {
    let baseline = o4_snapshot(false, 0, &[]);
    let actions = vec![
        o4_action(band_parameter(1, BandField::Frequency), 223),
        o4_action(0x07, 1),
        o4_action(0x06, 1),
    ];
    let transaction = ApplyTransactionV1::stage_projected(baseline.clone(), actions).unwrap();
    let plan = transaction.apply_plan();
    let desired = plan.desired().clone();
    assert!(plan.uncontained_changes(&desired).unwrap().is_empty());
    assert!(plan.uncontained_changes(&baseline).unwrap().is_empty());

    let trailer1 = DUMP1_RESPONSE_LEN - 2;
    let trailer0 = DUMP0_RESPONSE_LEN - 2;
    let offsets = plan.projected_offsets();
    assert!(offsets.contains(&(SnapshotSection::Dump1, trailer1)));
    assert!(!offsets.contains(&(SnapshotSection::Dump0, trailer0)));
    assert!(
        offsets
            .iter()
            .all(|(section, _)| *section == SnapshotSection::Dump1)
    );

    let mutate = |section: SnapshotSection, offset: usize| {
        let mut identity = desired.frame(SnapshotSection::Identity).to_vec();
        let mut dump0 = desired.frame(SnapshotSection::Dump0).to_vec();
        let mut dump1 = desired.frame(SnapshotSection::Dump1).to_vec();
        let frame = match section {
            SnapshotSection::Identity => &mut identity,
            SnapshotSection::Dump0 => &mut dump0,
            SnapshotSection::Dump1 => &mut dump1,
        };
        frame[offset] ^= 0x01;
        SnapshotV1::from_frames(&identity, &dump0, &dump1).unwrap()
    };
    // The Dump1 trailer and a projected byte are contained.
    assert!(
        plan.uncontained_changes(&mutate(SnapshotSection::Dump1, trailer1))
            .unwrap()
            .is_empty()
    );
    let projected_low = output_location(4, band_parameter(1, BandField::Frequency))
        .unwrap()
        .low_offset();
    assert!(
        plan.uncontained_changes(&mutate(SnapshotSection::Dump1, projected_low))
            .unwrap()
            .is_empty()
    );
    // An unrelated Dump1 byte, the Dump0 trailer, and a Dump0 payload byte are not.
    let neighbour = output_location(4, band_parameter(2, BandField::Frequency))
        .unwrap()
        .low_offset();
    for (section, offset) in [
        (SnapshotSection::Dump1, neighbour),
        (SnapshotSection::Dump0, trailer0),
        (SnapshotSection::Dump0, 200),
        (SnapshotSection::Identity, 10),
    ] {
        let changes = plan.uncontained_changes(&mutate(section, offset)).unwrap();
        assert_eq!(changes.len(), 1, "{section:?} {offset}");
        assert_eq!((changes[0].section, changes[0].offset), (section, offset));
    }
}
