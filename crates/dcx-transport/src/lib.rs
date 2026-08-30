//! Injected, bounded transport contract for DCX2496 discovery.
//!
//! This crate does not enumerate or open ports and has no operating-system
//! serial dependency. It exposes one operation: the exact broadcast Search
//! request. A platform adapter may implement [`SearchTransport`]; Legalab owns
//! the external hardware and operator preconditions for invoking it.

use std::{error::Error as StdError, fmt, time::Duration};

use dcx_core::{
    discovery::{
        DiscoveryAttempt, DiscoveryAttemptKind, DiscoveryError, DiscoveryState, FALLBACK_BAUD,
        PRIMARY_BAUD, QueryOnlyDiscovery, SerialSettings,
    },
    protocol::{DeviceId, SEARCH_RESPONSE_LEN, SearchResponse26},
};
use thiserror::Error;

pub mod snapshot;

/// Exact outbound byte count for the only operation this boundary exposes.
pub const SEARCH_REQUEST_LEN: usize = 8;
/// Maximum inbound byte count for one Search attempt.
pub const SEARCH_RESPONSE_LIMIT: usize = SEARCH_RESPONSE_LEN;
/// Total deadline an adapter must enforce for one configure/write/read attempt.
pub const SEARCH_ATTEMPT_TIMEOUT: Duration = Duration::from_millis(500);
/// Maximum number of attempts represented by the executor.
pub const MAX_SEARCH_ATTEMPTS: usize = 2;
/// Nominal sum of the two per-attempt deadlines, excluding immediate errors.
pub const SEARCH_DISCOVERY_BUDGET: Duration = Duration::from_millis(1_000);
/// Number of same-baud Search trials after the first identity.
pub const REPEAT_SEARCH_COUNT: usize = 9;
/// Minimum delay before every repeat Search.
///
/// The pinned `DuinoDCX` behavioral reference searches on a five-second cadence.
/// Keeping the same cadence avoids overrunning the legacy device's discovery
/// response path.
pub const REPEAT_SEARCH_GAP: Duration = Duration::from_secs(5);
/// Whole nominal budget for the fixed nine-trial repeat session.
pub const REPEAT_SEARCH_BUDGET: Duration = Duration::from_secs(60);

const SEARCH_REQUEST_BYTES: [u8; SEARCH_REQUEST_LEN] =
    [0xf0, 0x00, 0x20, 0x32, 0x20, 0x0e, 0x40, 0xf7];

/// The sole outbound request available to a transport implementation.
///
/// Fields are private and there is no public constructor. Callers cannot use
/// this boundary to supply arbitrary protocol or configuration bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchRequest([u8; SEARCH_REQUEST_LEN]);

impl SearchRequest {
    const fn new() -> Self {
        Self(SEARCH_REQUEST_BYTES)
    }

    /// Return the exact eight Search bytes.
    pub const fn as_bytes(&self) -> &[u8; SEARCH_REQUEST_LEN] {
        &self.0
    }
}

/// Closed kind of one typed Search operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOperationKind {
    /// Vendor-documented 115200 attempt in the generic discovery plan.
    Primary,
    /// 38400 attempt reached after an empty primary timeout.
    SingleFallback,
    /// Direct 38400 binding used by the observed MVP device path.
    Known38400,
}

/// One immutable operation issued by a Search executor.
///
/// The timeout covers the complete adapter attempt: serial configuration,
/// writing all eight Search bytes, and reading no more than 26 bytes. The
/// future adapter is responsible for enforcing that deadline; this injected
/// executor deliberately owns no clock, thread, file descriptor, or TTY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOperation {
    kind: SearchOperationKind,
    settings: SerialSettings,
    expected_device: DeviceId,
}

impl SearchOperation {
    const fn new(attempt: DiscoveryAttempt, expected_device: DeviceId) -> Self {
        Self {
            kind: match attempt.kind() {
                DiscoveryAttemptKind::Primary => SearchOperationKind::Primary,
                DiscoveryAttemptKind::SingleFallback => SearchOperationKind::SingleFallback,
            },
            settings: attempt.settings(),
            expected_device,
        }
    }

    /// Return the closed operation kind.
    pub const fn kind(self) -> SearchOperationKind {
        self.kind
    }

    /// Return exact line settings for this attempt.
    pub const fn settings(self) -> SerialSettings {
        self.settings
    }

    /// Return the expected unit address bound by the caller.
    pub const fn expected_device(self) -> DeviceId {
        self.expected_device
    }

    /// Return the only outbound request exposed by this crate.
    pub const fn request(self) -> SearchRequest {
        SearchRequest::new()
    }

    /// Return the total deadline the adapter must enforce for this attempt.
    pub const fn timeout(self) -> Duration {
        SEARCH_ATTEMPT_TIMEOUT
    }

    /// Return the hard input ceiling for this attempt.
    pub const fn response_limit(self) -> usize {
        SEARCH_RESPONSE_LIMIT
    }
}

fn operation_for_attempt(
    attempt: DiscoveryAttemptKind,
    expected_device: DeviceId,
) -> Result<SearchOperation, DiscoveryError> {
    let mut discovery = QueryOnlyDiscovery::new(expected_device);
    if attempt == DiscoveryAttemptKind::SingleFallback {
        discovery.timeout_current()?;
    }
    discovery
        .current_attempt()
        .map(|planned| SearchOperation::new(planned, expected_device))
        .ok_or(DiscoveryError::NoPendingAttempt)
}

fn operation_for_kind(
    kind: SearchOperationKind,
    expected_device: DeviceId,
) -> Result<SearchOperation, DiscoveryError> {
    let planned = match kind {
        SearchOperationKind::Primary => DiscoveryAttemptKind::Primary,
        SearchOperationKind::SingleFallback | SearchOperationKind::Known38400 => {
            DiscoveryAttemptKind::SingleFallback
        }
    };
    let mut operation = operation_for_attempt(planned, expected_device)?;
    operation.kind = kind;
    Ok(operation)
}

/// How one bounded adapter read ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchReadEnd {
    /// Exactly 26 bytes were received before the deadline.
    Complete,
    /// The 500 ms deadline elapsed with fewer than 26 bytes.
    TimedOut,
}

/// Bounded response bytes returned by a transport implementation.
///
/// The raw bytes are intentionally omitted from `Debug`; captures and opaque
/// identity payloads must remain outside Git. Construction enforces that a
/// complete read is exactly 26 bytes and that a timeout contains at most 25.
#[derive(Clone, PartialEq, Eq)]
pub struct SearchRead {
    end: SearchReadEnd,
    bytes: [u8; SEARCH_RESPONSE_LIMIT],
    received: usize,
}

impl SearchRead {
    /// Construct an exact-length completed read.
    ///
    /// # Errors
    ///
    /// Returns [`SearchReadError::CompleteLength`] unless `bytes` has exactly
    /// 26 elements.
    pub fn complete(bytes: &[u8]) -> Result<Self, SearchReadError> {
        if bytes.len() != SEARCH_RESPONSE_LIMIT {
            return Err(SearchReadError::CompleteLength(bytes.len()));
        }
        let mut bounded = [0; SEARCH_RESPONSE_LIMIT];
        bounded.copy_from_slice(bytes);
        Ok(Self {
            end: SearchReadEnd::Complete,
            bytes: bounded,
            received: SEARCH_RESPONSE_LIMIT,
        })
    }

    /// Construct a deadline result carrying any partial bytes already read.
    ///
    /// # Errors
    ///
    /// Returns [`SearchReadError::TimeoutLength`] for 26 or more bytes. A full
    /// response must be returned through [`Self::complete`].
    pub fn timed_out(bytes: &[u8]) -> Result<Self, SearchReadError> {
        if bytes.len() >= SEARCH_RESPONSE_LIMIT {
            return Err(SearchReadError::TimeoutLength(bytes.len()));
        }
        let mut bounded = [0; SEARCH_RESPONSE_LIMIT];
        bounded[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            end: SearchReadEnd::TimedOut,
            bytes: bounded,
            received: bytes.len(),
        })
    }

    /// Return how the bounded read ended.
    pub const fn end(&self) -> SearchReadEnd {
        self.end
    }

    /// Return the number of bytes read, without exposing their contents.
    pub const fn received_len(&self) -> usize {
        self.received
    }

    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.received_len()]
    }
}

impl fmt::Debug for SearchRead {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SearchRead")
            .field("end", &self.end)
            .field("received", &self.received)
            .field("bytes", &"[redacted]")
            .finish()
    }
}

/// Invalid adapter result construction.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SearchReadError {
    /// A completed read was not the exact response size.
    #[error("complete Search read has {0} bytes; expected exactly 26")]
    CompleteLength(usize),
    /// A timeout incorrectly contained a complete or oversized response.
    #[error("timed-out Search read has {0} bytes; expected at most 25")]
    TimeoutLength(usize),
}

/// Injected boundary implemented later by a separately reviewed serial adapter.
///
/// An implementation must configure the supplied settings, write only
/// [`SearchOperation::request`], read at most
/// [`SearchOperation::response_limit`] bytes, and return by
/// [`SearchOperation::timeout`]. It must not enumerate ports, retry internally,
/// issue another query type, or retain raw response bytes. The executor owns
/// attempt ordering and fallback policy.
pub trait SearchTransport {
    /// Adapter-specific error. Errors always stop discovery without fallback.
    type Error: StdError + Send + Sync + 'static;

    /// Execute exactly one supplied Search operation.
    ///
    /// # Errors
    ///
    /// Returns an adapter-specific failure. The executor stops immediately
    /// and does not expose the fallback after any adapter error.
    fn search(&mut self, operation: SearchOperation) -> Result<SearchRead, Self::Error>;
}

/// Injected monotonic clock and delay boundary for an exact repeat session.
///
/// Implementations must wait for at least the supplied duration. Keeping this
/// boundary separate from [`SearchTransport`] leaves ordinary discovery free
/// of retry or sleep behavior and makes the nine-trial policy testable without
/// wall-clock delays.
pub trait RepeatPacer {
    /// Pacer-specific failure.
    type Error: StdError + Send + Sync + 'static;

    /// Return monotonic elapsed time from an implementation-owned epoch.
    fn elapsed(&mut self) -> Duration;

    /// Wait for at least the supplied minimum interval.
    ///
    /// # Errors
    ///
    /// Returns an implementation-specific failure without issuing a Search.
    fn wait(&mut self, minimum: Duration) -> Result<(), Self::Error>;
}

/// Successful attempt derived directly from the first validated Search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepeatSearchBinding {
    expected_device: DeviceId,
    successful_kind: SearchOperationKind,
}

impl RepeatSearchBinding {
    /// Derive the repeat session from the successful typed Search result.
    pub const fn from_identified(search: &IdentifiedSearch) -> Self {
        Self {
            expected_device: search.device(),
            successful_kind: search.kind(),
        }
    }

    /// Expected device address from the first successful receipt.
    pub const fn expected_device(self) -> DeviceId {
        self.expected_device
    }

    /// Successful first-attempt baud pinned for all nine trials.
    pub const fn successful_baud(self) -> u32 {
        match self.successful_kind {
            SearchOperationKind::Primary => PRIMARY_BAUD,
            SearchOperationKind::SingleFallback | SearchOperationKind::Known38400 => FALLBACK_BAUD,
        }
    }
}

/// Sanitized success summary for all nine same-baud trials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepeatedSearch {
    expected_device: DeviceId,
    successful_baud: u32,
    valid_response_count: usize,
}

impl RepeatedSearch {
    /// Expected and observed device address for every response.
    pub const fn device(self) -> DeviceId {
        self.expected_device
    }

    /// Baud rate reused from the first successful Search.
    pub const fn baud(self) -> u32 {
        self.successful_baud
    }

    /// Number of valid responses, always nine for a returned value.
    pub const fn valid_response_count(self) -> usize {
        self.valid_response_count
    }
}

/// Terminal failure from the fixed repeat executor.
#[derive(Debug, Error)]
pub enum RepeatSearchError<T: StdError + Send + Sync + 'static, P: StdError + Send + Sync + 'static>
{
    /// The injected pacer failed before the named trial.
    #[error("repeat pacing failed before trial {trial}")]
    Pacing {
        /// One-based repeat trial number.
        trial: usize,
        /// Pacer-specific cause.
        #[source]
        source: P,
    },
    /// The one-minute whole-session budget was exceeded.
    #[error("repeat session exceeded its 60 second budget at trial {trial}")]
    BudgetExceeded {
        /// One-based repeat trial number.
        trial: usize,
    },
    /// The injected carrier failed; no later trial is eligible.
    #[error("repeat Search transport failed during trial {trial}")]
    Transport {
        /// One-based repeat trial number.
        trial: usize,
        /// Carrier-specific cause.
        #[source]
        source: T,
    },
    /// A repeat timed out, including an empty timeout; no fallback is allowed.
    #[error("repeat Search timed out with {received} bytes during trial {trial}")]
    Timeout {
        /// One-based repeat trial number.
        trial: usize,
        /// Bounded bytes received before the deadline.
        received: usize,
    },
    /// Exact response or expected-device validation failed.
    #[error("repeat Search validation failed during trial {trial}: {source}")]
    Validation {
        /// One-based repeat trial number.
        trial: usize,
        /// Pure discovery failure.
        #[source]
        source: DiscoveryError,
    },
}

/// Execute exactly nine same-baud Searches from reviewed first-success evidence.
///
/// Every trial is preceded by at least five seconds of injected pacing. The baud is
/// pinned to the successful primary or fallback rate; a repeat never restarts
/// discovery and therefore can never change baud. Any timeout, invalid response,
/// transport error, pacing error, or whole-session budget overrun stops without
/// issuing a later trial. Opaque response payloads are validated but neither
/// retained nor compared for equality.
///
/// # Errors
///
/// Returns [`RepeatSearchError`] on the first terminal failure.
pub fn execute_search_repeat<T: SearchTransport, P: RepeatPacer>(
    transport: &mut T,
    pacer: &mut P,
    binding: RepeatSearchBinding,
) -> Result<RepeatedSearch, RepeatSearchError<T::Error, P::Error>> {
    let start = pacer.elapsed();
    let operation = operation_for_kind(binding.successful_kind, binding.expected_device)
        .map_err(|source| RepeatSearchError::Validation { trial: 1, source })?;

    for index in 0..REPEAT_SEARCH_COUNT {
        let trial = index + 1;
        pacer
            .wait(REPEAT_SEARCH_GAP)
            .map_err(|source| RepeatSearchError::Pacing { trial, source })?;
        let elapsed = pacer.elapsed().saturating_sub(start);
        if elapsed > REPEAT_SEARCH_BUDGET.saturating_sub(operation.timeout()) {
            return Err(RepeatSearchError::BudgetExceeded { trial });
        }

        let read = transport
            .search(operation)
            .map_err(|source| RepeatSearchError::Transport { trial, source })?;
        if pacer.elapsed().saturating_sub(start) > REPEAT_SEARCH_BUDGET {
            return Err(RepeatSearchError::BudgetExceeded { trial });
        }

        match read.end() {
            SearchReadEnd::Complete => {
                let mut discovery = QueryOnlyDiscovery::new(binding.expected_device);
                if binding.successful_kind != SearchOperationKind::Primary {
                    discovery
                        .timeout_current()
                        .map_err(|source| RepeatSearchError::Validation { trial, source })?;
                }
                discovery
                    .accept_candidates(&[read.bytes()])
                    .map_err(|source| RepeatSearchError::Validation { trial, source })?;
            }
            SearchReadEnd::TimedOut => {
                return Err(RepeatSearchError::Timeout {
                    trial,
                    received: read.received_len(),
                });
            }
        }
    }

    Ok(RepeatedSearch {
        expected_device: binding.expected_device,
        successful_baud: binding.successful_baud(),
        valid_response_count: REPEAT_SEARCH_COUNT,
    })
}

/// Successful exact identity returned by the executor.
pub struct IdentifiedSearch {
    kind: SearchOperationKind,
    response: SearchResponse26,
}

impl IdentifiedSearch {
    /// Return the operation kind that produced the validated identity.
    pub const fn kind(&self) -> SearchOperationKind {
        self.kind
    }

    /// Return the validated expected unit address.
    pub const fn device(&self) -> DeviceId {
        self.response.device()
    }

    /// Borrow the typed exact response for bounded local evidence handling.
    ///
    /// The opaque payload may contain device-specific material. Do not log or
    /// commit it; durable receipts should store only a digest and byte count.
    pub const fn response(&self) -> &SearchResponse26 {
        &self.response
    }
}

impl fmt::Debug for IdentifiedSearch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IdentifiedSearch")
            .field("kind", &self.kind)
            .field("device", &self.device())
            .field("response_bytes", &SEARCH_RESPONSE_LIMIT)
            .field("response", &"[redacted]")
            .finish()
    }
}

/// Terminal result from the exact two-attempt policy.
#[derive(Debug)]
pub enum SearchOutcome {
    /// One exact response matched the caller-bound unit address.
    Identified(IdentifiedSearch),
    /// Both attempts reached their deadline without receiving any bytes.
    Exhausted,
}

/// Fail-closed execution errors. None permit the fallback except an empty
/// primary timeout, which is represented internally rather than as an error.
#[derive(Debug, Error)]
pub enum SearchExecutionError<E: StdError + Send + Sync + 'static> {
    /// The injected adapter failed; no later attempt is eligible.
    #[error("Search transport failed during {attempt:?} attempt")]
    Transport {
        /// Attempt on which the adapter failed.
        attempt: DiscoveryAttemptKind,
        /// Adapter-specific cause.
        #[source]
        source: E,
    },
    /// A deadline arrived after a partial response; changing baud is unsafe.
    #[error("Search timed out with {received} partial bytes during {attempt:?} attempt")]
    PartialTimeout {
        /// Attempt on which partial input was received.
        attempt: DiscoveryAttemptKind,
        /// Bounded number of bytes received before the deadline.
        received: usize,
    },
    /// Exact response validation or pure state progression failed.
    #[error("Search validation failed during {attempt:?} attempt: {source}")]
    Validation {
        /// Attempt on which validation failed.
        attempt: DiscoveryAttemptKind,
        /// Pure discovery failure.
        #[source]
        source: DiscoveryError,
    },
}

/// Result from one direct Search at the observed 38400 MVP binding.
#[derive(Debug)]
pub enum Known38400SearchOutcome {
    /// One exact response matched the caller-bound unit address.
    Identified(IdentifiedSearch),
    /// The single attempt reached its deadline without receiving bytes.
    TimedOut,
}

/// Fail-closed error from one direct 38400 Search.
#[derive(Debug, Error)]
pub enum Known38400SearchError<E: StdError + Send + Sync + 'static> {
    /// The injected adapter failed.
    #[error("known 38400 Search transport failed")]
    Transport {
        /// Adapter-specific cause.
        #[source]
        source: E,
    },
    /// A deadline arrived after a partial response.
    #[error("known 38400 Search timed out with {received} partial bytes")]
    PartialTimeout {
        /// Bounded number of bytes received before the deadline.
        received: usize,
    },
    /// Exact response validation or operation construction failed.
    #[error("known 38400 Search validation failed: {source}")]
    Validation {
        /// Pure discovery failure.
        #[source]
        source: DiscoveryError,
    },
}

/// Execute one Search at the closed 38400 MVP binding.
///
/// This path exists for the named device after its working baud has been
/// observed. It issues no 115200 probe and exposes no caller-selected baud.
///
/// # Errors
///
/// Returns [`Known38400SearchError`] for transport, partial-timeout, protocol,
/// identity, or impossible state errors.
pub fn execute_known_38400_search<T: SearchTransport>(
    transport: &mut T,
    expected_device: DeviceId,
) -> Result<Known38400SearchOutcome, Known38400SearchError<T::Error>> {
    let operation = operation_for_kind(SearchOperationKind::Known38400, expected_device)
        .map_err(|source| Known38400SearchError::Validation { source })?;
    let read = transport
        .search(operation)
        .map_err(|source| Known38400SearchError::Transport { source })?;

    match read.end() {
        SearchReadEnd::Complete => {
            let mut discovery = QueryOnlyDiscovery::new(expected_device);
            discovery
                .timeout_current()
                .map_err(|source| Known38400SearchError::Validation { source })?;
            let response = discovery
                .accept_candidates(&[read.bytes()])
                .map_err(|source| Known38400SearchError::Validation { source })?;
            Ok(Known38400SearchOutcome::Identified(IdentifiedSearch {
                kind: SearchOperationKind::Known38400,
                response,
            }))
        }
        SearchReadEnd::TimedOut if read.received_len() != 0 => {
            Err(Known38400SearchError::PartialTimeout {
                received: read.received_len(),
            })
        }
        SearchReadEnd::TimedOut => Ok(Known38400SearchOutcome::TimedOut),
    }
}

/// Execute the exact Search-only discovery policy through an injected adapter.
///
/// Attempt one is 115200 8N1/no-flow. Exactly one 38400 8N1/no-flow fallback
/// is issued only when attempt one reaches 500 ms with zero input bytes. Any
/// partial timeout, transport error, malformed frame, or wrong identity stops
/// immediately. The executor supplies no arbitrary bytes and performs no I/O
/// except through [`SearchTransport::search`].
///
/// # Errors
///
/// Returns [`SearchExecutionError`] for transport, partial-timeout, protocol,
/// identity, or impossible state errors.
pub fn execute_search<T: SearchTransport>(
    transport: &mut T,
    expected_device: DeviceId,
) -> Result<SearchOutcome, SearchExecutionError<T::Error>> {
    let mut discovery = QueryOnlyDiscovery::new(expected_device);

    loop {
        let attempt =
            discovery
                .current_attempt()
                .ok_or_else(|| SearchExecutionError::Validation {
                    attempt: DiscoveryAttemptKind::SingleFallback,
                    source: DiscoveryError::NoPendingAttempt,
                })?;
        let kind = attempt.kind();
        let operation = SearchOperation::new(attempt, expected_device);
        let operation_kind = operation.kind();
        let read =
            transport
                .search(operation)
                .map_err(|source| SearchExecutionError::Transport {
                    attempt: kind,
                    source,
                })?;

        match read.end() {
            SearchReadEnd::Complete => {
                let response = discovery
                    .accept_candidates(&[read.bytes()])
                    .map_err(|source| SearchExecutionError::Validation {
                        attempt: kind,
                        source,
                    })?;
                return Ok(SearchOutcome::Identified(IdentifiedSearch {
                    kind: operation_kind,
                    response,
                }));
            }
            SearchReadEnd::TimedOut if read.received_len() != 0 => {
                return Err(SearchExecutionError::PartialTimeout {
                    attempt: kind,
                    received: read.received_len(),
                });
            }
            SearchReadEnd::TimedOut => {
                let state = discovery.timeout_current().map_err(|source| {
                    SearchExecutionError::Validation {
                        attempt: kind,
                        source,
                    }
                })?;
                if state == DiscoveryState::Exhausted {
                    return Ok(SearchOutcome::Exhausted);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use dcx_core::{
        discovery::{FALLBACK_BAUD, PRIMARY_BAUD, SerialFlowControl, SerialParity},
        protocol::{ProtocolError, Query},
    };

    use super::*;

    #[derive(Debug, Error)]
    #[error("synthetic transport failure")]
    struct FakeError;

    enum Step {
        Read(SearchRead),
        Fail,
    }

    #[derive(Default)]
    struct FakeTransport {
        steps: VecDeque<Step>,
        operations: Vec<SearchOperation>,
    }

    #[derive(Debug, Error)]
    #[error("synthetic pacing failure")]
    struct FakePacingError;

    #[derive(Default)]
    struct FakePacer {
        now: Duration,
        waits: Vec<Duration>,
        next_wait: Option<Duration>,
        fail: bool,
    }

    impl RepeatPacer for FakePacer {
        type Error = FakePacingError;

        fn elapsed(&mut self) -> Duration {
            self.now
        }

        fn wait(&mut self, minimum: Duration) -> Result<(), Self::Error> {
            self.waits.push(minimum);
            if self.fail {
                return Err(FakePacingError);
            }
            self.now += self.next_wait.take().unwrap_or(minimum);
            Ok(())
        }
    }

    impl FakeTransport {
        fn new(steps: impl IntoIterator<Item = Step>) -> Self {
            Self {
                steps: steps.into_iter().collect(),
                operations: Vec::new(),
            }
        }
    }

    impl SearchTransport for FakeTransport {
        type Error = FakeError;

        fn search(&mut self, operation: SearchOperation) -> Result<SearchRead, Self::Error> {
            self.operations.push(operation);
            match self.steps.pop_front().expect("test supplied a step") {
                Step::Read(read) => Ok(read),
                Step::Fail => Err(FakeError),
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

    fn assert_operation(operation: SearchOperation, kind: SearchOperationKind, baud: u32) {
        assert_eq!(operation.kind(), kind);
        assert_eq!(operation.expected_device(), DeviceId::new(0).unwrap());
        assert_eq!(operation.settings().baud(), baud);
        assert_eq!(operation.settings().data_bits(), 8);
        assert_eq!(operation.settings().stop_bits(), 1);
        assert_eq!(operation.settings().parity(), SerialParity::None);
        assert_eq!(operation.settings().flow_control(), SerialFlowControl::None);
        assert_eq!(operation.request().as_bytes(), &SEARCH_REQUEST_BYTES);
        assert_eq!(operation.timeout(), Duration::from_millis(500));
        assert_eq!(operation.response_limit(), 26);
    }

    fn identified(kind: SearchOperationKind, device: u8) -> IdentifiedSearch {
        IdentifiedSearch {
            kind,
            response: SearchResponse26::parse(&synthetic_response(device)).unwrap(),
        }
    }

    #[test]
    fn exact_search_request_cannot_drift_from_the_pure_typed_query() {
        assert_eq!(
            Query::Search.encode().unwrap().as_slice(),
            SEARCH_REQUEST_BYTES
        );
    }

    #[test]
    fn exact_primary_identity_stops_after_one_attempt() {
        let frame = synthetic_response(0);
        let mut transport = FakeTransport::new([Step::Read(SearchRead::complete(&frame).unwrap())]);

        let outcome = execute_search(&mut transport, DeviceId::new(0).unwrap()).unwrap();
        let SearchOutcome::Identified(identity) = outcome else {
            panic!("expected exact identity")
        };
        assert_eq!(identity.kind(), SearchOperationKind::Primary);
        assert_eq!(identity.device(), DeviceId::new(0).unwrap());
        assert_eq!(identity.response().opaque_payload(), b"SYNTHETIC-IDENTITY");
        assert_eq!(transport.operations.len(), 1);
        assert_operation(
            transport.operations[0],
            SearchOperationKind::Primary,
            PRIMARY_BAUD,
        );
    }

    #[test]
    fn one_empty_primary_timeout_unlocks_exactly_one_fallback() {
        let frame = synthetic_response(0);
        let mut transport = FakeTransport::new([
            Step::Read(SearchRead::timed_out(&[]).unwrap()),
            Step::Read(SearchRead::complete(&frame).unwrap()),
        ]);

        let outcome = execute_search(&mut transport, DeviceId::new(0).unwrap()).unwrap();
        let SearchOutcome::Identified(identity) = outcome else {
            panic!("expected fallback identity")
        };
        assert_eq!(identity.kind(), SearchOperationKind::SingleFallback);
        assert_eq!(transport.operations.len(), MAX_SEARCH_ATTEMPTS);
        assert_operation(
            transport.operations[0],
            SearchOperationKind::Primary,
            PRIMARY_BAUD,
        );
        assert_operation(
            transport.operations[1],
            SearchOperationKind::SingleFallback,
            FALLBACK_BAUD,
        );
    }

    #[test]
    fn known_38400_search_issues_one_truthful_operation() {
        let frame = synthetic_response(0);
        let mut transport = FakeTransport::new([Step::Read(SearchRead::complete(&frame).unwrap())]);

        let Known38400SearchOutcome::Identified(identity) =
            execute_known_38400_search(&mut transport, DeviceId::new(0).unwrap()).unwrap()
        else {
            panic!("expected known-38400 identity")
        };

        assert_eq!(identity.kind(), SearchOperationKind::Known38400);
        assert_eq!(identity.device(), DeviceId::new(0).unwrap());
        assert_eq!(transport.operations.len(), 1);
        assert_operation(
            transport.operations[0],
            SearchOperationKind::Known38400,
            FALLBACK_BAUD,
        );
        assert_eq!(
            RepeatSearchBinding::from_identified(&identity).successful_baud(),
            FALLBACK_BAUD
        );
    }

    #[test]
    fn two_empty_timeouts_exhaust_without_a_third_attempt() {
        let mut transport = FakeTransport::new([
            Step::Read(SearchRead::timed_out(&[]).unwrap()),
            Step::Read(SearchRead::timed_out(&[]).unwrap()),
        ]);

        assert!(matches!(
            execute_search(&mut transport, DeviceId::new(0).unwrap()).unwrap(),
            SearchOutcome::Exhausted
        ));
        assert_eq!(transport.operations.len(), MAX_SEARCH_ATTEMPTS);
    }

    #[test]
    fn partial_timeout_stops_without_fallback() {
        let mut transport =
            FakeTransport::new([Step::Read(SearchRead::timed_out(&[0xf0, 0x00]).unwrap())]);

        assert!(matches!(
            execute_search(&mut transport, DeviceId::new(0).unwrap()),
            Err(SearchExecutionError::PartialTimeout {
                attempt: DiscoveryAttemptKind::Primary,
                received: 2,
            })
        ));
        assert_eq!(transport.operations.len(), 1);
    }

    #[test]
    fn invalid_identity_stops_without_fallback() {
        let frame = synthetic_response(1);
        let mut transport = FakeTransport::new([Step::Read(SearchRead::complete(&frame).unwrap())]);

        assert!(matches!(
            execute_search(&mut transport, DeviceId::new(0).unwrap()),
            Err(SearchExecutionError::Validation {
                attempt: DiscoveryAttemptKind::Primary,
                source: DiscoveryError::UnexpectedDevice {
                    expected: 0,
                    actual: 1,
                },
            })
        ));
        assert_eq!(transport.operations.len(), 1);
    }

    #[test]
    fn transport_error_stops_without_fallback() {
        let mut transport = FakeTransport::new([Step::Fail]);

        assert!(matches!(
            execute_search(&mut transport, DeviceId::new(0).unwrap()),
            Err(SearchExecutionError::Transport {
                attempt: DiscoveryAttemptKind::Primary,
                ..
            })
        ));
        assert_eq!(transport.operations.len(), 1);
    }

    #[test]
    fn malformed_exact_length_frame_stops_without_fallback() {
        let mut frame = synthetic_response(0);
        frame[5] = 0x0f;
        let mut transport = FakeTransport::new([Step::Read(SearchRead::complete(&frame).unwrap())]);

        assert!(matches!(
            execute_search(&mut transport, DeviceId::new(0).unwrap()),
            Err(SearchExecutionError::Validation {
                source: DiscoveryError::Protocol(ProtocolError::WrongModel(0x0f)),
                ..
            })
        ));
        assert_eq!(transport.operations.len(), 1);
    }

    #[test]
    fn repeat_executes_exactly_nine_primary_searches_with_fixed_pacing() {
        assert_eq!(REPEAT_SEARCH_COUNT, 9);
        let frame = synthetic_response(0);
        let steps =
            (0..REPEAT_SEARCH_COUNT).map(|_| Step::Read(SearchRead::complete(&frame).unwrap()));
        let mut transport = FakeTransport::new(steps);
        let mut pacer = FakePacer::default();
        let first = identified(SearchOperationKind::Primary, 0);
        let binding = RepeatSearchBinding::from_identified(&first);

        let repeated = execute_search_repeat(&mut transport, &mut pacer, binding).unwrap();
        assert_eq!(repeated.device(), DeviceId::new(0).unwrap());
        assert_eq!(repeated.baud(), PRIMARY_BAUD);
        assert_eq!(repeated.valid_response_count(), REPEAT_SEARCH_COUNT);
        assert_eq!(transport.operations.len(), REPEAT_SEARCH_COUNT);
        assert!(transport.operations.iter().all(|operation| {
            operation.kind() == SearchOperationKind::Primary
                && operation.settings().baud() == PRIMARY_BAUD
        }));
        assert_eq!(pacer.waits, [REPEAT_SEARCH_GAP; REPEAT_SEARCH_COUNT]);
    }

    #[test]
    fn repeat_pins_fallback_baud_and_stops_on_first_timeout() {
        let frame = synthetic_response(0);
        let mut transport = FakeTransport::new([
            Step::Read(SearchRead::complete(&frame).unwrap()),
            Step::Read(SearchRead::timed_out(&[]).unwrap()),
            Step::Read(SearchRead::complete(&frame).unwrap()),
        ]);
        let mut pacer = FakePacer::default();
        let first = identified(SearchOperationKind::SingleFallback, 0);
        let binding = RepeatSearchBinding::from_identified(&first);

        assert!(matches!(
            execute_search_repeat(&mut transport, &mut pacer, binding),
            Err(RepeatSearchError::Timeout {
                trial: 2,
                received: 0,
            })
        ));
        assert_eq!(transport.operations.len(), 2);
        assert!(transport.operations.iter().all(|operation| {
            operation.kind() == SearchOperationKind::SingleFallback
                && operation.settings().baud() == FALLBACK_BAUD
        }));
        assert_eq!(pacer.waits.len(), 2);
    }

    #[test]
    fn repeat_rejects_insufficient_remaining_budget() {
        let frame = synthetic_response(0);
        let mut transport = FakeTransport::new([Step::Read(SearchRead::complete(&frame).unwrap())]);
        let mut pacer = FakePacer {
            next_wait: Some(
                REPEAT_SEARCH_BUDGET.saturating_sub(SEARCH_ATTEMPT_TIMEOUT)
                    + Duration::from_millis(1),
            ),
            ..FakePacer::default()
        };
        let first = identified(SearchOperationKind::Primary, 0);
        let binding = RepeatSearchBinding::from_identified(&first);

        assert!(matches!(
            execute_search_repeat(&mut transport, &mut pacer, binding),
            Err(RepeatSearchError::BudgetExceeded { trial: 1 })
        ));
        assert!(transport.operations.is_empty());
    }

    #[test]
    fn read_debug_output_redacts_raw_identity_bytes() {
        let frame = synthetic_response(0);
        let debug = format!("{:?}", SearchRead::complete(&frame).unwrap());
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("SYNTHETIC"));
    }
}
