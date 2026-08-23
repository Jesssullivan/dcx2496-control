//! Strict bounded import of Room EQ Wizard Generic EQ text into DCX-native steps.

use std::collections::BTreeSet;

use serde::Serialize;
use thiserror::Error;

use crate::profile::FilterKind;

/// Version of the REW import report.
pub const REW_REPORT_SCHEMA_VERSION: u16 = 1;
/// Maximum accepted REW file size.
pub const MAX_REW_BYTES: usize = 64 * 1024;
/// Maximum logical lines scanned in one REW file.
pub const MAX_REW_LINES: usize = 256;
/// Maximum bytes accepted in one logical line.
pub const MAX_REW_LINE_BYTES: usize = 256;
/// Maximum filter declarations, including disabled filters.
pub const MAX_REW_FILTER_LINES: usize = 99;

/// One requested REW filter before device quantization.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RequestedFilter {
    /// REW filter number.
    pub index: u8,
    /// Filter response type.
    pub kind: FilterKind,
    /// Requested center/corner frequency.
    pub frequency_hz: f64,
    /// Requested gain. The MVP accepts cuts and zero only.
    pub gain_db: f64,
    /// Requested Q.
    pub q: f64,
}

/// One filter expressed in inferred DCX parameter codes and decoded values.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QuantizedFilter {
    /// Original request.
    pub requested: RequestedFilter,
    /// Inferred DCX frequency code, 0 through 320.
    pub frequency_code: u16,
    /// Frequency represented by the inferred code.
    pub encoded_frequency_hz: f64,
    /// Encoded minus requested frequency.
    pub frequency_delta_hz: f64,
    /// Inferred gain code, 0 through 150 (-15 through 0 dB in this MVP).
    pub gain_code: u16,
    /// Gain represented by the inferred code.
    pub encoded_gain_db: f64,
    /// Encoded minus requested gain.
    pub gain_delta_db: f64,
    /// Inferred Q code, 0 through 40.
    pub q_code: u8,
    /// Q represented by the inferred code.
    pub encoded_q: f64,
    /// Encoded minus requested Q.
    pub q_delta: f64,
}

/// Auditable result of one REW text import.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RewImportReportV1 {
    /// Must equal [`REW_REPORT_SCHEMA_VERSION`].
    pub schema_version: u16,
    /// One explicitly named DCX output, 1 through 6.
    pub target_output: u8,
    /// Always true for the current MVP; no boost mode exists.
    pub cut_only: bool,
    /// Disabled REW filters omitted from the result.
    pub skipped_disabled: usize,
    /// Quantized filters in source order.
    pub filters: Vec<QuantizedFilter>,
}

#[derive(Debug)]
struct ParsedFilter {
    enabled: bool,
    requested: RequestedFilter,
}

/// Parse a REW Generic EQ text export and quantize it conservatively.
///
/// The format, file/line/token counts, numeric ranges, and target are all hard
/// bounded. Positive gain is not representable in this MVP API.
///
/// # Errors
///
/// Returns [`RewParseError`] for malformed or unsupported input, enabled boosts,
/// device-range violations, missing/duplicate headers, or any size bound.
pub fn import_rew(text: &str, target_output: u8) -> Result<RewImportReportV1, RewParseError> {
    if text.len() > MAX_REW_BYTES {
        return Err(RewParseError::InputTooLarge(text.len()));
    }
    if !(1..=6).contains(&target_output) {
        return Err(RewParseError::InvalidTargetOutput(target_output));
    }

    let mut filters = Vec::new();
    let mut seen = BTreeSet::new();
    let mut skipped_disabled = 0;
    let mut filter_lines = 0;
    let mut saw_header = false;
    for (line_index, raw_line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        if line_number > MAX_REW_LINES {
            return Err(RewParseError::TooManyLines(line_number));
        }
        if raw_line.len() > MAX_REW_LINE_BYTES {
            return Err(RewParseError::LineTooLong {
                line: line_number,
                bytes: raw_line.len(),
            });
        }
        let without_bom = if line_number == 1 {
            raw_line.strip_prefix('\u{feff}').unwrap_or(raw_line)
        } else {
            raw_line
        };
        let line = without_bom.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('*') {
            continue;
        }
        if line == "Equaliser: Generic" {
            if saw_header || filter_lines != 0 {
                return Err(RewParseError::DuplicateOrLateHeader(line_number));
            }
            saw_header = true;
            continue;
        }
        if !line.starts_with("Filter ") {
            return Err(RewParseError::UnexpectedLine(line_number));
        }
        if !saw_header {
            return Err(RewParseError::MissingHeader);
        }
        filter_lines += 1;
        if filter_lines > MAX_REW_FILTER_LINES {
            return Err(RewParseError::TooManyFilterLines(filter_lines));
        }

        let parsed = parse_filter_line(line, line_number)?;
        if !seen.insert(parsed.requested.index) {
            return Err(RewParseError::DuplicateFilter(parsed.requested.index));
        }
        validate_requested(&parsed.requested)?;
        if !parsed.enabled {
            skipped_disabled += 1;
            continue;
        }
        if parsed.requested.gain_db > 0.0 {
            return Err(RewParseError::PositiveGain {
                index: parsed.requested.index,
                gain_db: parsed.requested.gain_db,
            });
        }
        filters.push(quantize(parsed.requested)?);
        if filters.len() > 9 {
            return Err(RewParseError::TooManyFilters(filters.len()));
        }
    }
    if !saw_header {
        return Err(RewParseError::MissingHeader);
    }
    if filters.is_empty() {
        return Err(RewParseError::NoEnabledFilters);
    }
    Ok(RewImportReportV1 {
        schema_version: REW_REPORT_SCHEMA_VERSION,
        target_output,
        cut_only: true,
        skipped_disabled,
        filters,
    })
}

fn parse_filter_line(line: &str, line_number: usize) -> Result<ParsedFilter, RewParseError> {
    let tokens: Vec<_> = line.split_whitespace().collect();
    if tokens.len() != 12 {
        return Err(RewParseError::MalformedLine {
            line: line_number,
            reason: "expected exactly `Filter <n>: <ON|OFF> <PK|LS|HS> Fc <Hz> Hz Gain <dB> dB Q <value>`".into(),
        });
    }
    if tokens[0] != "Filter" {
        return Err(RewParseError::MalformedLine {
            line: line_number,
            reason: "filter declaration must start with exact `Filter`".into(),
        });
    }
    let Some(index_text) = tokens[1].strip_suffix(':') else {
        return Err(RewParseError::MalformedLine {
            line: line_number,
            reason: "filter index must end with `:`".into(),
        });
    };
    if index_text.is_empty() || !index_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(RewParseError::MalformedLine {
            line: line_number,
            reason: "invalid filter index".into(),
        });
    }
    let index = index_text
        .parse::<u8>()
        .map_err(|_| RewParseError::MalformedLine {
            line: line_number,
            reason: "invalid filter index".into(),
        })?;
    if !(1..=99).contains(&index) {
        return Err(RewParseError::MalformedLine {
            line: line_number,
            reason: "filter index must be 1 through 99".into(),
        });
    }
    let enabled = if tokens[2].eq_ignore_ascii_case("ON") {
        true
    } else if tokens[2].eq_ignore_ascii_case("OFF") {
        false
    } else {
        return Err(RewParseError::MalformedLine {
            line: line_number,
            reason: "filter state must be ON or OFF".into(),
        });
    };
    let kind = match tokens[3].to_ascii_uppercase().as_str() {
        "PK" => FilterKind::Peak,
        "LS" => FilterKind::LowShelf,
        "HS" => FilterKind::HighShelf,
        other => {
            return Err(RewParseError::UnsupportedFilter {
                line: line_number,
                kind: other.into(),
            });
        }
    };
    if !tokens[4].eq_ignore_ascii_case("Fc")
        || !tokens[6].eq_ignore_ascii_case("Hz")
        || !tokens[7].eq_ignore_ascii_case("Gain")
        || !tokens[9].eq_ignore_ascii_case("dB")
        || !tokens[10].eq_ignore_ascii_case("Q")
    {
        return Err(RewParseError::MalformedLine {
            line: line_number,
            reason: "field labels or units are not in the supported Generic EQ order".into(),
        });
    }
    Ok(ParsedFilter {
        enabled,
        requested: RequestedFilter {
            index,
            kind,
            frequency_hz: parse_number(tokens[5], "Fc", line_number)?,
            gain_db: parse_number(tokens[8], "Gain", line_number)?,
            q: parse_number(tokens[11], "Q", line_number)?,
        },
    })
}

fn parse_number(token: &str, label: &str, line: usize) -> Result<f64, RewParseError> {
    let value = token
        .parse::<f64>()
        .map_err(|_| RewParseError::MalformedLine {
            line,
            reason: format!("invalid number after {label}"),
        })?;
    if !value.is_finite() {
        return Err(RewParseError::MalformedLine {
            line,
            reason: format!("non-finite number after {label}"),
        });
    }
    Ok(value)
}

fn validate_requested(requested: &RequestedFilter) -> Result<(), RewParseError> {
    if !(20.0..=20_000.0).contains(&requested.frequency_hz) {
        return Err(RewParseError::FrequencyOutOfRange {
            index: requested.index,
            value: requested.frequency_hz,
        });
    }
    if !(-15.0..=15.0).contains(&requested.gain_db) {
        return Err(RewParseError::GainOutOfRange {
            index: requested.index,
            value: requested.gain_db,
        });
    }
    if !(0.1..=10.0).contains(&requested.q) {
        return Err(RewParseError::QOutOfRange {
            index: requested.index,
            value: requested.q,
        });
    }
    Ok(())
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn quantize(requested: RequestedFilter) -> Result<QuantizedFilter, RewParseError> {
    validate_requested(&requested)?;
    if requested.gain_db > 0.0 {
        return Err(RewParseError::PositiveGain {
            index: requested.index,
            gain_db: requested.gain_db,
        });
    }

    // These mappings are implementation hypotheses from public behavioral
    // references. Offline tests establish determinism only, not device truth.
    let frequency_code = (32.0 * (requested.frequency_hz / 20.0).log2())
        .round()
        .clamp(0.0, 320.0) as u16;
    let encoded_frequency_hz = 20.0 * 2.0_f64.powf(f64::from(frequency_code) / 32.0);
    let gain_code = ((requested.gain_db + 15.0) * 10.0)
        .round()
        .clamp(0.0, 150.0) as u16;
    let encoded_gain_db = f64::from(gain_code) / 10.0 - 15.0;
    let q_code = (20.0 * (requested.q / 0.1).log10())
        .round()
        .clamp(0.0, 40.0) as u8;
    let encoded_q = 0.1 * 10.0_f64.powf(f64::from(q_code) / 20.0);
    let frequency_delta_hz = encoded_frequency_hz - requested.frequency_hz;
    let gain_delta_db = encoded_gain_db - requested.gain_db;
    let q_delta = encoded_q - requested.q;

    Ok(QuantizedFilter {
        requested,
        frequency_code,
        encoded_frequency_hz,
        frequency_delta_hz,
        gain_code,
        encoded_gain_db,
        gain_delta_db,
        q_code,
        encoded_q,
        q_delta,
    })
}

/// REW parsing or DCX-compatibility failure.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum RewParseError {
    /// Input exceeded the hard byte bound.
    #[error("REW input has {0} bytes; maximum is {MAX_REW_BYTES}")]
    InputTooLarge(usize),
    /// Input exceeded the hard logical-line bound.
    #[error("REW input has at least {0} lines; maximum is {MAX_REW_LINES}")]
    TooManyLines(usize),
    /// One line exceeded the hard byte bound.
    #[error("REW line {line} has {bytes} bytes; maximum is {MAX_REW_LINE_BYTES}")]
    LineTooLong { line: usize, bytes: usize },
    /// The exact Generic EQ header is required.
    #[error("REW input must contain `Equaliser: Generic` before filters")]
    MissingHeader,
    /// Header was repeated or appeared after filter declarations.
    #[error("duplicate or late REW header on line {0}")]
    DuplicateOrLateHeader(usize),
    /// Unknown content is rejected instead of silently skipped.
    #[error("unexpected REW content on line {0}")]
    UnexpectedLine(usize),
    /// Too many filter declarations were scanned.
    #[error("REW input has {0} filter declarations; maximum is {MAX_REW_FILTER_LINES}")]
    TooManyFilterLines(usize),
    /// Output must be explicitly selected.
    #[error("target output must be 1 through 6: {0}")]
    InvalidTargetOutput(u8),
    /// Filter line did not match the supported generic format.
    #[error("malformed REW line {line}: {reason}")]
    MalformedLine { line: usize, reason: String },
    /// Only filters with a direct inferred DCX mapping are accepted.
    #[error("unsupported REW filter type on line {line}: {kind}")]
    UnsupportedFilter { line: usize, kind: String },
    /// Positive boosts have no API lane in this MVP.
    #[error("filter {index} has positive gain {gain_db} dB; MVP is cut-only")]
    PositiveGain { index: u8, gain_db: f64 },
    /// Frequency did not fit the inferred device range.
    #[error("filter {index} frequency is outside 20..=20000 Hz: {value}")]
    FrequencyOutOfRange { index: u8, value: f64 },
    /// Gain did not fit the described input range.
    #[error("filter {index} gain is outside -15..=15 dB: {value}")]
    GainOutOfRange { index: u8, value: f64 },
    /// Q did not fit the inferred device range.
    #[error("filter {index} Q is outside 0.1..=10: {value}")]
    QOutOfRange { index: u8, value: f64 },
    /// A filter number occurred twice.
    #[error("duplicate REW filter number: {0}")]
    DuplicateFilter(u8),
    /// Device supports at most nine static filters per target output.
    #[error("REW import has {0} enabled filters; maximum is 9")]
    TooManyFilters(usize),
    /// No actionable filters were present.
    #[error("REW import contains no enabled supported filters")]
    NoEnabledFilters,
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    const HEADER: &str = "Equaliser: Generic\n";

    #[test]
    fn parses_and_quantizes_exact_generic_export() {
        let text = format!(
            "{HEADER}Filter 1: ON PK Fc 63.0 Hz Gain -3.2 dB Q 4.00\n\
             Filter 2: OFF PK Fc 100 Hz Gain 2 dB Q 1.0\n"
        );
        let report = import_rew(&text, 3).unwrap();
        assert!(report.cut_only);
        assert_eq!(report.skipped_disabled, 1);
        assert_eq!(report.filters.len(), 1);
        assert_eq!(report.filters[0].gain_code, 118);
        assert!((report.filters[0].encoded_gain_db + 3.2).abs() < f64::EPSILON * 8.0);
    }

    #[test]
    fn cut_only_rejects_boosts() {
        let text = format!("{HEADER}Filter 1: ON PK Fc 100 Hz Gain 0.1 dB Q 1\n");
        let error = import_rew(&text, 1).unwrap_err();
        assert!(matches!(error, RewParseError::PositiveGain { .. }));
    }

    #[test]
    fn exact_grammar_rejects_unknown_or_trailing_content() {
        assert!(matches!(
            import_rew("Filter 1: ON PK Fc 100 Hz Gain -1 dB Q 1\n", 1),
            Err(RewParseError::MissingHeader)
        ));
        let trailing = format!("{HEADER}Filter 1: ON PK Fc 100 Hz Gain -1 dB Q 1 surprise\n");
        assert!(matches!(
            import_rew(&trailing, 1),
            Err(RewParseError::MalformedLine { .. })
        ));
        let unknown = format!("{HEADER}Not a filter\n");
        assert!(matches!(
            import_rew(&unknown, 1),
            Err(RewParseError::UnexpectedLine(2))
        ));
    }

    #[test]
    fn every_input_dimension_is_bounded() {
        let too_large = "x".repeat(MAX_REW_BYTES + 1);
        assert!(matches!(
            import_rew(&too_large, 1),
            Err(RewParseError::InputTooLarge(_))
        ));
        let long_line = format!("{HEADER}#{}\n", "x".repeat(MAX_REW_LINE_BYTES));
        assert!(matches!(
            import_rew(&long_line, 1),
            Err(RewParseError::LineTooLong { .. })
        ));
    }

    #[test]
    fn enabled_filter_cardinality_bound_is_exact() {
        let mut text = HEADER.to_owned();
        for index in 1..=9 {
            writeln!(text, "Filter {index}: ON PK Fc 100 Hz Gain -1 dB Q 1").unwrap();
        }
        assert_eq!(import_rew(&text, 1).unwrap().filters.len(), 9);
        text.push_str("Filter 10: ON PK Fc 100 Hz Gain -1 dB Q 1\n");
        assert!(matches!(
            import_rew(&text, 1),
            Err(RewParseError::TooManyFilters(10))
        ));
    }

    #[test]
    fn native_grid_endpoints_are_stable_and_cut_only() {
        let low = quantize(RequestedFilter {
            index: 1,
            kind: FilterKind::Peak,
            frequency_hz: 20.0,
            gain_db: -15.0,
            q: 0.1,
        })
        .unwrap();
        assert_eq!((low.frequency_code, low.gain_code, low.q_code), (0, 0, 0));
        let high = quantize(RequestedFilter {
            index: 1,
            kind: FilterKind::Peak,
            frequency_hz: 20_000.0,
            gain_db: 0.0,
            q: 10.0,
        })
        .unwrap();
        assert_eq!(
            (high.frequency_code, high.gain_code, high.q_code),
            (319, 150, 40)
        );
    }
}
