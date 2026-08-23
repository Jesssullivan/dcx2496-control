//! Pure, query-only planning for bounded DCX2496 discovery.
//!
//! This module has no transport dependency and never opens a serial, MIDI,
//! filesystem, network, or device handle. An outer boundary may execute the
//! represented attempts only after Legalab's hardware gate is satisfied.

use serde::Serialize;
use thiserror::Error;

use crate::protocol::{DeviceId, ProtocolError, Query, SearchResponse26};

/// Vendor-documented DCX2496 RS-232 rate.
pub const PRIMARY_BAUD: u32 = 115_200;
/// Single legacy fallback hypothesis retained for the reviewed probe plan.
pub const FALLBACK_BAUD: u32 = 38_400;

/// Serial parity represented by the offline plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SerialParity {
    /// No parity bit.
    None,
}

/// Serial flow control represented by the offline plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SerialFlowControl {
    /// No software or hardware flow control.
    None,
}

/// Exact immutable line settings for one planned search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SerialSettings {
    baud: u32,
    data_bits: u8,
    stop_bits: u8,
    parity: SerialParity,
    flow_control: SerialFlowControl,
}

impl SerialSettings {
    /// Return the planned baud rate.
    pub const fn baud(self) -> u32 {
        self.baud
    }

    /// Return the planned data-bit count.
    pub const fn data_bits(self) -> u8 {
        self.data_bits
    }

    /// Return the planned stop-bit count.
    pub const fn stop_bits(self) -> u8 {
        self.stop_bits
    }

    /// Return the planned parity mode.
    pub const fn parity(self) -> SerialParity {
        self.parity
    }

    /// Return the planned flow-control mode.
    pub const fn flow_control(self) -> SerialFlowControl {
        self.flow_control
    }
}

/// Position of one search in the immutable two-attempt plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryAttemptKind {
    /// First and required attempt at the vendor-documented RS-232 rate.
    Primary,
    /// One fallback attempt, reachable only after an explicit primary timeout.
    SingleFallback,
}

/// One query-only attempt returned to a future gated I/O boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryAttempt {
    kind: DiscoveryAttemptKind,
    settings: SerialSettings,
}

impl DiscoveryAttempt {
    const fn primary() -> Self {
        Self {
            kind: DiscoveryAttemptKind::Primary,
            settings: SerialSettings {
                baud: PRIMARY_BAUD,
                data_bits: 8,
                stop_bits: 1,
                parity: SerialParity::None,
                flow_control: SerialFlowControl::None,
            },
        }
    }

    const fn fallback() -> Self {
        Self {
            kind: DiscoveryAttemptKind::SingleFallback,
            settings: SerialSettings {
                baud: FALLBACK_BAUD,
                data_bits: 8,
                stop_bits: 1,
                parity: SerialParity::None,
                flow_control: SerialFlowControl::None,
            },
        }
    }

    /// Return this attempt's fixed position in the plan.
    pub const fn kind(self) -> DiscoveryAttemptKind {
        self.kind
    }

    /// Return this attempt's fixed 8N1/no-flow settings.
    pub const fn settings(self) -> SerialSettings {
        self.settings
    }

    /// Return the only request this plan can expose.
    pub const fn query(self) -> Query {
        Query::Search
    }
}

/// Pure planner state; names represent caller-supplied evidence only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DiscoveryState {
    /// The required 115200 8N1 attempt is pending.
    AwaitingPrimary,
    /// The primary timed out and the sole 38400 8N1 fallback is pending.
    AwaitingSingleFallback,
    /// One exact response matched the expected device address.
    Identified {
        /// Attempt on which the response was supplied.
        attempt: DiscoveryAttemptKind,
        /// Exact expected and observed address.
        device: DeviceId,
    },
    /// Both represented attempts were explicitly reported as timed out.
    Exhausted,
}

/// Offline-only discovery state machine with no transport implementation.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QueryOnlyDiscovery {
    expected_device: DeviceId,
    state: DiscoveryState,
}

impl QueryOnlyDiscovery {
    /// Start with the primary 115200 8N1 search for one expected address.
    pub const fn new(expected_device: DeviceId) -> Self {
        Self {
            expected_device,
            state: DiscoveryState::AwaitingPrimary,
        }
    }

    /// Inspect the current pure planner state.
    pub const fn state(&self) -> DiscoveryState {
        self.state
    }

    /// Return the one currently eligible query-only attempt.
    pub const fn current_attempt(&self) -> Option<DiscoveryAttempt> {
        match self.state {
            DiscoveryState::AwaitingPrimary => Some(DiscoveryAttempt::primary()),
            DiscoveryState::AwaitingSingleFallback => Some(DiscoveryAttempt::fallback()),
            DiscoveryState::Identified { .. } | DiscoveryState::Exhausted => None,
        }
    }

    /// Record a caller-reported timeout for exactly the current attempt.
    ///
    /// One primary timeout exposes one fallback. A fallback timeout exhausts
    /// the plan; there is no third attempt or retry loop.
    ///
    /// # Errors
    ///
    /// Returns [`DiscoveryError::NoPendingAttempt`] after identification or
    /// exhaustion. No clock or transport is read by this operation.
    pub fn timeout_current(&mut self) -> Result<DiscoveryState, DiscoveryError> {
        self.state = match self.state {
            DiscoveryState::AwaitingPrimary => DiscoveryState::AwaitingSingleFallback,
            DiscoveryState::AwaitingSingleFallback => DiscoveryState::Exhausted,
            DiscoveryState::Identified { .. } | DiscoveryState::Exhausted => {
                return Err(DiscoveryError::NoPendingAttempt);
            }
        };
        Ok(self.state)
    }

    /// Validate exactly one candidate frame for the current attempt.
    ///
    /// Empty and multi-frame candidate sets fail closed. A sole candidate must
    /// be an exact [`SearchResponse26`] and must match `expected_device`. State
    /// changes only after all checks pass.
    ///
    /// # Errors
    ///
    /// Returns [`DiscoveryError`] for absent, ambiguous, malformed, partial,
    /// wrong-identity, or out-of-sequence evidence.
    pub fn accept_candidates(
        &mut self,
        candidates: &[&[u8]],
    ) -> Result<SearchResponse26, DiscoveryError> {
        let attempt = self
            .current_attempt()
            .ok_or(DiscoveryError::NoPendingAttempt)?;
        let candidate = match candidates {
            [] => return Err(DiscoveryError::NoCandidate),
            [candidate] => *candidate,
            many => return Err(DiscoveryError::AmbiguousCandidates(many.len())),
        };
        let response = SearchResponse26::parse(candidate)?;
        if response.device() != self.expected_device {
            return Err(DiscoveryError::UnexpectedDevice {
                expected: self.expected_device.get(),
                actual: response.device().get(),
            });
        }
        self.state = DiscoveryState::Identified {
            attempt: attempt.kind(),
            device: response.device(),
        };
        Ok(response)
    }
}

/// Fail-closed offline discovery validation failures.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    /// The caller supplied no complete response candidate.
    #[error("no search response candidate was supplied")]
    NoCandidate,
    /// More than one candidate is ambiguous for the single-unit MVP.
    #[error("ambiguous search response candidates: {0}")]
    AmbiguousCandidates(usize),
    /// The observed address did not match the reviewed profile binding.
    #[error("unexpected device id: expected {expected}, found {actual}")]
    UnexpectedDevice { expected: u8, actual: u8 },
    /// Identification or exhaustion leaves no attempt eligible.
    #[error("no query-only discovery attempt is pending")]
    NoPendingAttempt,
    /// The sole candidate failed exact protocol validation.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_response(device: u8) -> Vec<u8> {
        let mut frame = vec![0xf0, 0x00, 0x20, 0x32, device, 0x0e, 0x00];
        frame.extend_from_slice(b"SYNTHETIC-IDENTITY");
        frame.push(0xf7);
        frame
    }

    #[test]
    fn plan_is_exactly_primary_then_one_fallback() {
        let mut discovery = QueryOnlyDiscovery::new(DeviceId::new(0).unwrap());
        let primary = discovery.current_attempt().unwrap();
        assert_eq!(primary.kind(), DiscoveryAttemptKind::Primary);
        assert_eq!(primary.settings().baud(), PRIMARY_BAUD);
        assert_eq!(primary.settings().data_bits(), 8);
        assert_eq!(primary.settings().stop_bits(), 1);
        assert_eq!(primary.settings().parity(), SerialParity::None);
        assert_eq!(primary.settings().flow_control(), SerialFlowControl::None);
        assert_eq!(
            primary.query().encode().unwrap(),
            [0xf0, 0x00, 0x20, 0x32, 0x20, 0x0e, 0x40, 0xf7]
        );

        assert_eq!(
            discovery.timeout_current().unwrap(),
            DiscoveryState::AwaitingSingleFallback
        );
        let fallback = discovery.current_attempt().unwrap();
        assert_eq!(fallback.kind(), DiscoveryAttemptKind::SingleFallback);
        assert_eq!(fallback.settings().baud(), FALLBACK_BAUD);
        assert_eq!(
            discovery.timeout_current().unwrap(),
            DiscoveryState::Exhausted
        );
        assert_eq!(discovery.current_attempt(), None);
        assert_eq!(
            discovery.timeout_current(),
            Err(DiscoveryError::NoPendingAttempt)
        );
    }

    #[test]
    fn exact_expected_identity_completes_current_attempt() {
        let frame = synthetic_response(0);
        let mut discovery = QueryOnlyDiscovery::new(DeviceId::new(0).unwrap());
        let response = discovery.accept_candidates(&[&frame]).unwrap();
        assert_eq!(response.device(), DeviceId::new(0).unwrap());
        assert_eq!(
            discovery.state(),
            DiscoveryState::Identified {
                attempt: DiscoveryAttemptKind::Primary,
                device: DeviceId::new(0).unwrap(),
            }
        );
        assert_eq!(discovery.current_attempt(), None);
    }

    #[test]
    fn absent_ambiguous_wrong_and_partial_evidence_leave_state_unchanged() {
        let expected = DeviceId::new(0).unwrap();
        let frame = synthetic_response(0);
        let wrong = synthetic_response(1);
        let partial = &frame[..frame.len() - 1];

        let cases: Vec<(Vec<&[u8]>, DiscoveryError)> = vec![
            (vec![], DiscoveryError::NoCandidate),
            (vec![&frame, &frame], DiscoveryError::AmbiguousCandidates(2)),
            (
                vec![&wrong],
                DiscoveryError::UnexpectedDevice {
                    expected: 0,
                    actual: 1,
                },
            ),
            (
                vec![partial],
                DiscoveryError::Protocol(ProtocolError::InvalidSearchResponseLength(25)),
            ),
        ];

        for (candidates, expected_error) in cases {
            let mut discovery = QueryOnlyDiscovery::new(expected);
            assert_eq!(
                discovery.accept_candidates(&candidates),
                Err(expected_error)
            );
            assert_eq!(discovery.state(), DiscoveryState::AwaitingPrimary);
        }
    }

    #[test]
    fn a_valid_fallback_identity_records_the_fallback_attempt() {
        let frame = synthetic_response(0);
        let mut discovery = QueryOnlyDiscovery::new(DeviceId::new(0).unwrap());
        discovery.timeout_current().unwrap();
        discovery.accept_candidates(&[&frame]).unwrap();
        assert!(matches!(
            discovery.state(),
            DiscoveryState::Identified {
                attempt: DiscoveryAttemptKind::SingleFallback,
                ..
            }
        ));
    }
}
