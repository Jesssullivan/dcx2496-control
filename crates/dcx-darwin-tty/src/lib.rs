//! Darwin-only persistent tty carrier for the bounded DCX2496 control boundary.
//!
//! The public carrier exists only on macOS. It accepts one explicit, validated
//! callout-device path and exposes only typed Search, remote-mode, Dump, and
//! direct-parameter operations. There is no port enumeration, generic
//! byte-write method, retry loop, or durable raw capture. While one descriptor
//! remains open, the carrier may retain exactly one fixed-size accepted Search
//! response solely to reject or consume a byte-identical late replay. That
//! ephemeral value is never logged, serialized, or included in receipts. Tests
//! exercise the same state machine through injected fake syscalls without
//! opening a device.

use std::{
    fmt,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    time::Duration,
};

use dcx_core::{
    discovery::FALLBACK_BAUD,
    protocol::{
        DeviceId, DirectParameterCommand, MAX_FRAME_LEN, ProtocolError, RemoteMode,
        RemoteModeCommand,
    },
};
use dcx_transport::{
    SEARCH_ATTEMPT_TIMEOUT, SEARCH_REQUEST_LEN, SEARCH_RESPONSE_LIMIT, SearchOperation,
    SearchOperationKind, SearchRead, SearchReadEnd, SearchReadError, SearchTransport,
    snapshot::{
        PersistentApplySession, PersistentSnapshotSession, SnapshotOperation,
        SnapshotOperationKind, SnapshotRead, SnapshotReadEnd, SnapshotReadError,
    },
};
use serde::{Serialize, Serializer};
use sha2::{Digest, Sha256};
use thiserror::Error;

const PRIVATE_CALLOUT_PREFIX: &[u8] = b"/dev/cu.usbserial-";
const SHA256_PREFIX: &str = "sha256/";
/// Portion of the 500 ms whole-attempt budget reserved for restoration/close.
pub const SEARCH_CLEANUP_RESERVE: Duration = Duration::from_millis(25);
/// Quiet observation after the explicit `ReceiveDirect` recovery write.
const RECOVERY_QUIET_WINDOW: Duration = Duration::from_millis(25);

/// A lower-case, prefixed SHA-256 digest safe for sanitized receipts.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    fn of_bytes(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(SHA256_PREFIX)?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Serialize for Sha256Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// One explicit Darwin callout path for the persistent control carrier.
///
/// The path is validated at construction and again immediately before every
/// open. It is intentionally omitted from `Debug` and carrier receipts.
pub struct PrivateTtyBinding {
    path: PathBuf,
    digest: Sha256Digest,
}

impl PrivateTtyBinding {
    /// Bind one exact FTDI-style Darwin callout path.
    ///
    /// # Errors
    ///
    /// Returns [`BindingError`] when the path is outside the narrow
    /// `/dev/cu.usbserial-*` callout namespace.
    pub fn new(path: PathBuf) -> Result<Self, BindingError> {
        validate_private_path(&path)?;
        let digest = Sha256Digest::of_bytes(path.as_os_str().as_bytes());
        Ok(Self { path, digest })
    }

    /// Return only the sanitized path digest.
    pub const fn digest(&self) -> Sha256Digest {
        self.digest
    }

    fn verify(&self) -> Result<(), BindingError> {
        validate_private_path(&self.path)
    }
}

impl fmt::Debug for PrivateTtyBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PrivateTtyBinding")
            .field("path", &"[redacted]")
            .field("digest", &self.digest)
            .finish()
    }
}

fn validate_private_path(path: &Path) -> Result<(), BindingError> {
    let bytes = path.as_os_str().as_bytes();
    let Some(suffix) = bytes.strip_prefix(PRIVATE_CALLOUT_PREFIX) else {
        return Err(BindingError::UnsupportedPrivatePath);
    };
    if suffix.is_empty() || suffix.contains(&b'/') || suffix.contains(&0) {
        return Err(BindingError::UnsupportedPrivatePath);
    }
    Ok(())
}

/// Fail-closed callout-path validation failures.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum BindingError {
    /// The path was not one exact FTDI-style Darwin callout node.
    #[error("tty path is outside the supported Darwin callout namespace")]
    UnsupportedPrivatePath,
}

/// A named carrier operation suitable for sanitized failure receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CarrierStage {
    /// Reject an operation that does not match the open session binding.
    VerifyOperation,
    /// Revalidate the explicit callout path.
    VerifyBinding,
    /// Open the callout with the declared nonblocking/no-controlling-tty policy.
    OpenExclusive,
    /// Snapshot the complete termios structure.
    SnapshotTermios,
    /// Snapshot all modem control-line bits.
    SnapshotControlLines,
    /// Apply exact raw 8N1/no-flow settings.
    Configure,
    /// Check queued input before configuration or the sole outbound write.
    CheckPreexistingInput,
    /// Perform one exact typed request write syscall.
    Write,
    /// Query queued input without consuming bytes.
    BytesAvailable,
    /// Wait for readability within the remaining monotonic deadline.
    WaitReadable,
    /// Consume no more than the remaining response allowance.
    Read,
    /// Discard queued input during explicit recovery or terminal cleanup.
    DiscardInput,
    /// Restore the saved termios structure.
    RestoreTermios,
    /// Read termios back and compare every represented field with the snapshot.
    VerifyTermiosRestore,
    /// Restore the saved modem control-line bits.
    RestoreControlLines,
    /// Read modem control-line bits back and compare them with the snapshot.
    VerifyControlLinesRestore,
    /// Close the owned descriptor after all restoration attempts.
    Close,
}

/// Broad terminal reason retained when cleanup must replace a primary error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CarrierFailureKind {
    /// An operation did not match the fixed session binding.
    Operation,
    /// Private binding did not revalidate.
    Binding,
    /// The monotonic attempt budget was exhausted outside the read timeout.
    Deadline,
    /// A system call failed.
    System,
    /// One typed write did not accept its complete frame.
    ShortWrite,
    /// Input was already queued before a typed write.
    PreexistingInput,
    /// More input was pending than the current typed response permits.
    Overflow,
    /// The tty reached end-of-file.
    EndOfFile,
    /// An internal bounded-read result invariant failed.
    ReadInvariant,
}

/// Error from one bounded Darwin tty operation.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DarwinCarrierError {
    /// A typed operation did not match the fixed baud of an open session.
    #[error(
        "Search operation baud {actual_baud} does not match persistent session baud {expected_baud}"
    )]
    OperationMismatch {
        /// Baud configured once when the session opened.
        expected_baud: u32,
        /// Baud requested by the rejected typed operation.
        actual_baud: u32,
    },
    /// A recovery-capable operation was supplied a mode other than receive-direct.
    #[error("recovery quiescence requires ReceiveDirect, got {actual:?}")]
    RecoveryModeMismatch {
        /// Rejected closed mode.
        actual: RemoteMode,
    },
    /// Receive-direct cleanup targeted a different device than the enabling mode.
    #[error("recovery quiescence device mismatch")]
    RecoveryDeviceMismatch {
        /// Device whose enabling-mode attempt armed cleanup.
        expected: DeviceId,
        /// Device supplied to the receive-direct cleanup.
        actual: DeviceId,
    },
    /// An earlier queued/trailing-input failure made this session terminal.
    #[error("tty session requires input cleanup and close before another operation")]
    InputRecoveryRequired,
    /// Exact callout-path validation failed before any open.
    #[error(transparent)]
    Binding(#[from] BindingError),
    /// The monotonic 500 ms operation deadline expired.
    #[error("typed tty operation exceeded its monotonic deadline at {stage:?}")]
    Deadline {
        /// Operation that was about to run or had just completed.
        stage: CarrierStage,
    },
    /// A Darwin system operation failed; no path or raw bytes are retained.
    #[error("Darwin tty system operation failed at {stage:?} (errno {errno})")]
    System {
        /// Failing operation.
        stage: CarrierStage,
        /// Stable numeric Darwin errno.
        errno: i32,
    },
    /// One typed write did not accept its complete frame.
    #[error("typed tty write accepted {written} bytes; expected exactly {expected}")]
    ShortWrite {
        /// Number accepted by the single syscall.
        written: usize,
        /// Exact encoded request length.
        expected: usize,
    },
    /// Input was queued before configuration or a typed write.
    #[error("typed tty operation blocked because {queued} pre-existing input bytes were queued")]
    PreexistingInput {
        /// Bytes observed without consuming them during operation admission.
        queued: usize,
    },
    /// More bytes were queued than the exact response budget permits.
    #[error("typed response overflow: {received} received and {queued} additional queued")]
    Overflow {
        /// Bytes already consumed within the bounded frame reader.
        received: usize,
        /// Bytes observed pending without consuming them.
        queued: usize,
    },
    /// The tty returned EOF before an exact response or timeout.
    #[error("tty reached EOF after {received} response bytes")]
    EndOfFile {
        /// Bytes received before EOF.
        received: usize,
    },
    /// One trailing frame was partial or differed from the accepted response.
    #[error("typed response had an unexpected trailing frame of {received} bytes")]
    UnexpectedTrailingFrame {
        /// Bytes consumed from the unexpected trailing candidate.
        received: usize,
    },
    /// The bounded result constructor rejected internal state.
    #[error(transparent)]
    ReadInvariant(#[from] SearchReadError),
    /// The bounded snapshot result constructor rejected internal state.
    #[error(transparent)]
    SnapshotReadInvariant(#[from] SnapshotReadError),
    /// A checked typed command failed its final protocol encoding invariant.
    #[error("checked typed command failed to encode: {0}")]
    ProtocolEncoding(#[from] ProtocolError),
    /// Input cleanup or mandatory state restoration failed; close still ran.
    #[error(
        "tty cleanup failed (primary={primary:?}, input_failed={input_failed}, termios_failed={termios_failed}, control_lines_failed={control_lines_failed})"
    )]
    Cleanup {
        /// Broad primary failure, if cleanup followed an earlier failure.
        primary: Option<CarrierFailureKind>,
        /// Whether the one-shot input discard/readback cleanup failed.
        input_failed: bool,
        /// Whether restoring termios failed.
        termios_failed: bool,
        /// Whether restoring modem control lines failed.
        control_lines_failed: bool,
    },
}

impl DarwinCarrierError {
    const fn kind(&self) -> CarrierFailureKind {
        match self {
            Self::OperationMismatch { .. }
            | Self::RecoveryModeMismatch { .. }
            | Self::RecoveryDeviceMismatch { .. }
            | Self::InputRecoveryRequired
            | Self::ProtocolEncoding(_) => CarrierFailureKind::Operation,
            Self::Binding(_) => CarrierFailureKind::Binding,
            Self::Deadline { .. } => CarrierFailureKind::Deadline,
            Self::System { .. } | Self::Cleanup { .. } => CarrierFailureKind::System,
            Self::ShortWrite { .. } => CarrierFailureKind::ShortWrite,
            Self::PreexistingInput { .. } => CarrierFailureKind::PreexistingInput,
            Self::Overflow { .. } => CarrierFailureKind::Overflow,
            Self::EndOfFile { .. } => CarrierFailureKind::EndOfFile,
            Self::UnexpectedTrailingFrame { .. }
            | Self::ReadInvariant(_)
            | Self::SnapshotReadInvariant(_) => CarrierFailureKind::ReadInvariant,
        }
    }
}

/// Cleanup state represented without exposing a descriptor or terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupDisposition {
    /// No carrier setting had been applied.
    NotRequired,
    /// The restore write succeeded and exact post-restore readback matched.
    VerifiedRestored,
    /// Restoration was attempted and failed closed.
    Failed,
}

/// Sanitized outcome of the carrier layer, before protocol validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SanitizedAttemptOutcome {
    /// The typed operation did not match the fixed open-session binding.
    OperationRejected,
    /// Exactly 26 bytes were received.
    Complete,
    /// The deadline elapsed with zero to 25 bytes.
    TimedOut,
    /// Binding validation blocked the open.
    BindingRejected,
    /// A system operation failed.
    SystemError,
    /// The monotonic budget was exceeded.
    DeadlineExceeded,
    /// The sole write was short.
    ShortWrite,
    /// Input was already queued before any Search byte was transmitted.
    PreexistingInput,
    /// Input exceeded the bounded frame shape.
    Overflow,
    /// The tty reached EOF.
    EndOfFile,
    /// A bounded result invariant failed.
    ReadInvariant,
    /// Restoration failed; the descriptor was still closed.
    CleanupFailed,
}

/// Closed Search operation kind represented in a sanitized receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SanitizedAttemptKind {
    /// Vendor-documented 115200 baud attempt.
    Primary,
    /// One 38400 baud compatibility fallback.
    SingleFallback,
    /// Direct 38400 operation for the observed MVP binding.
    Known38400,
}

impl From<SearchOperation> for SanitizedAttemptKind {
    fn from(operation: SearchOperation) -> Self {
        match operation.kind() {
            SearchOperationKind::Primary => Self::Primary,
            SearchOperationKind::SingleFallback => Self::SingleFallback,
            SearchOperationKind::Known38400 => Self::Known38400,
        }
    }
}

/// Machine-readable receipt that never contains a tty path or raw response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizedAttemptReceipt {
    /// Closed Search operation kind.
    pub attempt: SanitizedAttemptKind,
    /// Digest of the validated callout path; the path itself is never emitted.
    pub binding_digest: Sha256Digest,
    /// Requested line rate.
    pub baud: u32,
    /// Fixed total attempt budget.
    pub deadline_millis: u64,
    /// Portion of the total budget withheld from active I/O for cleanup.
    pub cleanup_reserve_millis: u64,
    /// Observed monotonic duration, rounded up to microseconds.
    pub elapsed_micros: u64,
    /// Bytes accepted by the sole write syscall.
    pub tx_bytes: usize,
    /// Exact request-echo bytes consumed before the response, either zero or eight.
    pub request_echo_bytes: usize,
    /// Total wire bytes consumed, including an exact matched request echo.
    pub wire_bytes: usize,
    /// Accepted response bytes retained, never more than 26.
    pub rx_bytes: usize,
    /// Digest of the accepted response only, omitted for empty reads.
    pub rx_digest: Option<Sha256Digest>,
    /// Consumed bytes reported by an overflow, or zero for another outcome.
    pub overflow_received_bytes: usize,
    /// Bytes left queued when overflow was detected, or zero for another outcome.
    pub overflow_queued_bytes: usize,
    /// Exact duplicate responses consumed after the accepted response.
    pub duplicate_response_count: usize,
    /// Bytes consumed from a partial or different trailing frame.
    pub unexpected_trailing_bytes: usize,
    /// Sanitized carrier result.
    pub outcome: SanitizedAttemptOutcome,
    /// Termios restoration result.
    pub termios_cleanup: CleanupDisposition,
    /// Modem control-line restoration result.
    pub control_lines_cleanup: CleanupDisposition,
    /// True after the owned descriptor was dropped on every opened path.
    pub closed: bool,
}

/// Sanitized cleanup result for one persistent serial session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizedSessionReceipt {
    /// Digest of the validated callout path; the path itself is never emitted.
    pub binding_digest: Sha256Digest,
    /// Line rate configured exactly once for the session.
    pub baud: u32,
    /// Number of typed Search operations attempted while the descriptor was held.
    pub attempt_count: usize,
    /// Total session lifetime, rounded up to microseconds.
    pub elapsed_micros: u64,
    /// Termios restoration result.
    pub termios_cleanup: CleanupDisposition,
    /// Modem control-line restoration result.
    pub control_lines_cleanup: CleanupDisposition,
    /// True after the owned descriptor was closed.
    pub closed: bool,
}

/// Sanitized result of one explicit `ReceiveDirect` recovery operation.
///
/// A successful receipt proves that the exact typed frame was accepted by one
/// kernel write and that no input became readable during the bounded quiet
/// window. It is not a device acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizedRecoveryReceipt {
    /// Digest of the validated callout path; the path itself is never emitted.
    pub binding_digest: Sha256Digest,
    /// Exact device address encoded into the closed recovery command.
    pub device: DeviceId,
    /// Source-documented mode encoded into the closed recovery command.
    pub remote_mode: RemoteMode,
    /// Fixed line rate used for the operation.
    pub baud: u32,
    /// Fixed whole-operation budget.
    pub deadline_millis: u64,
    /// Duration observed for input after the post-write discard.
    pub quiet_window_millis: u64,
    /// Observed monotonic duration, rounded up to microseconds.
    pub elapsed_micros: u64,
    /// Bytes accepted by the single recovery write syscall.
    pub tx_bytes: usize,
    /// Successful input-only discards before and after the sole write.
    pub input_discard_count: usize,
    /// Termios restoration result.
    pub termios_cleanup: CleanupDisposition,
    /// Modem control-line restoration result.
    pub control_lines_cleanup: CleanupDisposition,
    /// True after the owned descriptor was closed.
    pub closed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SystemFault {
    errno: i32,
}

impl SystemFault {
    const fn new(errno: i32) -> Self {
        Self { errno }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadProgress {
    Bytes(usize),
    WouldBlock,
    #[cfg_attr(bazel_test_no_native, allow(dead_code))]
    EndOfFile,
}

trait SerialBackend {
    type TermiosSnapshot: Clone;

    fn monotonic_now(&mut self) -> Duration;
    fn open_exclusive_noctty(&mut self, path: &Path) -> Result<(), SystemFault>;
    fn snapshot_termios(&mut self) -> Result<Self::TermiosSnapshot, SystemFault>;
    fn snapshot_control_lines(&mut self) -> Result<i32, SystemFault>;
    fn configure(&mut self, snapshot: &Self::TermiosSnapshot, baud: u32)
    -> Result<(), SystemFault>;
    fn write_once(&mut self, bytes: &[u8]) -> Result<usize, SystemFault>;
    fn bytes_available(&mut self) -> Result<usize, SystemFault>;
    fn wait_readable(&mut self, remaining: Duration) -> Result<bool, SystemFault>;
    fn read_once(&mut self, bytes: &mut [u8]) -> Result<ReadProgress, SystemFault>;
    fn discard_input(&mut self) -> Result<(), SystemFault>;
    fn restore_termios(&mut self, snapshot: &Self::TermiosSnapshot) -> Result<(), SystemFault>;
    fn verify_termios_restore(
        &mut self,
        snapshot: &Self::TermiosSnapshot,
    ) -> Result<bool, SystemFault>;
    fn restore_control_lines(&mut self, state: i32) -> Result<(), SystemFault>;
    fn verify_control_lines_restore(&mut self, state: i32) -> Result<bool, SystemFault>;
    fn close(&mut self);
}

struct ReceiptBuilder {
    attempt: SanitizedAttemptKind,
    binding_digest: Sha256Digest,
    baud: u32,
    deadline_millis: u64,
    cleanup_reserve_millis: u64,
    tx_bytes: usize,
    request_echo_bytes: usize,
    wire_bytes: usize,
    rx_bytes: usize,
    rx_hasher: Sha256,
    overflow_received_bytes: usize,
    overflow_queued_bytes: usize,
    duplicate_response_count: usize,
    unexpected_trailing_bytes: usize,
    termios_cleanup: CleanupDisposition,
    control_lines_cleanup: CleanupDisposition,
    closed: bool,
}

impl ReceiptBuilder {
    fn new(binding: &PrivateTtyBinding, operation: SearchOperation) -> Self {
        Self {
            attempt: operation.into(),
            binding_digest: binding.digest(),
            baud: operation.settings().baud(),
            deadline_millis: duration_millis(operation.timeout()),
            cleanup_reserve_millis: duration_millis(SEARCH_CLEANUP_RESERVE),
            tx_bytes: 0,
            request_echo_bytes: 0,
            wire_bytes: 0,
            rx_bytes: 0,
            rx_hasher: Sha256::new(),
            overflow_received_bytes: 0,
            overflow_queued_bytes: 0,
            duplicate_response_count: 0,
            unexpected_trailing_bytes: 0,
            termios_cleanup: CleanupDisposition::NotRequired,
            control_lines_cleanup: CleanupDisposition::NotRequired,
            closed: false,
        }
    }

    fn received(&mut self, bytes: &[u8]) {
        self.rx_bytes += bytes.len();
        self.rx_hasher.update(bytes);
    }

    fn overflow(&mut self, received: usize, queued: usize) -> DarwinCarrierError {
        self.overflow_received_bytes = received;
        self.overflow_queued_bytes = queued;
        DarwinCarrierError::Overflow { received, queued }
    }

    fn finish(
        self,
        elapsed: Duration,
        outcome: SanitizedAttemptOutcome,
    ) -> SanitizedAttemptReceipt {
        SanitizedAttemptReceipt {
            attempt: self.attempt,
            binding_digest: self.binding_digest,
            baud: self.baud,
            deadline_millis: self.deadline_millis,
            cleanup_reserve_millis: self.cleanup_reserve_millis,
            elapsed_micros: duration_micros_ceil(elapsed),
            tx_bytes: self.tx_bytes,
            request_echo_bytes: self.request_echo_bytes,
            wire_bytes: self.wire_bytes,
            rx_bytes: self.rx_bytes,
            rx_digest: (self.rx_bytes != 0).then(|| Sha256Digest(self.rx_hasher.finalize().into())),
            overflow_received_bytes: self.overflow_received_bytes,
            overflow_queued_bytes: self.overflow_queued_bytes,
            duplicate_response_count: self.duplicate_response_count,
            unexpected_trailing_bytes: self.unexpected_trailing_bytes,
            outcome,
            termios_cleanup: self.termios_cleanup,
            control_lines_cleanup: self.control_lines_cleanup,
            closed: self.closed,
        }
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn duration_micros_ceil(duration: Duration) -> u64 {
    let nanos = duration.as_nanos();
    let micros = nanos.saturating_add(999) / 1_000;
    u64::try_from(micros).unwrap_or(u64::MAX)
}

#[cfg(test)]
struct Carrier<B> {
    binding: PrivateTtyBinding,
    backend: B,
    receipts: Vec<SanitizedAttemptReceipt>,
}

#[cfg(test)]
impl<B> Carrier<B> {
    fn new(binding: PrivateTtyBinding, backend: B) -> Self {
        Self {
            binding,
            backend,
            receipts: Vec::new(),
        }
    }

    fn receipts(&self) -> &[SanitizedAttemptReceipt] {
        &self.receipts
    }

    fn take_receipts(&mut self) -> Vec<SanitizedAttemptReceipt> {
        std::mem::take(&mut self.receipts)
    }
}

/// One descriptor held across a sequence of fixed-baud typed control operations.
///
/// Construction opens, snapshots, and configures the tty exactly once. Callers
/// must consume the session with [`Self::finish`] to obtain verified restoration;
/// the native backend retains a best-effort `Drop` fallback for abnormal exits.
struct PersistentCarrier<B: SerialBackend> {
    binding: PrivateTtyBinding,
    backend: B,
    receipts: Vec<SanitizedAttemptReceipt>,
    attempt_count: usize,
    baud: u32,
    started: Duration,
    termios_snapshot: B::TermiosSnapshot,
    control_lines_snapshot: i32,
    input_discard_required: bool,
    last_search_response: Option<[u8; SEARCH_RESPONSE_LIMIT]>,
    receive_direct_cleanup_device: Option<DeviceId>,
}

impl<B: SerialBackend> PersistentCarrier<B> {
    fn open(
        binding: PrivateTtyBinding,
        mut backend: B,
        baud: u32,
    ) -> Result<Self, DarwinCarrierError> {
        let started = backend.monotonic_now();
        let deadline = active_io_deadline(started, SEARCH_ATTEMPT_TIMEOUT);
        let mut opened = false;
        let mut configuration_attempted = false;
        let mut termios_snapshot = None;
        let mut control_lines_snapshot = None;

        let setup = (|| {
            binding.verify()?;
            ensure_before_deadline(&mut backend, deadline, CarrierStage::OpenExclusive)?;
            backend
                .open_exclusive_noctty(&binding.path)
                .map_err(|fault| system_error(CarrierStage::OpenExclusive, fault))?;
            opened = true;

            ensure_before_deadline(&mut backend, deadline, CarrierStage::SnapshotTermios)?;
            let snapshot = backend
                .snapshot_termios()
                .map_err(|fault| system_error(CarrierStage::SnapshotTermios, fault))?;
            termios_snapshot = Some(snapshot.clone());
            ensure_before_deadline(&mut backend, deadline, CarrierStage::SnapshotControlLines)?;
            control_lines_snapshot = Some(
                backend
                    .snapshot_control_lines()
                    .map_err(|fault| system_error(CarrierStage::SnapshotControlLines, fault))?,
            );

            reject_preexisting_input(&mut backend, deadline)?;
            ensure_before_deadline(&mut backend, deadline, CarrierStage::Configure)?;
            configuration_attempted = true;
            backend
                .configure(&snapshot, baud)
                .map_err(|fault| system_error(CarrierStage::Configure, fault))?;
            reject_preexisting_input(&mut backend, deadline)
        })();

        if let Err(error) = setup {
            let primary = Some(error.kind());
            let input_cleanup =
                recover_input_state(&mut backend, opened && requires_input_discard(&error));
            let input_failed = input_cleanup == CleanupDisposition::Failed;
            let mut termios_failed = false;
            let mut control_lines_failed = false;
            if configuration_attempted {
                termios_failed = termios_snapshot.as_ref().is_none_or(|snapshot| {
                    backend.restore_termios(snapshot).is_err()
                        || backend.verify_termios_restore(snapshot) != Ok(true)
                });
                control_lines_failed = control_lines_snapshot.is_none_or(|state| {
                    backend.restore_control_lines(state).is_err()
                        || backend.verify_control_lines_restore(state) != Ok(true)
                });
            }
            if opened {
                backend.close();
            }
            if input_failed || termios_failed || control_lines_failed {
                return Err(DarwinCarrierError::Cleanup {
                    primary,
                    input_failed,
                    termios_failed,
                    control_lines_failed,
                });
            }
            return Err(error);
        }

        let termios_snapshot = termios_snapshot.ok_or(DarwinCarrierError::Cleanup {
            primary: Some(CarrierFailureKind::System),
            input_failed: false,
            termios_failed: true,
            control_lines_failed: false,
        })?;
        let control_lines_snapshot = control_lines_snapshot.ok_or(DarwinCarrierError::Cleanup {
            primary: Some(CarrierFailureKind::System),
            input_failed: false,
            termios_failed: false,
            control_lines_failed: true,
        })?;

        Ok(Self {
            binding,
            backend,
            receipts: Vec::new(),
            attempt_count: 0,
            baud,
            started,
            termios_snapshot,
            control_lines_snapshot,
            input_discard_required: false,
            last_search_response: None,
            receive_direct_cleanup_device: None,
        })
    }

    fn receipts(&self) -> &[SanitizedAttemptReceipt] {
        &self.receipts
    }

    fn take_receipts(&mut self) -> Vec<SanitizedAttemptReceipt> {
        std::mem::take(&mut self.receipts)
    }

    fn require_usable_input(&self) -> Result<(), DarwinCarrierError> {
        if self.input_discard_required {
            Err(DarwinCarrierError::InputRecoveryRequired)
        } else {
            Ok(())
        }
    }

    fn remember_input_uncertainty<T>(
        &mut self,
        result: &Result<T, DarwinCarrierError>,
        post_write_uncertain: bool,
    ) {
        if post_write_uncertain || result.as_ref().is_err_and(requires_input_discard) {
            self.input_discard_required = true;
        }
    }

    /// Reconcile queued late replays of the previous accepted Search response.
    ///
    /// This runs only at a closed operation boundary, before the next write.
    /// It never flushes or counts a replay as a new identity. Only complete,
    /// byte-identical frames are accepted; partial or different input remains
    /// terminal under the existing operation deadline.
    fn reconcile_late_search_response<O: WireObserver>(
        &mut self,
        deadline: Duration,
        observer: &mut O,
    ) -> Result<(), DarwinCarrierError> {
        loop {
            ensure_before_deadline(
                &mut self.backend,
                deadline,
                CarrierStage::CheckPreexistingInput,
            )?;
            let queued = self
                .backend
                .bytes_available()
                .map_err(|fault| system_error(CarrierStage::CheckPreexistingInput, fault))?;
            if queued == 0 {
                return Ok(());
            }
            let Some(expected) = self.last_search_response else {
                return Err(DarwinCarrierError::PreexistingInput { queued });
            };

            match read_one_frame(&mut self.backend, deadline, SEARCH_RESPONSE_LIMIT, observer)? {
                FramedRead::Complete(candidate) if candidate.as_slice() == expected.as_slice() => {
                    observer.duplicate();
                }
                FramedRead::Complete(other) | FramedRead::TimedOut(other) => {
                    return Err(observer.unexpected(other.len()));
                }
            }
        }
    }

    fn finish(mut self) -> Result<SanitizedSessionReceipt, DarwinCarrierError> {
        let quiescence = match self.receive_direct_cleanup_device {
            Some(device) => self.quiesce_receive_direct_command(RemoteModeCommand::new(
                device,
                RemoteMode::ReceiveDirect,
            )),
            None => Ok(()),
        };
        let primary = quiescence.as_ref().err().map(DarwinCarrierError::kind);
        let input_cleanup = recover_input_state(&mut self.backend, self.input_discard_required);
        let termios_cleanup = if self.backend.restore_termios(&self.termios_snapshot).is_ok()
            && self.backend.verify_termios_restore(&self.termios_snapshot) == Ok(true)
        {
            CleanupDisposition::VerifiedRestored
        } else {
            CleanupDisposition::Failed
        };
        let control_lines_cleanup = if self
            .backend
            .restore_control_lines(self.control_lines_snapshot)
            .is_ok()
            && self
                .backend
                .verify_control_lines_restore(self.control_lines_snapshot)
                == Ok(true)
        {
            CleanupDisposition::VerifiedRestored
        } else {
            CleanupDisposition::Failed
        };
        self.backend.close();
        let receipt = SanitizedSessionReceipt {
            binding_digest: self.binding.digest(),
            baud: self.baud,
            attempt_count: self.attempt_count,
            elapsed_micros: duration_micros_ceil(
                self.backend.monotonic_now().saturating_sub(self.started),
            ),
            termios_cleanup,
            control_lines_cleanup,
            closed: true,
        };
        if input_cleanup == CleanupDisposition::Failed
            || termios_cleanup == CleanupDisposition::Failed
            || control_lines_cleanup == CleanupDisposition::Failed
        {
            return Err(DarwinCarrierError::Cleanup {
                primary,
                input_failed: input_cleanup == CleanupDisposition::Failed,
                termios_failed: termios_cleanup == CleanupDisposition::Failed,
                control_lines_failed: control_lines_cleanup == CleanupDisposition::Failed,
            });
        }
        quiescence?;
        Ok(receipt)
    }

    fn run_search(&mut self, operation: SearchOperation) -> AttemptResult {
        let start = self.backend.monotonic_now();
        let active_io_deadline = active_io_deadline(start, operation.timeout());
        let mut receipt = ReceiptBuilder::new(&self.binding, operation);
        let mut write_attempted = false;
        let bounded_result = (|| {
            self.require_usable_input()?;
            if operation.settings().baud() != self.baud {
                return Err(DarwinCarrierError::OperationMismatch {
                    expected_baud: self.baud,
                    actual_baud: operation.settings().baud(),
                });
            }
            ensure_before_deadline(
                &mut self.backend,
                active_io_deadline,
                CarrierStage::VerifyOperation,
            )?;
            self.reconcile_late_search_response(active_io_deadline, &mut receipt)?;
            ensure_before_deadline(&mut self.backend, active_io_deadline, CarrierStage::Write)?;
            write_attempted = true;
            let written = self
                .backend
                .write_once(operation.request().as_bytes())
                .map_err(|fault| system_error(CarrierStage::Write, fault))?;
            receipt.tx_bytes = written;
            if written != operation.request().as_bytes().len() {
                return Err(DarwinCarrierError::ShortWrite {
                    written,
                    expected: operation.request().as_bytes().len(),
                });
            }
            read_bounded(
                &mut self.backend,
                active_io_deadline,
                operation.response_limit(),
                *operation.request().as_bytes(),
                &mut receipt,
            )
        })();
        let result = bounded_result.and_then(|bounded| {
            if let Some(response) = bounded.response {
                self.last_search_response = Some(fixed_search_response(response)?);
            }
            Ok(bounded.read)
        });
        let post_write_uncertain = write_attempted
            && (result.is_err()
                || result.as_ref().is_ok_and(|read| {
                    read.end() == SearchReadEnd::TimedOut && read.received_len() != 0
                }));
        self.remember_input_uncertainty(&result, post_write_uncertain);
        let elapsed = self.backend.monotonic_now().saturating_sub(start);
        let outcome = classify_outcome(&result);
        (result, receipt.finish(elapsed, outcome))
    }

    fn run_snapshot_exchange(
        &mut self,
        operation: SnapshotOperation,
    ) -> Result<SnapshotRead, DarwinCarrierError> {
        self.require_usable_input()?;
        let mut write_attempted = false;
        let result = self.run_snapshot_exchange_usable(operation, &mut write_attempted);
        let terminal_timeout = result.as_ref().is_ok_and(|read| {
            read.end() == SnapshotReadEnd::TimedOut
                && (read.received_len() != 0
                    || !matches!(operation.kind(), SnapshotOperationKind::Search { .. }))
        });
        self.remember_input_uncertainty(
            &result,
            write_attempted && (result.is_err() || terminal_timeout),
        );
        result
    }

    fn run_snapshot_exchange_usable(
        &mut self,
        operation: SnapshotOperation,
        write_attempted: &mut bool,
    ) -> Result<SnapshotRead, DarwinCarrierError> {
        let start = self.backend.monotonic_now();
        let deadline = active_io_deadline(start, operation.timeout());
        let mut observer = ();
        self.reconcile_late_search_response(deadline, &mut observer)?;
        ensure_before_deadline(&mut self.backend, deadline, CarrierStage::Write)?;
        let request = operation.request();
        let expected = request.as_bytes().len();
        *write_attempted = true;
        let written = self
            .backend
            .write_once(request.as_bytes())
            .map_err(|fault| system_error(CarrierStage::Write, fault))?;
        if written != expected {
            return Err(DarwinCarrierError::ShortWrite { written, expected });
        }
        if matches!(operation.kind(), SnapshotOperationKind::Search { .. }) {
            self.attempt_count += 1;
        }
        let bounded = read_snapshot_bounded(
            &mut self.backend,
            deadline,
            operation.response_limit(),
            request.as_bytes(),
            matches!(operation.kind(), SnapshotOperationKind::Search { .. }),
        )?;
        if matches!(operation.kind(), SnapshotOperationKind::Search { .. })
            && let Some(response) = bounded.response
        {
            self.last_search_response = Some(fixed_search_response(response)?);
        }
        Ok(bounded.read)
    }

    fn write_direct_command(
        &mut self,
        command: &DirectParameterCommand,
    ) -> Result<(), DarwinCarrierError> {
        self.write_typed_frame(&command.encode()?)
    }

    fn write_remote_mode_command(
        &mut self,
        command: RemoteModeCommand,
    ) -> Result<(), DarwinCarrierError> {
        match command.mode() {
            RemoteMode::ReceiveDirect => self.quiesce_receive_direct_command(command),
            RemoteMode::Transmit | RemoteMode::ReceiveAndTransmit => {
                self.receive_direct_cleanup_device = Some(command.device());
                self.write_typed_frame(&command.encode()?)
            }
        }
    }

    fn quiesce_receive_direct_command(
        &mut self,
        command: RemoteModeCommand,
    ) -> Result<(), DarwinCarrierError> {
        if command.mode() != RemoteMode::ReceiveDirect {
            return Err(DarwinCarrierError::RecoveryModeMismatch {
                actual: command.mode(),
            });
        }
        if let Some(expected) = self.receive_direct_cleanup_device
            && expected != command.device()
        {
            return Err(DarwinCarrierError::RecoveryDeviceMismatch {
                expected,
                actual: command.device(),
            });
        }

        let encoded = command.encode()?;
        let start = self.backend.monotonic_now();
        let deadline = active_io_deadline(start, SEARCH_ATTEMPT_TIMEOUT);
        let mut write_attempted = false;
        let result = execute_receive_direct_recovery_write(
            &mut self.backend,
            deadline,
            &encoded,
            &mut write_attempted,
        );
        match result {
            Ok(_) => {
                self.input_discard_required = false;
                self.last_search_response = None;
                self.receive_direct_cleanup_device = None;
                Ok(())
            }
            Err(error) => {
                self.input_discard_required = true;
                Err(error)
            }
        }
    }

    fn write_typed_frame(&mut self, frame: &[u8]) -> Result<(), DarwinCarrierError> {
        self.require_usable_input()?;
        let mut write_attempted = false;
        let result = self.write_typed_frame_usable(frame, &mut write_attempted);
        self.remember_input_uncertainty(&result, write_attempted && result.is_err());
        result
    }

    fn write_typed_frame_usable(
        &mut self,
        frame: &[u8],
        write_attempted: &mut bool,
    ) -> Result<(), DarwinCarrierError> {
        let start = self.backend.monotonic_now();
        let deadline = active_io_deadline(start, SEARCH_ATTEMPT_TIMEOUT);
        let mut observer = ();
        self.reconcile_late_search_response(deadline, &mut observer)?;
        ensure_before_deadline(&mut self.backend, deadline, CarrierStage::Write)?;
        let expected = frame.len();
        *write_attempted = true;
        let written = self
            .backend
            .write_once(frame)
            .map_err(|fault| system_error(CarrierStage::Write, fault))?;
        if written != expected {
            return Err(DarwinCarrierError::ShortWrite { written, expected });
        }
        if self.backend.monotonic_now().saturating_sub(start) > SEARCH_ATTEMPT_TIMEOUT {
            return Err(DarwinCarrierError::Deadline {
                stage: CarrierStage::Write,
            });
        }
        Ok(())
    }
}

impl<B: SerialBackend> SearchTransport for PersistentCarrier<B> {
    type Error = DarwinCarrierError;

    fn search(&mut self, operation: SearchOperation) -> Result<SearchRead, Self::Error> {
        let (result, receipt) = self.run_search(operation);
        self.attempt_count += 1;
        self.receipts.push(receipt);
        result
    }
}

impl<B: SerialBackend> PersistentSnapshotSession for PersistentCarrier<B> {
    type Error = DarwinCarrierError;

    fn exchange(&mut self, operation: SnapshotOperation) -> Result<SnapshotRead, Self::Error> {
        self.run_snapshot_exchange(operation)
    }

    fn write_remote_mode(&mut self, command: &RemoteModeCommand) -> Result<(), Self::Error> {
        self.write_remote_mode_command(*command)
    }

    fn finish(self) -> Result<(), Self::Error> {
        PersistentCarrier::finish(self).map(drop)
    }
}

impl<B: SerialBackend> PersistentApplySession for PersistentCarrier<B> {
    fn write_direct(&mut self, command: &DirectParameterCommand) -> Result<(), Self::Error> {
        self.write_direct_command(command)
    }
}

#[cfg(test)]
impl<B: SerialBackend> SearchTransport for Carrier<B> {
    type Error = DarwinCarrierError;

    fn search(&mut self, operation: SearchOperation) -> Result<SearchRead, Self::Error> {
        let (result, receipt) = run_attempt(&self.binding, &mut self.backend, operation);
        self.receipts.push(receipt);
        result
    }
}

type AttemptResult = (
    Result<SearchRead, DarwinCarrierError>,
    SanitizedAttemptReceipt,
);

fn execute_receive_direct_recovery_write<B: SerialBackend>(
    backend: &mut B,
    deadline: Duration,
    command: &[u8],
    write_attempted: &mut bool,
) -> Result<(usize, usize), DarwinCarrierError> {
    let mut discard_count = 0_usize;
    ensure_before_deadline(backend, deadline, CarrierStage::DiscardInput)?;
    backend
        .discard_input()
        .map_err(|fault| system_error(CarrierStage::DiscardInput, fault))?;
    discard_count += 1;

    ensure_before_deadline(backend, deadline, CarrierStage::Write)?;
    *write_attempted = true;
    let written = backend
        .write_once(command)
        .map_err(|fault| system_error(CarrierStage::Write, fault))?;
    if written != command.len() {
        return Err(DarwinCarrierError::ShortWrite {
            written,
            expected: command.len(),
        });
    }

    ensure_before_deadline(backend, deadline, CarrierStage::DiscardInput)?;
    backend
        .discard_input()
        .map_err(|fault| system_error(CarrierStage::DiscardInput, fault))?;
    discard_count += 1;

    drain_until_quiet(backend, deadline, &mut discard_count)?;
    Ok((written, discard_count))
}

/// Discard without parsing until one continuous quiet window is observed.
///
/// Resumed input restarts the fixed quiet observation but never retries the
/// typed write. The common operation deadline bounds the complete drain.
fn drain_until_quiet<B: SerialBackend>(
    backend: &mut B,
    deadline: Duration,
    discard_count: &mut usize,
) -> Result<(), DarwinCarrierError> {
    loop {
        ensure_before_deadline(backend, deadline, CarrierStage::WaitReadable)?;
        if remaining(backend, deadline) <= RECOVERY_QUIET_WINDOW {
            return Err(DarwinCarrierError::Deadline {
                stage: CarrierStage::WaitReadable,
            });
        }
        let quiet_started = backend.monotonic_now();
        if backend
            .wait_readable(RECOVERY_QUIET_WINDOW)
            .map_err(|fault| system_error(CarrierStage::WaitReadable, fault))?
        {
            ensure_before_deadline(backend, deadline, CarrierStage::DiscardInput)?;
            backend
                .discard_input()
                .map_err(|fault| system_error(CarrierStage::DiscardInput, fault))?;
            *discard_count = discard_count.saturating_add(1);
            if backend.monotonic_now() <= quiet_started {
                return Err(DarwinCarrierError::Deadline {
                    stage: CarrierStage::WaitReadable,
                });
            }
            continue;
        }
        if backend.monotonic_now().saturating_sub(quiet_started) < RECOVERY_QUIET_WINDOW {
            return Err(DarwinCarrierError::Deadline {
                stage: CarrierStage::WaitReadable,
            });
        }
        ensure_before_deadline(backend, deadline, CarrierStage::BytesAvailable)?;
        let queued = backend
            .bytes_available()
            .map_err(|fault| system_error(CarrierStage::BytesAvailable, fault))?;
        if queued == 0 {
            return Ok(());
        }
        ensure_before_deadline(backend, deadline, CarrierStage::DiscardInput)?;
        backend
            .discard_input()
            .map_err(|fault| system_error(CarrierStage::DiscardInput, fault))?;
        *discard_count = discard_count.saturating_add(1);
    }
}

#[cfg(any(test, all(target_os = "macos", not(bazel_test_no_native))))]
fn run_receive_direct_recovery<B: SerialBackend>(
    binding: &PrivateTtyBinding,
    backend: &mut B,
    device: DeviceId,
) -> Result<SanitizedRecoveryReceipt, DarwinCarrierError> {
    let command = RemoteModeCommand::new(device, RemoteMode::ReceiveDirect).encode()?;
    let start = backend.monotonic_now();
    let deadline = active_io_deadline(start, SEARCH_ATTEMPT_TIMEOUT);
    let mut opened = false;
    let mut configuration_attempted = false;
    let mut write_attempted = false;
    let mut termios_snapshot = None;
    let mut control_lines_snapshot = None;

    let primary = (|| {
        binding.verify()?;
        ensure_before_deadline(backend, deadline, CarrierStage::OpenExclusive)?;
        backend
            .open_exclusive_noctty(&binding.path)
            .map_err(|fault| system_error(CarrierStage::OpenExclusive, fault))?;
        opened = true;

        ensure_before_deadline(backend, deadline, CarrierStage::SnapshotTermios)?;
        let snapshot = backend
            .snapshot_termios()
            .map_err(|fault| system_error(CarrierStage::SnapshotTermios, fault))?;
        termios_snapshot = Some(snapshot.clone());
        ensure_before_deadline(backend, deadline, CarrierStage::SnapshotControlLines)?;
        control_lines_snapshot = Some(
            backend
                .snapshot_control_lines()
                .map_err(|fault| system_error(CarrierStage::SnapshotControlLines, fault))?,
        );

        ensure_before_deadline(backend, deadline, CarrierStage::Configure)?;
        configuration_attempted = true;
        backend
            .configure(&snapshot, FALLBACK_BAUD)
            .map_err(|fault| system_error(CarrierStage::Configure, fault))?;

        execute_receive_direct_recovery_write(backend, deadline, &command, &mut write_attempted)
    })();

    let primary_kind = primary.as_ref().err().map(DarwinCarrierError::kind);
    let input_cleanup = recover_input_state(backend, primary.is_err() && write_attempted);
    let input_failed = input_cleanup == CleanupDisposition::Failed;
    let (termios_cleanup, control_lines_cleanup, termios_failed, control_lines_failed) =
        restore_attempt_state(
            backend,
            termios_snapshot.as_ref(),
            control_lines_snapshot,
            configuration_attempted,
        );
    if opened {
        backend.close();
    }

    if input_failed || termios_failed || control_lines_failed {
        return Err(DarwinCarrierError::Cleanup {
            primary: primary_kind,
            input_failed,
            termios_failed,
            control_lines_failed,
        });
    }
    let (tx_bytes, input_discard_count) = primary?;
    let elapsed = backend.monotonic_now().saturating_sub(start);
    if elapsed > SEARCH_ATTEMPT_TIMEOUT {
        return Err(DarwinCarrierError::Deadline {
            stage: CarrierStage::Close,
        });
    }
    Ok(SanitizedRecoveryReceipt {
        binding_digest: binding.digest(),
        device,
        remote_mode: RemoteMode::ReceiveDirect,
        baud: FALLBACK_BAUD,
        deadline_millis: duration_millis(SEARCH_ATTEMPT_TIMEOUT),
        quiet_window_millis: duration_millis(RECOVERY_QUIET_WINDOW),
        elapsed_micros: duration_micros_ceil(elapsed),
        tx_bytes,
        input_discard_count,
        termios_cleanup,
        control_lines_cleanup,
        closed: true,
    })
}

#[cfg(test)]
fn run_attempt<B: SerialBackend>(
    binding: &PrivateTtyBinding,
    backend: &mut B,
    operation: SearchOperation,
) -> AttemptResult {
    let start = backend.monotonic_now();
    let active_io_deadline = active_io_deadline(start, operation.timeout());
    let mut receipt = ReceiptBuilder::new(binding, operation);
    let mut opened = false;
    let mut configuration_attempted = false;
    let mut termios_snapshot = None;
    let mut control_lines_snapshot = None;

    let primary = (|| {
        binding.verify()?;
        ensure_before_deadline(backend, active_io_deadline, CarrierStage::OpenExclusive)?;
        backend
            .open_exclusive_noctty(&binding.path)
            .map_err(|fault| system_error(CarrierStage::OpenExclusive, fault))?;
        opened = true;

        ensure_before_deadline(backend, active_io_deadline, CarrierStage::SnapshotTermios)?;
        let snapshot = backend
            .snapshot_termios()
            .map_err(|fault| system_error(CarrierStage::SnapshotTermios, fault))?;
        termios_snapshot = Some(snapshot.clone());
        ensure_before_deadline(
            backend,
            active_io_deadline,
            CarrierStage::SnapshotControlLines,
        )?;
        control_lines_snapshot = Some(
            backend
                .snapshot_control_lines()
                .map_err(|fault| system_error(CarrierStage::SnapshotControlLines, fault))?,
        );

        reject_preexisting_input(backend, active_io_deadline)?;
        ensure_before_deadline(backend, active_io_deadline, CarrierStage::Configure)?;
        configuration_attempted = true;
        backend
            .configure(&snapshot, operation.settings().baud())
            .map_err(|fault| system_error(CarrierStage::Configure, fault))?;
        reject_preexisting_input(backend, active_io_deadline)?;
        ensure_before_deadline(backend, active_io_deadline, CarrierStage::Write)?;

        let written = backend
            .write_once(operation.request().as_bytes())
            .map_err(|fault| system_error(CarrierStage::Write, fault))?;
        receipt.tx_bytes = written;
        if written != operation.request().as_bytes().len() {
            return Err(DarwinCarrierError::ShortWrite {
                written,
                expected: operation.request().as_bytes().len(),
            });
        }
        ensure_before_deadline(backend, active_io_deadline, CarrierStage::BytesAvailable)?;

        read_bounded(
            backend,
            active_io_deadline,
            operation.response_limit(),
            *operation.request().as_bytes(),
            &mut receipt,
        )
        .map(|bounded| bounded.read)
    })();

    let primary_kind = primary.as_ref().err().map(DarwinCarrierError::kind);
    let input_cleanup =
        recover_input_state(backend, primary.as_ref().is_err_and(requires_input_discard));
    let input_failed = input_cleanup == CleanupDisposition::Failed;
    let (termios_cleanup, control_lines_cleanup, termios_failed, control_lines_failed) =
        restore_attempt_state(
            backend,
            termios_snapshot.as_ref(),
            control_lines_snapshot,
            configuration_attempted,
        );
    receipt.termios_cleanup = termios_cleanup;
    receipt.control_lines_cleanup = control_lines_cleanup;

    if opened {
        backend.close();
        receipt.closed = true;
    }

    let cleanup_failed = input_failed || termios_failed || control_lines_failed;
    let mut result = if cleanup_failed {
        Err(DarwinCarrierError::Cleanup {
            primary: primary_kind,
            input_failed,
            termios_failed,
            control_lines_failed,
        })
    } else {
        primary
    };
    let elapsed = backend.monotonic_now().saturating_sub(start);
    if !cleanup_failed && elapsed > operation.timeout() {
        result = Err(DarwinCarrierError::Deadline {
            stage: CarrierStage::Close,
        });
    }
    let outcome = classify_outcome(&result);
    (result, receipt.finish(elapsed, outcome))
}

#[cfg(any(test, all(target_os = "macos", not(bazel_test_no_native))))]
fn restore_attempt_state<B: SerialBackend>(
    backend: &mut B,
    termios_snapshot: Option<&B::TermiosSnapshot>,
    control_lines_snapshot: Option<i32>,
    configuration_attempted: bool,
) -> (CleanupDisposition, CleanupDisposition, bool, bool) {
    if !configuration_attempted {
        return (
            CleanupDisposition::NotRequired,
            CleanupDisposition::NotRequired,
            false,
            false,
        );
    }
    let termios_cleanup = match termios_snapshot {
        Some(snapshot)
            if backend.restore_termios(snapshot).is_ok()
                && backend.verify_termios_restore(snapshot) == Ok(true) =>
        {
            CleanupDisposition::VerifiedRestored
        }
        Some(_) => CleanupDisposition::Failed,
        None => CleanupDisposition::NotRequired,
    };
    let control_lines_cleanup = match control_lines_snapshot {
        Some(state)
            if backend.restore_control_lines(state).is_ok()
                && backend.verify_control_lines_restore(state) == Ok(true) =>
        {
            CleanupDisposition::VerifiedRestored
        }
        Some(_) => CleanupDisposition::Failed,
        None => CleanupDisposition::NotRequired,
    };
    (
        termios_cleanup,
        control_lines_cleanup,
        termios_cleanup == CleanupDisposition::Failed,
        control_lines_cleanup == CleanupDisposition::Failed,
    )
}

fn active_io_deadline(start: Duration, timeout: Duration) -> Duration {
    let whole = start.checked_add(timeout).unwrap_or(Duration::MAX);
    whole.saturating_sub(SEARCH_CLEANUP_RESERVE)
}

fn requires_input_discard(error: &DarwinCarrierError) -> bool {
    matches!(
        error,
        DarwinCarrierError::PreexistingInput { .. }
            | DarwinCarrierError::Overflow { .. }
            | DarwinCarrierError::UnexpectedTrailingFrame { .. }
    )
}

/// Perform one input-only discard and one empty-queue verification.
///
/// This is cleanup after a terminal operation failure, never operation recovery:
/// callers either consume/close the session immediately or fail construction.
/// The fixed call count and cleanup reserve keep the cleanup bounded.
fn recover_input_state<B: SerialBackend>(backend: &mut B, required: bool) -> CleanupDisposition {
    if !required {
        return CleanupDisposition::NotRequired;
    }
    let started = backend.monotonic_now();
    let deadline = started
        .checked_add(SEARCH_CLEANUP_RESERVE)
        .unwrap_or(Duration::MAX);
    if backend.discard_input().is_err() || backend.monotonic_now() >= deadline {
        return CleanupDisposition::Failed;
    }
    match backend.bytes_available() {
        Ok(0) if backend.monotonic_now() < deadline => CleanupDisposition::VerifiedRestored,
        Ok(_) | Err(_) => CleanupDisposition::Failed,
    }
}

fn reject_preexisting_input<B: SerialBackend>(
    backend: &mut B,
    deadline: Duration,
) -> Result<(), DarwinCarrierError> {
    ensure_before_deadline(backend, deadline, CarrierStage::CheckPreexistingInput)?;
    let queued = backend
        .bytes_available()
        .map_err(|fault| system_error(CarrierStage::CheckPreexistingInput, fault))?;
    if queued == 0 {
        Ok(())
    } else {
        Err(DarwinCarrierError::PreexistingInput { queued })
    }
}

fn ensure_before_deadline<B: SerialBackend>(
    backend: &mut B,
    deadline: Duration,
    stage: CarrierStage,
) -> Result<(), DarwinCarrierError> {
    if backend.monotonic_now() >= deadline {
        return Err(DarwinCarrierError::Deadline { stage });
    }
    Ok(())
}

fn remaining<B: SerialBackend>(backend: &mut B, deadline: Duration) -> Duration {
    deadline.saturating_sub(backend.monotonic_now())
}

fn read_bounded<B: SerialBackend>(
    backend: &mut B,
    deadline: Duration,
    limit: usize,
    request: [u8; SEARCH_REQUEST_LEN],
    receipt: &mut ReceiptBuilder,
) -> Result<BoundedRead<SearchRead>, DarwinCarrierError> {
    debug_assert_eq!(limit, SEARCH_RESPONSE_LIMIT);
    let first = read_one_frame(backend, deadline, limit, receipt)?;
    let response = match first {
        FramedRead::TimedOut(partial) => {
            receipt.received(&partial);
            return Ok(BoundedRead {
                read: SearchRead::timed_out(&partial)?,
                response: None,
            });
        }
        FramedRead::Complete(frame) if frame.as_slice() == request => {
            receipt.request_echo_bytes = frame.len();
            match read_one_frame(backend, deadline, limit, receipt)? {
                FramedRead::Complete(response) => response,
                FramedRead::TimedOut(partial) => {
                    receipt.received(&partial);
                    return Ok(BoundedRead {
                        read: SearchRead::timed_out(&partial)?,
                        response: None,
                    });
                }
            }
        }
        FramedRead::Complete(response) => response,
    };

    let read = SearchRead::complete(&response)?;
    receipt.received(&response);
    finish_bounded_response(backend, deadline, limit, &response, receipt, true)?;
    Ok(BoundedRead {
        read,
        response: Some(response),
    })
}

enum FramedRead {
    Complete(Vec<u8>),
    TimedOut(Vec<u8>),
}

struct BoundedRead<T> {
    read: T,
    response: Option<Vec<u8>>,
}

fn fixed_search_response(
    response: Vec<u8>,
) -> Result<[u8; SEARCH_RESPONSE_LIMIT], DarwinCarrierError> {
    let received = response.len();
    response
        .try_into()
        .map_err(|_response: Vec<u8>| SearchReadError::CompleteLength(received).into())
}

trait WireObserver {
    fn consumed(&mut self, count: usize);
    fn duplicate(&mut self);
    fn unexpected(&mut self, received: usize) -> DarwinCarrierError;
    fn overflow(&mut self, received: usize, queued: usize) -> DarwinCarrierError;
}

impl WireObserver for ReceiptBuilder {
    fn consumed(&mut self, count: usize) {
        self.wire_bytes += count;
    }

    fn duplicate(&mut self) {
        self.duplicate_response_count = self.duplicate_response_count.saturating_add(1);
    }

    fn unexpected(&mut self, received: usize) -> DarwinCarrierError {
        self.unexpected_trailing_bytes = received;
        DarwinCarrierError::UnexpectedTrailingFrame { received }
    }

    fn overflow(&mut self, received: usize, queued: usize) -> DarwinCarrierError {
        ReceiptBuilder::overflow(self, received, queued)
    }
}

impl WireObserver for () {
    fn consumed(&mut self, _count: usize) {}

    fn duplicate(&mut self) {}

    fn unexpected(&mut self, received: usize) -> DarwinCarrierError {
        DarwinCarrierError::UnexpectedTrailingFrame { received }
    }

    fn overflow(&mut self, received: usize, queued: usize) -> DarwinCarrierError {
        DarwinCarrierError::Overflow { received, queued }
    }
}

/// Consume byte-identical response replays before the operation ends.
///
/// Search responses settle through their existing bounded receive deadline so a
/// replay that becomes readable just after the first frame terminator cannot
/// escape into the next paced operation. Dump handling retains the immediate
/// trailing-input check and accepts at most one queued exact replay because its
/// larger deadline is sized for wire transfer, not an added settle delay. Every
/// replay is independently frame-bounded; partial or different input is terminal.
fn finish_bounded_response<B: SerialBackend, O: WireObserver>(
    backend: &mut B,
    deadline: Duration,
    frame_limit: usize,
    response: &[u8],
    observer: &mut O,
    settle_until_deadline: bool,
) -> Result<(), DarwinCarrierError> {
    if !settle_until_deadline {
        ensure_before_deadline(backend, deadline, CarrierStage::BytesAvailable)?;
        let queued = backend
            .bytes_available()
            .map_err(|fault| system_error(CarrierStage::BytesAvailable, fault))?;
        if queued == 0 {
            return Ok(());
        }
        return match read_one_frame(backend, deadline, frame_limit, observer)? {
            FramedRead::Complete(duplicate) if duplicate == response => {
                observer.duplicate();
                ensure_before_deadline(backend, deadline, CarrierStage::BytesAvailable)?;
                let queued = backend
                    .bytes_available()
                    .map_err(|fault| system_error(CarrierStage::BytesAvailable, fault))?;
                if queued == 0 {
                    Ok(())
                } else {
                    Err(observer.overflow(response.len(), queued))
                }
            }
            FramedRead::Complete(other) | FramedRead::TimedOut(other) => {
                Err(observer.unexpected(other.len()))
            }
        };
    }

    loop {
        match read_one_frame(backend, deadline, frame_limit, observer)? {
            FramedRead::TimedOut(partial) if partial.is_empty() => return Ok(()),
            FramedRead::Complete(duplicate) if duplicate == response => observer.duplicate(),
            FramedRead::Complete(other) | FramedRead::TimedOut(other) => {
                return Err(observer.unexpected(other.len()));
            }
        }
    }
}

/// Consume every replay already begun at one snapshot-Search boundary.
///
/// An empty queue returns immediately. Once any replay byte is queued, finish
/// that already-started frame within the existing operation deadline. The
/// persistent carrier reconciles any replay that begins before the next typed
/// write. A partial that does not finish, different frame, or over-limit frame
/// is terminal.
fn finish_queued_snapshot_search_replays<B: SerialBackend, O: WireObserver>(
    backend: &mut B,
    deadline: Duration,
    frame_limit: usize,
    response: &[u8],
    observer: &mut O,
) -> Result<(), DarwinCarrierError> {
    loop {
        ensure_before_deadline(backend, deadline, CarrierStage::BytesAvailable)?;
        let queued = backend
            .bytes_available()
            .map_err(|fault| system_error(CarrierStage::BytesAvailable, fault))?;
        if queued == 0 {
            return Ok(());
        }
        match read_one_frame(backend, deadline, frame_limit, observer)? {
            FramedRead::Complete(duplicate) if duplicate == response => observer.duplicate(),
            FramedRead::Complete(other) | FramedRead::TimedOut(other) => {
                return Err(observer.unexpected(other.len()));
            }
        }
    }
}

fn read_one_frame<B: SerialBackend, O: WireObserver>(
    backend: &mut B,
    deadline: Duration,
    frame_limit: usize,
    observer: &mut O,
) -> Result<FramedRead, DarwinCarrierError> {
    let mut frame = Vec::with_capacity(SEARCH_RESPONSE_LIMIT);
    loop {
        let time_left = remaining(backend, deadline);
        if time_left.is_zero() {
            return Ok(FramedRead::TimedOut(frame));
        }
        let queued = backend
            .bytes_available()
            .map_err(|fault| system_error(CarrierStage::BytesAvailable, fault))?;
        if queued == 0 {
            let time_left = remaining(backend, deadline);
            if time_left.is_zero()
                || !backend
                    .wait_readable(time_left)
                    .map_err(|fault| system_error(CarrierStage::WaitReadable, fault))?
            {
                return Ok(FramedRead::TimedOut(frame));
            }
            continue;
        }
        if frame.len() == frame_limit.min(MAX_FRAME_LEN) {
            return Err(observer.overflow(frame.len(), queued));
        }
        let mut byte = [0_u8; 1];
        match backend
            .read_once(&mut byte)
            .map_err(|fault| system_error(CarrierStage::Read, fault))?
        {
            ReadProgress::Bytes(1) => {
                frame.push(byte[0]);
                observer.consumed(1);
            }
            ReadProgress::Bytes(_) | ReadProgress::EndOfFile => {
                return Err(DarwinCarrierError::EndOfFile {
                    received: frame.len(),
                });
            }
            ReadProgress::WouldBlock => continue,
        }
        if byte[0] == 0xf7 {
            return Ok(FramedRead::Complete(frame));
        }
    }
}

fn read_snapshot_bounded<B: SerialBackend>(
    backend: &mut B,
    deadline: Duration,
    limit: usize,
    request: &[u8],
    consume_all_queued_search_replays: bool,
) -> Result<BoundedRead<SnapshotRead>, DarwinCarrierError> {
    let mut observer = ();
    let first = read_one_frame(backend, deadline, limit, &mut observer)?;
    let response = match first {
        FramedRead::TimedOut(partial) => {
            return Ok(BoundedRead {
                read: SnapshotRead::timed_out(&partial)?,
                response: None,
            });
        }
        FramedRead::Complete(frame) if frame.as_slice() == request => {
            match read_one_frame(backend, deadline, limit, &mut observer)? {
                FramedRead::Complete(response) => response,
                FramedRead::TimedOut(partial) => {
                    return Ok(BoundedRead {
                        read: SnapshotRead::timed_out(&partial)?,
                        response: None,
                    });
                }
            }
        }
        FramedRead::Complete(response) => response,
    };
    let read = SnapshotRead::complete(&response)?;
    if consume_all_queued_search_replays {
        finish_queued_snapshot_search_replays(backend, deadline, limit, &response, &mut observer)?;
    } else {
        finish_bounded_response(backend, deadline, limit, &response, &mut observer, false)?;
    }
    Ok(BoundedRead {
        read,
        response: Some(response),
    })
}

const fn system_error(stage: CarrierStage, fault: SystemFault) -> DarwinCarrierError {
    DarwinCarrierError::System {
        stage,
        errno: fault.errno,
    }
}

fn classify_outcome(result: &Result<SearchRead, DarwinCarrierError>) -> SanitizedAttemptOutcome {
    match result {
        Ok(read) if read.end() == SearchReadEnd::Complete => SanitizedAttemptOutcome::Complete,
        Ok(_) => SanitizedAttemptOutcome::TimedOut,
        Err(
            DarwinCarrierError::OperationMismatch { .. }
            | DarwinCarrierError::RecoveryModeMismatch { .. }
            | DarwinCarrierError::RecoveryDeviceMismatch { .. }
            | DarwinCarrierError::InputRecoveryRequired
            | DarwinCarrierError::ProtocolEncoding(_),
        ) => SanitizedAttemptOutcome::OperationRejected,
        Err(DarwinCarrierError::Binding(_)) => SanitizedAttemptOutcome::BindingRejected,
        Err(DarwinCarrierError::System { .. }) => SanitizedAttemptOutcome::SystemError,
        Err(DarwinCarrierError::Deadline { .. }) => SanitizedAttemptOutcome::DeadlineExceeded,
        Err(DarwinCarrierError::ShortWrite { .. }) => SanitizedAttemptOutcome::ShortWrite,
        Err(DarwinCarrierError::PreexistingInput { .. }) => {
            SanitizedAttemptOutcome::PreexistingInput
        }
        Err(DarwinCarrierError::Overflow { .. }) => SanitizedAttemptOutcome::Overflow,
        Err(DarwinCarrierError::EndOfFile { .. }) => SanitizedAttemptOutcome::EndOfFile,
        Err(
            DarwinCarrierError::UnexpectedTrailingFrame { .. }
            | DarwinCarrierError::ReadInvariant(_)
            | DarwinCarrierError::SnapshotReadInvariant(_),
        ) => SanitizedAttemptOutcome::ReadInvariant,
        Err(DarwinCarrierError::Cleanup { .. }) => SanitizedAttemptOutcome::CleanupFailed,
    }
}

/// Execute one closed, fixed-38400 `ReceiveDirect` recovery operation.
///
/// # Errors
///
/// Fails closed on binding, open, configuration, write, observed post-write
/// input, or exact restoration failure. Pending input is discarded and never
/// parsed or exposed.
#[cfg(all(target_os = "macos", not(bazel_test_no_native)))]
pub fn recover_receive_direct_known_38400(
    binding: &PrivateTtyBinding,
    device: DeviceId,
) -> Result<SanitizedRecoveryReceipt, DarwinCarrierError> {
    run_receive_direct_recovery(binding, &mut macos::MacOsBackend::new(), device)
}

/// Persistent macOS implementation of fixed-38400 typed DCX control.
///
/// [`Self::open_known_38400`] opens and configures one exact callout descriptor.
/// Search, snapshot, remote-mode, and direct calls reuse that descriptor. Callers must
/// consume the session with [`Self::finish`] to restore and verify the original
/// terminal and modem-line state before close.
#[cfg(all(target_os = "macos", not(bazel_test_no_native)))]
#[must_use = "the persistent tty session must be consumed with finish()"]
pub struct DarwinSearchSession {
    inner: PersistentCarrier<macos::MacOsBackend>,
}

#[cfg(all(target_os = "macos", not(bazel_test_no_native)))]
impl DarwinSearchSession {
    /// Open one exact callout and configure the ratified 38400 8N1 binding once.
    ///
    /// # Errors
    ///
    /// Fails closed on binding, open, snapshot, configuration, queued-input, or
    /// cleanup failure. No Search bytes are written while opening the session.
    pub fn open_known_38400(binding: PrivateTtyBinding) -> Result<Self, DarwinCarrierError> {
        Ok(Self {
            inner: PersistentCarrier::open(binding, macos::MacOsBackend::new(), FALLBACK_BAUD)?,
        })
    }

    /// Borrow sanitized per-operation receipts.
    pub fn receipts(&self) -> &[SanitizedAttemptReceipt] {
        self.inner.receipts()
    }

    /// Drain sanitized per-operation receipts before consuming the session.
    pub fn take_receipts(&mut self) -> Vec<SanitizedAttemptReceipt> {
        self.inner.take_receipts()
    }

    /// Restore and verify original tty state, close, and return session cleanup.
    ///
    /// # Errors
    ///
    /// Returns a terminal cleanup error after close when either exact readback
    /// does not match the saved state.
    pub fn finish(self) -> Result<SanitizedSessionReceipt, DarwinCarrierError> {
        self.inner.finish()
    }
}

#[cfg(all(target_os = "macos", not(bazel_test_no_native)))]
impl fmt::Debug for DarwinSearchSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DarwinSearchSession")
            .field("binding", &self.inner.binding)
            .field("baud", &self.inner.baud)
            .field("attempt_count", &self.inner.attempt_count)
            .field("retained_receipt_count", &self.inner.receipts.len())
            .finish()
    }
}

#[cfg(all(target_os = "macos", not(bazel_test_no_native)))]
impl SearchTransport for DarwinSearchSession {
    type Error = DarwinCarrierError;

    fn search(&mut self, operation: SearchOperation) -> Result<SearchRead, Self::Error> {
        self.inner.search(operation)
    }
}

#[cfg(all(target_os = "macos", not(bazel_test_no_native)))]
impl PersistentSnapshotSession for DarwinSearchSession {
    type Error = DarwinCarrierError;

    fn exchange(&mut self, operation: SnapshotOperation) -> Result<SnapshotRead, Self::Error> {
        self.inner.run_snapshot_exchange(operation)
    }

    fn write_remote_mode(&mut self, command: &RemoteModeCommand) -> Result<(), Self::Error> {
        self.inner.write_remote_mode_command(*command)
    }

    fn finish(self) -> Result<(), Self::Error> {
        self.inner.finish().map(drop)
    }
}

#[cfg(all(target_os = "macos", not(bazel_test_no_native)))]
impl PersistentApplySession for DarwinSearchSession {
    fn write_direct(&mut self, command: &DirectParameterCommand) -> Result<(), Self::Error> {
        self.inner.write_direct_command(command)
    }
}

#[cfg(all(target_os = "macos", not(bazel_test_no_native)))]
mod macos {
    use std::{
        mem::MaybeUninit,
        os::fd::{AsRawFd, OwnedFd, RawFd},
        path::Path,
        time::{Duration, Instant},
    };

    use rustix::{
        fs::{self, FileType, Mode, OFlags},
        io::{self, Errno},
        termios::{self, ControlModes, OptionalActions, QueueSelector, SpecialCodeIndex, Termios},
    };

    use super::{ReadProgress, SerialBackend, SystemFault};

    pub(super) struct MacOsBackend {
        fd: Option<OwnedFd>,
        epoch: Instant,
        saved_termios: Option<Termios>,
        saved_control_lines: Option<i32>,
        configuration_attempted: bool,
    }

    impl MacOsBackend {
        pub(super) fn new() -> Self {
            Self {
                fd: None,
                epoch: Instant::now(),
                saved_termios: None,
                saved_control_lines: None,
                configuration_attempted: false,
            }
        }

        fn fd(&self) -> Result<&OwnedFd, SystemFault> {
            self.fd
                .as_ref()
                .ok_or_else(|| SystemFault::new(libc::EBADF))
        }

        fn best_effort_restore(&mut self) {
            if !self.configuration_attempted {
                return;
            }
            if let (Some(fd), Some(snapshot)) = (&self.fd, self.saved_termios.take()) {
                let _ = termios::tcsetattr(fd, OptionalActions::Now, &snapshot);
            }
            if let (Some(fd), Some(state)) = (&self.fd, self.saved_control_lines.take()) {
                let _ = abi::set_control_lines(fd.as_raw_fd(), state);
            }
        }
    }

    impl Drop for MacOsBackend {
        fn drop(&mut self) {
            self.best_effort_restore();
            self.fd.take();
        }
    }

    impl SerialBackend for MacOsBackend {
        type TermiosSnapshot = Termios;

        fn monotonic_now(&mut self) -> Duration {
            self.epoch.elapsed()
        }

        fn open_exclusive_noctty(&mut self, path: &Path) -> Result<(), SystemFault> {
            if self.fd.is_some() {
                return Err(SystemFault::new(libc::EBUSY));
            }
            let flags = OFlags::RDWR
                | OFlags::NOCTTY
                | OFlags::NONBLOCK
                | OFlags::CLOEXEC
                | OFlags::NOFOLLOW;
            let fd = fs::open(path, flags, Mode::empty()).map_err(fault)?;
            let stat = fs::fstat(&fd).map_err(fault)?;
            if !FileType::from_raw_mode(stat.st_mode).is_char_device() {
                return Err(SystemFault::new(libc::ENOTTY));
            }
            termios::ioctl_tiocexcl(&fd).map_err(fault)?;
            self.fd = Some(fd);
            self.saved_termios = None;
            self.saved_control_lines = None;
            self.configuration_attempted = false;
            Ok(())
        }

        fn snapshot_termios(&mut self) -> Result<Self::TermiosSnapshot, SystemFault> {
            let snapshot = termios::tcgetattr(self.fd()?).map_err(fault)?;
            self.saved_termios = Some(snapshot.clone());
            Ok(snapshot)
        }

        fn snapshot_control_lines(&mut self) -> Result<i32, SystemFault> {
            let snapshot = abi::get_control_lines(self.fd()?.as_raw_fd())?;
            self.saved_control_lines = Some(snapshot);
            Ok(snapshot)
        }

        fn configure(
            &mut self,
            snapshot: &Self::TermiosSnapshot,
            baud: u32,
        ) -> Result<(), SystemFault> {
            self.configuration_attempted = true;
            let mut configured = snapshot.clone();
            configured.make_raw();
            configured.control_modes.remove(
                ControlModes::CSIZE
                    | ControlModes::CSTOPB
                    | ControlModes::PARENB
                    | ControlModes::PARODD
                    | ControlModes::CRTSCTS,
            );
            configured
                .control_modes
                .insert(ControlModes::CS8 | ControlModes::CREAD | ControlModes::CLOCAL);
            configured.special_codes[SpecialCodeIndex::VMIN] = 0;
            configured.special_codes[SpecialCodeIndex::VTIME] = 0;
            configured.set_speed(baud).map_err(fault)?;
            termios::tcsetattr(self.fd()?, OptionalActions::Now, &configured).map_err(fault)
        }

        fn write_once(&mut self, bytes: &[u8]) -> Result<usize, SystemFault> {
            io::write(self.fd()?, bytes).map_err(fault)
        }

        fn bytes_available(&mut self) -> Result<usize, SystemFault> {
            let available = abi::bytes_available(self.fd()?.as_raw_fd())?;
            usize::try_from(available).map_err(|_| SystemFault::new(libc::EOVERFLOW))
        }

        fn wait_readable(&mut self, remaining: Duration) -> Result<bool, SystemFault> {
            abi::wait_readable(self.fd()?.as_raw_fd(), remaining)
        }

        fn read_once(&mut self, bytes: &mut [u8]) -> Result<ReadProgress, SystemFault> {
            match io::read(self.fd()?, bytes) {
                Ok(0) => Ok(ReadProgress::EndOfFile),
                Ok(count) => Ok(ReadProgress::Bytes(count)),
                Err(Errno::AGAIN) => Ok(ReadProgress::WouldBlock),
                Err(error) => Err(fault(error)),
            }
        }

        fn discard_input(&mut self) -> Result<(), SystemFault> {
            termios::tcflush(self.fd()?, QueueSelector::IFlush).map_err(fault)
        }

        fn restore_termios(&mut self, snapshot: &Self::TermiosSnapshot) -> Result<(), SystemFault> {
            termios::tcsetattr(self.fd()?, OptionalActions::Now, snapshot).map_err(fault)?;
            Ok(())
        }

        fn verify_termios_restore(
            &mut self,
            snapshot: &Self::TermiosSnapshot,
        ) -> Result<bool, SystemFault> {
            let observed = termios::tcgetattr(self.fd()?).map_err(fault)?;
            let matches = termios_matches(snapshot, &observed);
            if matches {
                self.saved_termios = None;
            }
            Ok(matches)
        }

        fn restore_control_lines(&mut self, state: i32) -> Result<(), SystemFault> {
            abi::set_control_lines(self.fd()?.as_raw_fd(), state)?;
            Ok(())
        }

        fn verify_control_lines_restore(&mut self, state: i32) -> Result<bool, SystemFault> {
            let observed = abi::get_control_lines(self.fd()?.as_raw_fd())?;
            let matches = observed == state;
            if matches {
                self.saved_control_lines = None;
            }
            Ok(matches)
        }

        fn close(&mut self) {
            self.best_effort_restore();
            self.fd.take();
            self.configuration_attempted = false;
        }
    }

    fn fault(error: Errno) -> SystemFault {
        SystemFault::new(error.raw_os_error())
    }

    fn termios_matches(expected: &Termios, observed: &Termios) -> bool {
        expected.input_modes == observed.input_modes
            && expected.output_modes == observed.output_modes
            && expected.control_modes == observed.control_modes
            && expected.local_modes == observed.local_modes
            && expected.input_speed() == observed.input_speed()
            && expected.output_speed() == observed.output_speed()
            // `SpecialCodes` intentionally has no `PartialEq`; its pinned Debug
            // implementation enumerates every c_cc slot and value.
            && format!("{:?}", expected.special_codes)
                == format!("{:?}", observed.special_codes)
    }

    mod abi {
        #![allow(unsafe_code)]

        use super::{Duration, MaybeUninit, RawFd, SystemFault};

        pub(super) fn get_control_lines(fd: RawFd) -> Result<i32, SystemFault> {
            let mut state = 0_i32;
            // SAFETY: `fd` is borrowed from a live `OwnedFd`; TIOCMGET writes
            // exactly one initialized `c_int` to the supplied valid pointer.
            let result = unsafe { libc::ioctl(fd, libc::TIOCMGET, &mut state) };
            cvt(result)?;
            Ok(state)
        }

        pub(super) fn set_control_lines(fd: RawFd, state: i32) -> Result<(), SystemFault> {
            // SAFETY: `fd` is borrowed from a live `OwnedFd`; TIOCMSET reads
            // exactly one initialized `c_int`. This restores a prior snapshot;
            // the carrier never issues TIOCMBIS/TIOCMBIC or DTR/RTS toggles.
            let result = unsafe { libc::ioctl(fd, libc::TIOCMSET, &state) };
            cvt(result)
        }

        pub(super) fn bytes_available(fd: RawFd) -> Result<i32, SystemFault> {
            let mut available = 0_i32;
            // SAFETY: `fd` is live and FIONREAD writes exactly one `c_int`.
            let result = unsafe { libc::ioctl(fd, libc::FIONREAD, &mut available) };
            cvt(result)?;
            if available < 0 {
                return Err(SystemFault::new(libc::EIO));
            }
            Ok(available)
        }

        pub(super) fn wait_readable(fd: RawFd, remaining: Duration) -> Result<bool, SystemFault> {
            if fd < 0 || usize::try_from(fd).map_or(true, |value| value >= libc::FD_SETSIZE) {
                return Err(SystemFault::new(libc::EINVAL));
            }

            let mut set = MaybeUninit::<libc::fd_set>::uninit();
            // SAFETY: FD_ZERO initializes the complete fd_set and the guarded
            // descriptor is within its representable range before FD_SET.
            let mut set = unsafe {
                libc::FD_ZERO(set.as_mut_ptr());
                let mut set = set.assume_init();
                libc::FD_SET(fd, &raw mut set);
                set
            };
            let total_micros = i64::try_from(remaining.as_micros().min(i64::MAX as u128))
                .map_err(|_| SystemFault::new(libc::EOVERFLOW))?;
            let mut timeout = libc::timeval {
                tv_sec: libc::time_t::try_from(total_micros / 1_000_000)
                    .map_err(|_| SystemFault::new(libc::EOVERFLOW))?,
                tv_usec: libc::suseconds_t::try_from(total_micros % 1_000_000)
                    .map_err(|_| SystemFault::new(libc::EOVERFLOW))?,
            };
            // SAFETY: every pointer is either null or points to initialized
            // storage valid for the call; `fd` remains owned for the duration.
            let result = unsafe {
                libc::select(
                    fd + 1,
                    &raw mut set,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &raw mut timeout,
                )
            };
            if result < 0 {
                return Err(last_fault());
            }
            Ok(result == 1)
        }

        fn cvt(result: i32) -> Result<(), SystemFault> {
            if result < 0 {
                Err(last_fault())
            } else {
                Ok(())
            }
        }

        fn last_fault() -> SystemFault {
            SystemFault::new(
                std::io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(libc::EIO),
            )
        }
    }
}

#[cfg(test)]
mod tests;
