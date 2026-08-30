//! Darwin-only persistent tty carrier for the bounded DCX2496 control boundary.
//!
//! The public carrier exists only on macOS. It accepts one explicit, validated
//! callout-device path and exposes only typed Search, remote-mode, Dump, and
//! direct-parameter operations. There is no port enumeration, generic
//! byte-write method, retry loop, or retained raw capture. Tests exercise the
//! same state machine through injected fake syscalls without opening a device.

use std::{
    fmt,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    time::Duration,
};

use dcx_core::{
    discovery::FALLBACK_BAUD,
    protocol::{DirectParameterCommand, ProtocolError, RemoteModeCommand},
};
use dcx_transport::{
    SEARCH_ATTEMPT_TIMEOUT, SEARCH_REQUEST_LEN, SEARCH_RESPONSE_LIMIT, SearchOperation,
    SearchOperationKind, SearchRead, SearchReadEnd, SearchReadError, SearchTransport,
    snapshot::{
        PersistentApplySession, PersistentSnapshotSession, SnapshotOperation,
        SnapshotOperationKind, SnapshotRead, SnapshotReadError,
    },
};
use serde::{Serialize, Serializer};
use sha2::{Digest, Sha256};
use thiserror::Error;

const PRIVATE_CALLOUT_PREFIX: &[u8] = b"/dev/cu.usbserial-";
const SHA256_PREFIX: &str = "sha256/";
/// Portion of the 500 ms whole-attempt budget reserved for restoration/close.
pub const SEARCH_CLEANUP_RESERVE: Duration = Duration::from_millis(25);
/// One exact request-prefix echo plus one exact Search response.
const SEARCH_WIRE_LIMIT: usize = SEARCH_REQUEST_LEN + SEARCH_RESPONSE_LIMIT;

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
        /// Bytes observed without consuming or flushing them.
        queued: usize,
    },
    /// More bytes were queued than the exact response budget permits.
    #[error("typed response overflow: {received} received and {queued} additional queued")]
    Overflow {
        /// Bytes already consumed within the 34-byte echo-plus-response bound.
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
    /// The bounded result constructor rejected internal state.
    #[error(transparent)]
    ReadInvariant(#[from] SearchReadError),
    /// The bounded snapshot result constructor rejected internal state.
    #[error(transparent)]
    SnapshotReadInvariant(#[from] SnapshotReadError),
    /// A checked typed command failed its final protocol encoding invariant.
    #[error("checked typed command failed to encode: {0}")]
    ProtocolEncoding(#[from] ProtocolError),
    /// One or both mandatory restoration operations failed; close still ran.
    #[error(
        "tty cleanup failed (primary={primary:?}, termios_failed={termios_failed}, control_lines_failed={control_lines_failed})"
    )]
    Cleanup {
        /// Broad primary failure, if cleanup followed an earlier failure.
        primary: Option<CarrierFailureKind>,
        /// Whether restoring termios failed.
        termios_failed: bool,
        /// Whether restoring modem control lines failed.
        control_lines_failed: bool,
    },
}

impl DarwinCarrierError {
    const fn kind(&self) -> CarrierFailureKind {
        match self {
            Self::OperationMismatch { .. } | Self::ProtocolEncoding(_) => {
                CarrierFailureKind::Operation
            }
            Self::Binding(_) => CarrierFailureKind::Binding,
            Self::Deadline { .. } => CarrierFailureKind::Deadline,
            Self::System { .. } | Self::Cleanup { .. } => CarrierFailureKind::System,
            Self::ShortWrite { .. } => CarrierFailureKind::ShortWrite,
            Self::PreexistingInput { .. } => CarrierFailureKind::PreexistingInput,
            Self::Overflow { .. } => CarrierFailureKind::Overflow,
            Self::EndOfFile { .. } => CarrierFailureKind::EndOfFile,
            Self::ReadInvariant(_) | Self::SnapshotReadInvariant(_) => {
                CarrierFailureKind::ReadInvariant
            }
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
    /// Input exceeded one exact optional request echo plus one response.
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
    /// Accepted response bytes retained, never more than 26.
    pub rx_bytes: usize,
    /// Digest of the accepted response only, omitted for empty reads.
    pub rx_digest: Option<Sha256Digest>,
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
    rx_bytes: usize,
    rx_hasher: Sha256,
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
            rx_bytes: 0,
            rx_hasher: Sha256::new(),
            termios_cleanup: CleanupDisposition::NotRequired,
            control_lines_cleanup: CleanupDisposition::NotRequired,
            closed: false,
        }
    }

    fn received(&mut self, bytes: &[u8]) {
        self.rx_bytes += bytes.len();
        self.rx_hasher.update(bytes);
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
            rx_bytes: self.rx_bytes,
            rx_digest: (self.rx_bytes != 0).then(|| Sha256Digest(self.rx_hasher.finalize().into())),
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
            if termios_failed || control_lines_failed {
                return Err(DarwinCarrierError::Cleanup {
                    primary,
                    termios_failed,
                    control_lines_failed,
                });
            }
            return Err(error);
        }

        let termios_snapshot = termios_snapshot.ok_or(DarwinCarrierError::Cleanup {
            primary: Some(CarrierFailureKind::System),
            termios_failed: true,
            control_lines_failed: false,
        })?;
        let control_lines_snapshot = control_lines_snapshot.ok_or(DarwinCarrierError::Cleanup {
            primary: Some(CarrierFailureKind::System),
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
        })
    }

    fn receipts(&self) -> &[SanitizedAttemptReceipt] {
        &self.receipts
    }

    fn take_receipts(&mut self) -> Vec<SanitizedAttemptReceipt> {
        std::mem::take(&mut self.receipts)
    }

    fn finish(mut self) -> Result<SanitizedSessionReceipt, DarwinCarrierError> {
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
        if termios_cleanup == CleanupDisposition::Failed
            || control_lines_cleanup == CleanupDisposition::Failed
        {
            return Err(DarwinCarrierError::Cleanup {
                primary: None,
                termios_failed: termios_cleanup == CleanupDisposition::Failed,
                control_lines_failed: control_lines_cleanup == CleanupDisposition::Failed,
            });
        }
        Ok(receipt)
    }

    fn run_search(&mut self, operation: SearchOperation) -> AttemptResult {
        let start = self.backend.monotonic_now();
        let active_io_deadline = active_io_deadline(start, operation.timeout());
        let mut receipt = ReceiptBuilder::new(&self.binding, operation);
        let result = (|| {
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
            reject_preexisting_input(&mut self.backend, active_io_deadline)?;
            ensure_before_deadline(&mut self.backend, active_io_deadline, CarrierStage::Write)?;
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
                operation.request().as_bytes(),
                &mut receipt,
            )
        })();
        let elapsed = self.backend.monotonic_now().saturating_sub(start);
        let result = if elapsed > operation.timeout() {
            Err(DarwinCarrierError::Deadline {
                stage: CarrierStage::Read,
            })
        } else {
            result
        };
        let outcome = classify_outcome(&result);
        (result, receipt.finish(elapsed, outcome))
    }

    fn run_snapshot_exchange(
        &mut self,
        operation: SnapshotOperation,
    ) -> Result<SnapshotRead, DarwinCarrierError> {
        let start = self.backend.monotonic_now();
        let deadline = active_io_deadline(start, operation.timeout());
        reject_preexisting_input(&mut self.backend, deadline)?;
        ensure_before_deadline(&mut self.backend, deadline, CarrierStage::Write)?;
        let request = operation.request();
        let expected = request.as_bytes().len();
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
        let read = read_snapshot_bounded(&mut self.backend, deadline, operation.response_limit())?;
        if self.backend.monotonic_now().saturating_sub(start) > operation.timeout() {
            return Err(DarwinCarrierError::Deadline {
                stage: CarrierStage::Read,
            });
        }
        Ok(read)
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
        self.write_typed_frame(&command.encode()?)
    }

    fn write_typed_frame(&mut self, frame: &[u8]) -> Result<(), DarwinCarrierError> {
        let start = self.backend.monotonic_now();
        let deadline = active_io_deadline(start, SEARCH_ATTEMPT_TIMEOUT);
        reject_preexisting_input(&mut self.backend, deadline)?;
        ensure_before_deadline(&mut self.backend, deadline, CarrierStage::Write)?;
        let expected = frame.len();
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
            operation.request().as_bytes(),
            &mut receipt,
        )
    })();

    let primary_kind = primary.as_ref().err().map(DarwinCarrierError::kind);
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

    let cleanup_failed = termios_failed || control_lines_failed;
    let mut result = if cleanup_failed {
        Err(DarwinCarrierError::Cleanup {
            primary: primary_kind,
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

#[cfg(test)]
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
    request: &[u8; SEARCH_REQUEST_LEN],
    receipt: &mut ReceiptBuilder,
) -> Result<SearchRead, DarwinCarrierError> {
    let mut wire = [0_u8; SEARCH_WIRE_LIMIT];
    let mut wire_received = 0;
    debug_assert_eq!(limit, SEARCH_RESPONSE_LIMIT);

    loop {
        let time_left = remaining(backend, deadline);
        if time_left.is_zero() {
            let response = search_response_candidate(&wire[..wire_received], request, receipt);
            receipt.received(response);
            return SearchRead::timed_out(response).map_err(Into::into);
        }

        let queued = backend
            .bytes_available()
            .map_err(|fault| system_error(CarrierStage::BytesAvailable, fault))?;
        if queued > SEARCH_WIRE_LIMIT - wire_received {
            return Err(DarwinCarrierError::Overflow {
                received: wire_received,
                queued,
            });
        }

        if queued == 0 {
            let time_left = remaining(backend, deadline);
            if time_left.is_zero()
                || !backend
                    .wait_readable(time_left)
                    .map_err(|fault| system_error(CarrierStage::WaitReadable, fault))?
            {
                let response = search_response_candidate(&wire[..wire_received], request, receipt);
                receipt.received(response);
                return SearchRead::timed_out(response).map_err(Into::into);
            }
            continue;
        }

        let end = wire_received + queued;
        match backend
            .read_once(&mut wire[wire_received..end])
            .map_err(|fault| system_error(CarrierStage::Read, fault))?
        {
            ReadProgress::Bytes(count) => {
                if count == 0 || count > queued {
                    return Err(DarwinCarrierError::EndOfFile {
                        received: wire_received,
                    });
                }
                wire_received += count;
            }
            ReadProgress::WouldBlock => continue,
            ReadProgress::EndOfFile => {
                return Err(DarwinCarrierError::EndOfFile {
                    received: wire_received,
                });
            }
        }

        let response = search_response_candidate(&wire[..wire_received], request, receipt);
        if response.len() > limit {
            return Err(DarwinCarrierError::Overflow {
                received: response.len(),
                queued: 0,
            });
        }
        if response.len() == limit {
            receipt.received(response);
            ensure_before_deadline(backend, deadline, CarrierStage::BytesAvailable)?;
            let queued = backend
                .bytes_available()
                .map_err(|fault| system_error(CarrierStage::BytesAvailable, fault))?;
            if queued != 0 {
                return Err(DarwinCarrierError::Overflow {
                    received: response.len(),
                    queued,
                });
            }
            return SearchRead::complete(response).map_err(Into::into);
        }
    }
}

fn search_response_candidate<'a>(
    wire: &'a [u8],
    request: &[u8; SEARCH_REQUEST_LEN],
    receipt: &mut ReceiptBuilder,
) -> &'a [u8] {
    if let Some(response) = wire.strip_prefix(request) {
        receipt.request_echo_bytes = SEARCH_REQUEST_LEN;
        response
    } else {
        wire
    }
}

fn read_snapshot_bounded<B: SerialBackend>(
    backend: &mut B,
    deadline: Duration,
    limit: usize,
) -> Result<SnapshotRead, DarwinCarrierError> {
    let mut bytes = vec![0_u8; limit];
    let mut received = 0;

    loop {
        let time_left = remaining(backend, deadline);
        if time_left.is_zero() {
            return SnapshotRead::timed_out(&bytes[..received]).map_err(Into::into);
        }

        let queued = backend
            .bytes_available()
            .map_err(|fault| system_error(CarrierStage::BytesAvailable, fault))?;
        if queued > limit.saturating_sub(received) {
            return Err(DarwinCarrierError::Overflow { received, queued });
        }

        if queued == 0 {
            let time_left = remaining(backend, deadline);
            if time_left.is_zero()
                || !backend
                    .wait_readable(time_left)
                    .map_err(|fault| system_error(CarrierStage::WaitReadable, fault))?
            {
                return SnapshotRead::timed_out(&bytes[..received]).map_err(Into::into);
            }
            continue;
        }

        let end = received + queued;
        match backend
            .read_once(&mut bytes[received..end])
            .map_err(|fault| system_error(CarrierStage::Read, fault))?
        {
            ReadProgress::Bytes(count) => {
                if count == 0 || count > queued {
                    return Err(DarwinCarrierError::EndOfFile { received });
                }
                received += count;
            }
            ReadProgress::WouldBlock => continue,
            ReadProgress::EndOfFile => {
                return Err(DarwinCarrierError::EndOfFile { received });
            }
        }

        if received == limit {
            ensure_before_deadline(backend, deadline, CarrierStage::BytesAvailable)?;
            let queued = backend
                .bytes_available()
                .map_err(|fault| system_error(CarrierStage::BytesAvailable, fault))?;
            if queued != 0 {
                return Err(DarwinCarrierError::Overflow { received, queued });
            }
            return SnapshotRead::complete(&bytes).map_err(Into::into);
        }
    }
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
        Err(DarwinCarrierError::OperationMismatch { .. }) => {
            SanitizedAttemptOutcome::OperationRejected
        }
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
            DarwinCarrierError::ReadInvariant(_) | DarwinCarrierError::SnapshotReadInvariant(_),
        ) => SanitizedAttemptOutcome::ReadInvariant,
        Err(DarwinCarrierError::ProtocolEncoding(_)) => SanitizedAttemptOutcome::OperationRejected,
        Err(DarwinCarrierError::Cleanup { .. }) => SanitizedAttemptOutcome::CleanupFailed,
    }
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
        termios::{self, ControlModes, OptionalActions, SpecialCodeIndex, Termios},
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
