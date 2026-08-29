//! Deterministic robustness coverage for the injected no-device boundary.

use std::collections::VecDeque;

use dcx_core::{
    discovery::{DiscoveryAttemptKind, DiscoveryError, FALLBACK_BAUD, PRIMARY_BAUD},
    protocol::DeviceId,
};
use dcx_transport::{
    MAX_SEARCH_ATTEMPTS, SEARCH_ATTEMPT_TIMEOUT, SEARCH_DISCOVERY_BUDGET, SEARCH_REQUEST_LEN,
    SEARCH_RESPONSE_LIMIT, SearchExecutionError, SearchOperation, SearchOperationKind,
    SearchOutcome, SearchRead, SearchReadError, SearchTransport, execute_search,
};
use thiserror::Error;

const SEARCH_REQUEST: [u8; SEARCH_REQUEST_LEN] = [0xf0, 0x00, 0x20, 0x32, 0x20, 0x0e, 0x40, 0xf7];

#[derive(Debug, Error)]
#[error("synthetic transport error")]
struct FakeError;

#[derive(Clone)]
enum Step {
    Read(SearchRead),
}

struct ScriptedTransport {
    steps: VecDeque<Step>,
    operations: Vec<SearchOperation>,
}

impl ScriptedTransport {
    fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            operations: Vec::new(),
        }
    }
}

impl SearchTransport for ScriptedTransport {
    type Error = FakeError;

    fn search(&mut self, operation: SearchOperation) -> Result<SearchRead, Self::Error> {
        self.operations.push(operation);
        match self.steps.pop_front().expect("property supplied a step") {
            Step::Read(read) => Ok(read),
        }
    }
}

fn synthetic_response(device: u8) -> [u8; SEARCH_RESPONSE_LIMIT] {
    let mut frame = [0; SEARCH_RESPONSE_LIMIT];
    frame[..7].copy_from_slice(&[0xf0, 0x00, 0x20, 0x32, device, 0x0e, 0x00]);
    frame[7..25].copy_from_slice(b"SYNTHETIC-IDENTITY");
    frame[25] = 0xf7;
    frame
}

fn assert_fixed_operation(operation: SearchOperation, expected: u8) {
    assert_eq!(operation.expected_device().get(), expected);
    assert_eq!(operation.request().as_bytes(), &SEARCH_REQUEST);
    assert_eq!(operation.timeout(), SEARCH_ATTEMPT_TIMEOUT);
    assert_eq!(operation.response_limit(), SEARCH_RESPONSE_LIMIT);
    assert_eq!(operation.settings().data_bits(), 8);
    assert_eq!(operation.settings().stop_bits(), 1);
}

#[test]
fn every_supported_address_requires_the_exact_expected_identity() {
    for expected in 0..=15 {
        for observed in 0..=15 {
            let response = synthetic_response(observed);
            let mut transport =
                ScriptedTransport::new([Step::Read(SearchRead::complete(&response).unwrap())]);
            let result = execute_search(&mut transport, DeviceId::new(expected).unwrap());

            if expected == observed {
                let SearchOutcome::Identified(identity) = result.unwrap() else {
                    panic!("matching exact identity must be identified")
                };
                assert_eq!(identity.device().get(), expected);
                assert_eq!(identity.kind(), SearchOperationKind::Primary);
            } else {
                assert!(matches!(
                    result,
                    Err(SearchExecutionError::Validation {
                        source: DiscoveryError::UnexpectedDevice {
                            expected: expected_error,
                            actual,
                        },
                        ..
                    }) if expected_error == expected && actual == observed
                ));
            }
            assert_eq!(transport.operations.len(), 1);
            assert_fixed_operation(transport.operations[0], expected);
            assert_eq!(transport.operations[0].settings().baud(), PRIMARY_BAUD);
        }
    }
}

#[test]
fn every_nonempty_partial_timeout_stops_before_fallback() {
    let response = synthetic_response(0);
    for length in 1..SEARCH_RESPONSE_LIMIT {
        let mut transport = ScriptedTransport::new([Step::Read(
            SearchRead::timed_out(&response[..length]).unwrap(),
        )]);
        assert!(matches!(
            execute_search(&mut transport, DeviceId::new(0).unwrap()),
            Err(SearchExecutionError::PartialTimeout {
                attempt: DiscoveryAttemptKind::Primary,
                received,
            }) if received == length
        ));
        assert_eq!(transport.operations.len(), 1);
    }
}

#[test]
fn read_result_constructors_enforce_the_exact_input_ceiling() {
    for length in 0..=(SEARCH_RESPONSE_LIMIT * 3) {
        let bytes = vec![0; length];
        match length {
            SEARCH_RESPONSE_LIMIT => assert!(SearchRead::complete(&bytes).is_ok()),
            _ => assert_eq!(
                SearchRead::complete(&bytes),
                Err(SearchReadError::CompleteLength(length))
            ),
        }
        if length < SEARCH_RESPONSE_LIMIT {
            assert!(SearchRead::timed_out(&bytes).is_ok());
        } else {
            assert_eq!(
                SearchRead::timed_out(&bytes),
                Err(SearchReadError::TimeoutLength(length))
            );
        }
    }
}

#[test]
fn every_envelope_or_identity_mutation_stops_without_fallback() {
    let original = synthetic_response(0);
    for position in [0_usize, 1, 2, 3, 4, 5, 6, 25] {
        for replacement in 0..=u8::MAX {
            if replacement == original[position] {
                continue;
            }
            let mut mutated = original;
            mutated[position] = replacement;
            let mut transport =
                ScriptedTransport::new([Step::Read(SearchRead::complete(&mutated).unwrap())]);
            assert!(
                execute_search(&mut transport, DeviceId::new(0).unwrap()).is_err(),
                "identity byte {position} accepted replacement {replacement:#04x}"
            );
            assert_eq!(transport.operations.len(), 1);
        }
    }
}

#[test]
fn all_addresses_follow_primary_then_one_fallback_with_no_third_attempt() {
    for expected in 0..=15 {
        let response = synthetic_response(expected);
        let mut transport = ScriptedTransport::new([
            Step::Read(SearchRead::timed_out(&[]).unwrap()),
            Step::Read(SearchRead::complete(&response).unwrap()),
        ]);
        let SearchOutcome::Identified(identity) =
            execute_search(&mut transport, DeviceId::new(expected).unwrap()).unwrap()
        else {
            panic!("fallback identity must complete")
        };
        assert_eq!(identity.kind(), SearchOperationKind::SingleFallback);
        assert_eq!(transport.operations.len(), MAX_SEARCH_ATTEMPTS);
        assert_eq!(transport.operations[0].settings().baud(), PRIMARY_BAUD);
        assert_eq!(transport.operations[1].settings().baud(), FALLBACK_BAUD);
        for operation in &transport.operations {
            assert_fixed_operation(*operation, expected);
        }
    }
}

#[test]
fn constants_make_the_total_nominal_deadline_explicit() {
    assert_eq!(SEARCH_ATTEMPT_TIMEOUT.as_millis(), 500);
    assert_eq!(SEARCH_DISCOVERY_BUDGET.as_millis(), 1_000);
    assert_eq!(
        SEARCH_ATTEMPT_TIMEOUT.as_millis() * MAX_SEARCH_ATTEMPTS as u128,
        SEARCH_DISCOVERY_BUDGET.as_millis()
    );
}
