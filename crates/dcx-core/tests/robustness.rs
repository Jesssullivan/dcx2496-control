//! Deterministic generated-property coverage suitable for ordinary CI.

use dcx_core::{
    LabProfileV1,
    discovery::{DiscoveryError, QueryOnlyDiscovery},
    protocol::{DeviceId, FrameDecoder, MAX_FRAME_LEN, Query, parse_frame},
    rew::{RewParseError, import_rew},
};

fn synthetic_search_response() -> Vec<u8> {
    include_str!("../../../fixtures/protocol/SYNTHETIC-search-response-26.hex")
        .split_ascii_whitespace()
        .map(|word| u8::from_str_radix(word, 16).unwrap())
        .collect()
}

#[test]
fn arbitrary_byte_streams_never_panic_or_retain_unbounded_candidates() {
    let mut seed = 0x2496_dc00_u64;
    for case in 0..2_000 {
        let length = case % (MAX_FRAME_LEN * 2);
        let mut decoder = FrameDecoder::default();
        for _ in 0..length {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let byte = seed.to_le_bytes()[4];
            let _ = decoder.push(byte);
        }
    }
}

#[test]
fn every_valid_inbound_device_and_function_parses_with_seven_bit_payloads() {
    for device in 0..=15 {
        for function in 0..=0x7f {
            let bytes = [
                0xf0, 0x00, 0x20, 0x32, device, 0x0e, function, 0, 1, 0x3f, 0x7f, 0xf7,
            ];
            let message = parse_frame(&bytes).unwrap();
            assert_eq!(message.address().wire_device_for_test(), Some(device));
            assert_eq!(message.function(), function);
            assert_eq!(message.data(), [0, 1, 0x3f, 0x7f]);
        }
    }
}

#[test]
fn no_single_byte_corruption_of_query_envelope_is_accepted_as_original() {
    let original_bytes = Query::Ping(DeviceId::new(0).unwrap()).encode().unwrap();
    let original = parse_frame(&original_bytes).unwrap();
    for index in [0, 1, 2, 3, 4, 5, original_bytes.len() - 1] {
        let mut corrupted = original_bytes.clone();
        corrupted[index] ^= 1;
        assert_ne!(parse_frame(&corrupted).ok(), Some(original.clone()));
    }
}

#[test]
fn every_device_address_requires_an_exact_expected_search_identity() {
    let template = synthetic_search_response();
    for expected in 0..=15 {
        for observed in 0..=15 {
            let mut frame = template.clone();
            frame[4] = observed;
            let mut discovery = QueryOnlyDiscovery::new(DeviceId::new(expected).unwrap());
            let result = discovery.accept_candidates(&[&frame]);
            if expected == observed {
                assert_eq!(result.unwrap().device().get(), expected);
            } else {
                assert_eq!(
                    result,
                    Err(DiscoveryError::UnexpectedDevice {
                        expected,
                        actual: observed,
                    })
                );
            }
        }
    }
}

#[test]
fn only_one_complete_candidate_can_cross_the_discovery_gate() {
    let frame = synthetic_search_response();
    for count in 0..=32 {
        let candidates: Vec<&[u8]> = (0..count).map(|_| frame.as_slice()).collect();
        let mut discovery = QueryOnlyDiscovery::new(DeviceId::new(0).unwrap());
        let result = discovery.accept_candidates(&candidates);
        match count {
            0 => assert_eq!(result, Err(DiscoveryError::NoCandidate)),
            1 => assert!(result.is_ok()),
            _ => assert_eq!(result, Err(DiscoveryError::AmbiguousCandidates(count))),
        }
    }
}

#[test]
fn every_nonidentity_envelope_mutation_fails_closed_without_state_change() {
    let original = synthetic_search_response();
    for position in [1_usize, 2, 3, 5, 6] {
        for replacement in 0..=0x7f {
            if replacement == original[position] {
                continue;
            }
            let mut mutated = original.clone();
            mutated[position] = replacement;
            let mut discovery = QueryOnlyDiscovery::new(DeviceId::new(0).unwrap());
            assert!(
                discovery.accept_candidates(&[&mutated]).is_err(),
                "identity byte {position} accepted replacement {replacement:#04x}"
            );
            assert_eq!(
                discovery.state(),
                dcx_core::discovery::DiscoveryState::AwaitingPrimary
            );
        }
    }
}

#[test]
fn every_nonexact_search_response_length_is_rejected() {
    let original = synthetic_search_response();
    for length in 0..=64 {
        if length == original.len() {
            continue;
        }
        let mut candidate = original.clone();
        candidate.resize(length, 0);
        let mut discovery = QueryOnlyDiscovery::new(DeviceId::new(0).unwrap());
        assert!(discovery.accept_candidates(&[&candidate]).is_err());
    }
}

#[test]
fn every_non_identity_output_role_rotation_is_rejected() {
    let profile_bytes = include_bytes!("../../../fixtures/profiles/safe-muted-v1.json");
    let baseline = LabProfileV1::from_json(profile_bytes).unwrap();
    for rotation in 1..6 {
        let mut profile = baseline.clone();
        let mut roles: Vec<_> = profile.outputs.iter().map(|output| output.role).collect();
        roles.rotate_left(rotation);
        for (output, role) in profile.outputs.iter_mut().zip(roles) {
            output.role = role;
        }
        assert!(profile.validate().is_err());
    }
}

#[test]
fn generated_cut_filters_stay_in_bounded_native_codes() {
    let mut seed = 0x5eed_2496_u64;
    for _ in 0..1_000 {
        seed = seed.wrapping_mul(2_862_933_555_777_941_757).wrapping_add(1);
        let unit = f64::from(seed.to_le_bytes()[3]) / 255.0;
        let frequency = 20.0 + unit * 19_980.0;
        let gain = -15.0 + unit * 15.0;
        let q = 0.1 + unit * 9.9;
        let text =
            format!("Equaliser: Generic\nFilter 1: ON PK Fc {frequency} Hz Gain {gain} dB Q {q}\n");
        let report = import_rew(&text, 1).unwrap();
        let filter = &report.filters[0];
        assert!(filter.frequency_code <= 320);
        assert!(filter.gain_code <= 150);
        assert!(filter.q_code <= 40);
    }
}

#[test]
fn generated_positive_gains_have_no_import_lane() {
    for tenth_db in 1..=150 {
        let gain = f64::from(tenth_db) / 10.0;
        let text = format!("Equaliser: Generic\nFilter 1: ON PK Fc 100 Hz Gain {gain} dB Q 1\n");
        assert!(matches!(
            import_rew(&text, 1),
            Err(RewParseError::PositiveGain { .. })
        ));
    }
}

trait AddressTestView {
    fn wire_device_for_test(self) -> Option<u8>;
}

impl AddressTestView for dcx_core::protocol::Address {
    fn wire_device_for_test(self) -> Option<u8> {
        match self {
            Self::Device(device) => Some(device.get()),
            Self::BroadcastSearch => None,
        }
    }
}
