//! Explicitly feature-gated Darwin operator surface for bounded DCX discovery.

use std::{
    convert::Infallible,
    error::Error,
    fmt,
    fs::File,
    io::{self, IsTerminal, Read, Write},
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use dcx_core::{discovery::DiscoveryError, protocol::DeviceId};
use dcx_darwin_tty::{
    DarwinCarrierError, DarwinSearchTransport, PrivateTtyBinding, SanitizedAttemptReceipt,
    Sha256Digest,
};
use dcx_transport::{
    REPEAT_SEARCH_BUDGET, REPEAT_SEARCH_COUNT, REPEAT_SEARCH_GAP, RepeatPacer, RepeatSearchBinding,
    RepeatSearchError, SEARCH_ATTEMPT_TIMEOUT, SEARCH_REQUEST_LEN, SEARCH_RESPONSE_LIMIT,
    SearchExecutionError, SearchOutcome, execute_search, execute_search_repeat,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_STDIN_BYTES: usize = 16 * 1024;
const MAX_SYSCTL_BYTES: usize = 4 * 1024;
const MAX_AUTHORIZATION_SECONDS: u64 = 15 * 60;
const SEARCH_WORD: &str = "DCX_QUERY_V1";
const REPEAT_WORD: &str = "DCX_QUERY_REPEAT_V1";
const TARGET_HOST_ROLE: &str = "petting-zoo-mini";
const ENVELOPE_SCHEMA: &str = "dcx.live-discovery-envelope/v1";
const PACKET_SCHEMA: &str = "dcx.live-discovery-word-packet/v1";
const RECEIPT_SCHEMA: &str = "dcx.native-discovery-receipt/v1";
const SAFE_PROFILE_EXPECTED_DEVICE_ID: u8 = 0;
const SAFE_PROFILE_DIGEST: &str =
    "sha256/177836a70709a1a12c9bbf52d9a359c691d91c6dcfabe729a1e16b546cf60b0d";
const QUERY_HEX: &str = "F0002032200E40F7";
const QUERY_DIGEST: &str =
    "sha256/b6e9f1f31d934708087d0796a50a3705e4bfd53dbd80fbc7012b4a5dd154ec49";

#[derive(Debug, Clone, Copy)]
pub enum LiveCommand {
    Prepare,
    Live,
    Repeat,
}

impl LiveCommand {
    const fn label(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::Live => "search",
            Self::Repeat => "repeat",
        }
    }
}

#[derive(Debug)]
pub struct LiveCommandFailed;

impl fmt::Display for LiveCommandFailed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("live discovery failed; see sanitized stdout receipt")
    }
}

impl Error for LiveCommandFailed {}

struct CommandOutput {
    value: serde_json::Value,
    success: bool,
}

#[derive(Debug, Clone, Copy)]
struct SanitizedFailure {
    code: &'static str,
}

impl SanitizedFailure {
    const fn new(code: &'static str) -> Self {
        Self { code }
    }
}

pub fn run(command: LiveCommand) -> Result<(), LiveCommandFailed> {
    let result = run_inner(command);
    let (value, success) = match result {
        Ok(output) => (output.value, output.success),
        Err(failure) => (
            serde_json::json!({
                "schemaVersion": RECEIPT_SCHEMA,
                "status": "failed",
                "sanitized": true,
                "action": command.label(),
                "failureCode": failure.code,
                "attempts": [],
                "effects": Effects::zero(),
            }),
            false,
        ),
    };

    let encoded = serde_json::to_string_pretty(&value).map_err(|_| LiveCommandFailed)?;
    writeln!(io::stdout().lock(), "{encoded}").map_err(|_| LiveCommandFailed)?;
    if success {
        Ok(())
    } else {
        Err(LiveCommandFailed)
    }
}

fn run_inner(command: LiveCommand) -> Result<CommandOutput, SanitizedFailure> {
    let bytes = read_bounded_stdin()?;
    let envelope: GateEnvelope =
        serde_json::from_slice(&bytes).map_err(|_| SanitizedFailure::new("invalid_envelope"))?;
    let validated = ValidatedGate::new(envelope, command)?;

    match command {
        LiveCommand::Prepare => Ok(prepare(&validated)),
        LiveCommand::Live => execute_live(validated),
        LiveCommand::Repeat => execute_repeat(validated),
    }
}

fn read_bounded_stdin() -> Result<Vec<u8>, SanitizedFailure> {
    if io::stdin().is_terminal() {
        return Err(SanitizedFailure::new("interactive_stdin_rejected"));
    }
    read_bounded(io::stdin().lock())
}

fn read_bounded(reader: impl Read) -> Result<Vec<u8>, SanitizedFailure> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_STDIN_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SanitizedFailure::new("stdin_read_failed"))?;
    if bytes.len() > MAX_STDIN_BYTES {
        return Err(SanitizedFailure::new("stdin_too_large"));
    }
    Ok(bytes)
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum GateAction {
    Search,
    Repeat,
}

impl GateAction {
    const fn word(self) -> &'static str {
        match self {
            Self::Search => SEARCH_WORD,
            Self::Repeat => REPEAT_WORD,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GateEnvelope {
    schema_version: String,
    action: GateAction,
    private_tty_path: PathBuf,
    binding_digest: String,
    expected_host: ExpectedHost,
    expected_device_id: u8,
    physical: PhysicalDeclaration,
    evidence: EvidenceDeclaration,
    issued_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    prior_search: Option<PriorSearch>,
    authorization: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpectedHost {
    role: String,
    hardware_model: String,
    os_build: String,
}

#[derive(Clone, Deserialize, Serialize)]
// Each boolean is a separately packet-bound physical safety declaration.
#[allow(clippy::struct_excessive_bools)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhysicalDeclaration {
    powered: bool,
    edition: String,
    firmware_version: String,
    port_mode: String,
    rear_rs232_connected: bool,
    adapter_rs232_electrical_verified: bool,
    speakers_disconnected: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EvidenceDeclaration {
    passive_receipt_digest: String,
    passive_captured_at_unix_seconds: u64,
    adapter_electrical_evidence_digest: String,
    profile_digest: String,
    source_revision: String,
    source_tree: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PriorSearch {
    first_packet: serde_json::Value,
    first_packet_digest: String,
    first_receipt_body: serde_json::Value,
    first_receipt_body_digest: String,
    first_response_digest: String,
    binding_digest: String,
    expected_device_id: u8,
    successful_baud: u32,
    first_host_identity_digest: String,
    first_boot_digest: String,
    first_executable_digest: String,
    first_profile_digest: String,
    first_passive_receipt_digest: String,
    first_adapter_electrical_evidence_digest: String,
    first_source_revision: String,
    first_source_tree: String,
    first_physical_digest: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeBinding {
    role: String,
    hardware_model: String,
    os_build: String,
    host_identity_digest: String,
    boot_digest: String,
    executable_digest: String,
}

impl RuntimeBinding {
    fn observe(expected: &ExpectedHost) -> Result<Self, SanitizedFailure> {
        if expected.role != TARGET_HOST_ROLE {
            return Err(SanitizedFailure::new("host_role_mismatch"));
        }
        let hardware_model = sysctl("hw.model")?;
        let os_build = sysctl("kern.osversion")?;
        let local_hostname = local_hostname()?;
        let boot = sysctl("kern.boottime")?;
        if hardware_model != expected.hardware_model || os_build != expected.os_build {
            return Err(SanitizedFailure::new("host_mismatch"));
        }
        if local_hostname != expected.role {
            return Err(SanitizedFailure::new("host_identity_mismatch"));
        }

        Ok(Self {
            role: expected.role.clone(),
            hardware_model,
            os_build,
            host_identity_digest: digest_bytes(local_hostname.as_bytes()),
            boot_digest: digest_bytes(boot.as_bytes()),
            executable_digest: digest_executable()?,
        })
    }
}

fn local_hostname() -> Result<String, SanitizedFailure> {
    bounded_probe("/usr/sbin/scutil", &["--get", "LocalHostName"])
}

fn sysctl(name: &str) -> Result<String, SanitizedFailure> {
    bounded_probe("/usr/sbin/sysctl", &["-n", name])
}

fn bounded_probe(program: &str, arguments: &[&str]) -> Result<String, SanitizedFailure> {
    let output = Command::new(program)
        .env_clear()
        .args(arguments)
        .output()
        .map_err(|_| SanitizedFailure::new("host_probe_failed"))?;
    if !output.status.success()
        || output.stdout.is_empty()
        || output.stdout.len() > MAX_SYSCTL_BYTES
    {
        return Err(SanitizedFailure::new("host_probe_failed"));
    }
    let value = std::str::from_utf8(&output.stdout)
        .map_err(|_| SanitizedFailure::new("host_probe_failed"))?
        .trim()
        .to_owned();
    if value.is_empty() {
        return Err(SanitizedFailure::new("host_probe_failed"));
    }
    Ok(value)
}

fn digest_executable() -> Result<String, SanitizedFailure> {
    let path =
        std::env::current_exe().map_err(|_| SanitizedFailure::new("executable_digest_failed"))?;
    let mut file =
        File::open(path).map_err(|_| SanitizedFailure::new("executable_digest_failed"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| SanitizedFailure::new("executable_digest_failed"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format_digest(hasher.finalize().into()))
}

fn digest_bytes(bytes: &[u8]) -> String {
    format_digest(Sha256::digest(bytes).into())
}

fn canonical_json_digest(value: &serde_json::Value) -> Result<String, SanitizedFailure> {
    serde_json::to_vec(value)
        .map(|bytes| digest_bytes(&bytes))
        .map_err(|_| SanitizedFailure::new("canonical_encoding_failed"))
}

fn serialized_digest<T: Serialize>(value: &T) -> Result<String, SanitizedFailure> {
    let value = serde_json::to_value(value)
        .map_err(|_| SanitizedFailure::new("canonical_encoding_failed"))?;
    canonical_json_digest(&value)
}

fn format_digest(bytes: [u8; 32]) -> String {
    let mut value = String::with_capacity(71);
    value.push_str("sha256/");
    for byte in bytes {
        use fmt::Write as _;
        write!(value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    value
}

fn canonical_digest(value: &str) -> Result<String, SanitizedFailure> {
    Sha256Digest::parse(value)
        .map(|digest| digest.to_string())
        .map_err(|_| SanitizedFailure::new("invalid_digest"))
}

fn canonical_git_object(value: &str) -> Result<String, SanitizedFailure> {
    if value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(value.to_owned())
    } else {
        Err(SanitizedFailure::new("invalid_source_revision"))
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GatePacket {
    schema_version: &'static str,
    action: GateAction,
    host: RuntimeBinding,
    binding_digest: String,
    expected_device_id: u8,
    basis: EvidenceBasis,
    operator_declared_physical: PhysicalDeclaration,
    operator_declared_physical_digest: String,
    operator_declared_evidence: EvidenceDeclaration,
    issued_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    prior_search: Option<PriorSearch>,
    limits: GateLimits,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceBasis {
    runtime_binding: &'static str,
    expected_device_id: &'static str,
    physical: &'static str,
    artifact_digests_and_source_revision: &'static str,
    protocol_result: &'static str,
}

impl EvidenceBasis {
    const fn exact() -> Self {
        Self {
            runtime_binding: "native_observed",
            expected_device_id: "desired_profile_declared",
            physical: "operator_reported",
            artifact_digests_and_source_revision: "operator_declared",
            protocol_result: "native_observed",
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum GateLimits {
    Search {
        query_hex: &'static str,
        query_digest: &'static str,
        primary_baud: u32,
        fallback_baud: u32,
        fallback_eligibility: &'static str,
        attempt_timeout_millis: u64,
        max_attempts: usize,
        tx_bytes_per_attempt: usize,
        rx_bytes_per_attempt: usize,
        max_total_tx_bytes: usize,
        max_total_rx_bytes: usize,
    },
    Repeat {
        query_hex: &'static str,
        query_digest: &'static str,
        successful_baud: u32,
        repeat_count: usize,
        minimum_gap_millis: u64,
        attempt_timeout_millis: u64,
        whole_budget_millis: u64,
        tx_bytes_per_attempt: usize,
        rx_bytes_per_attempt: usize,
        max_total_tx_bytes: usize,
        max_total_rx_bytes: usize,
    },
}

struct ValidatedGate {
    binding: PrivateTtyBinding,
    packet: GatePacket,
    packet_digest: String,
    required_word: String,
}

impl ValidatedGate {
    fn new(envelope: GateEnvelope, command: LiveCommand) -> Result<Self, SanitizedFailure> {
        let host = RuntimeBinding::observe(&envelope.expected_host)?;
        Self::new_with_runtime(envelope, command, host)
    }

    // This keeps the full fail-closed gate in one auditable decision path.
    #[allow(clippy::too_many_lines)]
    fn new_with_runtime(
        mut envelope: GateEnvelope,
        command: LiveCommand,
        mut host: RuntimeBinding,
    ) -> Result<Self, SanitizedFailure> {
        if envelope.schema_version != ENVELOPE_SCHEMA {
            return Err(SanitizedFailure::new("unsupported_envelope_schema"));
        }
        if envelope.expected_host.role != TARGET_HOST_ROLE
            || host.role != envelope.expected_host.role
            || host.hardware_model != envelope.expected_host.hardware_model
            || host.os_build != envelope.expected_host.os_build
        {
            return Err(SanitizedFailure::new("host_mismatch"));
        }
        host.host_identity_digest = canonical_digest(&host.host_identity_digest)?;
        host.boot_digest = canonical_digest(&host.boot_digest)?;
        host.executable_digest = canonical_digest(&host.executable_digest)?;
        let expected_action = match command {
            LiveCommand::Prepare => envelope.action,
            LiveCommand::Live => GateAction::Search,
            LiveCommand::Repeat => GateAction::Repeat,
        };
        if envelope.action != expected_action {
            return Err(SanitizedFailure::new("action_mismatch"));
        }
        if command.label() == "prepare" && envelope.authorization.is_some() {
            return Err(SanitizedFailure::new(
                "authorization_present_during_prepare",
            ));
        }

        validate_window(
            envelope.issued_at_unix_seconds,
            envelope.expires_at_unix_seconds,
            envelope.evidence.passive_captured_at_unix_seconds,
        )?;
        validate_physical(&envelope.physical)?;
        let expected_device = DeviceId::new(envelope.expected_device_id)
            .map_err(|_| SanitizedFailure::new("invalid_device_id"))?;

        envelope.binding_digest = canonical_digest(&envelope.binding_digest)?;
        envelope.evidence.passive_receipt_digest =
            canonical_digest(&envelope.evidence.passive_receipt_digest)?;
        envelope.evidence.adapter_electrical_evidence_digest =
            canonical_digest(&envelope.evidence.adapter_electrical_evidence_digest)?;
        envelope.evidence.profile_digest = canonical_digest(&envelope.evidence.profile_digest)?;
        if envelope.expected_device_id != SAFE_PROFILE_EXPECTED_DEVICE_ID
            || envelope.evidence.profile_digest != SAFE_PROFILE_DIGEST
        {
            return Err(SanitizedFailure::new("unsupported_live_profile"));
        }
        envelope.evidence.source_revision =
            canonical_git_object(&envelope.evidence.source_revision)?;
        envelope.evidence.source_tree = canonical_git_object(&envelope.evidence.source_tree)?;

        if envelope.action == GateAction::Search && envelope.prior_search.is_some() {
            return Err(SanitizedFailure::new("unexpected_prior_search"));
        }
        if let Some(prior) = envelope.prior_search.as_mut() {
            prior.first_packet_digest = canonical_digest(&prior.first_packet_digest)?;
            prior.first_receipt_body_digest = canonical_digest(&prior.first_receipt_body_digest)?;
            prior.first_response_digest = canonical_digest(&prior.first_response_digest)?;
            prior.binding_digest = canonical_digest(&prior.binding_digest)?;
            prior.first_host_identity_digest = canonical_digest(&prior.first_host_identity_digest)?;
            prior.first_boot_digest = canonical_digest(&prior.first_boot_digest)?;
            prior.first_executable_digest = canonical_digest(&prior.first_executable_digest)?;
            prior.first_profile_digest = canonical_digest(&prior.first_profile_digest)?;
            prior.first_passive_receipt_digest =
                canonical_digest(&prior.first_passive_receipt_digest)?;
            prior.first_adapter_electrical_evidence_digest =
                canonical_digest(&prior.first_adapter_electrical_evidence_digest)?;
            prior.first_source_revision = canonical_git_object(&prior.first_source_revision)?;
            prior.first_source_tree = canonical_git_object(&prior.first_source_tree)?;
            prior.first_physical_digest = canonical_digest(&prior.first_physical_digest)?;
            if canonical_json_digest(&prior.first_packet)? != prior.first_packet_digest
                || canonical_json_digest(&prior.first_receipt_body)?
                    != prior.first_receipt_body_digest
            {
                return Err(SanitizedFailure::new("prior_search_digest_mismatch"));
            }
            validate_prior_search_proof(prior, envelope.issued_at_unix_seconds)?;
            if prior.binding_digest != envelope.binding_digest
                || prior.expected_device_id != envelope.expected_device_id
            {
                return Err(SanitizedFailure::new("prior_search_binding_mismatch"));
            }
            RepeatSearchBinding::from_successful_baud(expected_device, prior.successful_baud)
                .map_err(|_| SanitizedFailure::new("unsupported_repeat_baud"))?;
        } else if envelope.action == GateAction::Repeat {
            return Err(SanitizedFailure::new("missing_prior_search"));
        }

        let binding = PrivateTtyBinding::new(envelope.private_tty_path, &envelope.binding_digest)
            .map_err(|_| SanitizedFailure::new("private_binding_rejected"))?;
        let physical_digest = serialized_digest(&envelope.physical)?;
        if let Some(prior) = envelope.prior_search.as_ref()
            && (prior.first_host_identity_digest != host.host_identity_digest
                || prior.first_boot_digest != host.boot_digest
                || prior.first_executable_digest != host.executable_digest
                || json_string(&prior.first_packet, "/host/role") != Some(host.role.as_str())
                || json_string(&prior.first_packet, "/host/hardwareModel")
                    != Some(host.hardware_model.as_str())
                || json_string(&prior.first_packet, "/host/osBuild")
                    != Some(host.os_build.as_str())
                || prior.first_profile_digest != envelope.evidence.profile_digest
                || prior.first_passive_receipt_digest != envelope.evidence.passive_receipt_digest
                || json_u64(
                    &prior.first_packet,
                    "/operatorDeclaredEvidence/passiveCapturedAtUnixSeconds",
                ) != Some(envelope.evidence.passive_captured_at_unix_seconds)
                || prior.first_adapter_electrical_evidence_digest
                    != envelope.evidence.adapter_electrical_evidence_digest
                || prior.first_source_revision != envelope.evidence.source_revision
                || prior.first_source_tree != envelope.evidence.source_tree
                || prior.first_physical_digest != physical_digest)
        {
            return Err(SanitizedFailure::new("prior_search_evidence_drift"));
        }
        let limits = match envelope.action {
            GateAction::Search => GateLimits::Search {
                query_hex: QUERY_HEX,
                query_digest: QUERY_DIGEST,
                primary_baud: 115_200,
                fallback_baud: 38_400,
                fallback_eligibility: "empty_primary_timeout_only",
                attempt_timeout_millis: 500,
                max_attempts: 2,
                tx_bytes_per_attempt: SEARCH_REQUEST_LEN,
                rx_bytes_per_attempt: SEARCH_RESPONSE_LIMIT,
                max_total_tx_bytes: SEARCH_REQUEST_LEN * 2,
                max_total_rx_bytes: SEARCH_RESPONSE_LIMIT,
            },
            GateAction::Repeat => GateLimits::Repeat {
                query_hex: QUERY_HEX,
                query_digest: QUERY_DIGEST,
                successful_baud: envelope
                    .prior_search
                    .as_ref()
                    .expect("repeat prior search validated")
                    .successful_baud,
                repeat_count: REPEAT_SEARCH_COUNT,
                minimum_gap_millis: duration_millis(REPEAT_SEARCH_GAP),
                attempt_timeout_millis: duration_millis(SEARCH_ATTEMPT_TIMEOUT),
                whole_budget_millis: duration_millis(REPEAT_SEARCH_BUDGET),
                tx_bytes_per_attempt: SEARCH_REQUEST_LEN,
                rx_bytes_per_attempt: SEARCH_RESPONSE_LIMIT,
                max_total_tx_bytes: SEARCH_REQUEST_LEN * REPEAT_SEARCH_COUNT,
                max_total_rx_bytes: SEARCH_RESPONSE_LIMIT * REPEAT_SEARCH_COUNT,
            },
        };
        let packet = GatePacket {
            schema_version: PACKET_SCHEMA,
            action: envelope.action,
            host,
            binding_digest: envelope.binding_digest,
            expected_device_id: envelope.expected_device_id,
            basis: EvidenceBasis::exact(),
            operator_declared_physical: envelope.physical,
            operator_declared_physical_digest: physical_digest,
            operator_declared_evidence: envelope.evidence,
            issued_at_unix_seconds: envelope.issued_at_unix_seconds,
            expires_at_unix_seconds: envelope.expires_at_unix_seconds,
            prior_search: envelope.prior_search,
            limits,
        };
        let packet_digest = serialized_digest(&packet)?;
        let required_word = format!("WORD {} {packet_digest}", packet.action.word());
        match command {
            LiveCommand::Prepare => {}
            LiveCommand::Live | LiveCommand::Repeat => {
                if envelope.authorization.as_deref() != Some(required_word.as_str()) {
                    return Err(SanitizedFailure::new("authorization_mismatch"));
                }
            }
        }

        Ok(Self {
            binding,
            packet,
            packet_digest,
            required_word,
        })
    }
}

fn validate_window(issued: u64, expires: u64, passive: u64) -> Result<(), SanitizedFailure> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| SanitizedFailure::new("system_time_invalid"))?
        .as_secs();
    if issued > now || now >= expires || expires.saturating_sub(issued) > MAX_AUTHORIZATION_SECONDS
    {
        return Err(SanitizedFailure::new("authorization_expired_or_invalid"));
    }
    if passive > issued || passive > now || now.saturating_sub(passive) > MAX_AUTHORIZATION_SECONDS
    {
        return Err(SanitizedFailure::new("passive_evidence_stale"));
    }
    Ok(())
}

fn validate_physical(physical: &PhysicalDeclaration) -> Result<(), SanitizedFailure> {
    if !physical.powered
        || physical.edition != "standard-non-le"
        || physical.firmware_version != "1.17"
        || physical.port_mode != "RS-232"
        || !physical.rear_rs232_connected
        || !physical.adapter_rs232_electrical_verified
        || !physical.speakers_disconnected
    {
        return Err(SanitizedFailure::new("physical_precondition_failed"));
    }
    Ok(())
}

// The strict v1 proof validator intentionally names every bound receipt field.
#[allow(clippy::too_many_lines)]
fn validate_prior_search_proof(
    prior: &PriorSearch,
    repeat_issued_at: u64,
) -> Result<(), SanitizedFailure> {
    const PACKET_KEYS: &[&str] = &[
        "schemaVersion",
        "action",
        "host",
        "bindingDigest",
        "expectedDeviceId",
        "basis",
        "operatorDeclaredPhysical",
        "operatorDeclaredPhysicalDigest",
        "operatorDeclaredEvidence",
        "issuedAtUnixSeconds",
        "expiresAtUnixSeconds",
        "priorSearch",
        "limits",
    ];
    const RECEIPT_KEYS: &[&str] = &[
        "schemaVersion",
        "status",
        "sanitized",
        "packet",
        "packetDigest",
        "completedAtUnixSeconds",
        "failureCode",
        "selectedBaud",
        "observedDeviceId",
        "validResponseCount",
        "rxDigests",
        "attempts",
        "effects",
    ];
    let packet = &prior.first_packet;
    let receipt = &prior.first_receipt_body;
    let strict_shapes = has_exact_keys(packet, PACKET_KEYS)
        && has_exact_keys(receipt, RECEIPT_KEYS)
        && packet.pointer("/host").is_some_and(|value| {
            has_exact_keys(
                value,
                &[
                    "role",
                    "hardwareModel",
                    "osBuild",
                    "hostIdentityDigest",
                    "bootDigest",
                    "executableDigest",
                ],
            )
        })
        && packet.pointer("/basis").is_some_and(|value| {
            has_exact_keys(
                value,
                &[
                    "runtimeBinding",
                    "expectedDeviceId",
                    "physical",
                    "artifactDigestsAndSourceRevision",
                    "protocolResult",
                ],
            )
        })
        && packet
            .pointer("/operatorDeclaredPhysical")
            .is_some_and(|value| {
                has_exact_keys(
                    value,
                    &[
                        "powered",
                        "edition",
                        "firmwareVersion",
                        "portMode",
                        "rearRs232Connected",
                        "adapterRs232ElectricalVerified",
                        "speakersDisconnected",
                    ],
                )
            })
        && packet
            .pointer("/operatorDeclaredEvidence")
            .is_some_and(|value| {
                has_exact_keys(
                    value,
                    &[
                        "passiveReceiptDigest",
                        "passiveCapturedAtUnixSeconds",
                        "adapterElectricalEvidenceDigest",
                        "profileDigest",
                        "sourceRevision",
                        "sourceTree",
                    ],
                )
            })
        && packet.pointer("/limits").is_some_and(|value| {
            has_exact_keys(
                value,
                &[
                    "action",
                    "queryHex",
                    "queryDigest",
                    "primaryBaud",
                    "fallbackBaud",
                    "fallbackEligibility",
                    "attemptTimeoutMillis",
                    "maxAttempts",
                    "txBytesPerAttempt",
                    "rxBytesPerAttempt",
                    "maxTotalTxBytes",
                    "maxTotalRxBytes",
                ],
            )
        })
        && receipt.pointer("/effects").is_some_and(|value| {
            has_exact_keys(
                value,
                &[
                    "queryTxBytes",
                    "queryRxBytes",
                    "deviceConfigurationWrites",
                    "audioRoutingChanged",
                    "soundOutput",
                    "clockChanged",
                    "driverChanged",
                    "systemExtensionChanged",
                    "sipChanged",
                    "persistentHostMutation",
                    "transientHostFileStaging",
                ],
            )
        });
    let expected_basis = serde_json::to_value(EvidenceBasis::exact())
        .map_err(|_| SanitizedFailure::new("canonical_encoding_failed"))?;
    let expected_limits = serde_json::to_value(GateLimits::Search {
        query_hex: QUERY_HEX,
        query_digest: QUERY_DIGEST,
        primary_baud: 115_200,
        fallback_baud: 38_400,
        fallback_eligibility: "empty_primary_timeout_only",
        attempt_timeout_millis: 500,
        max_attempts: 2,
        tx_bytes_per_attempt: SEARCH_REQUEST_LEN,
        rx_bytes_per_attempt: SEARCH_RESPONSE_LIMIT,
        max_total_tx_bytes: SEARCH_REQUEST_LEN * 2,
        max_total_rx_bytes: SEARCH_RESPONSE_LIMIT,
    })
    .map_err(|_| SanitizedFailure::new("canonical_encoding_failed"))?;
    let embedded_physical_digest = packet
        .pointer("/operatorDeclaredPhysical")
        .map(canonical_json_digest)
        .transpose()?;
    let first_issued_at = json_u64(packet, "/issuedAtUnixSeconds");
    let first_expires_at = json_u64(packet, "/expiresAtUnixSeconds");
    let first_passive_at = json_u64(
        packet,
        "/operatorDeclaredEvidence/passiveCapturedAtUnixSeconds",
    );
    let first_completed_at = json_u64(receipt, "/completedAtUnixSeconds");
    let chronology_valid = matches!(
        (first_passive_at, first_issued_at, first_completed_at, first_expires_at),
        (Some(passive), Some(issued), Some(completed), Some(expires))
            if passive <= issued
                && issued <= completed
                && completed < expires
                && completed <= repeat_issued_at
                && repeat_issued_at.saturating_sub(completed) <= MAX_AUTHORIZATION_SECONDS
    );
    let packet_valid = json_string(packet, "/schemaVersion") == Some(PACKET_SCHEMA)
        && json_string(packet, "/action") == Some("search")
        && packet
            .pointer("/priorSearch")
            .is_some_and(serde_json::Value::is_null)
        && json_string(packet, "/bindingDigest") == Some(prior.binding_digest.as_str())
        && json_u64(packet, "/expectedDeviceId") == Some(u64::from(prior.expected_device_id))
        && json_string(packet, "/host/hostIdentityDigest")
            == Some(prior.first_host_identity_digest.as_str())
        && json_string(packet, "/host/bootDigest") == Some(prior.first_boot_digest.as_str())
        && json_string(packet, "/host/executableDigest")
            == Some(prior.first_executable_digest.as_str())
        && embedded_physical_digest.as_deref() == Some(prior.first_physical_digest.as_str())
        && json_string(packet, "/operatorDeclaredPhysicalDigest")
            == Some(prior.first_physical_digest.as_str())
        && json_string(packet, "/operatorDeclaredEvidence/profileDigest")
            == Some(prior.first_profile_digest.as_str())
        && json_string(packet, "/operatorDeclaredEvidence/passiveReceiptDigest")
            == Some(prior.first_passive_receipt_digest.as_str())
        && json_string(
            packet,
            "/operatorDeclaredEvidence/adapterElectricalEvidenceDigest",
        ) == Some(prior.first_adapter_electrical_evidence_digest.as_str())
        && json_string(packet, "/operatorDeclaredEvidence/sourceRevision")
            == Some(prior.first_source_revision.as_str())
        && json_string(packet, "/operatorDeclaredEvidence/sourceTree")
            == Some(prior.first_source_tree.as_str())
        && packet.pointer("/basis") == Some(&expected_basis)
        && packet.pointer("/limits") == Some(&expected_limits);
    let digests_valid = receipt
        .pointer("/rxDigests")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|digests| {
            digests.len() == 1 && digests[0].as_str() == Some(prior.first_response_digest.as_str())
        });
    let attempts_valid = receipt
        .pointer("/attempts")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|attempts| validate_prior_attempts(attempts, prior));
    let expected_tx = if prior.successful_baud == 115_200 {
        8
    } else {
        16
    };
    let effects_valid = json_u64(receipt, "/effects/queryTxBytes") == Some(expected_tx)
        && json_u64(receipt, "/effects/queryRxBytes") == Some(26)
        && json_u64(receipt, "/effects/deviceConfigurationWrites") == Some(0)
        && false_effect(receipt, "/effects/audioRoutingChanged")
        && false_effect(receipt, "/effects/soundOutput")
        && false_effect(receipt, "/effects/clockChanged")
        && false_effect(receipt, "/effects/driverChanged")
        && false_effect(receipt, "/effects/systemExtensionChanged")
        && false_effect(receipt, "/effects/sipChanged")
        && false_effect(receipt, "/effects/persistentHostMutation")
        && json_string(receipt, "/effects/transientHostFileStaging")
            == Some("outside_process_not_observed");
    let receipt_valid = json_string(receipt, "/schemaVersion") == Some(RECEIPT_SCHEMA)
        && json_string(receipt, "/status") == Some("succeeded")
        && receipt
            .pointer("/sanitized")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        && receipt
            .pointer("/failureCode")
            .is_some_and(serde_json::Value::is_null)
        && json_string(receipt, "/packetDigest") == Some(prior.first_packet_digest.as_str())
        && receipt.pointer("/packet") == Some(packet)
        && json_u64(receipt, "/completedAtUnixSeconds").is_some_and(|value| value != 0)
        && json_u64(receipt, "/observedDeviceId") == Some(u64::from(prior.expected_device_id))
        && json_u64(receipt, "/selectedBaud") == Some(u64::from(prior.successful_baud))
        && json_u64(receipt, "/validResponseCount") == Some(1)
        && digests_valid
        && attempts_valid
        && effects_valid
        && chronology_valid;
    if !strict_shapes
        || contains_private_material(packet)
        || contains_private_material(receipt)
        || !packet_valid
        || !receipt_valid
    {
        return Err(SanitizedFailure::new("invalid_prior_search_proof"));
    }
    Ok(())
}

fn validate_prior_attempts(attempts: &[serde_json::Value], prior: &PriorSearch) -> bool {
    match (prior.successful_baud, attempts) {
        (115_200, [primary]) => validate_attempt(
            primary,
            prior,
            "primary",
            115_200,
            "complete",
            26,
            Some(prior.first_response_digest.as_str()),
        ),
        (38_400, [primary, fallback]) => {
            validate_attempt(primary, prior, "primary", 115_200, "timed_out", 0, None)
                && validate_attempt(
                    fallback,
                    prior,
                    "single_fallback",
                    38_400,
                    "complete",
                    26,
                    Some(prior.first_response_digest.as_str()),
                )
        }
        _ => false,
    }
}

fn validate_attempt(
    attempt: &serde_json::Value,
    prior: &PriorSearch,
    kind: &str,
    baud: u32,
    outcome: &str,
    rx_bytes: u64,
    rx_digest: Option<&str>,
) -> bool {
    has_exact_keys(
        attempt,
        &[
            "attempt",
            "bindingDigest",
            "baud",
            "deadlineMillis",
            "cleanupReserveMillis",
            "elapsedMicros",
            "txBytes",
            "rxBytes",
            "rxDigest",
            "outcome",
            "termiosCleanup",
            "controlLinesCleanup",
            "closed",
        ],
    ) && json_string(attempt, "/attempt") == Some(kind)
        && json_string(attempt, "/bindingDigest") == Some(prior.binding_digest.as_str())
        && json_u64(attempt, "/baud") == Some(u64::from(baud))
        && json_u64(attempt, "/deadlineMillis") == Some(500)
        && json_u64(attempt, "/cleanupReserveMillis") == Some(25)
        && json_u64(attempt, "/elapsedMicros").is_some_and(|elapsed| elapsed <= 500_000)
        && json_u64(attempt, "/txBytes") == Some(8)
        && json_u64(attempt, "/rxBytes") == Some(rx_bytes)
        && match rx_digest {
            Some(digest) => json_string(attempt, "/rxDigest") == Some(digest),
            None => attempt
                .pointer("/rxDigest")
                .is_some_and(serde_json::Value::is_null),
        }
        && json_string(attempt, "/outcome") == Some(outcome)
        && json_string(attempt, "/termiosCleanup") == Some("restored")
        && json_string(attempt, "/controlLinesCleanup") == Some("restored")
        && attempt
            .pointer("/closed")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
}

fn false_effect(value: &serde_json::Value, pointer: &str) -> bool {
    value.pointer(pointer).and_then(serde_json::Value::as_bool) == Some(false)
}

fn has_exact_keys(value: &serde_json::Value, expected: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
    })
}

fn contains_private_material(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => object.iter().any(|(key, value)| {
            matches!(key.as_str(), "privateTtyPath" | "authorization")
                || contains_private_material(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(contains_private_material),
        serde_json::Value::String(value) => {
            value.starts_with("/dev/cu.") || value.starts_with("WORD ")
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            false
        }
    }
}

fn json_string<'a>(value: &'a serde_json::Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(serde_json::Value::as_str)
}

fn json_u64(value: &serde_json::Value, pointer: &str) -> Option<u64> {
    value.pointer(pointer).and_then(serde_json::Value::as_u64)
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn prepare(validated: &ValidatedGate) -> CommandOutput {
    let value = serde_json::json!({
        "schemaVersion": RECEIPT_SCHEMA,
        "status": "prepared",
        "sanitized": true,
        "packet": validated.packet,
        "packetDigest": validated.packet_digest,
        "requiredWord": validated.required_word,
        "attempts": [],
        "effects": Effects::zero(),
    });
    CommandOutput {
        value,
        success: true,
    }
}

#[derive(Serialize)]
// These booleans are explicit non-effects in the persisted receipt contract.
#[allow(clippy::struct_excessive_bools)]
#[serde(rename_all = "camelCase")]
struct Effects {
    query_tx_bytes: usize,
    query_rx_bytes: usize,
    device_configuration_writes: usize,
    audio_routing_changed: bool,
    sound_output: bool,
    clock_changed: bool,
    driver_changed: bool,
    system_extension_changed: bool,
    sip_changed: bool,
    persistent_host_mutation: bool,
    transient_host_file_staging: &'static str,
}

impl Effects {
    const fn zero() -> Self {
        Self {
            query_tx_bytes: 0,
            query_rx_bytes: 0,
            device_configuration_writes: 0,
            audio_routing_changed: false,
            sound_output: false,
            clock_changed: false,
            driver_changed: false,
            system_extension_changed: false,
            sip_changed: false,
            persistent_host_mutation: false,
            transient_host_file_staging: "outside_process_not_observed",
        }
    }

    fn from_attempts(attempts: &[SanitizedAttemptReceipt]) -> Self {
        Self {
            query_tx_bytes: attempts.iter().map(|attempt| attempt.tx_bytes).sum(),
            query_rx_bytes: attempts.iter().map(|attempt| attempt.rx_bytes).sum(),
            ..Self::zero()
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeReceiptBody {
    schema_version: &'static str,
    status: &'static str,
    sanitized: bool,
    packet: GatePacket,
    packet_digest: String,
    completed_at_unix_seconds: u64,
    failure_code: Option<&'static str>,
    selected_baud: Option<u32>,
    observed_device_id: Option<u8>,
    valid_response_count: usize,
    rx_digests: Vec<Sha256Digest>,
    attempts: Vec<SanitizedAttemptReceipt>,
    effects: Effects,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeReceipt {
    #[serde(flatten)]
    body: NativeReceiptBody,
    receipt_body_digest: String,
}

fn receipt_output(
    packet: GatePacket,
    packet_digest: String,
    attempts: Vec<SanitizedAttemptReceipt>,
    failure_code: Option<&'static str>,
    selected_baud: Option<u32>,
    observed_device_id: Option<u8>,
    valid_response_count: usize,
) -> Result<CommandOutput, SanitizedFailure> {
    let rx_digests = attempts
        .iter()
        .filter_map(|attempt| attempt.rx_digest)
        .collect();
    let effects = Effects::from_attempts(&attempts);
    let success = failure_code.is_none();
    let body = NativeReceiptBody {
        schema_version: RECEIPT_SCHEMA,
        status: if success { "succeeded" } else { "failed" },
        sanitized: true,
        packet,
        packet_digest,
        completed_at_unix_seconds: unix_now()?,
        failure_code,
        selected_baud,
        observed_device_id,
        valid_response_count,
        rx_digests,
        attempts,
        effects,
    };
    let receipt_body_digest = serialized_digest(&body)?;
    let receipt = NativeReceipt {
        body,
        receipt_body_digest,
    };
    let value = serde_json::to_value(receipt)
        .map_err(|_| SanitizedFailure::new("receipt_encoding_failed"))?;
    Ok(CommandOutput { value, success })
}

fn execute_live(validated: ValidatedGate) -> Result<CommandOutput, SanitizedFailure> {
    let ValidatedGate {
        binding,
        packet,
        packet_digest,
        required_word: _,
    } = validated;
    let expected = DeviceId::new(packet.expected_device_id)
        .map_err(|_| SanitizedFailure::new("invalid_device_id"))?;
    let mut transport = DarwinSearchTransport::new(binding);
    let outcome = execute_search(&mut transport, expected);
    let attempts = transport.take_receipts();
    match outcome {
        Ok(SearchOutcome::Identified(identity)) => {
            let selected_baud = attempts.last().map(|attempt| attempt.baud);
            debug_assert_eq!(identity.device(), expected);
            receipt_output(
                packet,
                packet_digest,
                attempts,
                None,
                selected_baud,
                Some(identity.device().get()),
                1,
            )
        }
        Ok(SearchOutcome::Exhausted) => receipt_output(
            packet,
            packet_digest,
            attempts,
            Some("search_exhausted"),
            None,
            None,
            0,
        ),
        Err(error) => receipt_output(
            packet,
            packet_digest,
            attempts,
            Some(search_error_code(&error)),
            None,
            search_error_device(&error),
            0,
        ),
    }
}

fn search_error_code(error: &SearchExecutionError<DarwinCarrierError>) -> &'static str {
    match error {
        SearchExecutionError::PartialTimeout { .. } => "partial_timeout",
        SearchExecutionError::Validation { .. } => "identity_validation_failed",
        SearchExecutionError::Transport { source, .. } => carrier_error_code(source),
    }
}

fn search_error_device(error: &SearchExecutionError<DarwinCarrierError>) -> Option<u8> {
    match error {
        SearchExecutionError::Validation {
            source: DiscoveryError::UnexpectedDevice { actual, .. },
            ..
        } => Some(*actual),
        SearchExecutionError::Transport { .. }
        | SearchExecutionError::PartialTimeout { .. }
        | SearchExecutionError::Validation { .. } => None,
    }
}

fn carrier_error_code(error: &DarwinCarrierError) -> &'static str {
    match error {
        DarwinCarrierError::Binding(_) => "binding_rejected",
        DarwinCarrierError::Deadline { .. } => "attempt_deadline_exceeded",
        DarwinCarrierError::System { .. } => "carrier_system_error",
        DarwinCarrierError::ShortWrite { .. } => "short_write",
        DarwinCarrierError::PreexistingInput { .. } => "preexisting_input",
        DarwinCarrierError::Overflow { .. } => "response_overflow",
        DarwinCarrierError::EndOfFile { .. } => "unexpected_eof",
        DarwinCarrierError::ReadInvariant(_) => "read_invariant_failed",
        DarwinCarrierError::Cleanup { .. } => "cleanup_failed",
    }
}

struct SystemPacer {
    start: Instant,
}

impl SystemPacer {
    fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl RepeatPacer for SystemPacer {
    type Error = Infallible;

    fn elapsed(&mut self) -> Duration {
        self.start.elapsed()
    }

    fn wait(&mut self, minimum: Duration) -> Result<(), Self::Error> {
        thread::sleep(minimum);
        Ok(())
    }
}

fn execute_repeat(validated: ValidatedGate) -> Result<CommandOutput, SanitizedFailure> {
    let ValidatedGate {
        binding,
        packet,
        packet_digest,
        required_word: _,
    } = validated;
    let expected = DeviceId::new(packet.expected_device_id)
        .map_err(|_| SanitizedFailure::new("invalid_device_id"))?;
    let successful_baud = packet
        .prior_search
        .as_ref()
        .ok_or_else(|| SanitizedFailure::new("missing_prior_search"))?
        .successful_baud;
    let repeat_binding = RepeatSearchBinding::from_successful_baud(expected, successful_baud)
        .map_err(|_| SanitizedFailure::new("unsupported_repeat_baud"))?;
    let mut transport = DarwinSearchTransport::new(binding);
    let mut pacer = SystemPacer::new();
    let outcome = execute_search_repeat(&mut transport, &mut pacer, repeat_binding);
    let attempts = transport.take_receipts();
    match outcome {
        Ok(repeated) => receipt_output(
            packet,
            packet_digest,
            attempts,
            None,
            Some(repeated.baud()),
            Some(repeated.device().get()),
            repeated.valid_response_count(),
        ),
        Err(error) => {
            let valid_response_count = repeat_valid_response_count(&error);
            let observed_device_id = repeat_error_device(&error)
                .or_else(|| (valid_response_count != 0).then_some(expected.get()));
            receipt_output(
                packet,
                packet_digest,
                attempts,
                Some(repeat_error_code(&error)),
                Some(successful_baud),
                observed_device_id,
                valid_response_count,
            )
        }
    }
}

fn repeat_error_device(error: &RepeatSearchError<DarwinCarrierError, Infallible>) -> Option<u8> {
    match error {
        RepeatSearchError::Validation {
            source: DiscoveryError::UnexpectedDevice { actual, .. },
            ..
        } => Some(*actual),
        RepeatSearchError::Pacing { .. }
        | RepeatSearchError::BudgetExceeded { .. }
        | RepeatSearchError::Transport { .. }
        | RepeatSearchError::Timeout { .. }
        | RepeatSearchError::Validation { .. } => None,
    }
}

fn repeat_valid_response_count(error: &RepeatSearchError<DarwinCarrierError, Infallible>) -> usize {
    let trial = match error {
        RepeatSearchError::Pacing { trial, .. }
        | RepeatSearchError::BudgetExceeded { trial }
        | RepeatSearchError::Transport { trial, .. }
        | RepeatSearchError::Timeout { trial, .. }
        | RepeatSearchError::Validation { trial, .. } => *trial,
    };
    trial.saturating_sub(1)
}

fn unix_now() -> Result<u64, SanitizedFailure> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| SanitizedFailure::new("system_time_invalid"))
}

fn repeat_error_code(error: &RepeatSearchError<DarwinCarrierError, Infallible>) -> &'static str {
    match error {
        RepeatSearchError::Pacing { source, .. } => match *source {},
        RepeatSearchError::BudgetExceeded { .. } => "repeat_budget_exceeded",
        RepeatSearchError::Transport { source, .. } => carrier_error_code(source),
        RepeatSearchError::Timeout { received: 0, .. } => "repeat_empty_timeout",
        RepeatSearchError::Timeout { .. } => "repeat_partial_timeout",
        RepeatSearchError::Validation { .. } => "repeat_identity_validation_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYNTHETIC_PATH: &str = "/dev/cu.usbserial-SYNTHETIC-PRIVATE";

    fn synthetic_runtime() -> RuntimeBinding {
        RuntimeBinding {
            role: TARGET_HOST_ROLE.to_owned(),
            hardware_model: "Mac16,10".to_owned(),
            os_build: "25G220".to_owned(),
            host_identity_digest: digest_bytes(b"synthetic-host"),
            boot_digest: digest_bytes(b"synthetic-boot"),
            executable_digest: digest_bytes(b"synthetic-executable"),
        }
    }

    fn synthetic_envelope(action: GateAction) -> GateEnvelope {
        let now = unix_now().unwrap();
        GateEnvelope {
            schema_version: ENVELOPE_SCHEMA.to_owned(),
            action,
            private_tty_path: PathBuf::from(SYNTHETIC_PATH),
            binding_digest: digest_bytes(SYNTHETIC_PATH.as_bytes()),
            expected_host: ExpectedHost {
                role: TARGET_HOST_ROLE.to_owned(),
                hardware_model: "Mac16,10".to_owned(),
                os_build: "25G220".to_owned(),
            },
            expected_device_id: SAFE_PROFILE_EXPECTED_DEVICE_ID,
            physical: PhysicalDeclaration {
                powered: true,
                edition: "standard-non-le".to_owned(),
                firmware_version: "1.17".to_owned(),
                port_mode: "RS-232".to_owned(),
                rear_rs232_connected: true,
                adapter_rs232_electrical_verified: true,
                speakers_disconnected: true,
            },
            evidence: EvidenceDeclaration {
                passive_receipt_digest: digest_bytes(b"synthetic-passive-receipt"),
                passive_captured_at_unix_seconds: now,
                adapter_electrical_evidence_digest: digest_bytes(b"synthetic-rs232-evidence"),
                profile_digest: SAFE_PROFILE_DIGEST.to_owned(),
                source_revision: "a".repeat(40),
                source_tree: "b".repeat(40),
            },
            issued_at_unix_seconds: now,
            expires_at_unix_seconds: now + 600,
            prior_search: None,
            authorization: None,
        }
    }

    fn failure_code(result: &Result<ValidatedGate, SanitizedFailure>) -> &'static str {
        match result {
            Ok(_) => panic!("expected gate rejection"),
            Err(failure) => failure.code,
        }
    }

    #[test]
    fn digest_parser_accepts_only_canonical_receipt_form() {
        assert!(canonical_digest(QUERY_DIGEST).is_ok());
        assert!(canonical_digest(&QUERY_DIGEST.to_uppercase()).is_err());
        assert!(canonical_digest("sha256/00").is_err());
    }

    #[test]
    fn physical_gate_requires_all_operator_reported_preconditions() {
        let mut physical = PhysicalDeclaration {
            powered: true,
            edition: "standard-non-le".to_owned(),
            firmware_version: "1.17".to_owned(),
            port_mode: "RS-232".to_owned(),
            rear_rs232_connected: true,
            adapter_rs232_electrical_verified: true,
            speakers_disconnected: true,
        };
        assert!(validate_physical(&physical).is_ok());
        physical.powered = false;
        assert!(validate_physical(&physical).is_err());
    }

    #[test]
    fn live_gate_is_compiled_for_the_exact_safe_profile_and_address() {
        let mut wrong_address = synthetic_envelope(GateAction::Search);
        wrong_address.expected_device_id = 1;
        assert_eq!(
            failure_code(&ValidatedGate::new_with_runtime(
                wrong_address,
                LiveCommand::Prepare,
                synthetic_runtime(),
            )),
            "unsupported_live_profile"
        );

        let mut wrong_profile = synthetic_envelope(GateAction::Search);
        wrong_profile.evidence.profile_digest = digest_bytes(b"different-profile");
        assert_eq!(
            failure_code(&ValidatedGate::new_with_runtime(
                wrong_profile,
                LiveCommand::Prepare,
                synthetic_runtime(),
            )),
            "unsupported_live_profile"
        );
    }

    #[test]
    fn effects_count_query_bytes_without_claiming_configuration_writes() {
        let effects = Effects::zero();
        assert_eq!(effects.query_tx_bytes, 0);
        assert_eq!(effects.device_configuration_writes, 0);
        assert!(!effects.audio_routing_changed);
        assert!(!effects.sound_output);
    }

    #[test]
    fn wrong_device_is_reported_as_wire_observed_and_stops() {
        let error = SearchExecutionError::<DarwinCarrierError>::Validation {
            attempt: dcx_core::discovery::DiscoveryAttemptKind::Primary,
            source: DiscoveryError::UnexpectedDevice {
                expected: 0,
                actual: 7,
            },
        };
        assert_eq!(search_error_code(&error), "identity_validation_failed");
        assert_eq!(search_error_device(&error), Some(7));
    }

    #[test]
    fn prepare_and_live_share_digest_without_serializing_private_input() {
        let envelope = synthetic_envelope(GateAction::Search);
        let prepared = ValidatedGate::new_with_runtime(
            envelope.clone(),
            LiveCommand::Prepare,
            synthetic_runtime(),
        )
        .unwrap();
        let packet_digest = prepared.packet_digest.clone();
        let word = prepared.required_word.clone();
        let packet_json = serde_json::to_string(&prepared.packet).unwrap();
        assert!(!packet_json.contains(SYNTHETIC_PATH));
        assert!(!packet_json.contains(&word));

        let mut authorized = envelope;
        authorized.authorization = Some(word.clone());
        let live =
            ValidatedGate::new_with_runtime(authorized, LiveCommand::Live, synthetic_runtime())
                .unwrap();
        assert_eq!(live.packet_digest, packet_digest);
        assert_eq!(live.required_word, word);

        let output = receipt_output(
            live.packet,
            live.packet_digest,
            Vec::new(),
            Some("search_exhausted"),
            None,
            None,
            0,
        )
        .unwrap();
        let receipt_json = serde_json::to_string(&output.value).unwrap();
        assert!(!receipt_json.contains(SYNTHETIC_PATH));
        assert!(!receipt_json.contains(&live.required_word));
        assert!(receipt_json.contains("receiptBodyDigest"));
    }

    #[test]
    fn gate_rejects_passive_evidence_after_issue_and_wrong_word() {
        let mut envelope = synthetic_envelope(GateAction::Search);
        envelope.evidence.passive_captured_at_unix_seconds = envelope.issued_at_unix_seconds + 1;
        assert_eq!(
            failure_code(&ValidatedGate::new_with_runtime(
                envelope,
                LiveCommand::Prepare,
                synthetic_runtime(),
            )),
            "passive_evidence_stale"
        );

        let mut envelope = synthetic_envelope(GateAction::Search);
        envelope.authorization = Some("WORD DCX_QUERY_V1 sha256/not-a-token".to_owned());
        assert_eq!(
            failure_code(&ValidatedGate::new_with_runtime(
                envelope,
                LiveCommand::Live,
                synthetic_runtime(),
            )),
            "authorization_mismatch"
        );
    }

    #[test]
    // Keep the complete strict prior receipt fixture adjacent to its drift assertions.
    #[allow(clippy::too_many_lines)]
    fn repeat_rejects_prior_proof_and_evidence_drift() {
        let search = ValidatedGate::new_with_runtime(
            synthetic_envelope(GateAction::Search),
            LiveCommand::Prepare,
            synthetic_runtime(),
        )
        .unwrap();
        let first_packet = serde_json::to_value(&search.packet).unwrap();
        let first_response_digest = digest_bytes(b"synthetic-first-response");
        let first_receipt_body = serde_json::json!({
            "schemaVersion": RECEIPT_SCHEMA,
            "status": "succeeded",
            "sanitized": true,
            "packet": first_packet,
            "packetDigest": search.packet_digest,
            "completedAtUnixSeconds": unix_now().unwrap(),
            "failureCode": null,
            "selectedBaud": 115_200,
            "observedDeviceId": SAFE_PROFILE_EXPECTED_DEVICE_ID,
            "validResponseCount": 1,
            "rxDigests": [first_response_digest],
            "attempts": [{
                "attempt": "primary",
                "bindingDigest": search.packet.binding_digest,
                "baud": 115_200,
                "deadlineMillis": 500,
                "cleanupReserveMillis": 25,
                "elapsedMicros": 1,
                "txBytes": 8,
                "rxBytes": 26,
                "rxDigest": first_response_digest,
                "outcome": "complete",
                "termiosCleanup": "restored",
                "controlLinesCleanup": "restored",
                "closed": true
            }],
            "effects": {
                "queryTxBytes": 8,
                "queryRxBytes": 26,
                "deviceConfigurationWrites": 0,
                "audioRoutingChanged": false,
                "soundOutput": false,
                "clockChanged": false,
                "driverChanged": false,
                "systemExtensionChanged": false,
                "sipChanged": false,
                "persistentHostMutation": false,
                "transientHostFileStaging": "outside_process_not_observed"
            }
        });
        let mut repeat = synthetic_envelope(GateAction::Repeat);
        repeat.prior_search = Some(PriorSearch {
            first_packet: serde_json::to_value(&search.packet).unwrap(),
            first_packet_digest: search.packet_digest,
            first_receipt_body: first_receipt_body.clone(),
            first_receipt_body_digest: canonical_json_digest(&first_receipt_body).unwrap(),
            first_response_digest,
            binding_digest: search.packet.binding_digest.clone(),
            expected_device_id: search.packet.expected_device_id,
            successful_baud: 115_200,
            first_host_identity_digest: search.packet.host.host_identity_digest.clone(),
            first_boot_digest: search.packet.host.boot_digest.clone(),
            first_executable_digest: search.packet.host.executable_digest.clone(),
            first_profile_digest: search
                .packet
                .operator_declared_evidence
                .profile_digest
                .clone(),
            first_passive_receipt_digest: search
                .packet
                .operator_declared_evidence
                .passive_receipt_digest
                .clone(),
            first_adapter_electrical_evidence_digest: search
                .packet
                .operator_declared_evidence
                .adapter_electrical_evidence_digest
                .clone(),
            first_source_revision: search
                .packet
                .operator_declared_evidence
                .source_revision
                .clone(),
            first_source_tree: search.packet.operator_declared_evidence.source_tree.clone(),
            first_physical_digest: search.packet.operator_declared_physical_digest,
        });

        let mut passive_drift = repeat.clone();
        passive_drift.evidence.passive_captured_at_unix_seconds -= 1;
        assert_eq!(
            failure_code(&ValidatedGate::new_with_runtime(
                passive_drift,
                LiveCommand::Prepare,
                synthetic_runtime(),
            )),
            "prior_search_evidence_drift"
        );

        let mut impossible_fallback = repeat.clone();
        let prior = impossible_fallback.prior_search.as_mut().unwrap();
        prior.successful_baud = 38_400;
        prior.first_receipt_body["selectedBaud"] = serde_json::json!(38_400);
        prior.first_receipt_body["attempts"][0]["attempt"] = serde_json::json!("single_fallback");
        prior.first_receipt_body["attempts"][0]["baud"] = serde_json::json!(38_400);
        prior.first_receipt_body_digest = canonical_json_digest(&prior.first_receipt_body).unwrap();
        assert_eq!(
            failure_code(&ValidatedGate::new_with_runtime(
                impossible_fallback,
                LiveCommand::Prepare,
                synthetic_runtime(),
            )),
            "invalid_prior_search_proof"
        );

        let mut rebooted = synthetic_runtime();
        rebooted.boot_digest = digest_bytes(b"different-boot");
        assert_eq!(
            failure_code(&ValidatedGate::new_with_runtime(
                repeat,
                LiveCommand::Prepare,
                rebooted,
            )),
            "prior_search_evidence_drift"
        );
    }

    #[test]
    fn strict_envelope_rejects_unknown_fields_and_oversized_input() {
        let mut value = serde_json::to_value(synthetic_envelope(GateAction::Search)).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("unexpected".to_owned(), serde_json::Value::Bool(true));
        assert!(serde_json::from_value::<GateEnvelope>(value).is_err());

        let oversized = vec![0_u8; MAX_STDIN_BYTES + 1];
        let failure = read_bounded(io::Cursor::new(oversized)).unwrap_err();
        assert_eq!(failure.code, "stdin_too_large");
    }
}
