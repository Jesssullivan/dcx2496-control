//! Darwin-only, Search-only tty carrier for the DCX2496 discovery boundary.
//!
//! The public carrier exists only on macOS. It accepts one private, in-memory
//! callout-device binding and implements only [`SearchTransport`]. There is no
//! port enumeration, CLI, generic byte-write method, retry loop, or retained
//! raw capture. Tests exercise the same state machine through injected fake
//! syscalls without opening any device.

use std::{
    fmt,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    time::Duration,
};

use dcx_transport::{
    SEARCH_RESPONSE_LIMIT, SearchOperation, SearchRead, SearchReadEnd, SearchReadError,
    SearchTransport,
};
use serde::{Serialize, Serializer};
use sha2::{Digest, Sha256};
use thiserror::Error;

const PRIVATE_CALLOUT_PREFIX: &[u8] = b"/dev/cu.usbserial-";
const SHA256_PREFIX: &str = "sha256/";
const SHA256_HEX_LEN: usize = 64;
/// Portion of the 500 ms whole-attempt budget reserved for restoration/close.
pub const SEARCH_CLEANUP_RESERVE: Duration = Duration::from_millis(25);

/// A lower-case, prefixed SHA-256 digest safe for sanitized receipts.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    /// Parse the exact `sha256/<64 lower-case hex>` receipt form.
    ///
    /// # Errors
    ///
    /// Returns [`BindingError::InvalidDigest`] for any other representation.
    pub fn parse(value: &str) -> Result<Self, BindingError> {
        let Some(hex) = value.strip_prefix(SHA256_PREFIX) else {
            return Err(BindingError::InvalidDigest);
        };
        if hex.len() != SHA256_HEX_LEN
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(BindingError::InvalidDigest);
        }

        let mut bytes = [0_u8; 32];
        let (pairs, remainder) = hex.as_bytes().as_chunks::<2>();
        debug_assert!(remainder.is_empty());
        for (index, pair) in pairs.iter().enumerate() {
            bytes[index] = (decode_hex(pair[0]) << 4) | decode_hex(pair[1]);
        }
        Ok(Self(bytes))
    }

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

const fn decode_hex(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => 0,
    }
}

/// A private callout path paired with an independently observed path digest.
///
/// The raw path is intentionally not cloneable, serializable, displayable, or
/// accessible after construction. Callers must obtain it through a private
/// in-process channel; command-line and environment-variable transport are not
/// provided. The digest is recomputed immediately before every open.
pub struct PrivateTtyBinding {
    path: PathBuf,
    expected_digest: Sha256Digest,
}

impl PrivateTtyBinding {
    /// Bind one exact FTDI-style Darwin callout path to an independent digest.
    ///
    /// # Errors
    ///
    /// Returns [`BindingError`] when the path is outside the narrow
    /// `/dev/cu.usbserial-*` envelope, the digest is malformed, or the supplied
    /// path does not match it.
    pub fn new(path: PathBuf, expected_digest: &str) -> Result<Self, BindingError> {
        validate_private_path(&path)?;
        let expected_digest = Sha256Digest::parse(expected_digest)?;
        let binding = Self {
            path,
            expected_digest,
        };
        binding.verify()?;
        Ok(binding)
    }

    /// Return only the sanitized path digest.
    pub const fn digest(&self) -> Sha256Digest {
        self.expected_digest
    }

    fn verify(&self) -> Result<(), BindingError> {
        validate_private_path(&self.path)?;
        let actual = Sha256Digest::of_bytes(self.path.as_os_str().as_bytes());
        if actual != self.expected_digest {
            return Err(BindingError::DigestMismatch {
                expected: self.expected_digest,
                actual,
            });
        }
        Ok(())
    }
}

impl fmt::Debug for PrivateTtyBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PrivateTtyBinding")
            .field("path", &"[redacted]")
            .field("digest", &self.expected_digest)
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

/// Fail-closed private-binding validation failures.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum BindingError {
    /// Digest syntax was not the canonical lower-case SHA-256 form.
    #[error("binding digest is not canonical sha256/<64 lower-case hex>")]
    InvalidDigest,
    /// The path was not one exact FTDI-style Darwin callout node.
    #[error("private tty binding is outside the approved Darwin callout envelope")]
    UnsupportedPrivatePath,
    /// The in-memory path did not match the independently supplied digest.
    #[error("private tty binding digest mismatch (expected {expected}, observed {actual})")]
    DigestMismatch {
        /// Independently observed digest.
        expected: Sha256Digest,
        /// Digest recomputed from the private path.
        actual: Sha256Digest,
    },
}

/// A named carrier operation suitable for sanitized failure receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CarrierStage {
    /// Revalidate the private path digest.
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
    /// Perform the sole eight-byte write syscall.
    Write,
    /// Query queued input without consuming bytes.
    BytesAvailable,
    /// Wait for readability within the remaining monotonic deadline.
    WaitReadable,
    /// Consume no more than the remaining response allowance.
    Read,
    /// Restore the saved termios structure.
    RestoreTermios,
    /// Restore the saved modem control-line bits.
    RestoreControlLines,
    /// Close the owned descriptor after all restoration attempts.
    Close,
}

/// Broad terminal reason retained when cleanup must replace a primary error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CarrierFailureKind {
    /// Private binding did not revalidate.
    Binding,
    /// The monotonic attempt budget was exhausted outside the read timeout.
    Deadline,
    /// A system call failed.
    System,
    /// The sole write did not accept exactly eight bytes.
    ShortWrite,
    /// Input was already queued before the Search write.
    PreexistingInput,
    /// More than 26 input bytes were pending.
    Overflow,
    /// The tty reached end-of-file.
    EndOfFile,
    /// An internal bounded-read result invariant failed.
    ReadInvariant,
}

/// Error from one Darwin Search attempt.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DarwinCarrierError {
    /// Exact private binding failed before any open.
    #[error(transparent)]
    Binding(#[from] BindingError),
    /// The monotonic 500 ms attempt deadline expired.
    #[error("Search attempt exceeded its monotonic deadline at {stage:?}")]
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
    /// The one write syscall accepted fewer than all eight Search bytes.
    #[error("sole Search write accepted {written} bytes; expected exactly 8")]
    ShortWrite {
        /// Number accepted by the single syscall.
        written: usize,
    },
    /// Input was queued before configuration or the Search write.
    #[error("Search blocked because {queued} pre-existing input bytes were queued")]
    PreexistingInput {
        /// Bytes observed without consuming or flushing them.
        queued: usize,
    },
    /// More bytes were queued than the exact response budget permits.
    #[error("Search response overflow: {received} received and {queued} additional queued")]
    Overflow {
        /// Bytes already consumed, always at most 26.
        received: usize,
        /// Bytes observed pending without consuming them.
        queued: usize,
    },
    /// The tty returned EOF before an exact response or timeout.
    #[error("Search tty reached EOF after {received} bytes")]
    EndOfFile {
        /// Bytes received before EOF.
        received: usize,
    },
    /// The bounded result constructor rejected internal state.
    #[error(transparent)]
    ReadInvariant(#[from] SearchReadError),
    /// One or both mandatory restoration operations failed; close still ran.
    #[error(
        "Search cleanup failed (primary={primary:?}, termios_failed={termios_failed}, control_lines_failed={control_lines_failed})"
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
            Self::Binding(_) => CarrierFailureKind::Binding,
            Self::Deadline { .. } => CarrierFailureKind::Deadline,
            Self::System { .. } | Self::Cleanup { .. } => CarrierFailureKind::System,
            Self::ShortWrite { .. } => CarrierFailureKind::ShortWrite,
            Self::PreexistingInput { .. } => CarrierFailureKind::PreexistingInput,
            Self::Overflow { .. } => CarrierFailureKind::Overflow,
            Self::EndOfFile { .. } => CarrierFailureKind::EndOfFile,
            Self::ReadInvariant(_) => CarrierFailureKind::ReadInvariant,
        }
    }
}

/// Cleanup state represented without exposing a descriptor or terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupDisposition {
    /// No carrier setting had been applied.
    NotRequired,
    /// The original state was restored.
    Restored,
    /// Restoration was attempted and failed closed.
    Failed,
}

/// Sanitized outcome of the carrier layer, before protocol validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SanitizedAttemptOutcome {
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
    /// Additional input was detected without crossing the 26-byte ceiling.
    Overflow,
    /// The tty reached EOF.
    EndOfFile,
    /// A bounded result invariant failed.
    ReadInvariant,
    /// Restoration failed; the descriptor was still closed.
    CleanupFailed,
}

/// Primary or sole fallback attempt represented in a sanitized receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SanitizedAttemptKind {
    /// Vendor-documented 115200 baud attempt.
    Primary,
    /// One 38400 baud compatibility fallback.
    SingleFallback,
}

impl From<SearchOperation> for SanitizedAttemptKind {
    fn from(operation: SearchOperation) -> Self {
        match operation.attempt() {
            dcx_core::discovery::DiscoveryAttemptKind::Primary => Self::Primary,
            dcx_core::discovery::DiscoveryAttemptKind::SingleFallback => Self::SingleFallback,
        }
    }
}

/// Machine-readable receipt that never contains a tty path or raw response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizedAttemptReceipt {
    /// Primary or sole fallback attempt.
    pub attempt: SanitizedAttemptKind,
    /// Independently supplied private-path digest.
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
    /// Bytes consumed, never more than 26.
    pub rx_bytes: usize,
    /// Digest of consumed input, omitted for empty reads.
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
    fn restore_control_lines(&mut self, state: i32) -> Result<(), SystemFault>;
    fn close(&mut self);
}

struct ReceiptBuilder {
    attempt: SanitizedAttemptKind,
    binding_digest: Sha256Digest,
    baud: u32,
    deadline_millis: u64,
    cleanup_reserve_millis: u64,
    tx_bytes: usize,
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

struct Carrier<B> {
    binding: PrivateTtyBinding,
    backend: B,
    receipts: Vec<SanitizedAttemptReceipt>,
}

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
            return Err(DarwinCarrierError::ShortWrite { written });
        }
        ensure_before_deadline(backend, active_io_deadline, CarrierStage::BytesAvailable)?;

        read_bounded(
            backend,
            active_io_deadline,
            operation.response_limit(),
            &mut receipt,
        )
    })();

    let primary_kind = primary.as_ref().err().map(DarwinCarrierError::kind);
    let mut termios_failed = false;
    let mut control_lines_failed = false;

    if configuration_attempted {
        receipt.termios_cleanup = match termios_snapshot.as_ref() {
            Some(snapshot) if backend.restore_termios(snapshot).is_ok() => {
                CleanupDisposition::Restored
            }
            Some(_) => {
                termios_failed = true;
                CleanupDisposition::Failed
            }
            None => CleanupDisposition::NotRequired,
        };
        receipt.control_lines_cleanup = match control_lines_snapshot {
            Some(state) if backend.restore_control_lines(state).is_ok() => {
                CleanupDisposition::Restored
            }
            Some(_) => {
                control_lines_failed = true;
                CleanupDisposition::Failed
            }
            None => CleanupDisposition::NotRequired,
        };
    }

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
    receipt: &mut ReceiptBuilder,
) -> Result<SearchRead, DarwinCarrierError> {
    let mut bytes = [0_u8; SEARCH_RESPONSE_LIMIT];
    let mut received = 0;
    debug_assert_eq!(limit, SEARCH_RESPONSE_LIMIT);

    loop {
        let time_left = remaining(backend, deadline);
        if time_left.is_zero() {
            return SearchRead::timed_out(&bytes[..received]).map_err(Into::into);
        }

        let queued = backend
            .bytes_available()
            .map_err(|fault| system_error(CarrierStage::BytesAvailable, fault))?;
        if queued > limit - received {
            return Err(DarwinCarrierError::Overflow { received, queued });
        }

        if queued == 0 {
            let time_left = remaining(backend, deadline);
            if time_left.is_zero()
                || !backend
                    .wait_readable(time_left)
                    .map_err(|fault| system_error(CarrierStage::WaitReadable, fault))?
            {
                return SearchRead::timed_out(&bytes[..received]).map_err(Into::into);
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
                receipt.received(&bytes[received..received + count]);
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
            return SearchRead::complete(&bytes).map_err(Into::into);
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
        Err(DarwinCarrierError::Binding(_)) => SanitizedAttemptOutcome::BindingRejected,
        Err(DarwinCarrierError::System { .. }) => SanitizedAttemptOutcome::SystemError,
        Err(DarwinCarrierError::Deadline { .. }) => SanitizedAttemptOutcome::DeadlineExceeded,
        Err(DarwinCarrierError::ShortWrite { .. }) => SanitizedAttemptOutcome::ShortWrite,
        Err(DarwinCarrierError::PreexistingInput { .. }) => {
            SanitizedAttemptOutcome::PreexistingInput
        }
        Err(DarwinCarrierError::Overflow { .. }) => SanitizedAttemptOutcome::Overflow,
        Err(DarwinCarrierError::EndOfFile { .. }) => SanitizedAttemptOutcome::EndOfFile,
        Err(DarwinCarrierError::ReadInvariant(_)) => SanitizedAttemptOutcome::ReadInvariant,
        Err(DarwinCarrierError::Cleanup { .. }) => SanitizedAttemptOutcome::CleanupFailed,
    }
}

/// Review-gated macOS implementation of the typed Search-only transport.
///
/// Construction does not open a descriptor. Each `search` call revalidates the
/// private binding, opens one descriptor, performs one bounded attempt, restores
/// prior state, and closes. A live call still requires Legalab's attended
/// hardware WORD gate; this type does not represent authorization.
#[cfg(all(target_os = "macos", not(bazel_test_no_native)))]
pub struct DarwinSearchTransport {
    inner: Carrier<macos::MacOsBackend>,
}

#[cfg(all(target_os = "macos", not(bazel_test_no_native)))]
impl DarwinSearchTransport {
    /// Create an inert carrier for one already validated private binding.
    pub fn new(binding: PrivateTtyBinding) -> Self {
        Self {
            inner: Carrier::new(binding, macos::MacOsBackend::new()),
        }
    }

    /// Borrow sanitized attempt receipts; raw paths and bytes are never stored.
    pub fn receipts(&self) -> &[SanitizedAttemptReceipt] {
        self.inner.receipts()
    }

    /// Drain sanitized receipts for a caller-controlled evidence sink.
    pub fn take_receipts(&mut self) -> Vec<SanitizedAttemptReceipt> {
        self.inner.take_receipts()
    }
}

#[cfg(all(target_os = "macos", not(bazel_test_no_native)))]
impl fmt::Debug for DarwinSearchTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DarwinSearchTransport")
            .field("binding", &self.inner.binding)
            .field("receipt_count", &self.inner.receipts.len())
            .finish()
    }
}

#[cfg(all(target_os = "macos", not(bazel_test_no_native)))]
impl SearchTransport for DarwinSearchTransport {
    type Error = DarwinCarrierError;

    fn search(&mut self, operation: SearchOperation) -> Result<SearchRead, Self::Error> {
        self.inner.search(operation)
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
            self.saved_termios = None;
            Ok(())
        }

        fn restore_control_lines(&mut self, state: i32) -> Result<(), SystemFault> {
            abi::set_control_lines(self.fd()?.as_raw_fd(), state)?;
            self.saved_control_lines = None;
            Ok(())
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
