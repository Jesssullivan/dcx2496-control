//! Thin macOS runner for one DCX Search followed by nine pinned-baud repeats.

use std::{
    convert::Infallible,
    error::Error,
    fmt,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use dcx_core::{
    discovery::{DiscoveryAttemptKind, FALLBACK_BAUD, PRIMARY_BAUD},
    protocol::DeviceId,
};
use dcx_darwin_tty::{DarwinSearchTransport, PrivateTtyBinding};
use dcx_transport::{
    REPEAT_SEARCH_COUNT, RepeatPacer, RepeatSearchBinding, SEARCH_RESPONSE_LIMIT, SearchOutcome,
    execute_search, execute_search_repeat,
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
    let mut transport = DarwinSearchTransport::new(binding);

    let first = match execute_search(&mut transport, expected) {
        Ok(SearchOutcome::Identified(identity)) => identity,
        Ok(SearchOutcome::Exhausted) => {
            let attempts = transport.take_receipts();
            return fail(
                "initial_search",
                "no response at 115200 or 38400 baud",
                serde_json::json!({
                    "schemaVersion": RESULT_SCHEMA,
                    "status": "not_found",
                    "expectedDevice": expected_device,
                    "attempts": attempts,
                }),
            );
        }
        Err(error) => {
            let detail = error.to_string();
            let attempts = transport.take_receipts();
            return fail(
                "initial_search",
                &detail,
                serde_json::json!({
                    "schemaVersion": RESULT_SCHEMA,
                    "status": "failed",
                    "phase": "initial_search",
                    "expectedDevice": expected_device,
                    "error": &detail,
                    "attempts": attempts,
                }),
            );
        }
    };

    let selected_baud = match first.attempt() {
        DiscoveryAttemptKind::Primary => PRIMARY_BAUD,
        DiscoveryAttemptKind::SingleFallback => FALLBACK_BAUD,
    };
    let repeat_binding = RepeatSearchBinding::from_identified(&first);
    let mut pacer = SystemPacer::new();

    match execute_search_repeat(&mut transport, &mut pacer, repeat_binding) {
        Ok(repeated) => {
            let attempts = transport.take_receipts();
            let attempt_count = attempts.len();
            emit(&serde_json::json!({
                "schemaVersion": RESULT_SCHEMA,
                "status": "identified",
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
            }))?;
            Ok(())
        }
        Err(error) => {
            let detail = error.to_string();
            let attempts = transport.take_receipts();
            fail(
                "repeat_search",
                &detail,
                serde_json::json!({
                    "schemaVersion": RESULT_SCHEMA,
                    "status": "failed",
                    "phase": "repeat_search",
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
                }),
            )
        }
    }
}

fn emit(value: &serde_json::Value) -> Result<(), serde_json::Error> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn fail(phase: &'static str, detail: &str, value: serde_json::Value) -> Result<(), Box<dyn Error>> {
    emit(&value)?;
    Err(Box::new(LiveSearchFailed {
        phase,
        detail: detail.to_owned(),
    }))
}
