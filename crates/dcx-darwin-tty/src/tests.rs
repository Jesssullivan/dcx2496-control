use std::{cell::RefCell, collections::VecDeque, path::PathBuf, rc::Rc, time::Duration};

use dcx_core::{
    SnapshotSection,
    protocol::{
        DUMP0_RESPONSE_LEN, DUMP1_RESPONSE_LEN, DeviceId, DirectParameterAction, RemoteMode,
    },
};
use dcx_transport::{
    Known38400SearchError, Known38400SearchOutcome, RepeatPacer, SearchExecutionError,
    SearchOperationKind, SearchOutcome, execute_known_38400_search, execute_search,
    snapshot::{SNAPSHOT_OPERATION_TIMEOUT, execute_persistent_snapshot},
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
    DiscardInput,
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
    post_response_input: usize,
    post_response_input_after_wait: Option<usize>,
    preexisting_script: VecDeque<usize>,
    termios_readback: FakeTermios,
    control_lines_readback: i32,
    shared_calls: Option<Rc<RefCell<Vec<Call>>>>,
    shared_writes: Option<Rc<RefCell<Vec<Vec<u8>>>>>,
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
            post_response_input: 0,
            post_response_input_after_wait: None,
            preexisting_script: VecDeque::new(),
            termios_readback: FakeTermios(8),
            control_lines_readback: 0x2496,
            shared_calls: None,
            shared_writes: None,
        }
    }

    fn with_shared_calls(mut self, shared_calls: Rc<RefCell<Vec<Call>>>) -> Self {
        self.shared_calls = Some(shared_calls);
        self
    }

    fn with_shared_writes(mut self, shared_writes: Rc<RefCell<Vec<Vec<u8>>>>) -> Self {
        self.shared_writes = Some(shared_writes);
        self
    }

    fn record(&mut self, call: Call) {
        self.calls.push(call);
        if let Some(shared_calls) = &self.shared_calls {
            shared_calls.borrow_mut().push(call);
        }
    }

    fn step(&mut self, stage: CarrierStage, call: Call) -> Result<(), SystemFault> {
        self.record(call);
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
        if let Some(shared_writes) = &self.shared_writes {
            shared_writes.borrow_mut().push(bytes.to_vec());
        }
        self.read_goal = match bytes.get(6).copied() {
            Some(0x40)
                if self
                    .inbound
                    .iter()
                    .take(bytes.len())
                    .copied()
                    .eq(bytes.iter().copied()) =>
            {
                SEARCH_REQUEST_LEN + SEARCH_RESPONSE_LIMIT
            }
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
        let remaining = self.read_goal.saturating_sub(self.read_since_write);
        if remaining == 0 {
            return Ok(self.post_response_input);
        }
        Ok(self
            .available_script
            .pop_front()
            .unwrap_or(self.inbound.len())
            .min(remaining))
    }

    fn wait_readable(&mut self, remaining: Duration) -> Result<bool, SystemFault> {
        self.step(CarrierStage::WaitReadable, Call::WaitReadable)?;
        let (ready, advance) = self.wait_script.pop_front().unwrap_or((false, remaining));
        self.now += advance.min(remaining);
        if ready && let Some(queued) = self.post_response_input_after_wait.take() {
            self.post_response_input = queued;
        }
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
        if self.read_since_write > self.read_goal {
            self.post_response_input = self.post_response_input.saturating_sub(count);
        }
        Ok(ReadProgress::Bytes(count))
    }

    fn discard_input(&mut self) -> Result<(), SystemFault> {
        self.step(CarrierStage::DiscardInput, Call::DiscardInput)?;
        self.inbound.clear();
        self.available_script.clear();
        self.preexisting_script.clear();
        self.preexisting_input = 0;
        self.post_response_input = 0;
        self.written = false;
        self.read_since_write = 0;
        self.read_goal = 0;
        Ok(())
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
        self.record(Call::Close);
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

const fn synthetic_search_request() -> [u8; SEARCH_REQUEST_LEN] {
    [0xf0, 0x00, 0x20, 0x32, 0x20, 0x0e, 0x40, 0xf7]
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
        carrier
            .backend
            .calls
            .iter()
            .filter(|call| **call == Call::Write(8))
            .count(),
        1
    );
    assert_eq!(
        carrier
            .backend
            .calls
            .iter()
            .filter(|call| **call == Call::Read(1))
            .count(),
        SEARCH_RESPONSE_LIMIT
    );
    assert!(carrier.backend.calls.starts_with(&[
        Call::Open,
        Call::SnapshotTermios,
        Call::SnapshotControlLines,
        Call::BytesAvailable,
        Call::Configure(115_200),
        Call::BytesAvailable,
        Call::Write(8),
    ]));
    assert!(carrier.backend.calls.ends_with(&[
        Call::RestoreTermios,
        Call::VerifyTermiosRestore,
        Call::RestoreControlLines,
        Call::VerifyControlLinesRestore,
        Call::Close,
    ]));

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
fn preexisting_input_blocks_write_then_discards_only_during_terminal_cleanup() {
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
    assert_eq!(
        carrier
            .backend
            .calls
            .iter()
            .filter(|call| **call == Call::DiscardInput)
            .count(),
        1
    );
    assert!(
        carrier
            .backend
            .calls
            .windows(2)
            .any(|calls| calls == [Call::DiscardInput, Call::BytesAvailable])
    );
    let discard = carrier
        .backend
        .calls
        .iter()
        .position(|call| *call == Call::DiscardInput)
        .unwrap();
    let close = carrier
        .backend
        .calls
        .iter()
        .position(|call| *call == Call::Close)
        .unwrap();
    assert!(discard < close);
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
fn input_discard_is_one_call_one_readback_and_reserve_bounded() {
    let mut backend = FakeBackend::with_inbound([]);
    backend.preexisting_input = 7;
    assert_eq!(
        recover_input_state(&mut backend, true),
        CleanupDisposition::VerifiedRestored
    );
    assert_eq!(backend.calls, [Call::DiscardInput, Call::BytesAvailable]);

    let mut expired = FakeBackend::with_inbound([]);
    expired.preexisting_input = 7;
    expired.advance_at = Some((CarrierStage::DiscardInput, SEARCH_CLEANUP_RESERVE));
    assert_eq!(
        recover_input_state(&mut expired, true),
        CleanupDisposition::Failed
    );
    assert_eq!(expired.calls, [Call::DiscardInput]);
}

#[test]
fn failed_input_discard_is_terminal_but_still_closes() {
    let mut backend = FakeBackend::with_inbound([]);
    backend.preexisting_script = [0, 7].into_iter().collect();
    backend.fail_at = Some(CarrierStage::DiscardInput);
    let mut carrier = Carrier::new(binding(), backend);

    assert!(matches!(
        execute_search(&mut carrier, DeviceId::new(0).unwrap()),
        Err(SearchExecutionError::Transport {
            source: DarwinCarrierError::Cleanup {
                primary: Some(CarrierFailureKind::PreexistingInput),
                input_failed: true,
                termios_failed: false,
                control_lines_failed: false,
            },
            ..
        })
    ));
    assert!(carrier.backend.calls.ends_with(&[
        Call::DiscardInput,
        Call::RestoreTermios,
        Call::VerifyTermiosRestore,
        Call::RestoreControlLines,
        Call::VerifyControlLinesRestore,
        Call::Close,
    ]));
    assert_eq!(
        carrier.receipts()[0].outcome,
        SanitizedAttemptOutcome::CleanupFailed
    );
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
    backend.available_script = (1..=5).rev().chain([0]).collect();
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
    let mut inbound = synthetic_search_request().to_vec();
    inbound.extend_from_slice(&response);
    let backend = FakeBackend::with_inbound(inbound);
    let mut carrier = Carrier::new(binding(), backend);

    assert!(execute_search(&mut carrier, DeviceId::new(0).unwrap()).is_ok());
    assert!(carrier.backend.inbound.is_empty());
    assert_eq!(carrier.receipts()[0].request_echo_bytes, SEARCH_REQUEST_LEN);
    assert_eq!(
        carrier.receipts()[0].wire_bytes,
        SEARCH_REQUEST_LEN + SEARCH_RESPONSE_LIMIT
    );
    assert_eq!(carrier.receipts()[0].rx_bytes, SEARCH_RESPONSE_LIMIT);
    assert_eq!(
        carrier.receipts()[0].outcome,
        SanitizedAttemptOutcome::Complete
    );
}

#[test]
fn chunked_request_echo_and_response_preserve_one_typed_identity() {
    let response = synthetic_response(0);
    let mut inbound = synthetic_search_request().to_vec();
    inbound.extend_from_slice(&response);
    let mut backend = FakeBackend::with_inbound(inbound);
    backend.available_script = (1..=SEARCH_REQUEST_LEN)
        .rev()
        .chain((1..=SEARCH_RESPONSE_LIMIT).rev())
        .chain([0])
        .collect();
    let mut carrier = Carrier::new(binding(), backend);

    assert!(execute_search(&mut carrier, DeviceId::new(0).unwrap()).is_ok());
    assert_eq!(carrier.receipts()[0].request_echo_bytes, SEARCH_REQUEST_LEN);
    assert_eq!(carrier.receipts()[0].rx_bytes, SEARCH_RESPONSE_LIMIT);
}

#[test]
fn exact_request_echo_without_a_response_is_an_empty_timeout() {
    let mut backend = FakeBackend::with_inbound(synthetic_search_request());
    backend.available_script = (1..=SEARCH_REQUEST_LEN).rev().chain([0]).collect();
    let mut carrier = Carrier::new(binding(), backend);

    assert!(matches!(
        execute_known_38400_search(&mut carrier, DeviceId::new(0).unwrap()).unwrap(),
        Known38400SearchOutcome::TimedOut
    ));
    assert_eq!(carrier.receipts()[0].request_echo_bytes, SEARCH_REQUEST_LEN);
    assert_eq!(carrier.receipts()[0].rx_bytes, 0);
    assert_eq!(
        carrier.receipts()[0].outcome,
        SanitizedAttemptOutcome::TimedOut
    );
}

#[test]
fn a_partial_trailing_frame_is_rejected_after_bounded_consumption() {
    let mut inbound = synthetic_response(0).to_vec();
    inbound.push(0);
    let mut backend = FakeBackend::with_inbound(inbound);
    backend.post_response_input = 1;
    let mut carrier = Carrier::new(binding(), backend);

    assert!(matches!(
        execute_search(&mut carrier, DeviceId::new(0).unwrap()),
        Err(SearchExecutionError::Transport {
            source: DarwinCarrierError::UnexpectedTrailingFrame { received: 1 },
            ..
        })
    ));
    assert!(carrier.backend.inbound.is_empty());
    assert_eq!(carrier.receipts()[0].wire_bytes, SEARCH_RESPONSE_LIMIT + 1);
    assert_eq!(carrier.receipts()[0].unexpected_trailing_bytes, 1);
    assert_eq!(
        carrier
            .backend
            .calls
            .iter()
            .filter(|call| **call == Call::DiscardInput)
            .count(),
        1
    );
}

#[test]
fn one_exact_duplicate_response_is_consumed_and_reported() {
    let response = synthetic_response(0);
    let inbound = response.into_iter().chain(response).collect::<Vec<_>>();
    let mut backend = FakeBackend::with_inbound(inbound);
    backend.post_response_input = SEARCH_RESPONSE_LIMIT;
    let mut carrier = Carrier::new(binding(), backend);

    assert!(execute_search(&mut carrier, DeviceId::new(0).unwrap()).is_ok());
    assert!(carrier.backend.inbound.is_empty());
    assert_eq!(carrier.receipts()[0].wire_bytes, SEARCH_RESPONSE_LIMIT * 2);
    assert_eq!(carrier.receipts()[0].duplicate_response_count, 1);
}

#[test]
fn one_late_exact_duplicate_is_settled_inside_the_search_deadline() {
    let response = synthetic_response(0);
    let inbound = response.into_iter().chain(response).collect::<Vec<_>>();
    let mut backend = FakeBackend::with_inbound(inbound);
    backend.post_response_input_after_wait = Some(SEARCH_RESPONSE_LIMIT);
    backend.wait_script = [
        (true, Duration::from_millis(10)),
        (false, Duration::from_millis(465)),
    ]
    .into_iter()
    .collect();
    let mut carrier = Carrier::new(binding(), backend);

    assert!(execute_search(&mut carrier, DeviceId::new(0).unwrap()).is_ok());
    assert!(carrier.backend.inbound.is_empty());
    assert_eq!(carrier.receipts()[0].wire_bytes, SEARCH_RESPONSE_LIMIT * 2);
    assert_eq!(carrier.receipts()[0].duplicate_response_count, 1);
    assert_eq!(carrier.receipts()[0].elapsed_micros, 475_000);
}

#[test]
fn multiple_duplicates_inside_the_search_deadline_are_consumed_and_reported() {
    let response = synthetic_response(0);
    let inbound = response
        .into_iter()
        .chain(response)
        .chain(response)
        .collect::<Vec<_>>();
    let mut backend = FakeBackend::with_inbound(inbound);
    backend.post_response_input = SEARCH_RESPONSE_LIMIT * 2;
    let mut carrier = Carrier::new(binding(), backend);

    assert!(execute_search(&mut carrier, DeviceId::new(0).unwrap()).is_ok());
    assert!(carrier.backend.inbound.is_empty());
    assert_eq!(carrier.receipts()[0].wire_bytes, SEARCH_RESPONSE_LIMIT * 3);
    assert_eq!(carrier.receipts()[0].duplicate_response_count, 2);
}

#[test]
fn a_different_frame_after_an_exact_duplicate_remains_terminal() {
    let response = synthetic_response(0);
    let inbound = response
        .into_iter()
        .chain(response)
        .chain(synthetic_response(1))
        .collect::<Vec<_>>();
    let mut backend = FakeBackend::with_inbound(inbound);
    backend.post_response_input = SEARCH_RESPONSE_LIMIT * 2;
    let mut carrier = Carrier::new(binding(), backend);

    assert!(matches!(
        execute_search(&mut carrier, DeviceId::new(0).unwrap()),
        Err(SearchExecutionError::Transport {
            source: DarwinCarrierError::UnexpectedTrailingFrame {
                received: SEARCH_RESPONSE_LIMIT,
            },
            ..
        })
    ));
    assert_eq!(carrier.receipts()[0].duplicate_response_count, 1);
    assert_eq!(
        carrier.receipts()[0].unexpected_trailing_bytes,
        SEARCH_RESPONSE_LIMIT
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
fn framed_reader_consumes_one_byte_at_a_time_through_the_terminator() {
    let response = synthetic_response(0);
    let backend = FakeBackend::with_inbound(response);
    let mut carrier = Carrier::new(binding(), backend);

    assert!(execute_search(&mut carrier, DeviceId::new(0).unwrap()).is_ok());
    let reads = carrier
        .backend
        .calls
        .iter()
        .filter_map(|call| match call {
            Call::Read(count) => Some(*count),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(reads, vec![1; SEARCH_RESPONSE_LIMIT]);
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
    assert!(json.contains("wireBytes"));
    assert!(json.contains("overflowQueuedBytes"));
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
fn persistent_open_discards_preexisting_input_after_rejection_and_preserves_primary_error() {
    let shared_calls = Rc::new(RefCell::new(Vec::new()));
    let mut backend = FakeBackend::with_inbound([]).with_shared_calls(Rc::clone(&shared_calls));
    backend.preexisting_input = 7;

    assert!(matches!(
        PersistentCarrier::open(binding(), backend, FALLBACK_BAUD),
        Err(DarwinCarrierError::PreexistingInput { queued: 7 })
    ));

    let calls = shared_calls.borrow();
    assert_eq!(
        calls.as_slice(),
        [
            Call::Open,
            Call::SnapshotTermios,
            Call::SnapshotControlLines,
            Call::BytesAvailable,
            Call::DiscardInput,
            Call::BytesAvailable,
            Call::Close,
        ]
    );
    assert!(!calls.iter().any(|call| matches!(call, Call::Write(_))));
}

#[test]
fn receive_direct_recovery_discards_without_reading_and_writes_one_closed_frame() {
    let mut backend = FakeBackend::with_inbound([0x01, 0x02, 0xff]);
    backend.preexisting_input = 12;
    let device = DeviceId::new(0).unwrap();

    let receipt = run_receive_direct_recovery(&binding(), &mut backend, device).unwrap();

    assert_eq!(
        backend.writes,
        [vec![0xf0, 0, 0x20, 0x32, 0, 0x0e, 0x3f, 0x04, 0, 0xf7]]
    );
    assert_eq!(
        backend
            .calls
            .iter()
            .filter(|call| matches!(call, Call::Write(_)))
            .count(),
        1
    );
    assert_eq!(
        backend
            .calls
            .iter()
            .filter(|call| **call == Call::DiscardInput)
            .count(),
        2
    );
    assert!(
        !backend
            .calls
            .iter()
            .any(|call| matches!(call, Call::Read(_)))
    );
    assert_eq!(receipt.device, device);
    assert_eq!(receipt.remote_mode, RemoteMode::ReceiveDirect);
    assert_eq!(receipt.baud, FALLBACK_BAUD);
    assert_eq!(receipt.tx_bytes, 10);
    assert_eq!(receipt.input_discard_count, 2);
    assert_eq!(
        receipt.termios_cleanup,
        CleanupDisposition::VerifiedRestored
    );
    assert_eq!(
        receipt.control_lines_cleanup,
        CleanupDisposition::VerifiedRestored
    );
    assert!(receipt.closed);
    let serialized = serde_json::to_string(&receipt).unwrap();
    assert!(!serialized.contains(SYNTHETIC_PATH));
    assert!(!serialized.contains("F0002032"));
}

#[test]
fn receive_direct_recovery_drains_resumed_input_until_quiet_then_restores_and_closes() {
    let mut backend = FakeBackend::with_inbound([]);
    backend.wait_script = [
        (true, Duration::from_millis(1)),
        (false, RECOVERY_QUIET_WINDOW),
    ]
    .into_iter()
    .collect();

    let receipt =
        run_receive_direct_recovery(&binding(), &mut backend, DeviceId::new(0).unwrap()).unwrap();

    assert_eq!(backend.writes.len(), 1);
    assert_eq!(
        backend
            .calls
            .iter()
            .filter(|call| **call == Call::DiscardInput)
            .count(),
        3
    );
    assert_eq!(receipt.input_discard_count, 3);
    assert!(
        !backend
            .calls
            .iter()
            .any(|call| matches!(call, Call::Read(_)))
    );
    assert!(backend.calls.contains(&Call::RestoreTermios));
    assert!(backend.calls.contains(&Call::VerifyTermiosRestore));
    assert!(backend.calls.contains(&Call::RestoreControlLines));
    assert!(backend.calls.contains(&Call::VerifyControlLinesRestore));
    assert_eq!(backend.calls.last(), Some(&Call::Close));
}

#[test]
fn receive_direct_recovery_fails_when_resumed_input_never_reaches_quiet() {
    let mut backend = FakeBackend::with_inbound([]);
    backend.wait_script = (0..19).map(|_| (true, RECOVERY_QUIET_WINDOW)).collect();

    assert_eq!(
        run_receive_direct_recovery(&binding(), &mut backend, DeviceId::new(0).unwrap()),
        Err(DarwinCarrierError::Deadline {
            stage: CarrierStage::WaitReadable,
        })
    );
    assert_eq!(backend.writes.len(), 1);
    assert!(
        backend
            .calls
            .iter()
            .filter(|call| **call == Call::DiscardInput)
            .count()
            > 2
    );
    assert!(
        !backend
            .calls
            .iter()
            .any(|call| matches!(call, Call::Read(_)))
    );
    assert!(backend.calls.contains(&Call::RestoreTermios));
    assert!(backend.calls.contains(&Call::VerifyTermiosRestore));
    assert!(backend.calls.contains(&Call::RestoreControlLines));
    assert!(backend.calls.contains(&Call::VerifyControlLinesRestore));
    assert_eq!(backend.calls.last(), Some(&Call::Close));
}

#[test]
fn receive_direct_recovery_post_config_failures_never_retry_and_always_restore() {
    let mut short_write = FakeBackend::with_inbound([]);
    short_write.short_write = Some(1);
    let mut insufficient_quiet_budget = FakeBackend::with_inbound([]);
    insufficient_quiet_budget.advance_at = Some((CarrierStage::Write, Duration::from_millis(460)));

    for (mut backend, expected) in [
        (
            short_write,
            DarwinCarrierError::ShortWrite {
                written: 1,
                expected: 10,
            },
        ),
        (
            insufficient_quiet_budget,
            DarwinCarrierError::Deadline {
                stage: CarrierStage::WaitReadable,
            },
        ),
    ] {
        assert_eq!(
            run_receive_direct_recovery(&binding(), &mut backend, DeviceId::new(0).unwrap()),
            Err(expected)
        );
        assert_eq!(backend.writes.len(), 1);
        assert_eq!(
            backend
                .calls
                .iter()
                .filter(|call| matches!(call, Call::Write(_)))
                .count(),
            1
        );
        assert!(
            !backend
                .calls
                .iter()
                .any(|call| matches!(call, Call::Read(_)))
        );
        assert!(backend.calls.contains(&Call::RestoreTermios));
        assert!(backend.calls.contains(&Call::VerifyTermiosRestore));
        assert!(backend.calls.contains(&Call::RestoreControlLines));
        assert!(backend.calls.contains(&Call::VerifyControlLinesRestore));
        assert_eq!(backend.calls.last(), Some(&Call::Close));
    }
}

#[test]
fn post_write_failure_taints_every_persistent_operation_until_consuming_cleanup() {
    let shared_calls = Rc::new(RefCell::new(Vec::new()));
    let mut backend = FakeBackend::with_inbound([]).with_shared_calls(Rc::clone(&shared_calls));
    backend.short_write = Some(1);
    let mut carrier = PersistentCarrier::open(binding(), backend, FALLBACK_BAUD).unwrap();
    let expected = DeviceId::new(0).unwrap();

    assert!(matches!(
        execute_known_38400_search(&mut carrier, expected),
        Err(Known38400SearchError::Transport {
            source: DarwinCarrierError::ShortWrite {
                written: 1,
                expected: 8
            },
        })
    ));
    carrier.backend.short_write = None;
    let calls_after_fault = shared_calls.borrow().len();
    assert!(matches!(
        execute_known_38400_search(&mut carrier, expected),
        Err(Known38400SearchError::Transport {
            source: DarwinCarrierError::InputRecoveryRequired,
        })
    ));
    assert!(matches!(
        carrier.write_remote_mode_command(RemoteModeCommand::new(
            expected,
            RemoteMode::ReceiveAndTransmit,
        )),
        Err(DarwinCarrierError::InputRecoveryRequired)
    ));
    let direct = DirectParameterCommand::new(
        expected,
        vec![DirectParameterAction::new(5, 0x3c, 40).unwrap()],
    )
    .unwrap();
    assert!(matches!(
        carrier.write_direct_command(&direct),
        Err(DarwinCarrierError::InputRecoveryRequired)
    ));
    assert_eq!(shared_calls.borrow().len(), calls_after_fault);
    assert_eq!(
        shared_calls
            .borrow()
            .iter()
            .filter(|call| matches!(call, Call::Write(_)))
            .count(),
        1
    );

    carrier.finish().unwrap();
    let calls = shared_calls.borrow();
    assert_eq!(
        calls
            .iter()
            .filter(|call| **call == Call::DiscardInput)
            .count(),
        2
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| matches!(call, Call::Write(_)))
            .count(),
        2
    );
    let discard = calls
        .iter()
        .position(|call| *call == Call::DiscardInput)
        .unwrap();
    let close = calls.iter().position(|call| *call == Call::Close).unwrap();
    assert!(discard < close);

    let snapshot_calls = Rc::new(RefCell::new(Vec::new()));
    let backend = FakeBackend::with_inbound([]).with_shared_calls(Rc::clone(&snapshot_calls));
    let mut carrier = PersistentCarrier::open(binding(), backend, FALLBACK_BAUD).unwrap();
    carrier.input_discard_required = true;
    let writes_before = snapshot_calls
        .borrow()
        .iter()
        .filter(|call| matches!(call, Call::Write(_)))
        .count();
    let mut pacer = FastPacer::default();
    assert!(execute_persistent_snapshot(carrier, &mut pacer, expected).is_err());
    assert_eq!(
        snapshot_calls
            .borrow()
            .iter()
            .filter(|call| matches!(call, Call::Write(_)))
            .count(),
        writes_before
    );
}

#[test]
fn receive_direct_quiescence_bypasses_taint_and_writes_once_without_reading() {
    let mut backend = FakeBackend::with_inbound([]);
    backend.wait_script = [
        (true, Duration::from_millis(1)),
        (false, RECOVERY_QUIET_WINDOW),
    ]
    .into_iter()
    .collect();
    let mut carrier = PersistentCarrier::open(binding(), backend, FALLBACK_BAUD).unwrap();
    carrier.input_discard_required = true;
    let device = DeviceId::new(0).unwrap();

    carrier
        .quiesce_receive_direct_command(RemoteModeCommand::new(device, RemoteMode::ReceiveDirect))
        .unwrap();

    assert!(!carrier.input_discard_required);
    assert_eq!(
        carrier.backend.writes,
        [vec![0xf0, 0, 0x20, 0x32, 0, 0x0e, 0x3f, 0x04, 0, 0xf7]]
    );
    assert_eq!(
        carrier
            .backend
            .calls
            .iter()
            .filter(|call| matches!(call, Call::Write(_)))
            .count(),
        1
    );
    assert_eq!(
        carrier
            .backend
            .calls
            .iter()
            .filter(|call| **call == Call::DiscardInput)
            .count(),
        3
    );
    assert!(
        !carrier
            .backend
            .calls
            .iter()
            .any(|call| matches!(call, Call::Read(_)))
    );
    carrier.finish().unwrap();
}

#[test]
fn receive_direct_quiescence_rejects_another_closed_mode_before_io() {
    let backend = FakeBackend::with_inbound([]);
    let mut carrier = PersistentCarrier::open(binding(), backend, FALLBACK_BAUD).unwrap();
    let device = DeviceId::new(0).unwrap();

    assert_eq!(
        carrier
            .quiesce_receive_direct_command(RemoteModeCommand::new(device, RemoteMode::Transmit,)),
        Err(DarwinCarrierError::RecoveryModeMismatch {
            actual: RemoteMode::Transmit,
        })
    );
    assert!(
        !carrier
            .backend
            .calls
            .iter()
            .any(|call| matches!(call, Call::Write(_) | Call::DiscardInput))
    );
    carrier.finish().unwrap();
}

#[test]
fn consuming_finish_quiesces_an_attempted_enabling_mode_before_restore_and_close() {
    let shared_calls = Rc::new(RefCell::new(Vec::new()));
    let shared_writes = Rc::new(RefCell::new(Vec::new()));
    let backend = FakeBackend::with_inbound([])
        .with_shared_calls(Rc::clone(&shared_calls))
        .with_shared_writes(Rc::clone(&shared_writes));
    let mut carrier = PersistentCarrier::open(binding(), backend, FALLBACK_BAUD).unwrap();
    let device = DeviceId::new(0).unwrap();

    carrier
        .write_remote_mode_command(RemoteModeCommand::new(device, RemoteMode::Transmit))
        .unwrap();
    carrier.finish().unwrap();

    assert_eq!(
        shared_writes.borrow().as_slice(),
        [
            vec![0xf0, 0, 0x20, 0x32, 0, 0x0e, 0x3f, 0x08, 0, 0xf7],
            vec![0xf0, 0, 0x20, 0x32, 0, 0x0e, 0x3f, 0x04, 0, 0xf7],
        ]
    );
    let calls = shared_calls.borrow();
    let second_write = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| matches!(call, Call::Write(_)))
        .nth(1)
        .map(|(index, _)| index)
        .unwrap();
    let restore = calls
        .iter()
        .position(|call| *call == Call::RestoreTermios)
        .unwrap();
    let close = calls.iter().position(|call| *call == Call::Close).unwrap();
    assert!(second_write < restore && restore < close);
}

#[test]
fn consuming_finish_quiesces_after_an_ambiguous_enabling_mode_write() {
    let shared_writes = Rc::new(RefCell::new(Vec::new()));
    let backend = FakeBackend::with_inbound([]).with_shared_writes(Rc::clone(&shared_writes));
    let mut carrier = PersistentCarrier::open(binding(), backend, FALLBACK_BAUD).unwrap();
    carrier.backend.short_write = Some(1);
    let device = DeviceId::new(0).unwrap();

    assert!(matches!(
        carrier.write_remote_mode_command(RemoteModeCommand::new(device, RemoteMode::Transmit)),
        Err(DarwinCarrierError::ShortWrite {
            written: 1,
            expected: 10,
        })
    ));
    carrier.backend.short_write = None;
    carrier.finish().unwrap();

    assert_eq!(shared_writes.borrow().len(), 2);
    assert_eq!(shared_writes.borrow()[0][7], 0x08);
    assert_eq!(shared_writes.borrow()[1][7], 0x04);
}

#[test]
fn successful_receive_direct_quiescence_prevents_a_second_finish_write() {
    let shared_writes = Rc::new(RefCell::new(Vec::new()));
    let backend = FakeBackend::with_inbound([]).with_shared_writes(Rc::clone(&shared_writes));
    let mut carrier = PersistentCarrier::open(binding(), backend, FALLBACK_BAUD).unwrap();
    let device = DeviceId::new(0).unwrap();

    carrier
        .write_remote_mode_command(RemoteModeCommand::new(device, RemoteMode::Transmit))
        .unwrap();
    carrier
        .write_remote_mode_command(RemoteModeCommand::new(device, RemoteMode::ReceiveDirect))
        .unwrap();
    carrier.finish().unwrap();

    assert_eq!(shared_writes.borrow().len(), 2);
    assert_eq!(shared_writes.borrow()[0][7], 0x08);
    assert_eq!(shared_writes.borrow()[1][7], 0x04);
}

#[test]
fn snapshot_dump_failure_is_quiesced_once_before_finish_restores_and_closes() {
    let identity = synthetic_response(0);
    let inbound = (0..10).flat_map(|_| identity).collect::<Vec<_>>();
    let shared_calls = Rc::new(RefCell::new(Vec::new()));
    let shared_writes = Rc::new(RefCell::new(Vec::new()));
    let backend = FakeBackend::with_inbound(inbound)
        .with_shared_calls(Rc::clone(&shared_calls))
        .with_shared_writes(Rc::clone(&shared_writes));
    let carrier = PersistentCarrier::open(binding(), backend, FALLBACK_BAUD).unwrap();
    let mut pacer = FastPacer::default();

    assert!(execute_persistent_snapshot(carrier, &mut pacer, DeviceId::new(0).unwrap()).is_err());

    let writes = shared_writes.borrow();
    assert_eq!(
        writes
            .iter()
            .filter(|frame| frame.get(6) == Some(&0x3f) && frame.get(7) == Some(&0x08))
            .count(),
        1
    );
    assert_eq!(
        writes
            .iter()
            .filter(|frame| frame.get(6) == Some(&0x3f) && frame.get(7) == Some(&0x04))
            .count(),
        1
    );
    let calls = shared_calls.borrow();
    let quiesce_write = calls
        .iter()
        .enumerate()
        .rfind(|(_, call)| matches!(call, Call::Write(10)))
        .map(|(index, _)| index)
        .unwrap();
    let restore = calls
        .iter()
        .position(|call| *call == Call::RestoreTermios)
        .unwrap();
    let close = calls.iter().position(|call| *call == Call::Close).unwrap();
    assert!(quiesce_write < restore && restore < close);
}

#[test]
fn direct_write_failure_is_quiesced_once_before_finish_restores_and_closes() {
    let shared_calls = Rc::new(RefCell::new(Vec::new()));
    let shared_writes = Rc::new(RefCell::new(Vec::new()));
    let backend = FakeBackend::with_inbound([])
        .with_shared_calls(Rc::clone(&shared_calls))
        .with_shared_writes(Rc::clone(&shared_writes));
    let mut carrier = PersistentCarrier::open(binding(), backend, FALLBACK_BAUD).unwrap();
    let device = DeviceId::new(0).unwrap();

    carrier
        .write_remote_mode_command(RemoteModeCommand::new(
            device,
            RemoteMode::ReceiveAndTransmit,
        ))
        .unwrap();
    carrier.backend.short_write = Some(1);
    let direct = DirectParameterCommand::new(
        device,
        vec![DirectParameterAction::new(5, 0x3c, 40).unwrap()],
    )
    .unwrap();
    assert!(matches!(
        carrier.write_direct_command(&direct),
        Err(DarwinCarrierError::ShortWrite { written: 1, .. })
    ));
    carrier.backend.short_write = None;
    carrier.finish().unwrap();

    let writes = shared_writes.borrow();
    assert_eq!(
        writes
            .iter()
            .filter(|frame| frame.get(6) == Some(&0x3f) && frame.get(7) == Some(&0x0c))
            .count(),
        1
    );
    assert_eq!(
        writes
            .iter()
            .filter(|frame| frame.get(6) == Some(&0x20))
            .count(),
        1
    );
    assert_eq!(
        writes
            .iter()
            .filter(|frame| frame.get(6) == Some(&0x3f) && frame.get(7) == Some(&0x04))
            .count(),
        1
    );
    let calls = shared_calls.borrow();
    let restore = calls
        .iter()
        .position(|call| *call == Call::RestoreTermios)
        .unwrap();
    let close = calls.iter().position(|call| *call == Call::Close).unwrap();
    assert!(restore < close);
}

#[test]
fn recovery_drain_rejects_an_always_readable_nonprogressing_backend() {
    let mut backend = FakeBackend::with_inbound([]);
    backend.wait_script = [(true, Duration::ZERO)].into_iter().collect();

    assert_eq!(
        run_receive_direct_recovery(&binding(), &mut backend, DeviceId::new(0).unwrap()),
        Err(DarwinCarrierError::Deadline {
            stage: CarrierStage::WaitReadable,
        })
    );
    assert_eq!(backend.writes.len(), 1);
    assert_eq!(backend.calls.last(), Some(&Call::Close));
}

#[test]
fn persistent_carrier_reconciles_one_late_duplicate_before_the_next_search() {
    let response = synthetic_response(0);
    let backend = FakeBackend::with_inbound(response);
    let mut carrier = PersistentCarrier::open(binding(), backend, FALLBACK_BAUD).unwrap();
    let expected = DeviceId::new(0).unwrap();

    assert!(matches!(
        execute_known_38400_search(&mut carrier, expected).unwrap(),
        Known38400SearchOutcome::Identified(_)
    ));
    carrier.backend.inbound.extend(response);
    carrier.backend.inbound.extend(response);
    carrier.backend.post_response_input = SEARCH_RESPONSE_LIMIT;

    assert!(matches!(
        execute_known_38400_search(&mut carrier, expected).unwrap(),
        Known38400SearchOutcome::Identified(_)
    ));
    assert!(carrier.backend.inbound.is_empty());
    assert_eq!(carrier.receipts().len(), 2);
    assert_eq!(carrier.receipts()[1].duplicate_response_count, 1);
    assert_eq!(carrier.receipts()[1].wire_bytes, SEARCH_RESPONSE_LIMIT * 2);
    assert_eq!(
        carrier
            .backend
            .calls
            .iter()
            .filter(|call| **call == Call::Write(SEARCH_REQUEST_LEN))
            .count(),
        2
    );
    carrier.finish().unwrap();
}

#[test]
fn persistent_carrier_reconciles_the_final_search_duplicate_before_remote_mode() {
    let response = synthetic_response(0);
    let backend = FakeBackend::with_inbound(response);
    let mut carrier = PersistentCarrier::open(binding(), backend, FALLBACK_BAUD).unwrap();
    let expected = DeviceId::new(0).unwrap();

    assert!(matches!(
        execute_known_38400_search(&mut carrier, expected).unwrap(),
        Known38400SearchOutcome::Identified(_)
    ));
    carrier.backend.inbound.extend(response);
    carrier.backend.post_response_input = SEARCH_RESPONSE_LIMIT;

    carrier
        .write_remote_mode_command(RemoteModeCommand::new(expected, RemoteMode::Transmit))
        .unwrap();
    assert!(carrier.backend.inbound.is_empty());
    assert_eq!(
        carrier
            .backend
            .calls
            .iter()
            .filter(|call| matches!(call, Call::Write(_)))
            .count(),
        2
    );
    carrier.finish().unwrap();
}

#[test]
fn persistent_carrier_reconciles_multiple_late_duplicates_before_remote_mode() {
    let response = synthetic_response(0);
    let backend = FakeBackend::with_inbound(response);
    let mut carrier = PersistentCarrier::open(binding(), backend, FALLBACK_BAUD).unwrap();
    let expected = DeviceId::new(0).unwrap();

    assert!(matches!(
        execute_known_38400_search(&mut carrier, expected).unwrap(),
        Known38400SearchOutcome::Identified(_)
    ));
    carrier.backend.inbound.extend(response);
    carrier.backend.inbound.extend(response);
    carrier.backend.post_response_input = SEARCH_RESPONSE_LIMIT * 2;

    carrier
        .write_remote_mode_command(RemoteModeCommand::new(expected, RemoteMode::Transmit))
        .unwrap();
    assert!(carrier.backend.inbound.is_empty());
    assert_eq!(
        carrier
            .backend
            .calls
            .iter()
            .filter(|call| matches!(call, Call::Write(_)))
            .count(),
        2
    );
    carrier.finish().unwrap();
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
fn persistent_snapshot_replays_one_empty_carrier_timeout() {
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
    backend.available_script = [0, 0].into_iter().collect();
    backend.wait_script = [(false, SNAPSHOT_OPERATION_TIMEOUT)].into_iter().collect();
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
}

#[test]
fn snapshot_search_reader_strips_echo_and_consumes_all_already_queued_replays() {
    let request = synthetic_search_request();
    let response = synthetic_response(0);
    let mut inbound = request.to_vec();
    inbound.extend_from_slice(&response);
    inbound.extend_from_slice(&response);
    inbound.extend_from_slice(&response);
    let mut backend = FakeBackend::with_inbound(inbound);
    backend.written = true;
    backend.read_goal = request.len() + response.len();
    backend.post_response_input = response.len() * 2;

    let bounded = read_snapshot_bounded(
        &mut backend,
        SNAPSHOT_OPERATION_TIMEOUT,
        SEARCH_RESPONSE_LIMIT,
        &request,
        true,
    )
    .unwrap();

    assert_eq!(bounded.read, SnapshotRead::complete(&response).unwrap());
    assert!(backend.inbound.is_empty());
    assert_eq!(backend.post_response_input, 0);
    assert_eq!(backend.now, Duration::ZERO);
    assert!(!backend.calls.contains(&Call::WaitReadable));
}

#[test]
fn snapshot_search_reader_returns_before_a_late_replay_becomes_readable() {
    let request = synthetic_search_request();
    let response = synthetic_response(0);
    let inbound = response.into_iter().chain(response).collect::<Vec<_>>();
    let mut backend = FakeBackend::with_inbound(inbound);
    backend.written = true;
    backend.read_goal = response.len();
    backend.post_response_input_after_wait = Some(response.len());
    backend.wait_script = [
        (true, Duration::from_millis(10)),
        (false, Duration::from_millis(465)),
    ]
    .into_iter()
    .collect();

    let bounded = read_snapshot_bounded(
        &mut backend,
        SNAPSHOT_OPERATION_TIMEOUT,
        SEARCH_RESPONSE_LIMIT,
        &request,
        true,
    )
    .unwrap();

    assert_eq!(bounded.read, SnapshotRead::complete(&response).unwrap());
    assert_eq!(
        backend.inbound.iter().copied().collect::<Vec<_>>(),
        response.to_vec()
    );
    assert_eq!(backend.post_response_input, 0);
    assert_eq!(backend.now, Duration::ZERO);
    assert!(!backend.calls.contains(&Call::WaitReadable));
}

#[test]
fn snapshot_search_reader_rejects_an_already_queued_partial_without_waiting() {
    let request = synthetic_search_request();
    let response = synthetic_response(0);
    let partial = &response[..13];
    let inbound = response
        .into_iter()
        .chain(partial.iter().copied())
        .collect::<Vec<_>>();
    let mut backend = FakeBackend::with_inbound(inbound);
    backend.written = true;
    backend.read_goal = response.len();
    backend.post_response_input = partial.len();

    assert!(matches!(
        read_snapshot_bounded(
            &mut backend,
            SNAPSHOT_OPERATION_TIMEOUT,
            SEARCH_RESPONSE_LIMIT,
            &request,
            true,
        ),
        Err(DarwinCarrierError::UnexpectedTrailingFrame { received })
            if received == partial.len()
    ));
    assert!(backend.inbound.is_empty());
    assert_eq!(backend.now, Duration::ZERO);
    assert!(!backend.calls.contains(&Call::WaitReadable));
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
