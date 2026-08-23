//! Pure fail-closed controller lifecycle.

use serde::Serialize;
use thiserror::Error;

use crate::profile::ProfileBinding;

/// Maximum lifetime for either one-use operator token (five minutes).
pub const MAX_TOKEN_TTL_MS: u64 = 300_000;

/// Controller lifecycle. Names describe evidence already supplied by the
/// caller; entering a state never performs hardware I/O.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ControllerState {
    /// No usable transport has been observed.
    Disconnected,
    /// Transport exists, but only read-only discovery is allowed.
    QueryOnly { device_id: u8 },
    /// A complete baseline was captured while all outputs were proven muted.
    BaselineCaptured {
        device_id: u8,
        baseline_digest: String,
    },
    /// A validated apply-ready profile and plan are bound to that baseline.
    Staged {
        device_id: u8,
        baseline_digest: String,
        plan_digest: String,
        profile_id: String,
        profile_revision: u32,
        profile_digest: String,
    },
    /// A short-lived one-use operator token authorizes one apply attempt.
    ArmedApply {
        device_id: u8,
        baseline_digest: String,
        plan_digest: String,
        profile_id: String,
        profile_revision: u32,
        profile_digest: String,
        apply_token_digest: String,
        expires_at_ms: u64,
    },
    /// The caller reports apply is in progress while physical outputs are muted.
    ApplyingMuted {
        device_id: u8,
        plan_digest: String,
        profile_id: String,
        profile_revision: u32,
        profile_digest: String,
        apply_token_digest: String,
    },
    /// Exact desired state and mute state have both been read back.
    VerifiedMuted {
        device_id: u8,
        plan_digest: String,
        profile_id: String,
        profile_revision: u32,
        profile_digest: String,
        readback_digest: String,
        apply_token_digest: String,
    },
    /// A separate, short-lived token authorizes activation of that readback.
    ArmedActivation {
        device_id: u8,
        plan_digest: String,
        profile_id: String,
        profile_revision: u32,
        profile_digest: String,
        readback_digest: String,
        activation_token_digest: String,
        expires_at_ms: u64,
    },
    /// A separately authorized activation has been reported.
    Active {
        device_id: u8,
        plan_digest: String,
        profile_id: String,
        profile_revision: u32,
        profile_digest: String,
        readback_digest: String,
    },
    /// Any uncertainty or panic request lands here until an explicit muted
    /// recovery observation is supplied.
    Faulted { reason: String },
}

/// Facts supplied to the state machine by an outer I/O boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A supported unit address was observed through a read-only probe.
    ObserveTransport { device_id: u8 },
    /// Complete state and physical mute were observed for this exact unit.
    CaptureMutedBaseline {
        device_id: u8,
        baseline_digest: String,
        outputs_muted: bool,
    },
    /// A validated plan was generated against the current exact baseline.
    StagePlan {
        baseline_digest: String,
        plan_digest: String,
        profile: ProfileBinding,
    },
    /// Operator apply intent was captured in a one-use expiring token.
    ArmApply {
        token_digest: String,
        expires_at_ms: u64,
        now_ms: u64,
    },
    /// Begin an apply with the exact one-use token.
    BeginApply { token_digest: String, now_ms: u64 },
    /// Hardware readback proves the exact staged device/plan/profile and mute state.
    VerifyMuted {
        device_id: u8,
        plan_digest: String,
        profile_id: String,
        profile_revision: u32,
        profile_digest: String,
        readback_digest: String,
        outputs_muted: bool,
    },
    /// Capture distinct operator activation intent after muted readback.
    ArmActivation {
        token_digest: String,
        expires_at_ms: u64,
        now_ms: u64,
    },
    /// Activate only the exact verified binding using its distinct token.
    Activate {
        device_id: u8,
        plan_digest: String,
        profile_id: String,
        profile_revision: u32,
        profile_digest: String,
        readback_digest: String,
        token_digest: String,
        now_ms: u64,
    },
    /// Immediate uncertainty/panic path; never interpreted as confirmed mute.
    PanicMute,
    /// Any outer-boundary error.
    Fault { reason: String },
    /// Physical disconnection.
    Disconnect,
    /// Recover only after the caller independently proves outputs muted.
    RecoverMuted,
}

/// Deterministic controller state machine.
///
/// This type intentionally implements serialization for receipts but neither
/// deserialization nor cloning. A caller cannot manufacture or duplicate an
/// armed or active machine through this API.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerMachine {
    state: ControllerState,
}

impl Default for ControllerMachine {
    fn default() -> Self {
        Self {
            state: ControllerState::Disconnected,
        }
    }
}

impl ControllerMachine {
    /// Inspect the current state.
    pub const fn state(&self) -> &ControllerState {
        &self.state
    }

    /// Apply one checked transition. On failure, state is unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`TransitionError`] for an illegal transition, expired or
    /// mismatched token, wrong identity/binding, unproven mute, or malformed
    /// evidence digest.
    #[allow(clippy::too_many_lines)]
    pub fn transition(&mut self, event: Event) -> Result<&ControllerState, TransitionError> {
        let next = match (&self.state, event) {
            (_, Event::Disconnect) | (ControllerState::Faulted { .. }, Event::RecoverMuted) => {
                ControllerState::Disconnected
            }
            (_, Event::PanicMute) => ControllerState::Faulted {
                reason: "panic_mute_requested_unverified".into(),
            },
            (_, Event::Fault { reason }) => {
                if reason.is_empty() {
                    return Err(TransitionError::InvalidEvidence("fault reason"));
                }
                ControllerState::Faulted { reason }
            }
            (ControllerState::Disconnected, Event::ObserveTransport { device_id }) => {
                validate_device(device_id)?;
                ControllerState::QueryOnly { device_id }
            }
            (
                ControllerState::QueryOnly {
                    device_id: expected,
                },
                Event::CaptureMutedBaseline {
                    device_id,
                    baseline_digest,
                    outputs_muted,
                },
            ) => {
                require_equal_u8("device id", *expected, device_id)?;
                require_muted(outputs_muted)?;
                validate_digest(&baseline_digest, "baseline digest")?;
                ControllerState::BaselineCaptured {
                    device_id,
                    baseline_digest,
                }
            }
            (
                ControllerState::BaselineCaptured {
                    device_id,
                    baseline_digest: expected_baseline,
                },
                Event::StagePlan {
                    baseline_digest,
                    plan_digest,
                    profile,
                },
            ) => {
                require_equal_u8("profile device id", *device_id, profile.device_id())?;
                require_equal("baseline digest", expected_baseline, &baseline_digest)?;
                validate_digest(&plan_digest, "plan digest")?;
                ControllerState::Staged {
                    device_id: *device_id,
                    baseline_digest,
                    plan_digest,
                    profile_id: profile.profile_id().to_owned(),
                    profile_revision: profile.profile_revision(),
                    profile_digest: profile.profile_digest().to_owned(),
                }
            }
            (
                ControllerState::Staged {
                    device_id,
                    baseline_digest,
                    plan_digest,
                    profile_id,
                    profile_revision,
                    profile_digest,
                },
                Event::ArmApply {
                    token_digest,
                    expires_at_ms,
                    now_ms,
                },
            ) => {
                validate_digest(&token_digest, "apply token digest")?;
                validate_token_window(expires_at_ms, now_ms)?;
                ControllerState::ArmedApply {
                    device_id: *device_id,
                    baseline_digest: baseline_digest.clone(),
                    plan_digest: plan_digest.clone(),
                    profile_id: profile_id.clone(),
                    profile_revision: *profile_revision,
                    profile_digest: profile_digest.clone(),
                    apply_token_digest: token_digest,
                    expires_at_ms,
                }
            }
            (
                ControllerState::ArmedApply {
                    device_id,
                    plan_digest,
                    profile_id,
                    profile_revision,
                    profile_digest,
                    apply_token_digest,
                    expires_at_ms,
                    ..
                },
                Event::BeginApply {
                    token_digest: presented,
                    now_ms,
                },
            ) => {
                require_unexpired(*expires_at_ms, now_ms)?;
                require_equal("apply token digest", apply_token_digest, &presented)
                    .map_err(|_| TransitionError::WrongToken("apply"))?;
                ControllerState::ApplyingMuted {
                    device_id: *device_id,
                    plan_digest: plan_digest.clone(),
                    profile_id: profile_id.clone(),
                    profile_revision: *profile_revision,
                    profile_digest: profile_digest.clone(),
                    apply_token_digest: apply_token_digest.clone(),
                }
            }
            (
                ControllerState::ApplyingMuted {
                    device_id: expected_device,
                    plan_digest: expected_plan,
                    profile_id: expected_profile_id,
                    profile_revision: expected_revision,
                    profile_digest: expected_profile,
                    apply_token_digest,
                },
                Event::VerifyMuted {
                    device_id,
                    plan_digest,
                    profile_id,
                    profile_revision,
                    profile_digest,
                    readback_digest,
                    outputs_muted,
                },
            ) => {
                require_muted(outputs_muted)?;
                require_equal_u8("readback device id", *expected_device, device_id)?;
                require_equal("readback plan digest", expected_plan, &plan_digest)?;
                require_equal("readback profile id", expected_profile_id, &profile_id)?;
                require_equal_u32(
                    "readback profile revision",
                    *expected_revision,
                    profile_revision,
                )?;
                require_equal("readback profile digest", expected_profile, &profile_digest)?;
                validate_digest(&readback_digest, "readback digest")?;
                ControllerState::VerifiedMuted {
                    device_id,
                    plan_digest,
                    profile_id,
                    profile_revision,
                    profile_digest,
                    readback_digest,
                    apply_token_digest: apply_token_digest.clone(),
                }
            }
            (
                ControllerState::VerifiedMuted {
                    device_id,
                    plan_digest,
                    profile_id,
                    profile_revision,
                    profile_digest,
                    readback_digest,
                    apply_token_digest,
                },
                Event::ArmActivation {
                    token_digest,
                    expires_at_ms,
                    now_ms,
                },
            ) => {
                validate_digest(&token_digest, "activation token digest")?;
                if token_digest == *apply_token_digest {
                    return Err(TransitionError::ReusedToken);
                }
                validate_token_window(expires_at_ms, now_ms)?;
                ControllerState::ArmedActivation {
                    device_id: *device_id,
                    plan_digest: plan_digest.clone(),
                    profile_id: profile_id.clone(),
                    profile_revision: *profile_revision,
                    profile_digest: profile_digest.clone(),
                    readback_digest: readback_digest.clone(),
                    activation_token_digest: token_digest,
                    expires_at_ms,
                }
            }
            (
                ControllerState::ArmedActivation {
                    device_id: expected_device,
                    plan_digest: expected_plan,
                    profile_id: expected_profile_id,
                    profile_revision: expected_revision,
                    profile_digest: expected_profile,
                    readback_digest: expected_readback,
                    activation_token_digest,
                    expires_at_ms,
                },
                Event::Activate {
                    device_id,
                    plan_digest,
                    profile_id,
                    profile_revision,
                    profile_digest,
                    readback_digest,
                    token_digest,
                    now_ms,
                },
            ) => {
                require_unexpired(*expires_at_ms, now_ms)?;
                require_equal(
                    "activation token digest",
                    activation_token_digest,
                    &token_digest,
                )
                .map_err(|_| TransitionError::WrongToken("activation"))?;
                require_equal_u8("activation device id", *expected_device, device_id)?;
                require_equal("activation plan digest", expected_plan, &plan_digest)?;
                require_equal("activation profile id", expected_profile_id, &profile_id)?;
                require_equal_u32(
                    "activation profile revision",
                    *expected_revision,
                    profile_revision,
                )?;
                require_equal(
                    "activation profile digest",
                    expected_profile,
                    &profile_digest,
                )?;
                require_equal(
                    "activation readback digest",
                    expected_readback,
                    &readback_digest,
                )?;
                ControllerState::Active {
                    device_id,
                    plan_digest,
                    profile_id,
                    profile_revision,
                    profile_digest,
                    readback_digest,
                }
            }
            (state, event) => {
                return Err(TransitionError::IllegalTransition {
                    state: state_name(state),
                    event: event_name(&event),
                });
            }
        };
        self.state = next;
        Ok(&self.state)
    }
}

fn validate_device(device_id: u8) -> Result<(), TransitionError> {
    if device_id <= 15 {
        Ok(())
    } else {
        Err(TransitionError::InvalidDeviceId(device_id))
    }
}

fn validate_digest(digest: &str, name: &'static str) -> Result<(), TransitionError> {
    if digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(TransitionError::InvalidEvidence(name))
    }
}

fn validate_token_window(expires_at_ms: u64, now_ms: u64) -> Result<(), TransitionError> {
    let Some(ttl) = expires_at_ms.checked_sub(now_ms) else {
        return Err(TransitionError::ExpiredToken);
    };
    if ttl == 0 {
        Err(TransitionError::ExpiredToken)
    } else if ttl > MAX_TOKEN_TTL_MS {
        Err(TransitionError::TokenLifetimeTooLong(ttl))
    } else {
        Ok(())
    }
}

fn require_unexpired(expires_at_ms: u64, now_ms: u64) -> Result<(), TransitionError> {
    if now_ms < expires_at_ms {
        Ok(())
    } else {
        Err(TransitionError::ExpiredToken)
    }
}

fn require_muted(outputs_muted: bool) -> Result<(), TransitionError> {
    if outputs_muted {
        Ok(())
    } else {
        Err(TransitionError::OutputsNotMuted)
    }
}

fn require_equal(field: &'static str, expected: &str, actual: &str) -> Result<(), TransitionError> {
    if expected == actual {
        Ok(())
    } else {
        Err(TransitionError::BindingMismatch(field))
    }
}

fn require_equal_u8(field: &'static str, expected: u8, actual: u8) -> Result<(), TransitionError> {
    if expected == actual {
        Ok(())
    } else {
        Err(TransitionError::BindingMismatch(field))
    }
}

fn require_equal_u32(
    field: &'static str,
    expected: u32,
    actual: u32,
) -> Result<(), TransitionError> {
    if expected == actual {
        Ok(())
    } else {
        Err(TransitionError::BindingMismatch(field))
    }
}

const fn state_name(state: &ControllerState) -> &'static str {
    match state {
        ControllerState::Disconnected => "disconnected",
        ControllerState::QueryOnly { .. } => "query_only",
        ControllerState::BaselineCaptured { .. } => "baseline_captured",
        ControllerState::Staged { .. } => "staged",
        ControllerState::ArmedApply { .. } => "armed_apply",
        ControllerState::ApplyingMuted { .. } => "applying_muted",
        ControllerState::VerifiedMuted { .. } => "verified_muted",
        ControllerState::ArmedActivation { .. } => "armed_activation",
        ControllerState::Active { .. } => "active",
        ControllerState::Faulted { .. } => "faulted",
    }
}

const fn event_name(event: &Event) -> &'static str {
    match event {
        Event::ObserveTransport { .. } => "observe_transport",
        Event::CaptureMutedBaseline { .. } => "capture_muted_baseline",
        Event::StagePlan { .. } => "stage_plan",
        Event::ArmApply { .. } => "arm_apply",
        Event::BeginApply { .. } => "begin_apply",
        Event::VerifyMuted { .. } => "verify_muted",
        Event::ArmActivation { .. } => "arm_activation",
        Event::Activate { .. } => "activate",
        Event::PanicMute => "panic_mute",
        Event::Fault { .. } => "fault",
        Event::Disconnect => "disconnect",
        Event::RecoverMuted => "recover_muted",
    }
}

/// Rejected transition or missing evidence.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TransitionError {
    /// Event is not legal in the present state.
    #[error("illegal transition: {state} + {event}")]
    IllegalTransition {
        state: &'static str,
        event: &'static str,
    },
    /// Device address was invalid.
    #[error("invalid device id: {0}")]
    InvalidDeviceId(u8),
    /// Required evidence was missing or malformed.
    #[error("missing or invalid {0}")]
    InvalidEvidence(&'static str),
    /// Staged, readback, or activation identity did not match exactly.
    #[error("binding mismatch for {0}")]
    BindingMismatch(&'static str),
    /// Physical mute was not affirmatively proven.
    #[error("all physical outputs must be affirmatively proven muted")]
    OutputsNotMuted,
    /// Operator token expired before use.
    #[error("operator token has expired")]
    ExpiredToken,
    /// Operator token lifetime exceeded the hard bound.
    #[error("operator token lifetime {0} ms exceeds {MAX_TOKEN_TTL_MS} ms")]
    TokenLifetimeTooLong(u64),
    /// Presented token did not match the staged authorization.
    #[error("{0} token does not match")]
    WrongToken(&'static str),
    /// Apply and activation must use distinct tokens.
    #[error("activation token must be distinct from the consumed apply token")]
    ReusedToken,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{
        DEVICE_MANUFACTURER, DEVICE_MODEL, InputCMode, InputCProfile, LEGALAB_PROFILE_ID,
        LEGALAB_PROFILE_REVISION, LabProfileV1, Limiter, OutputProfile, OutputRole,
        PROFILE_SCHEMA_VERSION,
    };

    const BASELINE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const PLAN: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const PROFILE: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const APPLY_TOKEN: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    const READBACK: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    const ACTIVATE_TOKEN: &str = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

    fn role(output: u8) -> OutputRole {
        match output {
            1 => OutputRole::MonitorLeft,
            2 => OutputRole::MonitorRight,
            3 => OutputRole::Subwoofer,
            4 => OutputRole::Pa,
            _ => OutputRole::Unused,
        }
    }

    fn profile_binding() -> ProfileBinding {
        LabProfileV1 {
            schema_version: PROFILE_SCHEMA_VERSION,
            profile_id: LEGALAB_PROFILE_ID.into(),
            profile_revision: LEGALAB_PROFILE_REVISION,
            device_manufacturer: DEVICE_MANUFACTURER.into(),
            device_model: DEVICE_MODEL.into(),
            device_id: 0,
            input_c: InputCProfile {
                mode: InputCMode::Line,
                auto_align_enabled: false,
                auto_eq_enabled: false,
                phantom_15v_enabled: false,
            },
            outputs: (1..=6)
                .map(|output| OutputProfile {
                    output,
                    role: role(output),
                    label: format!("output-{output}"),
                    sink_manufacturer: (output <= 4).then(|| "verified".into()),
                    sink_model: (output <= 4).then(|| format!("model-{output}")),
                    model_verified: output <= 4,
                    source: None,
                    muted: true,
                    gain_db: -15.0,
                    filters: Vec::new(),
                    limiter: (output <= 4).then_some(Limiter {
                        enabled: true,
                        threshold_db: -12.0,
                        release_ms: 100.0,
                    }),
                })
                .collect(),
        }
        .binding_for_apply(PROFILE)
        .unwrap()
    }

    fn applying() -> ControllerMachine {
        let mut machine = ControllerMachine::default();
        machine
            .transition(Event::ObserveTransport { device_id: 0 })
            .unwrap();
        machine
            .transition(Event::CaptureMutedBaseline {
                device_id: 0,
                baseline_digest: BASELINE.into(),
                outputs_muted: true,
            })
            .unwrap();
        machine
            .transition(Event::StagePlan {
                baseline_digest: BASELINE.into(),
                plan_digest: PLAN.into(),
                profile: profile_binding(),
            })
            .unwrap();
        machine
            .transition(Event::ArmApply {
                token_digest: APPLY_TOKEN.into(),
                expires_at_ms: 600,
                now_ms: 100,
            })
            .unwrap();
        machine
            .transition(Event::BeginApply {
                token_digest: APPLY_TOKEN.into(),
                now_ms: 200,
            })
            .unwrap();
        machine
    }

    fn verify_event() -> Event {
        Event::VerifyMuted {
            device_id: 0,
            plan_digest: PLAN.into(),
            profile_id: LEGALAB_PROFILE_ID.into(),
            profile_revision: LEGALAB_PROFILE_REVISION,
            profile_digest: PROFILE.into(),
            readback_digest: READBACK.into(),
            outputs_muted: true,
        }
    }

    fn activate_event(token: &str, now_ms: u64) -> Event {
        Event::Activate {
            device_id: 0,
            plan_digest: PLAN.into(),
            profile_id: LEGALAB_PROFILE_ID.into(),
            profile_revision: LEGALAB_PROFILE_REVISION,
            profile_digest: PROFILE.into(),
            readback_digest: READBACK.into(),
            token_digest: token.into(),
            now_ms,
        }
    }

    #[test]
    fn distinct_one_use_tokens_reach_active() {
        let mut machine = applying();
        machine.transition(verify_event()).unwrap();
        machine
            .transition(Event::ArmActivation {
                token_digest: ACTIVATE_TOKEN.into(),
                expires_at_ms: 900,
                now_ms: 700,
            })
            .unwrap();
        machine
            .transition(activate_event(ACTIVATE_TOKEN, 800))
            .unwrap();
        assert!(matches!(machine.state(), ControllerState::Active { .. }));
    }

    #[test]
    fn readback_requires_exact_binding_and_proven_mute() {
        let mut machine = applying();
        let before = machine.state().clone();
        let Event::VerifyMuted {
            device_id,
            plan_digest,
            profile_id,
            profile_revision,
            profile_digest,
            readback_digest,
            ..
        } = verify_event()
        else {
            unreachable!()
        };
        assert_eq!(
            machine
                .transition(Event::VerifyMuted {
                    device_id,
                    plan_digest,
                    profile_id,
                    profile_revision,
                    profile_digest,
                    readback_digest,
                    outputs_muted: false,
                })
                .unwrap_err(),
            TransitionError::OutputsNotMuted
        );
        assert_eq!(machine.state(), &before);

        let Event::VerifyMuted {
            device_id,
            profile_id,
            profile_revision,
            profile_digest,
            readback_digest,
            ..
        } = verify_event()
        else {
            unreachable!()
        };
        assert_eq!(
            machine
                .transition(Event::VerifyMuted {
                    device_id,
                    plan_digest: BASELINE.into(),
                    profile_id,
                    profile_revision,
                    profile_digest,
                    readback_digest,
                    outputs_muted: true,
                })
                .unwrap_err(),
            TransitionError::BindingMismatch("readback plan digest")
        );
        assert_eq!(machine.state(), &before);
    }

    #[test]
    fn activation_requires_distinct_matching_unexpired_token() {
        let mut machine = applying();
        machine.transition(verify_event()).unwrap();
        let before = machine.state().clone();
        assert_eq!(
            machine
                .transition(Event::ArmActivation {
                    token_digest: APPLY_TOKEN.into(),
                    expires_at_ms: 900,
                    now_ms: 700,
                })
                .unwrap_err(),
            TransitionError::ReusedToken
        );
        assert_eq!(machine.state(), &before);

        machine
            .transition(Event::ArmActivation {
                token_digest: ACTIVATE_TOKEN.into(),
                expires_at_ms: 900,
                now_ms: 700,
            })
            .unwrap();
        let armed = machine.state().clone();
        assert_eq!(
            machine
                .transition(activate_event(ACTIVATE_TOKEN, 900))
                .unwrap_err(),
            TransitionError::ExpiredToken
        );
        assert_eq!(machine.state(), &armed);
    }

    #[test]
    fn token_lifetime_is_hard_bounded() {
        let mut machine = ControllerMachine::default();
        machine
            .transition(Event::ObserveTransport { device_id: 0 })
            .unwrap();
        machine
            .transition(Event::CaptureMutedBaseline {
                device_id: 0,
                baseline_digest: BASELINE.into(),
                outputs_muted: true,
            })
            .unwrap();
        machine
            .transition(Event::StagePlan {
                baseline_digest: BASELINE.into(),
                plan_digest: PLAN.into(),
                profile: profile_binding(),
            })
            .unwrap();
        assert!(matches!(
            machine.transition(Event::ArmApply {
                token_digest: APPLY_TOKEN.into(),
                expires_at_ms: MAX_TOKEN_TTL_MS + 101,
                now_ms: 100,
            }),
            Err(TransitionError::TokenLifetimeTooLong(_))
        ));
    }

    #[test]
    fn panic_is_not_treated_as_confirmed_mute() {
        let mut machine = applying();
        machine.transition(Event::PanicMute).unwrap();
        assert!(matches!(machine.state(), ControllerState::Faulted { .. }));
        assert!(
            machine
                .transition(activate_event(ACTIVATE_TOKEN, 800))
                .is_err()
        );
    }
}
