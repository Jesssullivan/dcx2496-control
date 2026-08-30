use std::{collections::VecDeque, path::PathBuf, time::Duration};

use dcx_core::{
    SnapshotSection,
    protocol::{DUMP0_RESPONSE_LEN, DUMP1_RESPONSE_LEN, DeviceId},
};
use dcx_transport::{
    Known38400SearchOutcome, RepeatPacer, SearchExecutionError, SearchOperationKind, SearchOutcome,
    execute_known_38400_search, execute_search, snapshot::execute_persistent_snapshot,
};

use super::*;

const SYNTHETIC_PATH: &str = "/dev/cu.usbserial-SYNTHETIC";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Open,
    SnapshotTermios,
    SnapshotControlLines,
    Configure(u32),
    Write(usize),
    BytesAvailable,
    WaitReadable,
    Read(usize),
    RestoreTermios,
    VerifyTermiosRestore,
    RestoreControlLines,
    VerifyControlLinesRestore,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FakeTermios(u8);

struct FakeBackend {
    now: Duration,
    calls: Vec<Call>,
    inbound: VecDeque<u8>,
    available_script: VecDeque<usize>,
    wait_script: VecDeque<(bool, Duration)>,
    short_write: Option<usize>,
    fail_at: Option<CarrierStage>,
    advance_at: Option<(CarrierStage, Duration)>,
    opened: bool,
    written: bool,
    read_since_write: usize,
    read_goal: usize,
    writes: Vec<Vec<u8>>,
    preexisting_input: usize,
    preexisting_script: VecDeque<usize>,
    termios_readback: FakeTermios,
    control_lines_readback: i32,
}

impl FakeBackend {
    fn with_inbound(bytes: impl IntoIterator<Item = u8>) -> Self {
        Self {
            now: Duration::ZERO,
            calls: Vec::new(),
            inbound: bytes.into_iter().collect(),
            available_script: VecDeque::new(),
            wait_script: VecDeque::new(),
            short_write: None,
            fail_at: None,
            advance_at: None,
            opened: false,
            written: false,
            read_since_write: 0,
            read_goal: 0,
            writes: Vec::new(),
            preexisting_input: 0,
            preexisting_script: VecDeque::new(),
            termios_readback: FakeTermios(8),
            control_lines_readback: 0x2496,
        }
    }

    fn step(&mut self, stage: CarrierStage, call: Call) -> Result<(), SystemFault> {
        self.calls.push(call);
        if let Some((advance_stage, duration)) = self.advance_at
            && advance_stage == stage
        {
            self.now += duration;
        }
        if self.fail_at == Some(stage) {
            return Err(SystemFault::new(9_999));
        }
        Ok(())
    }
}

impl SerialBackend for FakeBackend {
    type TermiosSnapshot = FakeTermios;

    fn monotonic_now(&mut self) -> Duration {
        self.now
    }

    fn open_exclusive_noctty(&mut self, _path: &Path) -> Result<(), SystemFault> {
        self.step(CarrierStage::OpenExclusive, Call::Open)?;
        self.opened = true;
        Ok(())
    }

    fn snapshot_termios(&mut self) -> Result<Self::TermiosSnapshot, SystemFault> {
        self.step(CarrierStage::SnapshotTermios, Call::SnapshotTermios)?;
        Ok(FakeTermios(8))
    }

    fn snapshot_control_lines(&mut self) -> Result<i32, SystemFault> {
        self.step(
            CarrierStage::SnapshotControlLines,
            Call::SnapshotControlLines,
        )?;
        Ok(0x2496)
    }

    fn configure(
        &mut self,
        snapshot: &Self::TermiosSnapshot,
        baud: u32,
    ) -> Result<(), SystemFault> {
        assert_eq!(snapshot, &FakeTermios(8));
        self.step(CarrierStage::Configure, Call::Configure(baud))
    }

    fn write_once(&mut self, bytes: &[u8]) -> Result<usize, SystemFault> {
        self.step(CarrierStage::Write, Call::Write(bytes.len()))?;
        self.writes.push(bytes.to_vec());
        self.read_goal = match bytes.get(6).copied() {
            Some(0x40) => SEARCH_RESPONSE_LIMIT,
            Some(0x50) if bytes.get(9) == Some(&0) => DUMP0_RESPONSE_LEN,
            Some(0x50) if bytes.get(9) == Some(&1) => DUMP1_RESPONSE_LEN,
            Some(0x20 | 0x3f) => 0,
            _ => panic!("test carrier received an unexpected typed request"),
        };
        let written = self.short_write.unwrap_or(bytes.len());
        self.written = self.read_goal != 0;
        self.read_since_write = 0;
        Ok(written)
    }

    fn bytes_available(&mut self) -> Result<usize, SystemFault> {
        let stage = if self.written {
            CarrierStage::BytesAvailable
        } else {
            CarrierStage::CheckPreexistingInput
        };
        self.step(stage, Call::BytesAvailable)?;
        if !self.written {
            return Ok(self
                .preexisting_script
                .pop_front()
                .unwrap_or(self.preexisting_input));
        }
        Ok(self
            .available_script
            .pop_front()
            .unwrap_or(self.inbound.len()))
    }

    fn wait_readable(&mut self, remaining: Duration) -> Result<bool, SystemFault> {
        self.step(CarrierStage::WaitReadable, Call::WaitReadable)?;
        let (ready, advance) = self.wait_script.pop_front().unwrap_or((false, remaining));
        self.now += advance.min(remaining);
        Ok(ready)
    }

    fn read_once(&mut self, bytes: &mut [u8]) -> Result<ReadProgress, SystemFault> {
        self.step(CarrierStage::Read, Call::Read(bytes.len()))?;
        if self.inbound.is_empty() {
            return Ok(ReadProgress::WouldBlock);
        }
        let count = bytes.len().min(self.inbound.len());
        for slot in &mut bytes[..count] {
            *slot = self.inbound.pop_front().expect("length checked");
        }
        self.read_since_write += count;
        if self.read_since_write >= self.read_goal {
            self.written = false;
        }
        Ok(ReadProgress::Bytes(count))
    }

    fn restore_termios(&mut self, snapshot: &Self::TermiosSnapshot) -> Result<(), SystemFault> {
        assert_eq!(snapshot, &FakeTermios(8));
        self.step(CarrierStage::RestoreTermios, Call::RestoreTermios)
    }

    fn verify_termios_restore(
        &mut self,
        snapshot: &Self::TermiosSnapshot,
    ) -> Result<bool, SystemFault> {
        self.step(
            CarrierStage::VerifyTermiosRestore,
            Call::VerifyTermiosRestore,
        )?;
        Ok(&self.termios_readback == snapshot)
    }

    fn restore_control_lines(&mut self, state: i32) -> Result<(), SystemFault> {
        assert_eq!(state, 0x2496);
        self.step(CarrierStage::RestoreControlLines, Call::RestoreControlLines)
    }

    fn verify_control_lines_restore(&mut self, state: i32) -> Result<bool, SystemFault> {
        self.step(
            CarrierStage::VerifyControlLinesRestore,
            Call::VerifyControlLinesRestore,
        )?;
        Ok(self.control_lines_readback == state)
    }

    fn close(&mut self) {
        self.calls.push(Call::Close);
        self.opened = false;
        self.written = false;
        self.read_since_write = 0;
        self.read_goal = 0;
    }
}

fn synthetic_response(device: u8) -> [u8; SEARCH_RESPONSE_LIMIT] {
    let mut frame = [0_u8; SEARCH_RESPONSE_LIMIT];
    frame[..7].copy_from_slice(&[0xf0, 0x00, 0x20, 0x32, device, 0x0e, 0x00]);
    frame[7..25].copy_from_slice(b"SYNTHETIC-IDENTITY");
    frame[25] = 0xf7;
    frame
}

fn synthetic_dump(device: u8, part: u8) -> Vec<u8> {
    let length = match part {
        0 => DUMP0_RESPONSE_LEN,
        1 => DUMP1_RESPONSE_LEN,
        _ => panic!("synthetic dump part must be zero or one"),
    };
    let mut frame = vec![0; length];
    frame[..7].copy_from_slice(&[0xf0, 0, 0x20, 0x32, device, 0x0e, 0x10]);
    frame[12] = part;
    frame[length - 1] = 0xf7;
    frame
}

#[derive(Default)]
struct FastPacer {
    elapsed: Duration,
}

impl RepeatPacer for FastPacer {
    type Error = std::convert::Infallible;

    fn elapsed(&mut self) -> Duration {
        self.elapsed
    }

    fn wait(&mut self, minimum: Duration) -> Result<(), Self::Error> {
        self.elapsed += minimum;
        Ok(())
    }
}

fn binding() -> PrivateTtyBinding {
    PrivateTtyBinding::new(PathBuf::from(SYNTHETIC_PATH)).unwrap()
}

#[test]
fn primary_search_is_one_write_bounded_read_restore_and_close() {
    let response = synthetic_response(0);
    let backend = FakeBackend::with_inbound(response);
    let mut carrier = Carrier::new(binding(), backend);

    let SearchOutcome::Identified(identity) =
        execute_search(&mut carrier, DeviceId::new(0).unwrap()).unwrap()
    else {
        panic!("expected exact identity")
    };
    assert_eq!(identity.kind(), SearchOperationKind::Primary);
    assert_eq!(carrier.backend.inbound.len(), 0);
    assert_eq!(
        carrier.backend.calls,
        [
            Call::Open,
            Call::SnapshotTermios,
            Call::SnapshotControlLines,
            Call::BytesAvailable,
            Call::Configure(115_200),
            Call::BytesAvailable,
            Call::Write(8),
            Call::BytesAvailable,
            Call::Read(26),
            Call::BytesAvailable,
            Call::RestoreTermios,
            Call::VerifyTermiosRestore,
            Call::RestoreControlLines,
            Call::VerifyControlLinesRestore,
            Call::Close,
        ]
    );

    let receipt = &carrier.receipts()[0];
    assert_eq!(receipt.attempt, SanitizedAttemptKind::Primary);
    assert_eq!(receipt.tx_bytes, 8);
    assert_eq!(receipt.rx_bytes, 26);
    assert_eq!(receipt.deadline_millis, 500);
    assert_eq!(receipt.cleanup_reserve_millis, 25);
    assert_eq!(receipt.rx_digest, Some(Sha256Digest::of_bytes(&response)));
    assert_eq!(receipt.outcome, SanitizedAttemptOutcome::Complete);
    assert_eq!(
        receipt.termios_cleanup,
        CleanupDisposition::VerifiedRestored
    );
    assert_eq!(
        receipt.control_lines_cleanup,
        CleanupDisposition::VerifiedRestored
    );
    assert!(receipt.closed);
}

#[test]
fn preexisting_input_blocks_write_without_consuming_and_restores() {
    let mut backend = FakeBackend::with_inbound([]);
    backend.preexisting_input = 3;
    let mut carrier = Carrier::new(binding(), backend);

    assert!(matches!(
        execute_search(&mut carrier, DeviceId::new(0).unwrap()),
        Err(SearchExecutionError::Transport {
            source: DarwinCarrierError::PreexistingInput { queued: 3 },
            ..
        })
    ));
    assert!(
        !carrier
            .backend
            .calls
            .iter()
            .any(|call| matches!(call, Call::Write(_)))
    );
    assert!(
        !carrier
            .backend
            .calls
            .iter()
            .any(|call| matches!(call, Call::Configure(_)))
    );
    let receipt = &carrier.receipts()[0];
    assert_eq!(receipt.tx_bytes, 0);
    assert_eq!(receipt.rx_bytes, 0);
    assert_eq!(receipt.outcome, SanitizedAttemptOutcome::PreexistingInput);
    assert_eq!(receipt.termios_cleanup, CleanupDisposition::NotRequired);
    assert_eq!(
        receipt.control_lines_cleanup,
        CleanupDisposition::NotRequired
    );
    assert!(receipt.closed);
}

#[test]
fn input_arriving_during_configuration_blocks_write_and_restores() {
    let mut backend = FakeBackend::with_inbound([]);
    backend.preexisting_script = [0, 3].into_iter().collect();
    let mut carrier = Carrier::new(binding(), backend);

    assert!(matches!(
        execute_search(&mut carrier, DeviceId::new(0).unwrap()),
        Err(SearchExecutionError::Transport {
            source: DarwinCarrierError::PreexistingInput { queued: 3 },
            ..
        })
    ));
    assert!(
        !carrier
            .backend
            .calls
            .iter()
            .any(|call| matches!(call, Call::Write(_)))
    );
    let receipt = &carrier.receipts()[0];
    assert_eq!(receipt.tx_bytes, 0);
    assert_eq!(receipt.rx_bytes, 0);
    assert_eq!(receipt.outcome, SanitizedAttemptOutcome::PreexistingInput);
    assert_eq!(
        receipt.termios_cleanup,
        CleanupDisposition::VerifiedRestored
    );
    assert_eq!(
        receipt.control_lines_cleanup,
        CleanupDisposition::VerifiedRestored
    );
    assert!(receipt.closed);
}

#[test]
fn two_empty_timeouts_use_only_the_typed_fallback_and_close_twice() {
    let backend = FakeBackend::with_inbound([]);
    let mut carrier = Carrier::new(binding(), backend);

    assert!(matches!(
        execute_search(&mut carrier, DeviceId::new(0).unwrap()).unwrap(),
        SearchOutcome::Exhausted
    ));
    assert_eq!(carrier.receipts().len(), 2);
    assert_eq!(carrier.receipts()[0].baud, 115_200);
    assert_eq!(carrier.receipts()[1].baud, 38_400);
    assert_eq!(carrier.receipts()[0].rx_bytes, 0);
    assert_eq!(carrier.receipts()[1].rx_bytes, 0);
    assert_eq!(
        carrier
            .backend
            .calls
            .iter()
            .filter(|call| **call == Call::Write(8))
            .count(),
        2
    );
    assert_eq!(
        carrier
            .backend
            .calls
            .iter()
            .filter(|call| **call == Call::Close)
            .count(),
        2
    );
    assert!(carrier.receipts().iter().all(|receipt| {
        receipt.outcome == SanitizedAttemptOutcome::TimedOut
            && receipt.elapsed_micros == 475_000
            && receipt.closed
    }));
}

#[test]
fn partial_timeout_stops_without_fallback_and_keeps_only_a_digest() {
    let mut backend = FakeBackend::with_inbound(synthetic_response(0)[..5].iter().copied());
    backend.available_script = [5, 0].into_iter().collect();
    let mut carrier = Carrier::new(binding(), backend);

    assert!(matches!(
        execute_search(&mut carrier, DeviceId::new(0).unwrap()),
        Err(SearchExecutionError::PartialTimeout { received: 5, .. })
    ));
    assert_eq!(carrier.receipts().len(), 1);
    let receipt = &carrier.receipts()[0];
    assert_eq!(receipt.outcome, SanitizedAttemptOutcome::TimedOut);
    assert_eq!(receipt.rx_bytes, 5);
    assert!(receipt.rx_digest.is_some());
    assert!(receipt.closed);
}

#[test]
fn one_exact_request_echo_is_removed_before_response_validation() {
    let response = synthetic_response(0);
    let mut inbound = [0xf0, 0x00, 0x20, 0x32, 0x20, 0x0e, 0x40, 0xf7].to_vec();
    inbound.extend_from_slice(&response);
    let backend = FakeBackend::with_inbound(inbound);
    let mut carrier = Carrier::new(binding(), backend);

    assert!(execute_search(&mut carrier, DeviceId::new(0).unwrap()).is_ok());
    assert!(carrier.backend.inbound.is_empty());
    assert_eq!(carrier.receipts()[0].request_echo_bytes, SEARCH_REQUEST_LEN);
    assert_eq!(carrier.receipts()[0].rx_bytes, SEARCH_RESPONSE_LIMIT);
    assert_eq!(
        carrier.receipts()[0].outcome,
        SanitizedAttemptOutcome::Complete
    );
}

#[test]
fn input_above_one_echo_and_one_response_is_rejected_without_consuming() {
    let backend = FakeBackend::with_inbound([0_u8; SEARCH_WIRE_LIMIT + 1]);
    let mut carrier = Carrier::new(binding(), backend);

    assert!(matches!(
        execute_search(&mut carrier, DeviceId::new(0).unwrap()),
        Err(SearchExecutionError::Transport {
            source: DarwinCarrierError::Overflow {
                received: 0,
                queued,
            },
            ..
        }) if queued == SEARCH_WIRE_LIMIT + 1
    ));
    assert_eq!(carrier.backend.inbound.len(), SEARCH_WIRE_LIMIT + 1);
    assert!(
        !carrier
            .backend
            .calls
            .iter()
            .any(|call| matches!(call, Call::Read(_)))
    );
}

#[test]
fn a_short_write_is_never_retried_and_always_restores() {
    let mut backend = FakeBackend::with_inbound([]);
    backend.short_write = Some(7);
    let mut carrier = Carrier::new(binding(), backend);

    assert!(matches!(
        execute_search(&mut carrier, DeviceId::new(0).unwrap()),
        Err(SearchExecutionError::Transport {
            source: DarwinCarrierError::ShortWrite {
                written: 7,
                expected: 8,
            },
            ..
        })
    ));
    assert_eq!(
        carrier
            .backend
            .calls
            .iter()
            .filter(|call| **call == Call::Write(8))
            .count(),
        1
    );
    assert!(carrier.backend.calls.ends_with(&[
        Call::RestoreTermios,
        Call::VerifyTermiosRestore,
        Call::RestoreControlLines,
        Call::VerifyControlLinesRestore,
        Call::Close,
    ]));
}

#[test]
fn deadline_after_configuration_blocks_the_write_at_preexisting_input_check() {
    let mut backend = FakeBackend::with_inbound([]);
    backend.advance_at = Some((CarrierStage::Configure, Duration::from_millis(500)));
    let mut carrier = Carrier::new(binding(), backend);

    assert!(matches!(
        execute_search(&mut carrier, DeviceId::new(0).unwrap()),
        Err(SearchExecutionError::Transport {
            source: DarwinCarrierError::Deadline {
                stage: CarrierStage::CheckPreexistingInput,
            },
            ..
        })
    ));
    assert!(
        !carrier
            .backend
            .calls
            .iter()
            .any(|call| matches!(call, Call::Write(_)))
    );
    assert_eq!(carrier.receipts()[0].elapsed_micros, 500_000);
    assert!(carrier.receipts()[0].closed);
}

#[test]
fn cleanup_that_crosses_the_whole_attempt_budget_fails_closed() {
    let mut backend = FakeBackend::with_inbound(synthetic_response(0));
    backend.advance_at = Some((
        CarrierStage::RestoreControlLines,
        Duration::from_millis(501),
    ));
    let mut carrier = Carrier::new(binding(), backend);

    assert!(matches!(
        execute_search(&mut carrier, DeviceId::new(0).unwrap()),
        Err(SearchExecutionError::Transport {
            source: DarwinCarrierError::Deadline {
                stage: CarrierStage::Close,
            },
            ..
        })
    ));
    assert!(carrier.receipts()[0].closed);
    assert_eq!(
        carrier.receipts()[0].outcome,
        SanitizedAttemptOutcome::DeadlineExceeded
    );
}

#[test]
fn every_post_snapshot_failure_path_closes_and_restores_both_states() {
    for stage in [
        CarrierStage::Configure,
        CarrierStage::Write,
        CarrierStage::BytesAvailable,
        CarrierStage::Read,
        CarrierStage::WaitReadable,
    ] {
        let inbound = if stage == CarrierStage::WaitReadable {
            Vec::new()
        } else {
            synthetic_response(0).to_vec()
        };
        let mut backend = FakeBackend::with_inbound(inbound);
        backend.fail_at = Some(stage);
        let mut carrier = Carrier::new(binding(), backend);

        assert!(execute_search(&mut carrier, DeviceId::new(0).unwrap()).is_err());
        assert!(carrier.backend.calls.ends_with(&[
            Call::RestoreTermios,
            Call::VerifyTermiosRestore,
            Call::RestoreControlLines,
            Call::VerifyControlLinesRestore,
            Call::Close,
        ]));
        assert!(carrier.receipts()[0].closed);
    }
}

#[test]
fn cleanup_failure_is_terminal_but_still_runs_other_restore_and_close() {
    for failing_stage in [
        CarrierStage::RestoreTermios,
        CarrierStage::VerifyTermiosRestore,
        CarrierStage::RestoreControlLines,
        CarrierStage::VerifyControlLinesRestore,
    ] {
        let mut backend = FakeBackend::with_inbound(synthetic_response(0));
        backend.fail_at = Some(failing_stage);
        let mut carrier = Carrier::new(binding(), backend);

        assert!(matches!(
            execute_search(&mut carrier, DeviceId::new(0).unwrap()),
            Err(SearchExecutionError::Transport {
                source: DarwinCarrierError::Cleanup { .. },
                ..
            })
        ));
        assert_eq!(carrier.backend.calls.last(), Some(&Call::Close));
        assert!(carrier.backend.calls.contains(&Call::RestoreTermios));
        assert!(carrier.backend.calls.contains(&Call::RestoreControlLines));
        assert!(carrier.receipts()[0].closed);
        assert_eq!(
            carrier.receipts()[0].outcome,
            SanitizedAttemptOutcome::CleanupFailed
        );
    }
}

#[test]
fn cleanup_readback_mismatch_is_terminal_and_never_reports_verified_restore() {
    for (termios_readback, control_lines_readback) in
        [(FakeTermios(9), 0x2496), (FakeTermios(8), 0x2497)]
    {
        let mut backend = FakeBackend::with_inbound(synthetic_response(0));
        backend.termios_readback = termios_readback;
        backend.control_lines_readback = control_lines_readback;
        let mut carrier = Carrier::new(binding(), backend);

        assert!(matches!(
            execute_search(&mut carrier, DeviceId::new(0).unwrap()),
            Err(SearchExecutionError::Transport {
                source: DarwinCarrierError::Cleanup { .. },
                ..
            })
        ));
        let receipt = &carrier.receipts()[0];
        assert_eq!(receipt.outcome, SanitizedAttemptOutcome::CleanupFailed);
        assert!(
            receipt.termios_cleanup == CleanupDisposition::Failed
                || receipt.control_lines_cleanup == CleanupDisposition::Failed
        );
        assert!(receipt.closed);
    }
}

#[test]
fn generated_read_partitions_never_cross_the_26_byte_ceiling() {
    let response = synthetic_response(0);
    for maximum_chunk in 1..=SEARCH_RESPONSE_LIMIT {
        let mut backend = FakeBackend::with_inbound(response);
        let mut remaining = SEARCH_RESPONSE_LIMIT;
        while remaining != 0 {
            let chunk = remaining.min(maximum_chunk);
            backend.available_script.push_back(chunk);
            remaining -= chunk;
        }
        backend.available_script.push_back(0);
        let mut carrier = Carrier::new(binding(), backend);

        assert!(execute_search(&mut carrier, DeviceId::new(0).unwrap()).is_ok());
        let read_total: usize = carrier
            .backend
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::Read(count) => Some(*count),
                _ => None,
            })
            .sum();
        assert_eq!(read_total, SEARCH_RESPONSE_LIMIT);
        assert!(
            carrier
                .backend
                .calls
                .iter()
                .filter_map(|call| match call {
                    Call::Read(count) => Some(*count),
                    _ => None,
                })
                .all(|count| count <= maximum_chunk)
        );
    }
}

#[test]
fn binding_and_receipt_debug_output_never_expose_the_path_or_payload() {
    let binding = binding();
    let debug = format!("{binding:?}");
    assert!(!debug.contains(SYNTHETIC_PATH));
    assert!(debug.contains("[redacted]"));

    let backend = FakeBackend::with_inbound(synthetic_response(0));
    let mut carrier = Carrier::new(binding, backend);
    execute_search(&mut carrier, DeviceId::new(0).unwrap()).unwrap();
    let json = serde_json::to_string(&carrier.receipts()[0]).unwrap();
    assert!(!json.contains(SYNTHETIC_PATH));
    assert!(!json.contains("SYNTHETIC-IDENTITY"));
    assert!(json.contains("rxDigest"));
}

#[test]
fn explicit_binding_rejects_non_callout_paths() {
    assert!(matches!(
        PrivateTtyBinding::new(PathBuf::from("not-a-callout")),
        Err(BindingError::UnsupportedPrivatePath)
    ));
}

#[test]
fn draining_sanitized_receipts_does_not_retain_a_duplicate() {
    let backend = FakeBackend::with_inbound(synthetic_response(0));
    let mut carrier = Carrier::new(binding(), backend);
    execute_search(&mut carrier, DeviceId::new(0).unwrap()).unwrap();
    assert_eq!(carrier.take_receipts().len(), 1);
    assert!(carrier.receipts().is_empty());
}

#[test]
fn persistent_carrier_reuses_one_open_for_initial_search_and_nine_repeats() {
    let response = synthetic_response(0);
    let inbound = (0..10).flat_map(|_| response).collect::<Vec<_>>();
    let mut backend = FakeBackend::with_inbound(inbound);
    backend.available_script = (0..10).map(|_| SEARCH_RESPONSE_LIMIT).collect();
    let mut carrier = PersistentCarrier::open(binding(), backend, FALLBACK_BAUD).unwrap();
    let expected = DeviceId::new(0).unwrap();

    for _ in 0..10 {
        assert!(matches!(
            execute_known_38400_search(&mut carrier, expected).unwrap(),
            Known38400SearchOutcome::Identified(_)
        ));
    }

    assert_eq!(carrier.receipts().len(), 10);
    assert_eq!(
        carrier
            .backend
            .calls
            .iter()
            .filter(|call| **call == Call::Open)
            .count(),
        1
    );
    assert_eq!(
        carrier
            .backend
            .calls
            .iter()
            .filter(|call| matches!(call, Call::Configure(38_400)))
            .count(),
        1
    );
    assert_eq!(
        carrier
            .backend
            .calls
            .iter()
            .filter(|call| **call == Call::Write(8))
            .count(),
        10
    );
    assert!(!carrier.backend.calls.contains(&Call::Close));
    assert!(carrier.receipts().iter().all(|receipt| {
        receipt.outcome == SanitizedAttemptOutcome::Complete
            && receipt.termios_cleanup == CleanupDisposition::NotRequired
            && receipt.control_lines_cleanup == CleanupDisposition::NotRequired
            && !receipt.closed
    }));

    let attempts = carrier.take_receipts();
    assert_eq!(attempts.len(), 10);
    let session = carrier.finish().unwrap();
    assert_eq!(session.baud, FALLBACK_BAUD);
    assert_eq!(session.attempt_count, 10);
    assert_eq!(
        session.termios_cleanup,
        CleanupDisposition::VerifiedRestored
    );
    assert_eq!(
        session.control_lines_cleanup,
        CleanupDisposition::VerifiedRestored
    );
    assert!(session.closed);
}

#[test]
fn persistent_carrier_captures_exact_search_and_dump_lengths_before_verified_close() {
    let identity = synthetic_response(0);
    let dump0 = synthetic_dump(0, 0);
    let dump1 = synthetic_dump(0, 1);
    let mut inbound = Vec::new();
    for _ in 0..10 {
        inbound.extend_from_slice(&identity);
    }
    inbound.extend_from_slice(&dump0);
    inbound.extend_from_slice(&dump1);

    let mut backend = FakeBackend::with_inbound(inbound);
    backend.available_script = (0..10)
        .map(|_| SEARCH_RESPONSE_LIMIT)
        .chain([DUMP0_RESPONSE_LEN, DUMP1_RESPONSE_LEN])
        .collect();
    let carrier = PersistentCarrier::open(binding(), backend, FALLBACK_BAUD).unwrap();
    let mut pacer = FastPacer::default();

    let captured =
        execute_persistent_snapshot(carrier, &mut pacer, DeviceId::new(0).unwrap()).unwrap();

    assert_eq!(captured.valid_search_count(), 10);
    assert!(captured.close_verified());
    assert_eq!(
        captured.snapshot().frame(SnapshotSection::Identity),
        identity
    );
    assert_eq!(captured.snapshot().frame(SnapshotSection::Dump0), dump0);
    assert_eq!(captured.snapshot().frame(SnapshotSection::Dump1), dump1);
}

#[test]
fn persistent_carrier_rejects_a_different_baud_without_writing() {
    let backend = FakeBackend::with_inbound([]);
    let mut carrier = PersistentCarrier::open(binding(), backend, FALLBACK_BAUD).unwrap();

    assert!(matches!(
        execute_search(&mut carrier, DeviceId::new(0).unwrap()),
        Err(SearchExecutionError::Transport {
            source: DarwinCarrierError::OperationMismatch {
                expected_baud: FALLBACK_BAUD,
                actual_baud: 115_200,
            },
            ..
        })
    ));
    assert!(
        !carrier
            .backend
            .calls
            .iter()
            .any(|call| matches!(call, Call::Write(_)))
    );
    assert_eq!(
        carrier.receipts()[0].outcome,
        SanitizedAttemptOutcome::OperationRejected
    );
    carrier.finish().unwrap();
}
