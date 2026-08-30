//! Thin macOS runner for one known-38400 Search and nine repeats.

use std::{error::Error, fmt, path::PathBuf, thread, time::Instant};

use dcx_core::{discovery::FALLBACK_BAUD, protocol::DeviceId};
use dcx_darwin_tty::{
    DarwinSearchSession, PrivateTtyBinding, SanitizedAttemptReceipt, SanitizedSessionReceipt,
};
use dcx_transport::{
    Known38400SearchOutcome, REPEAT_SEARCH_BUDGET, REPEAT_SEARCH_COUNT, REPEAT_SEARCH_GAP,
    SEARCH_ATTEMPT_TIMEOUT, SEARCH_RESPONSE_LIMIT, execute_known_38400_search,
};

const RESULT_SCHEMA: &str = "dcx.live-search-result/v2";
const REQUIRED_VALID_RESPONSES: usize = REPEAT_SEARCH_COUNT + 1;

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

pub fn run(tty: PathBuf, expected_device: u8) -> Result<(), Box<dyn Error>> {
    let expected = DeviceId::new(expected_device)?;
    let binding = PrivateTtyBinding::new(tty)?;
    let mut transport = DarwinSearchSession::open_known_38400(binding)?;
    let started = Instant::now();
    let mut valid_responses = 0_usize;
    let mut empty_timeouts = 0_usize;

    for attempt_index in 0..REQUIRED_VALID_RESPONSES {
        if attempt_index != 0 {
            thread::sleep(REPEAT_SEARCH_GAP);
        }
        if started.elapsed() > REPEAT_SEARCH_BUDGET.saturating_sub(SEARCH_ATTEMPT_TIMEOUT) {
            let attempts = transport.take_receipts();
            let session = finish_session(transport, &attempts, expected_device)?;
            return fail(
                "qualification_budget",
                "ten-Search qualification exceeded its bounded session budget",
                &qualification_result(
                    "failed",
                    expected_device,
                    valid_responses,
                    empty_timeouts,
                    &attempts,
                    &session,
                ),
            );
        }

        match execute_known_38400_search(&mut transport, expected) {
            Ok(Known38400SearchOutcome::Identified(_)) => valid_responses += 1,
            Ok(Known38400SearchOutcome::TimedOut) => empty_timeouts += 1,
            Err(error) => {
                let detail = error.to_string();
                let attempts = transport.take_receipts();
                let session = finish_session(transport, &attempts, expected_device)?;
                let mut result = qualification_result(
                    "failed",
                    expected_device,
                    valid_responses,
                    empty_timeouts,
                    &attempts,
                    &session,
                );
                result["phase"] = serde_json::json!("qualification_search");
                result["error"] = serde_json::json!(detail);
                return fail("qualification_search", &detail, &result);
            }
        }
    }

    let attempts = transport.take_receipts();
    let session = finish_session(transport, &attempts, expected_device)?;
    if valid_responses == REQUIRED_VALID_RESPONSES {
        emit(&qualification_result(
            "identified",
            expected_device,
            valid_responses,
            empty_timeouts,
            &attempts,
            &session,
        ))?;
        Ok(())
    } else {
        let detail = format!(
            "{valid_responses} of {REQUIRED_VALID_RESPONSES} required Search identities were valid"
        );
        fail(
            "qualification_incomplete",
            &detail,
            &qualification_result(
                "not_repeatable",
                expected_device,
                valid_responses,
                empty_timeouts,
                &attempts,
                &session,
            ),
        )
    }
}

fn qualification_result(
    status: &'static str,
    expected_device: u8,
    valid_responses: usize,
    empty_timeouts: usize,
    attempts: &[SanitizedAttemptReceipt],
    session: &SanitizedSessionReceipt,
) -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": RESULT_SCHEMA,
        "status": status,
        "searchBinding": "known_38400",
        "protocolIdentity": (valid_responses != 0).then(|| serde_json::json!({
            "manufacturer": "Behringer",
            "model": "DCX2496",
            "deviceAddress": expected_device,
            "responseBytes": SEARCH_RESPONSE_LIMIT,
        })),
        "selectedBaud": FALLBACK_BAUD,
        "validResponses": valid_responses,
        "emptyTimeouts": empty_timeouts,
        "requiredValidResponses": REQUIRED_VALID_RESPONSES,
        "carrierAttempts": attempts.len(),
        "attempts": attempts,
        "session": session,
    })
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
