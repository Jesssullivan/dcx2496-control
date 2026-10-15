//! Thin macOS runner for one Search identity and ten read-only meter queries.

use std::{
    convert::Infallible,
    error::Error,
    fmt,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use dcx_core::{discovery::FALLBACK_BAUD, meter::MeterReadingV1, protocol::DeviceId};
use dcx_darwin_tty::{DarwinSearchSession, PrivateTtyBinding};
use dcx_transport::{
    RepeatPacer,
    meter::{
        LIVE_METER_BUDGET, LIVE_METER_GAP, LIVE_METER_IDENTITY_ATTEMPT_LIMIT,
        LIVE_METER_REQUEST_COUNT, execute_live_meter,
    },
};

const RESULT_SCHEMA: &str = "dcx.live-meter-result/v1";

struct SystemPacer {
    start: Instant,
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

#[derive(Debug)]
struct LiveMeterFailed(String);

impl fmt::Display for LiveMeterFailed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "DCX live meter failed: {}", self.0)
    }
}

impl Error for LiveMeterFailed {}

pub fn run(tty: PathBuf, expected_device: u8) -> Result<(), Box<dyn Error>> {
    let expected = DeviceId::new(expected_device)?;
    let binding = PrivateTtyBinding::new(tty)?;
    let session = DarwinSearchSession::open_known_38400(binding)?;
    let mut pacer = SystemPacer {
        start: Instant::now(),
    };
    match execute_live_meter(session, &mut pacer, expected) {
        Ok(capture) => {
            emit(&result(
                "complete",
                expected_device,
                capture.readings(),
                None,
            ))?;
            Ok(())
        }
        Err(error) => {
            let detail = error.to_string();
            emit(&result(
                "failed",
                expected_device,
                error.readings(),
                Some(&detail),
            ))?;
            Err(Box::new(LiveMeterFailed(detail)))
        }
    }
}

fn result(
    status: &'static str,
    expected_device: u8,
    readings: &[MeterReadingV1],
    error: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": RESULT_SCHEMA,
        "status": status,
        "mode": "read_only_meter",
        "remoteModeWrites": 0,
        "parameterWrites": 0,
        "selectedBaud": FALLBACK_BAUD,
        "expectedDevice": expected_device,
        "identityAttemptLimit": LIVE_METER_IDENTITY_ATTEMPT_LIMIT,
        "requestedReadings": LIVE_METER_REQUEST_COUNT,
        "readingGapMillis": LIVE_METER_GAP.as_millis(),
        "sessionBudgetSeconds": LIVE_METER_BUDGET.as_secs(),
        "validReadings": readings.len(),
        "peakLevel": readings.iter().map(MeterReadingV1::peak_level).max(),
        "readings": readings,
        "error": error,
    })
}

fn emit(value: &serde_json::Value) -> Result<(), serde_json::Error> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
