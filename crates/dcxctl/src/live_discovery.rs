//! Thin macOS runner for one known-38400 Search and nine repeats.

use std::{
    convert::Infallible,
    error::Error,
    fmt,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use dcx_core::protocol::DeviceId;
use dcx_darwin_tty::{
    DarwinSearchSession, PrivateTtyBinding, SanitizedAttemptReceipt, SanitizedSessionReceipt,
};
use dcx_transport::{
    Known38400SearchOutcome, REPEAT_SEARCH_COUNT, RepeatPacer, RepeatSearchBinding,
    SEARCH_RESPONSE_LIMIT, execute_known_38400_search, execute_search_repeat,
};

const RESULT_SCHEMA: &str = "dcx.live-search-result/v1";

#[derive(Debug)]
struct LiveSearchFailed {
    phase: &'static str,
    detail: String,
}

impl fmt::Display for LiveSearchFailed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "DCX live search failed during {}: {}",
            self.phase, self.detail
        )
    }
}

impl Error for LiveSearchFailed {}

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

pub fn run(tty: PathBuf, expected_device: u8) -> Result<(), Box<dyn Error>> {
    let expected = DeviceId::new(expected_device)?;
    let binding = PrivateTtyBinding::new(tty)?;
    let mut transport = DarwinSearchSession::open_known_38400(binding)?;

    let first = match execute_known_38400_search(&mut transport, expected) {
        Ok(Known38400SearchOutcome::Identified(identity)) => identity,
        Ok(Known38400SearchOutcome::TimedOut) => {
            let attempts = transport.take_receipts();
            let session = finish_session(transport, &attempts, expected_device)?;
            return fail(
                "initial_search",
                "no response at the MVP 38400 baud binding",
                &serde_json::json!({
                    "schemaVersion": RESULT_SCHEMA,
                    "status": "not_found",
                    "searchBinding": "known_38400",
                    "expectedDevice": expected_device,
                    "attempts": attempts,
                    "session": session,
                }),
            );
        }
        Err(error) => {
            let detail = error.to_string();
            let attempts = transport.take_receipts();
            let session = finish_session(transport, &attempts, expected_device)?;
            return fail(
                "initial_search",
                &detail,
                &serde_json::json!({
                    "schemaVersion": RESULT_SCHEMA,
                    "status": "failed",
                    "phase": "initial_search",
                    "searchBinding": "known_38400",
                    "expectedDevice": expected_device,
                    "error": &detail,
                    "attempts": attempts,
                    "session": session,
                }),
            );
        }
    };

    let repeat_binding = RepeatSearchBinding::from_identified(&first);
    let selected_baud = repeat_binding.successful_baud();
    let mut pacer = SystemPacer::new();

    match execute_search_repeat(&mut transport, &mut pacer, repeat_binding) {
        Ok(repeated) => {
            let attempts = transport.take_receipts();
            let attempt_count = attempts.len();
            let session = finish_session(transport, &attempts, expected_device)?;
            emit(&serde_json::json!({
                "schemaVersion": RESULT_SCHEMA,
                "status": "identified",
                "searchBinding": "known_38400",
                "protocolIdentity": {
                    "manufacturer": "Behringer",
                    "model": "DCX2496",
                    "deviceAddress": repeated.device().get(),
                    "responseBytes": SEARCH_RESPONSE_LIMIT,
                },
                "selectedBaud": repeated.baud(),
                "initialValidResponses": 1,
                "repeatValidResponses": repeated.valid_response_count(),
                "validResponses": 1 + repeated.valid_response_count(),
                "requiredRepeatResponses": REPEAT_SEARCH_COUNT,
                "carrierAttempts": attempt_count,
                "attempts": attempts,
                "session": session,
            }))?;
            Ok(())
        }
        Err(error) => {
            let detail = error.to_string();
            let attempts = transport.take_receipts();
            let session = finish_session(transport, &attempts, expected_device)?;
            fail(
                "repeat_search",
                &detail,
                &serde_json::json!({
                    "schemaVersion": RESULT_SCHEMA,
                    "status": "failed",
                    "phase": "repeat_search",
                    "searchBinding": "known_38400",
                    "protocolIdentity": {
                        "manufacturer": "Behringer",
                        "model": "DCX2496",
                        "deviceAddress": first.device().get(),
                        "responseBytes": SEARCH_RESPONSE_LIMIT,
                    },
                    "selectedBaud": selected_baud,
                    "initialValidResponses": 1,
                    "requiredRepeatResponses": REPEAT_SEARCH_COUNT,
                    "error": &detail,
                    "attempts": attempts,
                    "session": session,
                }),
            )
        }
    }
}

fn finish_session(
    transport: DarwinSearchSession,
    attempts: &[SanitizedAttemptReceipt],
    expected_device: u8,
) -> Result<SanitizedSessionReceipt, Box<dyn Error>> {
    match transport.finish() {
        Ok(receipt) => Ok(receipt),
        Err(error) => {
            let detail = error.to_string();
            emit(&serde_json::json!({
                "schemaVersion": RESULT_SCHEMA,
                "status": "failed",
                "phase": "session_finish",
                "searchBinding": "known_38400",
                "expectedDevice": expected_device,
                "error": &detail,
                "attempts": attempts,
            }))?;
            Err(Box::new(LiveSearchFailed {
                phase: "session_finish",
                detail,
            }))
        }
    }
}

fn emit(value: &serde_json::Value) -> Result<(), serde_json::Error> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn fail(
    phase: &'static str,
    detail: &str,
    value: &serde_json::Value,
) -> Result<(), Box<dyn Error>> {
    emit(value)?;
    Err(Box::new(LiveSearchFailed {
        phase,
        detail: detail.to_owned(),
    }))
}
