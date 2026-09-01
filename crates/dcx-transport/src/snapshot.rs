//! One-session snapshot, typed apply, readback, and rollback execution.
//!
//! A platform carrier opens and configures one exact 38400 session before
//! handing it to this executor. Snapshot execution owns a closed request
//! sequence: ten validated Search identities, closed transmit enable, Dump0,
//! Dump1, a post-capture receive-only transition, and consuming verified close.
//! Mutation accepts only closed remote modes plus [`DirectParameterCommand`];
//! there is no arbitrary frame operation.

use std::{error::Error as StdError, fmt, time::Duration};

use dcx_core::{
    ApplyTransactionError, ApplyTransactionV1, DirectParameterCommand, RemoteMode,
    RemoteModeCommand, SnapshotError, SnapshotSection, SnapshotV1,
    discovery::FALLBACK_BAUD,
    protocol::{
        DUMP0_RESPONSE_LEN, DUMP1_RESPONSE_LEN, DecodedMessage, DeviceId, DumpPart, MAX_FRAME_LEN,
        ProtocolError, Query, SEARCH_RESPONSE_LEN, SearchResponse26, decode, parse_frame,
    },
};
use thiserror::Error;

use crate::{REPEAT_SEARCH_GAP, RepeatPacer};

/// Total Search identities required in one persistent snapshot session.
pub const PERSISTENT_SEARCH_COUNT: usize = 10;
const SEARCH_ATTEMPTS_PER_REQUIRED_IDENTITY: usize = 2;
/// Maximum attempts allowed to collect the ten persistent Search identities.
pub const PERSISTENT_SEARCH_ATTEMPT_LIMIT: usize =
    PERSISTENT_SEARCH_COUNT * SEARCH_ATTEMPTS_PER_REQUIRED_IDENTITY;
/// Search count for post-mutation readback on the already-identified session.
pub const READBACK_SEARCH_COUNT: usize = 1;
/// Maximum Search attempts for post-mutation readback.
///
/// One empty timeout may be followed by one read-only Search replay. Partial,
/// invalid, or transport-failed responses remain terminal.
pub const READBACK_SEARCH_ATTEMPT_LIMIT: usize = READBACK_SEARCH_COUNT + 1;
/// Per-Search deadline enforced by the platform session.
pub const SNAPSHOT_OPERATION_TIMEOUT: Duration = Duration::from_millis(500);
/// Dump deadline, sized for one maximum response plus one exact replay at 38400.
pub const SNAPSHOT_DUMP_TIMEOUT: Duration = Duration::from_secs(2);
/// Whole ten-valid-identity plus Dump0/Dump1 snapshot budget.
pub const PERSISTENT_SNAPSHOT_BUDGET: Duration = Duration::from_secs(120);
/// Whole stale-baseline-check plus typed-apply/readback budget.
///
/// This includes the maximum twenty paced baseline Search attempts, complete
/// dumps, one typed mutation, and its complete readback.
pub const APPLY_TRANSACTION_BUDGET: Duration = Duration::from_secs(150);
/// Whole identity-check plus typed-rollback/readback budget.
pub const ROLLBACK_TRANSACTION_BUDGET: Duration = Duration::from_secs(20);
/// Maximum encoded query length among Search, Dump0, and Dump1.
pub const SNAPSHOT_REQUEST_LIMIT: usize = 11;

/// Closed operation kind in the persistent session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotOperationKind {
    /// One exact broadcast Search. `sequence` is the one-based attempt number.
    Search { sequence: usize },
    /// First complete dump query.
    Dump0,
    /// Second complete dump query.
    Dump1,
}

/// One private, exact, typed query request.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SnapshotRequest {
    bytes: [u8; SNAPSHOT_REQUEST_LIMIT],
    len: usize,
}

impl SnapshotRequest {
    fn from_query(query: Query) -> Result<Self, ProtocolError> {
        let encoded = query.encode()?;
        let mut bytes = [0; SNAPSHOT_REQUEST_LIMIT];
        bytes[..encoded.len()].copy_from_slice(&encoded);
        Ok(Self {
            bytes,
            len: encoded.len(),
        })
    }

    /// Exact request bytes. There is no caller-controlled constructor.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl fmt::Debug for SnapshotRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotRequest")
            .field("len", &self.len)
            .field("bytes", &self.as_bytes())
            .finish()
    }
}

/// Exact immutable operation supplied to a persistent platform session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotOperation {
    kind: SnapshotOperationKind,
    expected_device: DeviceId,
    request: SnapshotRequest,
    response_limit: usize,
}

impl SnapshotOperation {
    fn search(sequence: usize, expected_device: DeviceId) -> Result<Self, ProtocolError> {
        Ok(Self {
            kind: SnapshotOperationKind::Search { sequence },
            expected_device,
            request: SnapshotRequest::from_query(Query::Search)?,
            response_limit: SEARCH_RESPONSE_LEN,
        })
    }

    fn dump(part: DumpPart, expected_device: DeviceId) -> Result<Self, ProtocolError> {
        Ok(Self {
            kind: match part {
                DumpPart::Part0 => SnapshotOperationKind::Dump0,
                DumpPart::Part1 => SnapshotOperationKind::Dump1,
            },
            expected_device,
            request: SnapshotRequest::from_query(Query::Dump {
                device: expected_device,
                part,
            })?,
            response_limit: match part {
                DumpPart::Part0 => DUMP0_RESPONSE_LEN,
                DumpPart::Part1 => DUMP1_RESPONSE_LEN,
            },
        })
    }

    /// Closed operation kind.
    pub const fn kind(self) -> SnapshotOperationKind {
        self.kind
    }

    /// Device address required in the complete response.
    pub const fn expected_device(self) -> DeviceId {
        self.expected_device
    }

    /// Exact typed request bytes.
    pub const fn request(self) -> SnapshotRequest {
        self.request
    }

    /// Total operation deadline the carrier must enforce.
    pub const fn timeout(self) -> Duration {
        match self.kind {
            SnapshotOperationKind::Search { .. } => SNAPSHOT_OPERATION_TIMEOUT,
            SnapshotOperationKind::Dump0 | SnapshotOperationKind::Dump1 => SNAPSHOT_DUMP_TIMEOUT,
        }
    }

    /// Exact response byte limit for this response type.
    pub const fn response_limit(self) -> usize {
        self.response_limit
    }
}

/// How one bounded persistent-session read ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotReadEnd {
    /// One complete `f0` through `f7` frame was received.
    Complete,
    /// The operation deadline elapsed before one complete frame.
    TimedOut,
}

/// Bounded response returned by the injected platform session.
///
/// Raw frame bytes are redacted from `Debug` but retained until the executor
/// constructs the local immutable [`SnapshotV1`].
#[derive(Clone, PartialEq, Eq)]
pub struct SnapshotRead {
    end: SnapshotReadEnd,
    bytes: Vec<u8>,
}

impl SnapshotRead {
    /// Construct one complete, bounded candidate frame.
    ///
    /// # Errors
    ///
    /// Requires 8 through [`MAX_FRAME_LEN`] bytes. Exact response semantics
    /// are checked against the operation by the executor.
    pub fn complete(bytes: &[u8]) -> Result<Self, SnapshotReadError> {
        if !(8..=MAX_FRAME_LEN).contains(&bytes.len()) {
            return Err(SnapshotReadError::CompleteLength(bytes.len()));
        }
        Ok(Self {
            end: SnapshotReadEnd::Complete,
            bytes: bytes.to_vec(),
        })
    }

    /// Construct a bounded partial/empty deadline result.
    ///
    /// # Errors
    ///
    /// A buffer at the hard ceiling is an overflow, not a timeout.
    pub fn timed_out(bytes: &[u8]) -> Result<Self, SnapshotReadError> {
        if bytes.len() >= MAX_FRAME_LEN {
            return Err(SnapshotReadError::TimeoutLength(bytes.len()));
        }
        Ok(Self {
            end: SnapshotReadEnd::TimedOut,
            bytes: bytes.to_vec(),
        })
    }

    /// Read completion state.
    pub const fn end(&self) -> SnapshotReadEnd {
        self.end
    }

    /// Bounded byte count without exposing contents.
    pub const fn received_len(&self) -> usize {
        self.bytes.len()
    }

    fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl fmt::Debug for SnapshotRead {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotRead")
            .field("end", &self.end)
            .field("received", &self.bytes.len())
            .field("bytes", &"[redacted]")
            .finish()
    }
}

/// Invalid bounded read construction.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotReadError {
    /// A complete frame was shorter than the envelope or above the hard bound.
    #[error("complete snapshot read has invalid length {0}")]
    CompleteLength(usize),
    /// A timed-out read reached or exceeded the hard frame ceiling.
    #[error("timed-out snapshot read has {0} bytes; hard ceiling was reached")]
    TimeoutLength(usize),
}

/// One already-open, exclusive, fixed-38400 session.
///
/// Implementations open/configure exactly once before this value is supplied,
/// issue only each provided operation, and perform no enumeration or internal
/// retry. [`Self::finish`] consumes the session and succeeds only after saved
/// terminal state has been restored, read back, and the descriptor closed.
pub trait PersistentSnapshotSession: Sized {
    /// Platform-specific failure.
    type Error: StdError + Send + Sync + 'static;

    /// Execute one exact query on the same existing descriptor.
    ///
    /// # Errors
    ///
    /// Returns a platform error and issues no internal retry.
    fn exchange(&mut self, operation: SnapshotOperation) -> Result<SnapshotRead, Self::Error>;

    /// Write one exact checked function-`0x3f` remote-mode command.
    ///
    /// Implementations enforce [`SNAPSHOT_OPERATION_TIMEOUT`] and must not
    /// accept caller-supplied bytes or invent a disable operation.
    ///
    /// # Errors
    ///
    /// Returns a platform error and issues no internal retry.
    fn write_remote_mode(&mut self, command: &RemoteModeCommand) -> Result<(), Self::Error>;

    /// Restore, verify, and close the single session.
    ///
    /// # Errors
    ///
    /// Any uncertain restore/readback/close result is an error.
    fn finish(self) -> Result<(), Self::Error>;
}

/// Persistent session additionally capable of one closed typed mutation.
pub trait PersistentApplySession: PersistentSnapshotSession {
    /// Write exactly one checked function-`0x20` direct command.
    ///
    /// # Errors
    ///
    /// Returns a platform error. Implementations must not retry or accept raw
    /// caller bytes.
    fn write_direct(&mut self, command: &DirectParameterCommand) -> Result<(), Self::Error>;
}

/// Successful complete snapshot and sanitized persistent-session receipt.
pub struct CapturedSnapshotV1 {
    snapshot: SnapshotV1,
    valid_search_count: usize,
    baud: u32,
    close_verified: bool,
}

impl CapturedSnapshotV1 {
    /// Complete exact raw snapshot.
    pub const fn snapshot(&self) -> &SnapshotV1 {
        &self.snapshot
    }

    /// Valid same-address Search count used for this capture.
    ///
    /// Standalone and pre-apply baseline captures use ten. A post-mutation
    /// readback on the same identified descriptor uses one.
    pub const fn valid_search_count(&self) -> usize {
        self.valid_search_count
    }

    /// Fixed session baud, always 38400.
    pub const fn baud(&self) -> u32 {
        self.baud
    }

    /// Whether consuming restore/readback/close succeeded.
    pub const fn close_verified(&self) -> bool {
        self.close_verified
    }

    /// Consume the receipt and return the complete snapshot.
    pub fn into_snapshot(self) -> SnapshotV1 {
        self.snapshot
    }
}

impl fmt::Debug for CapturedSnapshotV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapturedSnapshotV1")
            .field("snapshot_digest", &self.snapshot.digest())
            .field("device", &self.snapshot.device())
            .field("valid_search_count", &self.valid_search_count)
            .field("baud", &self.baud)
            .field("close_verified", &self.close_verified)
            .finish()
    }
}

enum CaptureBodyError<E: StdError + Send + Sync + 'static, P: StdError + Send + Sync + 'static> {
    Pacing {
        trial: usize,
        source: P,
    },
    BudgetExceeded {
        operation: SnapshotOperationKind,
    },
    SearchQualificationIncomplete {
        valid: usize,
        required: usize,
        attempts: usize,
    },
    Transport {
        operation: SnapshotOperationKind,
        source: E,
    },
    RemoteMode {
        mode: RemoteMode,
        source: E,
    },
    Timeout {
        operation: SnapshotOperationKind,
        received: usize,
    },
    ResponseLimit {
        operation: SnapshotOperationKind,
        received: usize,
        limit: usize,
    },
    Validation {
        operation: SnapshotOperationKind,
        source: SnapshotError,
    },
}

/// Terminal one-session capture failure.
#[derive(Debug, Error)]
pub enum SnapshotCaptureError<
    E: StdError + Send + Sync + 'static,
    P: StdError + Send + Sync + 'static,
> {
    /// Pacing failed before a repeat Search.
    #[error("persistent Search cadence failed before trial {trial}")]
    Pacing {
        /// One-based Search attempt number.
        trial: usize,
        /// Pacer-specific cause.
        #[source]
        source: P,
        /// Session close failure, when close also failed.
        finish_error: Option<E>,
    },
    /// The fixed whole-session time budget was exceeded.
    #[error("persistent snapshot session exceeded its budget at {operation:?}")]
    BudgetExceeded {
        /// Operation that was not safely eligible.
        operation: SnapshotOperationKind,
        /// Session close failure, when close also failed.
        finish_error: Option<E>,
    },
    /// The bounded attempt window ended before enough valid identities arrived.
    #[error(
        "persistent Search qualification produced {valid} of {required} valid identities in {attempts} attempts"
    )]
    SearchQualificationIncomplete {
        /// Valid exact identities observed.
        valid: usize,
        /// Required valid identities.
        required: usize,
        /// Total bounded Search attempts issued.
        attempts: usize,
        /// Session close failure, when close also failed.
        finish_error: Option<E>,
    },
    /// Platform exchange failed.
    #[error("persistent snapshot transport failed at {operation:?}")]
    Transport {
        /// Failed operation.
        operation: SnapshotOperationKind,
        /// Platform-specific cause.
        #[source]
        source: E,
        /// Session close failure, when close also failed.
        finish_error: Option<E>,
    },
    /// A closed remote-mode prerequisite could not be sent.
    #[error("persistent snapshot remote mode {mode:?} failed")]
    RemoteMode {
        /// Exact closed mode that failed.
        mode: RemoteMode,
        /// Platform-specific cause.
        #[source]
        source: E,
        /// Session close failure, when close also failed.
        finish_error: Option<E>,
    },
    /// A deadline elapsed without a complete response.
    #[error("persistent snapshot timed out with {received} bytes at {operation:?}")]
    Timeout {
        /// Timed-out operation.
        operation: SnapshotOperationKind,
        /// Bounded partial byte count.
        received: usize,
        /// Session close failure, when close also failed.
        finish_error: Option<E>,
    },
    /// A carrier exceeded the exact response limit.
    #[error("persistent snapshot received {received} bytes at {operation:?}; limit is {limit}")]
    ResponseLimit {
        /// Operation whose limit was exceeded.
        operation: SnapshotOperationKind,
        /// Received byte count.
        received: usize,
        /// Exact operation limit.
        limit: usize,
        /// Session close failure, when close also failed.
        finish_error: Option<E>,
    },
    /// A complete candidate failed exact type/identity validation.
    #[error("persistent snapshot validation failed at {operation:?}: {source}")]
    Validation {
        /// Rejected operation.
        operation: SnapshotOperationKind,
        /// Pure validation failure.
        #[source]
        source: SnapshotError,
        /// Session close failure, when close also failed.
        finish_error: Option<E>,
    },
    /// All reads succeeded, but restore/readback/close did not.
    #[error("persistent snapshot session finish was not verified")]
    Finish {
        /// Platform-specific cause.
        #[source]
        source: E,
    },
}

/// Run ten Searches, transmit-enable, Dump0, Dump1, and a receive-only
/// transition on one 38400 session.
///
/// Search attempt one is immediate; every later attempt is preceded by the
/// existing five-second device cadence. Remote mode and Dump0 follow the final
/// bounded Search settlement immediately. Empty Search timeouts may be replayed
/// until ten valid identities arrive or twenty attempts are exhausted. Every
/// non-empty response must have its exact part-specific length and expected
/// device address. The executor always calls [`PersistentSnapshotSession::finish`],
/// including after an earlier failure, and returns a snapshot only when finish
/// succeeds.
///
/// # Errors
///
/// Stops on the first pacing, budget, transport, partial timeout, limit,
/// protocol, identity, part, qualification, or verified-finish failure. No
/// later operation is issued.
pub fn execute_persistent_snapshot<S: PersistentSnapshotSession, P: RepeatPacer>(
    mut session: S,
    pacer: &mut P,
    expected_device: DeviceId,
) -> Result<CapturedSnapshotV1, SnapshotCaptureError<S::Error, P::Error>> {
    let started = pacer.elapsed();
    let body = capture_body(
        &mut session,
        pacer,
        expected_device,
        PERSISTENT_SEARCH_COUNT,
        PERSISTENT_SEARCH_ATTEMPT_LIMIT,
        started,
        PERSISTENT_SNAPSHOT_BUDGET,
    );
    let finish = session.finish();
    finish_capture(body, finish, PERSISTENT_SEARCH_COUNT)
}

fn finish_capture<E: StdError + Send + Sync + 'static, P: StdError + Send + Sync + 'static>(
    body: Result<SnapshotV1, CaptureBodyError<E, P>>,
    finish: Result<(), E>,
    valid_search_count: usize,
) -> Result<CapturedSnapshotV1, SnapshotCaptureError<E, P>> {
    match (body, finish) {
        (Ok(snapshot), Ok(())) => Ok(CapturedSnapshotV1 {
            snapshot,
            valid_search_count,
            baud: FALLBACK_BAUD,
            close_verified: true,
        }),
        (Ok(_), Err(source)) => Err(SnapshotCaptureError::Finish { source }),
        (Err(error), finish) => Err(with_finish(error, finish.err())),
    }
}

fn capture_body<S: PersistentSnapshotSession, P: RepeatPacer>(
    session: &mut S,
    pacer: &mut P,
    expected_device: DeviceId,
    search_count: usize,
    search_attempt_limit: usize,
    started: Duration,
    budget: Duration,
) -> Result<SnapshotV1, CaptureBodyError<S::Error, P::Error>> {
    let identity_frame = capture_search_identities(
        session,
        pacer,
        expected_device,
        search_count,
        search_attempt_limit,
        started,
        budget,
    )?;

    let mode = RemoteMode::Transmit;
    require_budget(pacer, started, budget, SnapshotOperationKind::Dump0)?;
    session
        .write_remote_mode(&RemoteModeCommand::new(expected_device, mode))
        .map_err(|source| CaptureBodyError::RemoteMode { mode, source })?;
    require_budget(pacer, started, budget, SnapshotOperationKind::Dump0)?;

    let dump0_operation =
        SnapshotOperation::dump(DumpPart::Part0, expected_device).map_err(|source| {
            CaptureBodyError::Validation {
                operation: SnapshotOperationKind::Dump0,
                source: SnapshotError::Protocol(source),
            }
        })?;
    require_budget(pacer, started, budget, SnapshotOperationKind::Dump0)?;
    let dump0_frame = exchange_validated(session, dump0_operation)?;
    require_budget(pacer, started, budget, SnapshotOperationKind::Dump0)?;

    let dump1_operation =
        SnapshotOperation::dump(DumpPart::Part1, expected_device).map_err(|source| {
            CaptureBodyError::Validation {
                operation: SnapshotOperationKind::Dump1,
                source: SnapshotError::Protocol(source),
            }
        })?;
    require_budget(pacer, started, budget, SnapshotOperationKind::Dump1)?;
    let dump1_frame = exchange_validated(session, dump1_operation)?;
    require_budget(pacer, started, budget, SnapshotOperationKind::Dump1)?;

    let snapshot =
        SnapshotV1::from_frames(&identity_frame, &dump0_frame, &dump1_frame).map_err(|source| {
            CaptureBodyError::Validation {
                operation: SnapshotOperationKind::Dump1,
                source,
            }
        })?;

    let mode = RemoteMode::ReceiveDirect;
    require_budget(pacer, started, budget, SnapshotOperationKind::Dump1)?;
    session
        .write_remote_mode(&RemoteModeCommand::new(expected_device, mode))
        .map_err(|source| CaptureBodyError::RemoteMode { mode, source })?;
    require_budget(pacer, started, budget, SnapshotOperationKind::Dump1)?;
    Ok(snapshot)
}

fn capture_search_identities<S: PersistentSnapshotSession, P: RepeatPacer>(
    session: &mut S,
    pacer: &mut P,
    expected_device: DeviceId,
    required: usize,
    attempt_limit: usize,
    started: Duration,
    budget: Duration,
) -> Result<Vec<u8>, CaptureBodyError<S::Error, P::Error>> {
    debug_assert!(required > 0);
    let mut valid = 0_usize;
    let mut identity_frame = None;

    for attempt in 1..=attempt_limit {
        if attempt != 1 {
            pacer
                .wait(REPEAT_SEARCH_GAP)
                .map_err(|source| CaptureBodyError::Pacing {
                    trial: attempt,
                    source,
                })?;
        }
        let kind = SnapshotOperationKind::Search { sequence: attempt };
        require_budget::<S::Error, P>(pacer, started, budget, kind)?;
        let operation = SnapshotOperation::search(attempt, expected_device).map_err(|source| {
            CaptureBodyError::Validation {
                operation: kind,
                source: SnapshotError::Protocol(source),
            }
        })?;
        match exchange_validated(session, operation) {
            Ok(frame) => {
                if identity_frame.is_none() {
                    identity_frame = Some(frame);
                }
                valid += 1;
                if valid == required {
                    require_budget::<S::Error, P>(pacer, started, budget, kind)?;
                    return identity_frame.ok_or(CaptureBodyError::SearchQualificationIncomplete {
                        valid,
                        required,
                        attempts: attempt,
                    });
                }
            }
            Err(error @ CaptureBodyError::Timeout { received: 0, .. })
                if attempt_limit == required =>
            {
                return Err(error);
            }
            Err(CaptureBodyError::Timeout { received: 0, .. }) => {}
            Err(error) => return Err(error),
        }
        require_budget::<S::Error, P>(pacer, started, budget, kind)?;
    }

    Err(CaptureBodyError::SearchQualificationIncomplete {
        valid,
        required,
        attempts: attempt_limit,
    })
}

fn require_budget<E: StdError + Send + Sync + 'static, P: RepeatPacer>(
    pacer: &mut P,
    started: Duration,
    budget: Duration,
    operation: SnapshotOperationKind,
) -> Result<(), CaptureBodyError<E, P::Error>> {
    let elapsed = pacer.elapsed().saturating_sub(started);
    if elapsed > budget.saturating_sub(operation_timeout(operation)) {
        Err(CaptureBodyError::BudgetExceeded { operation })
    } else {
        Ok(())
    }
}

const fn operation_timeout(operation: SnapshotOperationKind) -> Duration {
    match operation {
        SnapshotOperationKind::Search { .. } => SNAPSHOT_OPERATION_TIMEOUT,
        SnapshotOperationKind::Dump0 | SnapshotOperationKind::Dump1 => SNAPSHOT_DUMP_TIMEOUT,
    }
}

fn exchange_validated<S: PersistentSnapshotSession, P: StdError + Send + Sync + 'static>(
    session: &mut S,
    operation: SnapshotOperation,
) -> Result<Vec<u8>, CaptureBodyError<S::Error, P>> {
    let kind = operation.kind();
    let read = session
        .exchange(operation)
        .map_err(|source| CaptureBodyError::Transport {
            operation: kind,
            source,
        })?;
    if read.received_len() > operation.response_limit() {
        return Err(CaptureBodyError::ResponseLimit {
            operation: kind,
            received: read.received_len(),
            limit: operation.response_limit(),
        });
    }
    if read.end() == SnapshotReadEnd::TimedOut {
        return Err(CaptureBodyError::Timeout {
            operation: kind,
            received: read.received_len(),
        });
    }
    validate_response(operation, read.bytes()).map_err(|source| CaptureBodyError::Validation {
        operation: kind,
        source,
    })?;
    Ok(read.bytes)
}

fn validate_response(operation: SnapshotOperation, frame: &[u8]) -> Result<(), SnapshotError> {
    match operation.kind() {
        SnapshotOperationKind::Search { .. } => {
            let identity = SearchResponse26::parse(frame)?;
            if identity.device() != operation.expected_device() {
                return Err(SnapshotError::DeviceMismatch {
                    section: SnapshotSection::Identity,
                    expected: operation.expected_device().get(),
                    actual: identity.device().get(),
                });
            }
        }
        SnapshotOperationKind::Dump0 | SnapshotOperationKind::Dump1 => {
            let (expected_part, expected_section) = match operation.kind() {
                SnapshotOperationKind::Dump0 => (DumpPart::Part0, SnapshotSection::Dump0),
                SnapshotOperationKind::Dump1 => (DumpPart::Part1, SnapshotSection::Dump1),
                SnapshotOperationKind::Search { .. } => unreachable!(),
            };
            if frame.len() != operation.response_limit() {
                return Err(SnapshotError::InvalidDumpLength {
                    part: expected_part,
                    expected: operation.response_limit(),
                    actual: frame.len(),
                });
            }
            match decode(parse_frame(frame)?)? {
                DecodedMessage::DumpResponse { device, part, .. }
                    if device == operation.expected_device() && part == expected_part => {}
                DecodedMessage::DumpResponse { device, .. }
                    if device != operation.expected_device() =>
                {
                    return Err(SnapshotError::DeviceMismatch {
                        section: expected_section,
                        expected: operation.expected_device().get(),
                        actual: device.get(),
                    });
                }
                DecodedMessage::DumpResponse { part, .. } => {
                    return Err(SnapshotError::WrongDumpPart {
                        expected: expected_part,
                        actual: part,
                    });
                }
                _ => {
                    return Err(SnapshotError::UnexpectedMessage {
                        section: expected_section,
                        kind: "non_dump_response",
                    });
                }
            }
        }
    }
    Ok(())
}

fn with_finish<E: StdError + Send + Sync + 'static, P: StdError + Send + Sync + 'static>(
    error: CaptureBodyError<E, P>,
    finish_error: Option<E>,
) -> SnapshotCaptureError<E, P> {
    match error {
        CaptureBodyError::Pacing { trial, source } => SnapshotCaptureError::Pacing {
            trial,
            source,
            finish_error,
        },
        CaptureBodyError::BudgetExceeded { operation } => SnapshotCaptureError::BudgetExceeded {
            operation,
            finish_error,
        },
        CaptureBodyError::SearchQualificationIncomplete {
            valid,
            required,
            attempts,
        } => SnapshotCaptureError::SearchQualificationIncomplete {
            valid,
            required,
            attempts,
            finish_error,
        },
        CaptureBodyError::Transport { operation, source } => SnapshotCaptureError::Transport {
            operation,
            source,
            finish_error,
        },
        CaptureBodyError::RemoteMode { mode, source } => SnapshotCaptureError::RemoteMode {
            mode,
            source,
            finish_error,
        },
        CaptureBodyError::Timeout {
            operation,
            received,
        } => SnapshotCaptureError::Timeout {
            operation,
            received,
            finish_error,
        },
        CaptureBodyError::ResponseLimit {
            operation,
            received,
            limit,
        } => SnapshotCaptureError::ResponseLimit {
            operation,
            received,
            limit,
            finish_error,
        },
        CaptureBodyError::Validation { operation, source } => SnapshotCaptureError::Validation {
            operation,
            source,
            finish_error,
        },
    }
}

/// Exact result of typed apply plus complete readback.
#[derive(Debug)]
pub enum ApplyReadbackOutcome {
    /// Complete readback exactly matched desired state.
    Verified(CapturedSnapshotV1),
    /// Complete readback differed and rollback is now required.
    RollbackRequired(CapturedSnapshotV1),
}

/// Exact result of typed rollback plus complete readback.
#[derive(Debug)]
pub enum RollbackReadbackOutcome {
    /// Complete readback restored exact immutable baseline equality.
    RolledBack(CapturedSnapshotV1),
    /// Complete readback still differed from baseline.
    Faulted(CapturedSnapshotV1),
}

/// Typed mutation/readback orchestration failure.
#[derive(Debug, Error)]
pub enum TransactionExecutionError<
    E: StdError + Send + Sync + 'static,
    P: StdError + Send + Sync + 'static,
> {
    /// Pure transaction sequencing or binding failed.
    #[error("transaction state or binding failed: {source}")]
    Transaction {
        /// Pure transaction failure.
        #[source]
        source: ApplyTransactionError,
        /// Session finish failure, when an already-open session also failed to close.
        finish_error: Option<E>,
    },
    /// Live complete state no longer equaled the plan baseline before write.
    #[error("stale apply baseline: expected {expected}, observed {observed}")]
    StaleBaseline {
        /// Plan-bound immutable baseline digest.
        expected: String,
        /// Fresh complete pre-write snapshot digest.
        observed: String,
        /// Session finish failure, when close also failed.
        finish_error: Option<E>,
    },
    /// Typed remote-mode or direct mutation failed.
    #[error("typed mutation prerequisite or direct command failed during {phase:?}")]
    Mutation {
        /// Apply or rollback phase.
        phase: MutationPhase,
        /// Platform-specific cause.
        #[source]
        source: E,
        /// Session finish failure, when close also failed.
        finish_error: Option<E>,
    },
    /// Snapshot capture or verified close failed.
    #[error(transparent)]
    Capture(#[from] SnapshotCaptureError<E, P>),
}

impl<E: StdError + Send + Sync + 'static, P: StdError + Send + Sync + 'static>
    From<ApplyTransactionError> for TransactionExecutionError<E, P>
{
    fn from(source: ApplyTransactionError) -> Self {
        Self::Transaction {
            source,
            finish_error: None,
        }
    }
}

/// Typed mutation phase for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationPhase {
    /// Desired-state action command.
    Apply,
    /// Inverse action command.
    Rollback,
}

/// Verify the live baseline, execute one typed apply command, then read back.
///
/// The command is drawn only from the already-staged [`dcx_core::ApplyPlanV1`].
/// Snapshot/profile mapping is out of this executor: no arbitrary bytes enter
/// the transport. The complete ten-Search baseline check, typed write, and
/// one-valid-Search readback all use the same already-open descriptor. That
/// readback may replay one empty Search timeout without repeating the mutation.
/// A stale baseline is rejected before mutation. Any post-mutation capture or
/// close uncertainty moves the pure transaction to `RollbackRequired`.
///
/// # Errors
///
/// Returns transaction, mutation, or capture failure.
pub fn execute_apply_readback<S: PersistentApplySession, P: RepeatPacer>(
    mut session: S,
    pacer: &mut P,
    transaction: &mut ApplyTransactionV1,
) -> Result<ApplyReadbackOutcome, TransactionExecutionError<S::Error, P::Error>> {
    let started = pacer.elapsed();
    let expected_device = transaction.apply_plan().device();
    let baseline_body = capture_body(
        &mut session,
        pacer,
        expected_device,
        PERSISTENT_SEARCH_COUNT,
        PERSISTENT_SEARCH_ATTEMPT_LIMIT,
        started,
        APPLY_TRANSACTION_BUDGET,
    );
    let observed_baseline = match baseline_body {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let finish_error = session.finish().err();
            return Err(with_finish(error, finish_error).into());
        }
    };
    if &observed_baseline != transaction.apply_plan().baseline() {
        let expected = transaction.apply_plan().baseline().digest().to_owned();
        let observed = observed_baseline.digest().to_owned();
        return Err(TransactionExecutionError::StaleBaseline {
            expected,
            observed,
            finish_error: session.finish().err(),
        });
    }

    if let Err(source) = transaction.begin_apply() {
        return Err(TransactionExecutionError::Transaction {
            source,
            finish_error: session.finish().err(),
        });
    }

    if let Some(command) = transaction.apply_plan().command() {
        let mode = RemoteModeCommand::new(expected_device, RemoteMode::ReceiveAndTransmit);
        if let Err(source) = session.write_remote_mode(&mode) {
            let finish_error = session.finish().err();
            transaction.require_rollback_after_error()?;
            return Err(TransactionExecutionError::Mutation {
                phase: MutationPhase::Apply,
                source,
                finish_error,
            });
        }
        if let Err(source) = session.write_direct(command) {
            let finish_error = session.finish().err();
            transaction.require_rollback_after_error()?;
            return Err(TransactionExecutionError::Mutation {
                phase: MutationPhase::Apply,
                source,
                finish_error,
            });
        }
    }

    let body = capture_body(
        &mut session,
        pacer,
        expected_device,
        READBACK_SEARCH_COUNT,
        READBACK_SEARCH_ATTEMPT_LIMIT,
        started,
        APPLY_TRANSACTION_BUDGET,
    );
    let finish = session.finish();
    let captured = match finish_capture(body, finish, READBACK_SEARCH_COUNT) {
        Ok(captured) => captured,
        Err(error) => {
            transaction.require_rollback_after_error()?;
            return Err(error.into());
        }
    };
    let exact = match transaction.verify_apply_readback(captured.snapshot()) {
        Ok(receipt) => receipt.is_exact(),
        Err(source) => {
            transaction.require_rollback_after_error()?;
            return Err(TransactionExecutionError::Transaction {
                source,
                finish_error: None,
            });
        }
    };
    if exact {
        Ok(ApplyReadbackOutcome::Verified(captured))
    } else {
        Ok(ApplyReadbackOutcome::RollbackRequired(captured))
    }
}

/// Verify identity, execute one typed inverse command, then read back.
///
/// The pre-write identity, typed inverse, and one-valid-Search complete readback
/// all use the same already-open descriptor. That readback may replay one empty
/// Search timeout without repeating the inverse. No caller-controlled bytes
/// enter the transport.
///
/// # Errors
///
/// Returns transaction, mutation, or capture failure. Any uncertainty marks
/// the pure transaction terminal `Faulted`.
pub fn execute_rollback_readback<S: PersistentApplySession, P: RepeatPacer>(
    mut session: S,
    pacer: &mut P,
    transaction: &mut ApplyTransactionV1,
) -> Result<RollbackReadbackOutcome, TransactionExecutionError<S::Error, P::Error>> {
    let started = pacer.elapsed();
    let device = transaction.rollback_plan().baseline().device();
    let identity_kind = SnapshotOperationKind::Search { sequence: 1 };
    let identity_body = (|| {
        let operation = SnapshotOperation::search(1, device).map_err(|source| {
            CaptureBodyError::Validation {
                operation: identity_kind,
                source: SnapshotError::Protocol(source),
            }
        })?;
        require_budget::<S::Error, P>(pacer, started, ROLLBACK_TRANSACTION_BUDGET, identity_kind)?;
        drop(exchange_validated::<S, P::Error>(&mut session, operation)?);
        require_budget::<S::Error, P>(pacer, started, ROLLBACK_TRANSACTION_BUDGET, identity_kind)
    })();
    if let Err(error) = identity_body {
        let finish_error = session.finish().err();
        return Err(with_finish(error, finish_error).into());
    }

    if let Err(source) = transaction.begin_rollback() {
        return Err(TransactionExecutionError::Transaction {
            source,
            finish_error: session.finish().err(),
        });
    }

    if let Some(command) = transaction.rollback_plan().command() {
        let mode = RemoteModeCommand::new(device, RemoteMode::ReceiveAndTransmit);
        if let Err(source) = session.write_remote_mode(&mode) {
            let finish_error = session.finish().err();
            transaction.fault_rollback_after_error()?;
            return Err(TransactionExecutionError::Mutation {
                phase: MutationPhase::Rollback,
                source,
                finish_error,
            });
        }
        if let Err(source) = session.write_direct(command) {
            let finish_error = session.finish().err();
            transaction.fault_rollback_after_error()?;
            return Err(TransactionExecutionError::Mutation {
                phase: MutationPhase::Rollback,
                source,
                finish_error,
            });
        }
    }

    let body = capture_body(
        &mut session,
        pacer,
        device,
        READBACK_SEARCH_COUNT,
        READBACK_SEARCH_ATTEMPT_LIMIT,
        started,
        ROLLBACK_TRANSACTION_BUDGET,
    );
    let finish = session.finish();
    let captured = match finish_capture(body, finish, READBACK_SEARCH_COUNT) {
        Ok(captured) => captured,
        Err(error) => {
            transaction.fault_rollback_after_error()?;
            return Err(error.into());
        }
    };
    let exact = match transaction.verify_rollback_readback(captured.snapshot()) {
        Ok(receipt) => receipt.is_exact(),
        Err(source) => {
            transaction.fault_rollback_after_error()?;
            return Err(TransactionExecutionError::Transaction {
                source,
                finish_error: None,
            });
        }
    };
    if exact {
        Ok(RollbackReadbackOutcome::RolledBack(captured))
    } else {
        Ok(RollbackReadbackOutcome::Faulted(captured))
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};

    use dcx_core::{
        ApplyTransactionState, DirectParameterAction,
        protocol::{DUMP0_RESPONSE_LEN, DUMP1_RESPONSE_LEN},
    };

    use super::*;

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
        fail: bool,
    }

    impl RepeatPacer for FakePacer {
        type Error = FakePacerError;

        fn elapsed(&mut self) -> Duration {
            self.elapsed
        }

        fn wait(&mut self, minimum: Duration) -> Result<(), Self::Error> {
            self.waits.push(minimum);
            if self.fail {
                return Err(FakePacerError);
            }
            self.elapsed += minimum;
            Ok(())
        }
    }

    #[derive(Default)]
    struct SessionLog {
        operations: Vec<SnapshotOperation>,
        modes: Vec<RemoteModeCommand>,
        writes: Vec<DirectParameterCommand>,
        finishes: usize,
    }

    enum Step {
        Read(SnapshotRead),
        Fail,
    }

    struct FakeSession {
        steps: VecDeque<Step>,
        log: Rc<RefCell<SessionLog>>,
        fail_write: bool,
        fail_finish: bool,
    }

    impl FakeSession {
        fn new(steps: impl IntoIterator<Item = Step>) -> (Self, Rc<RefCell<SessionLog>>) {
            let log = Rc::new(RefCell::new(SessionLog::default()));
            (
                Self {
                    steps: steps.into_iter().collect(),
                    log: Rc::clone(&log),
                    fail_write: false,
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

    impl PersistentApplySession for FakeSession {
        fn write_direct(&mut self, command: &DirectParameterCommand) -> Result<(), Self::Error> {
            self.log.borrow_mut().writes.push(command.clone());
            if self.fail_write {
                Err(FakeSessionError)
            } else {
                Ok(())
            }
        }
    }

    fn identity(device: u8) -> Vec<u8> {
        let mut frame = vec![0xf0, 0x00, 0x20, 0x32, device, 0x0e, 0x00];
        frame.extend_from_slice(b"SYNTHETIC-IDENTITY");
        frame.push(0xf7);
        frame
    }

    fn dump(device: u8, part: u8, marker: u8) -> Vec<u8> {
        let length = match part {
            0 => DUMP0_RESPONSE_LEN,
            1 => DUMP1_RESPONSE_LEN,
            _ => panic!("synthetic test part must be zero or one"),
        };
        let mut frame = vec![0; length];
        frame[..7].copy_from_slice(&[0xf0, 0, 0x20, 0x32, device, 0x0e, 0x10]);
        frame[7] = marker;
        frame[12] = part;
        frame[length - 1] = 0xf7;
        frame
    }

    fn complete_steps(device: u8, marker0: u8, marker1: u8) -> Vec<Step> {
        let mut steps = Vec::new();
        for _ in 0..PERSISTENT_SEARCH_COUNT {
            steps.push(Step::Read(
                SnapshotRead::complete(&identity(device)).unwrap(),
            ));
        }
        steps.push(Step::Read(
            SnapshotRead::complete(&dump(device, 0, marker0)).unwrap(),
        ));
        steps.push(Step::Read(
            SnapshotRead::complete(&dump(device, 1, marker1)).unwrap(),
        ));
        steps
    }

    fn snapshot_steps(snapshot: &SnapshotV1, search_count: usize) -> Vec<Step> {
        let mut steps = Vec::new();
        for _ in 0..search_count {
            steps.push(Step::Read(
                SnapshotRead::complete(snapshot.frame(SnapshotSection::Identity)).unwrap(),
            ));
        }
        steps.push(Step::Read(
            SnapshotRead::complete(snapshot.frame(SnapshotSection::Dump0)).unwrap(),
        ));
        steps.push(Step::Read(
            SnapshotRead::complete(snapshot.frame(SnapshotSection::Dump1)).unwrap(),
        ));
        steps
    }

    fn snapshot(device: u8, marker0: u8, marker1: u8) -> SnapshotV1 {
        SnapshotV1::from_frames(
            &identity(device),
            &dump(device, 0, marker0),
            &dump(device, 1, marker1),
        )
        .unwrap()
    }

    #[test]
    fn one_session_executes_ten_searches_then_exact_dumps_and_verified_finish() {
        let (session, log) = FakeSession::new(complete_steps(0, 1, 2));
        let mut pacer = FakePacer::default();
        let captured =
            execute_persistent_snapshot(session, &mut pacer, DeviceId::new(0).unwrap()).unwrap();

        assert_eq!(captured.valid_search_count(), PERSISTENT_SEARCH_COUNT);
        assert_eq!(captured.baud(), FALLBACK_BAUD);
        assert!(captured.close_verified());
        assert_eq!(captured.snapshot(), &snapshot(0, 1, 2));
        let log = log.borrow();
        assert_eq!(log.finishes, 1);
        assert_eq!(
            log.modes,
            [
                RemoteModeCommand::new(DeviceId::new(0).unwrap(), RemoteMode::Transmit),
                RemoteModeCommand::new(DeviceId::new(0).unwrap(), RemoteMode::ReceiveDirect),
            ]
        );
        assert_eq!(log.operations.len(), PERSISTENT_SEARCH_COUNT + 2);
        for (index, operation) in log.operations[..PERSISTENT_SEARCH_COUNT].iter().enumerate() {
            assert_eq!(
                operation.kind(),
                SnapshotOperationKind::Search {
                    sequence: index + 1,
                }
            );
            assert_eq!(
                operation.request().as_bytes(),
                [0xf0, 0, 0x20, 0x32, 0x20, 0x0e, 0x40, 0xf7]
            );
            assert_eq!(operation.response_limit(), SEARCH_RESPONSE_LEN);
        }
        let dump0 = log.operations[PERSISTENT_SEARCH_COUNT];
        assert_eq!(dump0.kind(), SnapshotOperationKind::Dump0);
        assert_eq!(dump0.response_limit(), DUMP0_RESPONSE_LEN);
        assert_eq!(
            dump0.request().as_bytes(),
            [0xf0, 0, 0x20, 0x32, 0, 0x0e, 0x50, 1, 0, 0, 0xf7]
        );
        let dump1 = log.operations[PERSISTENT_SEARCH_COUNT + 1];
        assert_eq!(dump1.kind(), SnapshotOperationKind::Dump1);
        assert_eq!(dump1.response_limit(), DUMP1_RESPONSE_LEN);
        assert_eq!(
            pacer.waits,
            [REPEAT_SEARCH_GAP; PERSISTENT_SEARCH_COUNT - 1]
        );
    }

    #[test]
    fn empty_search_timeouts_are_replayed_until_ten_valid_identities_arrive() {
        let mut steps = Vec::new();
        for _ in 0..PERSISTENT_SEARCH_COUNT {
            steps.push(Step::Read(SnapshotRead::timed_out(&[]).unwrap()));
            steps.push(Step::Read(SnapshotRead::complete(&identity(0)).unwrap()));
        }
        steps.push(Step::Read(SnapshotRead::complete(&dump(0, 0, 1)).unwrap()));
        steps.push(Step::Read(SnapshotRead::complete(&dump(0, 1, 2)).unwrap()));
        let (session, log) = FakeSession::new(steps);
        let mut pacer = FakePacer::default();

        let captured =
            execute_persistent_snapshot(session, &mut pacer, DeviceId::new(0).unwrap()).unwrap();

        assert_eq!(captured.valid_search_count(), PERSISTENT_SEARCH_COUNT);
        assert_eq!(captured.snapshot(), &snapshot(0, 1, 2));
        let log = log.borrow();
        assert_eq!(log.operations.len(), PERSISTENT_SEARCH_ATTEMPT_LIMIT + 2);
        assert_eq!(
            log.operations[PERSISTENT_SEARCH_ATTEMPT_LIMIT].kind(),
            SnapshotOperationKind::Dump0
        );
        assert_eq!(pacer.waits.len(), PERSISTENT_SEARCH_ATTEMPT_LIMIT - 1);
        assert_eq!(log.finishes, 1);
    }

    #[test]
    fn wrong_repeat_identity_stops_before_dumps_and_still_finishes() {
        let steps = [
            Step::Read(SnapshotRead::complete(&identity(0)).unwrap()),
            Step::Read(SnapshotRead::complete(&identity(1)).unwrap()),
        ];
        let (session, log) = FakeSession::new(steps);
        assert!(matches!(
            execute_persistent_snapshot(
                session,
                &mut FakePacer::default(),
                DeviceId::new(0).unwrap(),
            ),
            Err(SnapshotCaptureError::Validation {
                operation: SnapshotOperationKind::Search { sequence: 2 },
                source: SnapshotError::DeviceMismatch { .. },
                finish_error: None,
            })
        ));
        assert_eq!(log.borrow().operations.len(), 2);
        assert_eq!(log.borrow().finishes, 1);
    }

    #[test]
    fn partial_dump_cannot_be_promoted_to_a_complete_snapshot() {
        let mut steps = complete_steps(0, 1, 2);
        steps[PERSISTENT_SEARCH_COUNT] =
            Step::Read(SnapshotRead::complete(&dump(0, 0, 1)[..100]).unwrap());
        let (session, log) = FakeSession::new(steps);
        assert!(matches!(
            execute_persistent_snapshot(
                session,
                &mut FakePacer::default(),
                DeviceId::new(0).unwrap(),
            ),
            Err(SnapshotCaptureError::Validation {
                operation: SnapshotOperationKind::Dump0,
                source: SnapshotError::InvalidDumpLength {
                    expected: DUMP0_RESPONSE_LEN,
                    actual: 100,
                    ..
                },
                ..
            })
        ));
        assert_eq!(log.borrow().operations.len(), PERSISTENT_SEARCH_COUNT + 1);
        assert_eq!(log.borrow().finishes, 1);
    }

    #[test]
    fn timeout_transport_and_finish_failures_are_terminal_and_consuming() {
        let (session, log) =
            FakeSession::new([Step::Read(SnapshotRead::timed_out(&[0xf0]).unwrap())]);
        assert!(matches!(
            execute_persistent_snapshot(
                session,
                &mut FakePacer::default(),
                DeviceId::new(0).unwrap(),
            ),
            Err(SnapshotCaptureError::Timeout {
                operation: SnapshotOperationKind::Search { sequence: 1 },
                received: 1,
                finish_error: None,
            })
        ));
        assert_eq!(log.borrow().finishes, 1);

        let (session, log) = FakeSession::new([Step::Fail]);
        assert!(matches!(
            execute_persistent_snapshot(
                session,
                &mut FakePacer::default(),
                DeviceId::new(0).unwrap(),
            ),
            Err(SnapshotCaptureError::Transport {
                operation: SnapshotOperationKind::Search { sequence: 1 },
                finish_error: None,
                ..
            })
        ));
        assert_eq!(log.borrow().finishes, 1);

        let (mut session, log) = FakeSession::new(complete_steps(0, 1, 2));
        session.fail_finish = true;
        assert!(matches!(
            execute_persistent_snapshot(
                session,
                &mut FakePacer::default(),
                DeviceId::new(0).unwrap(),
            ),
            Err(SnapshotCaptureError::Finish { .. })
        ));
        assert_eq!(log.borrow().finishes, 1);
    }

    #[test]
    fn typed_apply_and_inverse_commands_drive_readback_and_rollback() {
        let baseline = snapshot(0, 0, 0);
        let apply = DirectParameterAction::new(5, 0x3c, 40).unwrap();
        let inverse = DirectParameterAction::new(5, 0x3c, 0).unwrap();
        let desired = baseline.project_direct_actions(&[apply]).unwrap();
        let mut transaction =
            ApplyTransactionV1::stage(baseline.clone(), desired, vec![apply], vec![inverse])
                .unwrap();

        let mut apply_steps = snapshot_steps(&baseline, PERSISTENT_SEARCH_COUNT);
        apply_steps.extend(snapshot_steps(&snapshot(0, 0, 1), READBACK_SEARCH_COUNT));
        let (session, apply_log) = FakeSession::new(apply_steps);
        assert!(matches!(
            execute_apply_readback(session, &mut FakePacer::default(), &mut transaction).unwrap(),
            ApplyReadbackOutcome::RollbackRequired(_)
        ));
        assert_eq!(transaction.state(), ApplyTransactionState::RollbackRequired);
        assert_eq!(apply_log.borrow().writes.len(), 1);
        assert_eq!(apply_log.borrow().writes[0].actions(), [apply]);
        assert_eq!(
            apply_log
                .borrow()
                .modes
                .iter()
                .map(|command| command.mode())
                .collect::<Vec<_>>(),
            [
                RemoteMode::Transmit,
                RemoteMode::ReceiveDirect,
                RemoteMode::ReceiveAndTransmit,
                RemoteMode::Transmit,
                RemoteMode::ReceiveDirect,
            ]
        );

        let mut rollback_steps = vec![Step::Read(
            SnapshotRead::complete(baseline.frame(SnapshotSection::Identity)).unwrap(),
        )];
        rollback_steps.extend(snapshot_steps(&baseline, READBACK_SEARCH_COUNT));
        let (session, rollback_log) = FakeSession::new(rollback_steps);
        let mut rollback_pacer = FakePacer::default();
        assert!(matches!(
            execute_rollback_readback(session, &mut rollback_pacer, &mut transaction).unwrap(),
            RollbackReadbackOutcome::RolledBack(_)
        ));
        assert!(rollback_pacer.waits.is_empty());
        assert_eq!(transaction.state(), ApplyTransactionState::RolledBack);
        assert_eq!(rollback_log.borrow().writes.len(), 1);
        assert_eq!(rollback_log.borrow().writes[0].actions(), [inverse]);
        assert_eq!(
            rollback_log
                .borrow()
                .modes
                .iter()
                .map(|command| command.mode())
                .collect::<Vec<_>>(),
            [
                RemoteMode::ReceiveAndTransmit,
                RemoteMode::Transmit,
                RemoteMode::ReceiveDirect,
            ]
        );
    }

    #[test]
    fn mutation_failure_requires_rollback_and_verified_finish() {
        let baseline = snapshot(0, 0, 0);
        let apply = DirectParameterAction::new(5, 0x3c, 40).unwrap();
        let inverse = DirectParameterAction::new(5, 0x3c, 0).unwrap();
        let desired = baseline.project_direct_actions(&[apply]).unwrap();
        let mut transaction =
            ApplyTransactionV1::stage(baseline, desired, vec![apply], vec![inverse]).unwrap();
        let (mut session, log) = FakeSession::new(complete_steps(0, 0, 0));
        session.fail_write = true;
        assert!(matches!(
            execute_apply_readback(session, &mut FakePacer::default(), &mut transaction),
            Err(TransactionExecutionError::Mutation {
                phase: MutationPhase::Apply,
                finish_error: None,
                ..
            })
        ));
        assert_eq!(transaction.state(), ApplyTransactionState::RollbackRequired);
        assert_eq!(log.borrow().finishes, 1);
    }

    #[test]
    fn stale_live_baseline_is_rejected_before_write() {
        let baseline = snapshot(0, 0, 0);
        let apply = DirectParameterAction::new(5, 0x3c, 40).unwrap();
        let desired = baseline.project_direct_actions(&[apply]).unwrap();
        let inverse = baseline.inverse_actions_for(&[apply]).unwrap();
        let mut transaction =
            ApplyTransactionV1::stage(baseline, desired, vec![apply], inverse).unwrap();
        let observed = snapshot(0, 1, 0);
        let (session, log) = FakeSession::new(snapshot_steps(&observed, PERSISTENT_SEARCH_COUNT));

        assert!(matches!(
            execute_apply_readback(session, &mut FakePacer::default(), &mut transaction),
            Err(TransactionExecutionError::StaleBaseline {
                finish_error: None,
                ..
            })
        ));
        assert_eq!(transaction.state(), ApplyTransactionState::Staged);
        let log = log.borrow();
        assert!(log.writes.is_empty());
        assert_eq!(log.finishes, 1);
        assert_eq!(log.operations.len(), PERSISTENT_SEARCH_COUNT + 2);
    }

    #[test]
    fn exact_post_apply_readback_verifies_with_one_search() {
        let baseline = snapshot(0, 0, 0);
        let apply = DirectParameterAction::new(5, 0x3c, 40).unwrap();
        let desired = baseline.project_direct_actions(&[apply]).unwrap();
        let inverse = baseline.inverse_actions_for(&[apply]).unwrap();
        let mut transaction =
            ApplyTransactionV1::stage(baseline.clone(), desired.clone(), vec![apply], inverse)
                .unwrap();
        let mut steps = snapshot_steps(&baseline, PERSISTENT_SEARCH_COUNT);
        steps.extend(snapshot_steps(&desired, READBACK_SEARCH_COUNT));
        let (session, log) = FakeSession::new(steps);

        let outcome =
            execute_apply_readback(session, &mut FakePacer::default(), &mut transaction).unwrap();
        let ApplyReadbackOutcome::Verified(captured) = outcome else {
            panic!("exact readback must verify")
        };
        assert_eq!(captured.valid_search_count(), READBACK_SEARCH_COUNT);
        assert_eq!(transaction.state(), ApplyTransactionState::Verified);
        assert_eq!(log.borrow().operations.len(), PERSISTENT_SEARCH_COUNT + 5);
    }

    #[test]
    fn post_apply_readback_replays_one_empty_search_without_repeating_mutation() {
        let baseline = snapshot(0, 0, 0);
        let apply = DirectParameterAction::new(5, 0x3c, 40).unwrap();
        let desired = baseline.project_direct_actions(&[apply]).unwrap();
        let inverse = baseline.inverse_actions_for(&[apply]).unwrap();
        let mut transaction =
            ApplyTransactionV1::stage(baseline.clone(), desired.clone(), vec![apply], inverse)
                .unwrap();
        let mut steps = snapshot_steps(&baseline, PERSISTENT_SEARCH_COUNT);
        steps.push(Step::Read(SnapshotRead::timed_out(&[]).unwrap()));
        steps.extend(snapshot_steps(&desired, READBACK_SEARCH_COUNT));
        let (session, log) = FakeSession::new(steps);
        let mut pacer = FakePacer::default();

        let outcome = execute_apply_readback(session, &mut pacer, &mut transaction).unwrap();
        let ApplyReadbackOutcome::Verified(captured) = outcome else {
            panic!("read-only Search replay must preserve exact apply verification")
        };

        assert_eq!(captured.snapshot(), &desired);
        assert_eq!(captured.valid_search_count(), READBACK_SEARCH_COUNT);
        assert_eq!(transaction.state(), ApplyTransactionState::Verified);
        let log = log.borrow();
        assert_eq!(log.writes.len(), 1);
        assert_eq!(log.writes[0].actions(), [apply]);
        let readback_start = PERSISTENT_SEARCH_COUNT + 2;
        assert_eq!(
            log.operations[readback_start..]
                .iter()
                .map(|operation| operation.kind())
                .collect::<Vec<_>>(),
            [
                SnapshotOperationKind::Search { sequence: 1 },
                SnapshotOperationKind::Search { sequence: 2 },
                SnapshotOperationKind::Dump0,
                SnapshotOperationKind::Dump1,
            ]
        );
        assert_eq!(pacer.waits.len(), PERSISTENT_SEARCH_COUNT);
    }

    #[test]
    fn post_rollback_readback_replays_one_empty_search_without_repeating_inverse() {
        let baseline = snapshot(0, 0, 0);
        let apply = DirectParameterAction::new(5, 0x3c, 40).unwrap();
        let inverse = DirectParameterAction::new(5, 0x3c, 0).unwrap();
        let desired = baseline.project_direct_actions(&[apply]).unwrap();
        let staged =
            ApplyTransactionV1::stage(baseline.clone(), desired, vec![apply], vec![inverse])
                .unwrap();
        let rollback_plan = staged.rollback_plan().clone();
        let mut transaction = ApplyTransactionV1::resume_rollback(&rollback_plan).unwrap();
        let mut steps = vec![Step::Read(
            SnapshotRead::complete(baseline.frame(SnapshotSection::Identity)).unwrap(),
        )];
        steps.push(Step::Read(SnapshotRead::timed_out(&[]).unwrap()));
        steps.extend(snapshot_steps(&baseline, READBACK_SEARCH_COUNT));
        let (session, log) = FakeSession::new(steps);
        let mut pacer = FakePacer::default();

        let outcome = execute_rollback_readback(session, &mut pacer, &mut transaction).unwrap();
        let RollbackReadbackOutcome::RolledBack(captured) = outcome else {
            panic!("read-only Search replay must preserve exact rollback verification")
        };

        assert_eq!(captured.snapshot(), &baseline);
        assert_eq!(captured.valid_search_count(), READBACK_SEARCH_COUNT);
        assert_eq!(transaction.state(), ApplyTransactionState::RolledBack);
        let log = log.borrow();
        assert_eq!(log.writes.len(), 1);
        assert_eq!(log.writes[0].actions(), [inverse]);
        assert_eq!(
            log.operations
                .iter()
                .map(|operation| operation.kind())
                .collect::<Vec<_>>(),
            [
                SnapshotOperationKind::Search { sequence: 1 },
                SnapshotOperationKind::Search { sequence: 1 },
                SnapshotOperationKind::Search { sequence: 2 },
                SnapshotOperationKind::Dump0,
                SnapshotOperationKind::Dump1,
            ]
        );
        assert_eq!(pacer.waits, [REPEAT_SEARCH_GAP]);
    }

    #[test]
    fn raw_snapshot_reads_are_redacted_from_debug() {
        let debug = format!("{:?}", SnapshotRead::complete(&identity(0)).unwrap());
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("SYNTHETIC"));
    }
}
