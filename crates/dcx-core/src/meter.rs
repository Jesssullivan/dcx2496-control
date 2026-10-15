//! Typed, read-only decoding of one DCX2496 level-meter reply.
//!
//! The meter request is the existing [`Query::Ping`](crate::protocol::Query::Ping)
//! frame (`F0 00 20 32 <dev> 0E 44 00 00 F7`). Its reply is one exact 25-byte
//! function-`0x04` frame. Frame bytes 8 through 10 carry inputs A, B, and C;
//! bytes 11 through 16 carry outputs 1 through 6. In each meter byte the low
//! five bits are a level step from 0 through 6 (5 is the warning step, 6 is
//! clip) and bit 5 (`0x20`) reports limiter activity.
//!
//! This layout is a behavioral hypothesis carried from the DCX lane findings;
//! it is unverified on the named device until a live capture agrees. Every
//! byte outside the documented domain fails closed: a level above 6 or a set
//! bit 6 is an error, never a clamp. The remaining payload bytes stay opaque
//! and are neither interpreted nor serialized.

use serde::Serialize;
use thiserror::Error;

use crate::protocol::{Address, DeviceId, ProtocolError, parse_frame};

/// Exact total byte length of one meter reply frame, including the envelope.
pub const METER_RESPONSE_LEN: usize = 25;
/// Function byte of a meter reply.
pub const METER_RESPONSE_FUNCTION: u8 = 0x04;
/// Highest documented level step.
pub const MAX_METER_LEVEL: u8 = 6;
/// Level step the front panel shows as a warning.
pub const METER_WARN_LEVEL: u8 = 5;
/// Level step the front panel shows as clip.
pub const METER_CLIP_LEVEL: u8 = 6;
/// Evidence class attached to every decoded reading.
pub const METER_EVIDENCE_CLASS: &str = "lane_hypothesis_unverified_on_named_device";

const FIRST_INPUT_INDEX: usize = 8;
const FIRST_OUTPUT_INDEX: usize = 11;
const INPUT_COUNT: usize = 3;
const OUTPUT_COUNT: usize = 6;
const LEVEL_MASK: u8 = 0x1f;
const LIMITER_BIT: u8 = 0x20;
const RESERVED_BIT: u8 = 0x40;
const _: () = assert!(FIRST_INPUT_INDEX + INPUT_COUNT == FIRST_OUTPUT_INDEX);
const _: () = assert!(FIRST_OUTPUT_INDEX + OUTPUT_COUNT < METER_RESPONSE_LEN);

/// Named meter source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterChannel {
    /// Input A.
    InputA,
    /// Input B.
    InputB,
    /// Input C.
    InputC,
    /// Physical output 1 through 6.
    Output(u8),
}

impl MeterChannel {
    fn label(self) -> String {
        match self {
            Self::InputA => "input A".to_owned(),
            Self::InputB => "input B".to_owned(),
            Self::InputC => "input C".to_owned(),
            Self::Output(output) => format!("output {output}"),
        }
    }
}

/// One decoded meter byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MeterValue {
    level: u8,
    warn: bool,
    clip: bool,
    limiter: bool,
}

impl MeterValue {
    const SILENT: Self = Self {
        level: 0,
        warn: false,
        clip: false,
        limiter: false,
    };

    /// Decode one raw meter byte.
    ///
    /// # Errors
    ///
    /// Returns [`MeterError::ReservedBit`] when bit 6 is set and
    /// [`MeterError::LevelOutOfRange`] for a level step above 6.
    pub fn decode(channel: MeterChannel, raw: u8) -> Result<Self, MeterError> {
        if raw & RESERVED_BIT != 0 {
            return Err(MeterError::ReservedBit {
                channel: channel.label(),
                raw,
            });
        }
        let level = raw & LEVEL_MASK;
        if level > MAX_METER_LEVEL {
            return Err(MeterError::LevelOutOfRange {
                channel: channel.label(),
                level,
                raw,
            });
        }
        Ok(Self {
            level,
            warn: level >= METER_WARN_LEVEL,
            clip: level == METER_CLIP_LEVEL,
            limiter: raw & LIMITER_BIT != 0,
        })
    }

    /// Level step, 0 through 6.
    pub const fn level(self) -> u8 {
        self.level
    }

    /// Whether the step is at or above the warning step.
    pub const fn warn(self) -> bool {
        self.warn
    }

    /// Whether the step is the clip step.
    pub const fn clip(self) -> bool {
        self.clip
    }

    /// Whether the limiter bit is set.
    pub const fn limiter(self) -> bool {
        self.limiter
    }
}

/// Decoded inputs A/B/C.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MeterInputs {
    /// Input A.
    pub a: MeterValue,
    /// Input B.
    pub b: MeterValue,
    /// Input C.
    pub c: MeterValue,
}

/// One complete, typed meter reading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MeterReadingV1 {
    device: DeviceId,
    evidence_class: &'static str,
    inputs: MeterInputs,
    outputs: [MeterValue; OUTPUT_COUNT],
}

impl MeterReadingV1 {
    /// Validate and decode one complete candidate meter reply.
    ///
    /// # Errors
    ///
    /// Returns [`MeterError`] for any frame that is not exactly 25 bytes, has
    /// an invalid envelope, uses a function other than `0x04`, originates from
    /// the broadcast address or another device, or carries an out-of-domain
    /// meter byte.
    pub fn parse(frame: &[u8], expected_device: DeviceId) -> Result<Self, MeterError> {
        if frame.len() != METER_RESPONSE_LEN {
            return Err(MeterError::InvalidLength(frame.len()));
        }
        let message = parse_frame(frame)?;
        let device = match message.address() {
            Address::Device(device) => device,
            Address::BroadcastSearch => return Err(MeterError::BroadcastAddress),
        };
        if message.function() != METER_RESPONSE_FUNCTION {
            return Err(MeterError::UnexpectedFunction(message.function()));
        }
        if device != expected_device {
            return Err(MeterError::UnexpectedDevice {
                expected: expected_device.get(),
                actual: device.get(),
            });
        }

        let input = |index: usize, channel: MeterChannel| {
            MeterValue::decode(channel, frame[FIRST_INPUT_INDEX + index])
        };
        let inputs = MeterInputs {
            a: input(0, MeterChannel::InputA)?,
            b: input(1, MeterChannel::InputB)?,
            c: input(2, MeterChannel::InputC)?,
        };
        let mut outputs = [MeterValue::SILENT; OUTPUT_COUNT];
        for ((output, slot), raw) in (1_u8..)
            .zip(outputs.iter_mut())
            .zip(&frame[FIRST_OUTPUT_INDEX..FIRST_OUTPUT_INDEX + OUTPUT_COUNT])
        {
            *slot = MeterValue::decode(MeterChannel::Output(output), *raw)?;
        }
        Ok(Self {
            device,
            evidence_class: METER_EVIDENCE_CLASS,
            inputs,
            outputs,
        })
    }

    /// Validated reply address.
    pub const fn device(&self) -> DeviceId {
        self.device
    }

    /// Inputs A, B, and C.
    pub const fn inputs(&self) -> MeterInputs {
        self.inputs
    }

    /// Outputs 1 through 6, in output order.
    pub const fn outputs(&self) -> &[MeterValue; OUTPUT_COUNT] {
        &self.outputs
    }

    /// Highest level step across all nine meters.
    pub fn peak_level(&self) -> u8 {
        [self.inputs.a, self.inputs.b, self.inputs.c]
            .iter()
            .chain(self.outputs.iter())
            .map(|value| value.level)
            .max()
            .unwrap_or(0)
    }
}

/// Fail-closed meter decoding failures.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MeterError {
    /// The reply was not exactly 25 bytes.
    #[error("meter reply must be exactly 25 bytes: found {0}")]
    InvalidLength(usize),
    /// The reply used the broadcast request address.
    #[error("meter reply used the broadcast request address")]
    BroadcastAddress,
    /// The reply function was not `0x04`.
    #[error("unexpected meter reply function: {0:#04x}")]
    UnexpectedFunction(u8),
    /// The reply originated from another address.
    #[error("unexpected meter device id: expected {expected}, found {actual}")]
    UnexpectedDevice { expected: u8, actual: u8 },
    /// A meter byte set the undocumented bit 6.
    #[error("{channel} meter byte {raw:#04x} sets undocumented bit 6")]
    ReservedBit { channel: String, raw: u8 },
    /// A level step exceeded the documented 0 through 6 range.
    #[error("{channel} meter byte {raw:#04x} has level {level}, above the documented maximum 6")]
    LevelOutOfRange { channel: String, level: u8, raw: u8 },
    /// The envelope failed protocol validation.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Query;

    fn device(value: u8) -> DeviceId {
        DeviceId::new(value).unwrap()
    }

    /// Synthetic reply: header, one opaque byte, nine meter bytes, seven
    /// opaque bytes, terminator.
    fn synthetic(device: u8, meters: [u8; 9]) -> Vec<u8> {
        let mut frame = vec![0xf0, 0x00, 0x20, 0x32, device, 0x0e, 0x04, 0x00];
        frame.extend_from_slice(&meters);
        frame.extend_from_slice(&[0; 7]);
        frame.push(0xf7);
        assert_eq!(frame.len(), METER_RESPONSE_LEN);
        frame
    }

    #[test]
    fn meter_request_is_the_exact_ping_frame() {
        assert_eq!(
            Query::Ping(device(0)).encode().unwrap(),
            [0xf0, 0x00, 0x20, 0x32, 0x00, 0x0e, 0x44, 0x00, 0x00, 0xf7]
        );
    }

    #[test]
    fn decodes_inputs_outputs_warn_clip_and_limiter() {
        let frame = synthetic(0, [0, 3, 6, 0x21, 0x02, 0x05, 0x26, 0x00, 0x04]);
        let reading = MeterReadingV1::parse(&frame, device(0)).unwrap();
        assert_eq!(reading.device(), device(0));
        let inputs = reading.inputs();
        assert_eq!(inputs.a.level(), 0);
        assert!(!inputs.a.warn());
        assert_eq!(inputs.b.level(), 3);
        assert_eq!(inputs.c.level(), 6);
        assert!(inputs.c.warn() && inputs.c.clip() && !inputs.c.limiter());

        let outputs = reading.outputs();
        assert_eq!(outputs[0].level(), 1);
        assert!(outputs[0].limiter());
        assert_eq!(outputs[1].level(), 2);
        assert!(!outputs[1].limiter());
        assert_eq!(outputs[2].level(), 5);
        assert!(outputs[2].warn() && !outputs[2].clip());
        assert_eq!(outputs[3].level(), 6);
        assert!(outputs[3].clip() && outputs[3].limiter());
        assert_eq!(outputs[4].level(), 0);
        assert_eq!(outputs[5].level(), 4);
        assert_eq!(reading.peak_level(), 6);
    }

    #[test]
    fn serialized_reading_names_channels_and_omits_opaque_bytes() {
        let mut frame = synthetic(0, [1, 2, 3, 4, 5, 6, 0, 1, 2]);
        frame[7] = 0x5a;
        frame[20] = 0x33;
        let json = serde_json::to_value(MeterReadingV1::parse(&frame, device(0)).unwrap()).unwrap();
        assert_eq!(json["device"], 0);
        assert_eq!(json["evidence_class"], METER_EVIDENCE_CLASS);
        assert_eq!(json["inputs"]["c"]["level"], 3);
        assert_eq!(json["outputs"][2]["level"], 6);
        assert_eq!(json["outputs"][2]["clip"], true);
        assert_eq!(json.as_object().unwrap().len(), 4);
        let text = json.to_string();
        assert!(!text.contains("90") && !text.contains("51"));
    }

    #[test]
    fn exact_length_function_and_device_are_required() {
        let frame = synthetic(0, [0; 9]);
        assert_eq!(
            MeterReadingV1::parse(&frame[..24], device(0)),
            Err(MeterError::InvalidLength(24))
        );
        let mut long = frame.clone();
        long.insert(24, 0);
        assert_eq!(
            MeterReadingV1::parse(&long, device(0)),
            Err(MeterError::InvalidLength(26))
        );

        let mut wrong_function = frame.clone();
        wrong_function[6] = 0x00;
        assert_eq!(
            MeterReadingV1::parse(&wrong_function, device(0)),
            Err(MeterError::UnexpectedFunction(0x00))
        );

        assert_eq!(
            MeterReadingV1::parse(&synthetic(1, [0; 9]), device(0)),
            Err(MeterError::UnexpectedDevice {
                expected: 0,
                actual: 1,
            })
        );

        let mut wrong_model = frame.clone();
        wrong_model[5] = 0x0f;
        assert_eq!(
            MeterReadingV1::parse(&wrong_model, device(0)),
            Err(MeterError::Protocol(ProtocolError::WrongModel(0x0f)))
        );

        let mut high_bit = frame;
        high_bit[12] = 0x81;
        assert!(matches!(
            MeterReadingV1::parse(&high_bit, device(0)),
            Err(MeterError::Protocol(ProtocolError::NonSevenBitData { .. }))
        ));
    }

    #[test]
    fn out_of_domain_meter_bytes_fail_closed() {
        assert_eq!(
            MeterReadingV1::parse(&synthetic(0, [7, 0, 0, 0, 0, 0, 0, 0, 0]), device(0)),
            Err(MeterError::LevelOutOfRange {
                channel: "input A".to_owned(),
                level: 7,
                raw: 0x07,
            })
        );
        assert_eq!(
            MeterReadingV1::parse(&synthetic(0, [0, 0, 0, 0, 0, 0, 0, 0, 0x3f]), device(0)),
            Err(MeterError::LevelOutOfRange {
                channel: "output 6".to_owned(),
                level: 0x1f,
                raw: 0x3f,
            })
        );
        assert_eq!(
            MeterReadingV1::parse(&synthetic(0, [0, 0, 0, 0x41, 0, 0, 0, 0, 0]), device(0)),
            Err(MeterError::ReservedBit {
                channel: "output 1".to_owned(),
                raw: 0x41,
            })
        );
    }

    #[test]
    fn every_documented_byte_decodes_and_every_other_byte_fails() {
        for raw in 0_u8..=0x7f {
            let decoded = MeterValue::decode(MeterChannel::InputB, raw);
            let documented = raw & RESERVED_BIT == 0 && raw & LEVEL_MASK <= MAX_METER_LEVEL;
            assert_eq!(decoded.is_ok(), documented, "raw {raw:#04x}");
            if let Ok(value) = decoded {
                assert_eq!(value.level(), raw & LEVEL_MASK);
                assert_eq!(value.limiter(), raw & LIMITER_BIT != 0);
            }
        }
    }
}
