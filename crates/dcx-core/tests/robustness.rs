//! Deterministic generated-property coverage suitable for ordinary CI.

use dcx_core::{
    DirectParameterAction, DirectParameterCommand, LabProfileV1, SnapshotSection, SnapshotV1,
    discovery::{DiscoveryError, QueryOnlyDiscovery},
    protocol::{
        DUMP0_RESPONSE_LEN, DUMP1_RESPONSE_LEN, DecodedMessage, DeviceId, FrameDecoder,
        MAX_FRAME_LEN, Query, decode, parse_frame,
    },
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

fn synthetic_dump(device: u8, part: u8) -> Vec<u8> {
    let length = match part {
        0 => DUMP0_RESPONSE_LEN,
        1 => DUMP1_RESPONSE_LEN,
        _ => panic!("synthetic part must be zero or one"),
    };
    let mut frame = vec![0; length];
    frame[..7].copy_from_slice(&[0xf0, 0, 0x20, 0x32, device, 0x0e, 0x10]);
    frame[12] = part;
    frame[length - 1] = 0xf7;
    frame
}

fn synthetic_snapshot() -> SnapshotV1 {
    SnapshotV1::from_frames(
        &synthetic_search_response(),
        &synthetic_dump(0, 0),
        &synthetic_dump(0, 1),
    )
    .unwrap()
}

#[test]
fn every_fourteen_bit_direct_value_round_trips_through_the_closed_frame() {
    for value in 0..=0x3fff {
        let action = DirectParameterAction::new(5, 0x3b, value).unwrap();
        let command = DirectParameterCommand::new(DeviceId::new(0).unwrap(), vec![action]).unwrap();
        assert_eq!(
            decode(parse_frame(&command.encode().unwrap()).unwrap()).unwrap(),
            DecodedMessage::DirectParameters {
                device: DeviceId::new(0).unwrap(),
                changes: vec![dcx_core::protocol::ParameterChange {
                    channel: 5,
                    parameter: 0x3b,
                    value,
                }],
            }
        );
    }
}

#[test]
fn every_fourteen_bit_value_projects_through_the_reviewed_split_layout() {
    let baseline = synthetic_snapshot();
    for (parameter, low, middle, bit, high) in [(0x3b, 843, 844, 6, 845), (0x3d, 848, 852, 3, 849)]
    {
        for value in 0..=0x3fff {
            let action = DirectParameterAction::new(5, parameter, value).unwrap();
            let desired = baseline.project_direct_actions(&[action]).unwrap();
            let dump = desired.frame(SnapshotSection::Dump0);
            let reconstructed = u16::from(dump[low])
                | (u16::from((dump[middle] >> bit) & 1) << 7)
                | (u16::from(dump[high]) << 8);
            assert_eq!(reconstructed, value);
        }
    }
}

#[test]
fn every_unreviewed_direct_address_is_rejected_by_snapshot_projection() {
    let baseline = synthetic_snapshot();
    for channel in 0..=10 {
        for parameter in 0..=0x7f {
            let action = DirectParameterAction::new(channel, parameter, 0).unwrap();
            let projected = baseline.project_direct_actions(&[action]);
            if channel == 5 && (0x3b..=0x3e).contains(&parameter) {
                assert!(projected.is_ok());
            } else {
                assert!(projected.is_err());
            }
        }
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
