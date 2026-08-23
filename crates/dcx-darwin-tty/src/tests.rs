use std::{collections::VecDeque, path::PathBuf, time::Duration};

use dcx_core::{discovery::DiscoveryAttemptKind, protocol::DeviceId};
use dcx_transport::{SearchExecutionError, SearchOutcome, execute_search};

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
    RestoreControlLines,
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
        assert_eq!(bytes, &[0xf0, 0x00, 0x20, 0x32, 0x20, 0x0e, 0x40, 0xf7]);
        Ok(self.short_write.unwrap_or(bytes.len()))
    }

    fn bytes_available(&mut self) -> Result<usize, SystemFault> {
        self.step(CarrierStage::BytesAvailable, Call::BytesAvailable)?;
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
        Ok(ReadProgress::Bytes(count))
    }

    fn restore_termios(&mut self, snapshot: &Self::TermiosSnapshot) -> Result<(), SystemFault> {
        assert_eq!(snapshot, &FakeTermios(8));
        self.step(CarrierStage::RestoreTermios, Call::RestoreTermios)
    }

    fn restore_control_lines(&mut self, state: i32) -> Result<(), SystemFault> {
        assert_eq!(state, 0x2496);
        self.step(CarrierStage::RestoreControlLines, Call::RestoreControlLines)
    }

    fn close(&mut self) {
        self.calls.push(Call::Close);
        self.opened = false;
    }
}

fn synthetic_response(device: u8) -> [u8; SEARCH_RESPONSE_LIMIT] {
    let mut frame = [0_u8; SEARCH_RESPONSE_LIMIT];
    frame[..7].copy_from_slice(&[0xf0, 0x00, 0x20, 0x32, device, 0x0e, 0x00]);
    frame[7..25].copy_from_slice(b"SYNTHETIC-IDENTITY");
    frame[25] = 0xf7;
    frame
}

fn binding() -> PrivateTtyBinding {
    let path = PathBuf::from(SYNTHETIC_PATH);
    let digest = Sha256Digest::of_bytes(path.as_os_str().as_bytes()).to_string();
    PrivateTtyBinding::new(path, &digest).unwrap()
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
    assert_eq!(identity.attempt(), DiscoveryAttemptKind::Primary);
    assert_eq!(carrier.backend.inbound.len(), 0);
    assert_eq!(
        carrier.backend.calls,
        [
            Call::Open,
            Call::SnapshotTermios,
            Call::SnapshotControlLines,
            Call::Configure(115_200),
            Call::Write(8),
            Call::BytesAvailable,
            Call::Read(26),
            Call::BytesAvailable,
            Call::RestoreTermios,
            Call::RestoreControlLines,
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
    assert_eq!(receipt.termios_cleanup, CleanupDisposition::Restored);
    assert_eq!(receipt.control_lines_cleanup, CleanupDisposition::Restored);
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
fn queued_overflow_is_detected_without_consuming_byte_twenty_seven() {
    let backend = FakeBackend::with_inbound([0_u8; SEARCH_RESPONSE_LIMIT + 1]);
    let mut carrier = Carrier::new(binding(), backend);

    assert!(matches!(
        execute_search(&mut carrier, DeviceId::new(0).unwrap()),
        Err(SearchExecutionError::Transport {
            source: DarwinCarrierError::Overflow {
                received: 0,
                queued: 27,
            },
            ..
        })
    ));
    assert_eq!(carrier.backend.inbound.len(), 27);
    assert!(
        !carrier
            .backend
            .calls
            .iter()
            .any(|call| matches!(call, Call::Read(_)))
    );
    assert_eq!(
        carrier.receipts()[0].outcome,
        SanitizedAttemptOutcome::Overflow
    );
    assert!(carrier.receipts()[0].closed);
}

#[test]
fn a_short_write_is_never_retried_and_always_restores() {
    let mut backend = FakeBackend::with_inbound([]);
    backend.short_write = Some(7);
    let mut carrier = Carrier::new(binding(), backend);

    assert!(matches!(
        execute_search(&mut carrier, DeviceId::new(0).unwrap()),
        Err(SearchExecutionError::Transport {
            source: DarwinCarrierError::ShortWrite { written: 7 },
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
        Call::RestoreControlLines,
        Call::Close,
    ]));
}

#[test]
fn deadline_includes_configuration_and_blocks_the_write_at_500_ms() {
    let mut backend = FakeBackend::with_inbound([]);
    backend.advance_at = Some((CarrierStage::Configure, Duration::from_millis(500)));
    let mut carrier = Carrier::new(binding(), backend);

    assert!(matches!(
        execute_search(&mut carrier, DeviceId::new(0).unwrap()),
        Err(SearchExecutionError::Transport {
            source: DarwinCarrierError::Deadline {
                stage: CarrierStage::Write,
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
            Call::RestoreControlLines,
            Call::Close,
        ]));
        assert!(carrier.receipts()[0].closed);
    }
}

#[test]
fn cleanup_failure_is_terminal_but_still_runs_other_restore_and_close() {
    for failing_stage in [
        CarrierStage::RestoreTermios,
        CarrierStage::RestoreControlLines,
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
        assert!(carrier.backend.calls.ends_with(&[
            Call::RestoreTermios,
            Call::RestoreControlLines,
            Call::Close,
        ]));
        assert!(carrier.receipts()[0].closed);
        assert_eq!(
            carrier.receipts()[0].outcome,
            SanitizedAttemptOutcome::CleanupFailed
        );
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
fn binding_and_receipt_debug_output_never_expose_the_private_path_or_payload() {
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
fn private_binding_rejects_noncanonical_digest_and_non_callout_paths() {
    assert!(matches!(
        PrivateTtyBinding::new(PathBuf::from(SYNTHETIC_PATH), "SHA256/not-canonical"),
        Err(BindingError::InvalidDigest)
    ));
    let digest = Sha256Digest::of_bytes(b"not-a-callout").to_string();
    assert!(matches!(
        PrivateTtyBinding::new(PathBuf::from("not-a-callout"), &digest),
        Err(BindingError::UnsupportedPrivatePath)
    ));
}

#[test]
fn binding_digest_mismatch_is_sanitized_and_prevents_construction() {
    let expected = Sha256Digest::of_bytes(b"different-private-binding");
    let error =
        PrivateTtyBinding::new(PathBuf::from(SYNTHETIC_PATH), &expected.to_string()).unwrap_err();
    assert!(matches!(error, BindingError::DigestMismatch { .. }));
    let message = error.to_string();
    assert!(!message.contains(SYNTHETIC_PATH));
    assert!(message.contains("digest mismatch"));
}

#[test]
fn draining_sanitized_receipts_does_not_retain_a_duplicate() {
    let backend = FakeBackend::with_inbound(synthetic_response(0));
    let mut carrier = Carrier::new(binding(), backend);
    execute_search(&mut carrier, DeviceId::new(0).unwrap()).unwrap();
    assert_eq!(carrier.take_receipts().len(), 1);
    assert!(carrier.receipts().is_empty());
}
