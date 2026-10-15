//! Read-only level-meter session on one persistent 38400 descriptor.
//!
//! The closed sequence is one validated Search identity (an empty first
//! Search may be replayed at the device cadence, at most twice), then exactly
//! [`LIVE_METER_REQUEST_COUNT`] paced meter queries. Each query is the fixed
//! [`Query::Ping`](dcx_core::protocol::Query::Ping) frame and each reply must
//! be one exact 25-byte function-`0x04` frame from the expected address.
//!
//! The executor never writes remote mode, direct parameters, or any other
//! frame. It changes no device state, so the consuming session finish has no
//! receive-direct quiescence to perform.

use std::{error::Error as StdError, time::Duration};

use dcx_core::{SnapshotError, meter::MeterReadingV1, protocol::DeviceId};
use thiserror::Error;

use crate::{
    RepeatPacer,
    snapshot::{
        CaptureBodyError, PersistentSnapshotSession, SnapshotCaptureError, SnapshotOperation,
        SnapshotOperationKind, capture_search_identities, exchange_validated, require_budget,
        with_finish,
    },
};

/// Valid Search identities required before the first meter query.
pub const LIVE_METER_IDENTITY_SEARCH_COUNT: usize = 1;
/// Maximum Search attempts for that identity; only empty timeouts replay.
pub const LIVE_METER_IDENTITY_ATTEMPT_LIMIT: usize = 3;
/// Exact number of meter queries in one session.
pub const LIVE_METER_REQUEST_COUNT: usize = 10;
/// Minimum delay before every meter query.
pub const LIVE_METER_GAP: Duration = Duration::from_millis(500);
/// Whole-session budget: worst-case identity plus every paced meter query.
pub const LIVE_METER_BUDGET: Duration = Duration::from_secs(30);

/// Complete read-only meter capture with a verified session close.
#[derive(Debug)]
pub struct LiveMeterCapture {
    device: DeviceId,
    readings: Vec<MeterReadingV1>,
}

impl LiveMeterCapture {
    /// Validated device address.
    pub const fn device(&self) -> DeviceId {
        self.device
    }

    /// Every typed reading, in request order.
    pub fn readings(&self) -> &[MeterReadingV1] {
        &self.readings
    }
}

/// Terminal meter-session failure, carrying readings already validated.
#[derive(Debug, Error)]
#[error(
    "live meter session stopped after {} of {} readings: {source}",
    .readings.len(),
    LIVE_METER_REQUEST_COUNT
)]
pub struct LiveMeterError<E: StdError + Send + Sync + 'static, P: StdError + Send + Sync + 'static>
{
    readings: Vec<MeterReadingV1>,
    #[source]
    source: SnapshotCaptureError<E, P>,
}

impl<E: StdError + Send + Sync + 'static, P: StdError + Send + Sync + 'static>
    LiveMeterError<E, P>
{
    /// Readings validated before the failure.
    pub fn readings(&self) -> &[MeterReadingV1] {
        &self.readings
    }

    /// Underlying bounded-session failure.
    pub const fn cause(&self) -> &SnapshotCaptureError<E, P> {
        &self.source
    }
}

/// Run one Search identity and ten paced read-only meter queries.
///
/// The executor always calls [`PersistentSnapshotSession::finish`], including
/// after an earlier failure, and returns a capture only when finish succeeds.
///
/// # Errors
///
/// Stops on the first pacing, budget, transport, timeout, response-limit,
/// identity, or meter-validation failure. No later query is issued.
pub fn execute_live_meter<S: PersistentSnapshotSession, P: RepeatPacer>(
    mut session: S,
    pacer: &mut P,
    expected_device: DeviceId,
) -> Result<LiveMeterCapture, LiveMeterError<S::Error, P::Error>> {
    let started = pacer.elapsed();
    let mut readings = Vec::with_capacity(LIVE_METER_REQUEST_COUNT);
    let body = meter_body(&mut session, pacer, expected_device, started, &mut readings);
    let finish = session.finish();
    match (body, finish) {
        (Ok(()), Ok(())) => Ok(LiveMeterCapture {
            device: expected_device,
            readings,
        }),
        (Ok(()), Err(source)) => Err(LiveMeterError {
            readings,
            source: SnapshotCaptureError::Finish { source },
        }),
        (Err(error), finish) => Err(LiveMeterError {
            readings,
            source: with_finish(error, finish.err()),
        }),
    }
}

fn meter_body<S: PersistentSnapshotSession, P: RepeatPacer>(
    session: &mut S,
    pacer: &mut P,
    expected_device: DeviceId,
    started: Duration,
    readings: &mut Vec<MeterReadingV1>,
) -> Result<(), CaptureBodyError<S::Error, P::Error>> {
    capture_search_identities(
        session,
        pacer,
        expected_device,
        LIVE_METER_IDENTITY_SEARCH_COUNT,
        LIVE_METER_IDENTITY_ATTEMPT_LIMIT,
        started,
        LIVE_METER_BUDGET,
    )?;

    for sequence in 1..=LIVE_METER_REQUEST_COUNT {
        let kind = SnapshotOperationKind::Meter { sequence };
        pacer
            .wait(LIVE_METER_GAP)
            .map_err(|source| CaptureBodyError::Pacing {
                trial: sequence,
                source,
            })?;
        require_budget::<S::Error, P>(pacer, started, LIVE_METER_BUDGET, kind)?;
        let operation = SnapshotOperation::meter(sequence, expected_device).map_err(|source| {
            CaptureBodyError::Validation {
                operation: kind,
                source: SnapshotError::Protocol(source),
            }
        })?;
        let frame = exchange_validated(session, operation)?;
        let reading = MeterReadingV1::parse(&frame, expected_device).map_err(|source| {
            CaptureBodyError::Validation {
                operation: kind,
                source: SnapshotError::Meter(source),
            }
        })?;
        readings.push(reading);
        require_budget::<S::Error, P>(pacer, started, LIVE_METER_BUDGET, kind)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};

    use dcx_core::{RemoteModeCommand, meter::MeterError, protocol::Query};

    use super::*;
    use crate::{
        REPEAT_SEARCH_GAP,
        snapshot::{SNAPSHOT_OPERATION_TIMEOUT, SnapshotRead},
    };

    #[derive(Debug, Error)]
    #[error("synthetic session failure")]
    struct FakeSessionError;

    #[derive(Debug, Error)]
    #[error("synthetic pacer failure")]
    struct FakePacerError;

    #[derive(Default)]
    struct FakePacer {
        elapsed: Duration,
        waits: Vec<Duration>,
        stall_after: Option<usize>,
    }

    impl RepeatPacer for FakePacer {
        type Error = FakePacerError;

        fn elapsed(&mut self) -> Duration {
            self.elapsed
        }

        fn wait(&mut self, minimum: Duration) -> Result<(), Self::Error> {
            self.waits.push(minimum);
            self.elapsed += minimum;
            if self.stall_after == Some(self.waits.len()) {
                self.elapsed += LIVE_METER_BUDGET;
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct SessionLog {
        operations: Vec<SnapshotOperation>,
        modes: Vec<RemoteModeCommand>,
        finishes: usize,
    }

    enum Step {
        Read(SnapshotRead),
        Fail,
    }

    struct FakeSession {
        steps: VecDeque<Step>,
        log: Rc<RefCell<SessionLog>>,
        fail_finish: bool,
    }

    impl FakeSession {
        fn new(steps: impl IntoIterator<Item = Step>) -> (Self, Rc<RefCell<SessionLog>>) {
            let log = Rc::new(RefCell::new(SessionLog::default()));
            (
                Self {
                    steps: steps.into_iter().collect(),
                    log: Rc::clone(&log),
                    fail_finish: false,
                },
                log,
            )
        }
    }

    impl PersistentSnapshotSession for FakeSession {
        type Error = FakeSessionError;

        fn exchange(&mut self, operation: SnapshotOperation) -> Result<SnapshotRead, Self::Error> {
            self.log.borrow_mut().operations.push(operation);
            match self.steps.pop_front().expect("test supplied a step") {
                Step::Read(read) => Ok(read),
                Step::Fail => Err(FakeSessionError),
            }
        }

        fn write_remote_mode(&mut self, command: &RemoteModeCommand) -> Result<(), Self::Error> {
            self.log.borrow_mut().modes.push(*command);
            Ok(())
        }

        fn finish(self) -> Result<(), Self::Error> {
            self.log.borrow_mut().finishes += 1;
            if self.fail_finish {
                Err(FakeSessionError)
            } else {
                Ok(())
            }
        }
    }

    fn device() -> DeviceId {
        DeviceId::new(0).unwrap()
    }

    fn identity(device: u8) -> Step {
        let mut frame = vec![0xf0, 0x00, 0x20, 0x32, device, 0x0e, 0x00];
        frame.extend_from_slice(b"SYNTHETIC-IDENTITY");
        frame.push(0xf7);
        Step::Read(SnapshotRead::complete(&frame).unwrap())
    }

    fn meter_frame(device: u8, level: u8) -> Vec<u8> {
        let mut frame = vec![0xf0, 0x00, 0x20, 0x32, device, 0x0e, 0x04, 0x00];
        frame.extend_from_slice(&[level; 9]);
        frame.extend_from_slice(&[0; 7]);
        frame.push(0xf7);
        frame
    }

    fn meter(level: u8) -> Step {
        Step::Read(SnapshotRead::complete(&meter_frame(0, level)).unwrap())
    }

    fn empty_timeout() -> Step {
        Step::Read(SnapshotRead::timed_out(&[]).unwrap())
    }

    fn complete_steps() -> Vec<Step> {
        let mut steps = vec![identity(0)];
        for index in 0..LIVE_METER_REQUEST_COUNT {
            steps.push(meter(u8::try_from(index % 7).unwrap()));
        }
        steps
    }

    #[test]
    fn exact_read_only_sequence_is_one_search_then_ten_paced_meters() {
        let (session, log) = FakeSession::new(complete_steps());
        let mut pacer = FakePacer::default();

        let capture = execute_live_meter(session, &mut pacer, device()).unwrap();
        assert_eq!(capture.device(), device());
        assert_eq!(capture.readings().len(), LIVE_METER_REQUEST_COUNT);
        assert_eq!(capture.readings()[6].peak_level(), 6);

        let log = log.borrow();
        assert!(log.modes.is_empty(), "meter session wrote remote mode");
        assert_eq!(log.finishes, 1);
        assert_eq!(log.operations.len(), 1 + LIVE_METER_REQUEST_COUNT);
        assert_eq!(
            log.operations[0].kind(),
            SnapshotOperationKind::Search { sequence: 1 }
        );
        assert_eq!(
            log.operations[0].request().as_bytes(),
            Query::Search.encode().unwrap().as_slice()
        );
        for (index, operation) in log.operations[1..].iter().enumerate() {
            assert_eq!(
                operation.kind(),
                SnapshotOperationKind::Meter {
                    sequence: index + 1
                }
            );
            assert_eq!(
                operation.request().as_bytes(),
                [0xf0, 0x00, 0x20, 0x32, 0x00, 0x0e, 0x44, 0x00, 0x00, 0xf7]
            );
            assert_eq!(operation.response_limit(), 25);
            assert_eq!(operation.timeout(), SNAPSHOT_OPERATION_TIMEOUT);
            assert_eq!(operation.expected_device(), device());
        }
        assert_eq!(pacer.waits, [LIVE_METER_GAP; LIVE_METER_REQUEST_COUNT]);
    }

    #[test]
    fn empty_first_searches_replay_at_device_cadence_up_to_the_limit() {
        let mut steps = vec![empty_timeout(), empty_timeout()];
        steps.extend(complete_steps());
        let (session, log) = FakeSession::new(steps);
        let mut pacer = FakePacer::default();

        let capture = execute_live_meter(session, &mut pacer, device()).unwrap();
        assert_eq!(capture.readings().len(), LIVE_METER_REQUEST_COUNT);
        assert_eq!(&pacer.waits[..2], [REPEAT_SEARCH_GAP; 2]);
        assert_eq!(
            log.borrow().operations.len(),
            LIVE_METER_IDENTITY_ATTEMPT_LIMIT + LIVE_METER_REQUEST_COUNT
        );

        let (session, log) = FakeSession::new([empty_timeout(), empty_timeout(), empty_timeout()]);
        let error = execute_live_meter(session, &mut FakePacer::default(), device()).unwrap_err();
        assert!(error.readings().is_empty());
        assert!(matches!(
            error.cause(),
            SnapshotCaptureError::SearchQualificationIncomplete {
                valid: 0,
                required: 1,
                attempts: 3,
                finish_error: None,
            }
        ));
        let log = log.borrow();
        assert_eq!(log.operations.len(), LIVE_METER_IDENTITY_ATTEMPT_LIMIT);
        assert_eq!(log.finishes, 1);
    }

    #[test]
    fn wrong_identity_stops_before_any_meter_query() {
        let (session, log) = FakeSession::new([identity(1)]);
        let error = execute_live_meter(session, &mut FakePacer::default(), device()).unwrap_err();
        assert!(matches!(
            error.cause(),
            SnapshotCaptureError::Validation {
                operation: SnapshotOperationKind::Search { sequence: 1 },
                ..
            }
        ));
        assert_eq!(log.borrow().operations.len(), 1);
        assert_eq!(log.borrow().finishes, 1);
    }

    #[test]
    fn meter_timeout_stops_and_keeps_validated_readings() {
        let (session, log) = FakeSession::new([
            identity(0),
            meter(1),
            meter(2),
            Step::Read(SnapshotRead::timed_out(&[0xf0, 0x00]).unwrap()),
        ]);
        let error = execute_live_meter(session, &mut FakePacer::default(), device()).unwrap_err();
        assert_eq!(error.readings().len(), 2);
        assert!(matches!(
            error.cause(),
            SnapshotCaptureError::Timeout {
                operation: SnapshotOperationKind::Meter { sequence: 3 },
                received: 2,
                ..
            }
        ));
        assert!(error.to_string().contains("after 2 of 10 readings"));
        assert_eq!(log.borrow().operations.len(), 4);
        assert_eq!(log.borrow().finishes, 1);
    }

    #[test]
    fn oversized_and_out_of_domain_meter_replies_fail_closed() {
        let mut long = meter_frame(0, 0);
        long.insert(24, 0);
        let (session, _) = FakeSession::new([
            identity(0),
            Step::Read(SnapshotRead::complete(&long).unwrap()),
        ]);
        let error = execute_live_meter(session, &mut FakePacer::default(), device()).unwrap_err();
        assert!(matches!(
            error.cause(),
            SnapshotCaptureError::ResponseLimit {
                received: 26,
                limit: 25,
                ..
            }
        ));

        let (session, _) = FakeSession::new([
            identity(0),
            Step::Read(SnapshotRead::complete(&meter_frame(0, 7)).unwrap()),
        ]);
        let error = execute_live_meter(session, &mut FakePacer::default(), device()).unwrap_err();
        assert!(matches!(
            error.cause(),
            SnapshotCaptureError::Validation {
                source: SnapshotError::Meter(MeterError::LevelOutOfRange { level: 7, .. }),
                ..
            }
        ));

        let (session, _) = FakeSession::new([
            identity(0),
            Step::Read(SnapshotRead::complete(&meter_frame(1, 0)).unwrap()),
        ]);
        let error = execute_live_meter(session, &mut FakePacer::default(), device()).unwrap_err();
        assert!(matches!(
            error.cause(),
            SnapshotCaptureError::Validation {
                source: SnapshotError::Meter(MeterError::UnexpectedDevice {
                    expected: 0,
                    actual: 1,
                }),
                ..
            }
        ));
    }

    #[test]
    fn transport_budget_and_finish_failures_are_terminal() {
        let (session, log) = FakeSession::new([identity(0), Step::Fail]);
        let error = execute_live_meter(session, &mut FakePacer::default(), device()).unwrap_err();
        assert!(matches!(
            error.cause(),
            SnapshotCaptureError::Transport {
                operation: SnapshotOperationKind::Meter { sequence: 1 },
                ..
            }
        ));
        assert_eq!(log.borrow().finishes, 1);

        let (session, log) = FakeSession::new(complete_steps());
        let mut pacer = FakePacer {
            stall_after: Some(4),
            ..FakePacer::default()
        };
        let error = execute_live_meter(session, &mut pacer, device()).unwrap_err();
        assert_eq!(error.readings().len(), 3);
        assert!(matches!(
            error.cause(),
            SnapshotCaptureError::BudgetExceeded {
                operation: SnapshotOperationKind::Meter { sequence: 4 },
                ..
            }
        ));
        assert_eq!(log.borrow().operations.len(), 4);

        let (mut session, _) = FakeSession::new(complete_steps());
        session.fail_finish = true;
        let error = execute_live_meter(session, &mut FakePacer::default(), device()).unwrap_err();
        assert_eq!(error.readings().len(), LIVE_METER_REQUEST_COUNT);
        assert!(matches!(error.cause(), SnapshotCaptureError::Finish { .. }));
    }
}
