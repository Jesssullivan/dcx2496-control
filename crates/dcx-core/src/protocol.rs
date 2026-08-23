//! Bounded parsing and construction for the DCX2496 SysEx-shaped protocol.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Maximum accepted frame size. The largest currently described dump is 1015
/// bytes, so this leaves bounded room for future observed variants.
pub const MAX_FRAME_LEN: usize = 2_048;
/// `SysEx` start byte.
pub const START: u8 = 0xf0;
/// `SysEx` terminator byte.
pub const END: u8 = 0xf7;
const MANUFACTURER: [u8; 3] = [0x00, 0x20, 0x32];
const MODEL: u8 = 0x0e;
const BROADCAST_SEARCH: u8 = 0x20;

/// A physical unit address. Valid device IDs are 0 through 15.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct DeviceId(u8);

impl DeviceId {
    /// Construct a checked device ID.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::InvalidDeviceId`] for values above 15.
    pub fn new(value: u8) -> Result<Self, ProtocolError> {
        if value <= 0x0f {
            Ok(Self(value))
        } else {
            Err(ProtocolError::InvalidDeviceId(value))
        }
    }

    /// Return the on-wire numeric ID.
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for DeviceId {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<DeviceId> for u8 {
    fn from(value: DeviceId) -> Self {
        value.get()
    }
}

/// A decoded protocol address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Address {
    /// Address of one unit.
    Device(DeviceId),
    /// Broadcast address accepted only by the discovery request.
    BroadcastSearch,
}

impl Address {
    const fn wire(self) -> u8 {
        match self {
            Self::Device(id) => id.get(),
            Self::BroadcastSearch => BROADCAST_SEARCH,
        }
    }
}

/// A syntactically valid bounded frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    /// Target or source address.
    address: Address,
    /// Raw seven-bit function identifier.
    function: u8,
    /// Raw seven-bit payload.
    data: Vec<u8>,
}

impl Message {
    /// Return the parsed source address.
    pub const fn address(&self) -> Address {
        self.address
    }

    /// Return the parsed function identifier.
    pub const fn function(&self) -> u8 {
        self.function
    }

    /// Return the parsed payload.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    // Deliberately private: only the typed, read-only Query API may construct
    // outbound bytes. Message exists solely as a bounded inbound parse result.
    fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        if self.function > 0x7f {
            return Err(ProtocolError::NonSevenBitData {
                index: 6,
                value: self.function,
            });
        }
        for (offset, value) in self.data.iter().copied().enumerate() {
            if value > 0x7f {
                return Err(ProtocolError::NonSevenBitData {
                    index: offset + 7,
                    value,
                });
            }
        }
        let frame_len = self.data.len() + 8;
        if frame_len > MAX_FRAME_LEN {
            return Err(ProtocolError::FrameTooLong(frame_len));
        }

        let mut bytes = Vec::with_capacity(frame_len);
        bytes.push(START);
        bytes.extend_from_slice(&MANUFACTURER);
        bytes.push(self.address.wire());
        bytes.push(MODEL);
        bytes.push(self.function);
        bytes.extend_from_slice(&self.data);
        bytes.push(END);
        Ok(bytes)
    }
}

/// Read-only requests that can be constructed by this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query {
    /// Broadcast discovery query.
    Search,
    /// Liveness/identity query for one device.
    Ping(DeviceId),
    /// Request one of the two described state dump parts.
    Dump { device: DeviceId, part: DumpPart },
}

/// One of the two state dump segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DumpPart {
    /// First segment.
    Part0,
    /// Second segment.
    Part1,
}

impl DumpPart {
    const fn wire(self) -> u8 {
        match self {
            Self::Part0 => 0,
            Self::Part1 => 1,
        }
    }
}

impl Query {
    /// Build the exact query frame. No direct-parameter or unmute constructor
    /// is exposed by this API.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError`] if the resulting bounded message cannot be
    /// represented, which is an invariant failure for these fixed queries.
    pub fn encode(self) -> Result<Vec<u8>, ProtocolError> {
        let message = match self {
            Self::Search => Message {
                address: Address::BroadcastSearch,
                function: 0x40,
                data: Vec::new(),
            },
            Self::Ping(device) => Message {
                address: Address::Device(device),
                function: 0x44,
                data: vec![0, 0],
            },
            Self::Dump { device, part } => Message {
                address: Address::Device(device),
                function: 0x50,
                data: vec![1, 0, part.wire()],
            },
        };
        message.encode()
    }
}

/// A four-byte direct-parameter tuple observed in inbound messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterChange {
    /// Setup/input/output channel index, 0 through 10.
    pub channel: u8,
    /// Seven-bit parameter number.
    pub parameter: u8,
    /// Fourteen-bit parameter value.
    pub value: u16,
}

/// Semantic classification of a valid frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecodedMessage {
    /// A discovery response. Payload interpretation awaits hardware evidence.
    SearchResponse { device: DeviceId, payload: Vec<u8> },
    /// A ping response. Payload interpretation awaits hardware evidence.
    PingResponse { device: DeviceId, payload: Vec<u8> },
    /// One state dump segment.
    DumpResponse {
        device: DeviceId,
        part: DumpPart,
        payload: Vec<u8>,
    },
    /// One or more direct parameter updates received from a unit.
    DirectParameters {
        device: DeviceId,
        changes: Vec<ParameterChange>,
    },
    /// A valid but not yet modeled function.
    Unknown(Message),
}

/// Parse one complete frame.
///
/// # Errors
///
/// Returns [`ProtocolError`] for an invalid envelope, address, seven-bit byte,
/// or size bound.
pub fn parse_frame(frame: &[u8]) -> Result<Message, ProtocolError> {
    if frame.len() < 8 {
        return Err(ProtocolError::FrameTooShort(frame.len()));
    }
    if frame.len() > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLong(frame.len()));
    }
    if frame[0] != START {
        return Err(ProtocolError::MissingStart(frame[0]));
    }
    if frame[frame.len() - 1] != END {
        return Err(ProtocolError::MissingTerminator(frame[frame.len() - 1]));
    }
    if frame[1..4] != MANUFACTURER {
        return Err(ProtocolError::WrongManufacturer([
            frame[1], frame[2], frame[3],
        ]));
    }
    if frame[5] != MODEL {
        return Err(ProtocolError::WrongModel(frame[5]));
    }
    for (index, value) in frame[1..frame.len() - 1].iter().copied().enumerate() {
        if value > 0x7f {
            return Err(ProtocolError::NonSevenBitData {
                index: index + 1,
                value,
            });
        }
    }

    let address = if frame[4] == BROADCAST_SEARCH && frame[6] == 0x40 {
        Address::BroadcastSearch
    } else {
        Address::Device(DeviceId::new(frame[4])?)
    };
    Ok(Message {
        address,
        function: frame[6],
        data: frame[7..frame.len() - 1].to_vec(),
    })
}

/// Decode a syntactically valid message into a conservative semantic form.
///
/// # Errors
///
/// Returns [`ProtocolError`] when a modeled payload is structurally invalid.
pub fn decode(message: Message) -> Result<DecodedMessage, ProtocolError> {
    let device = match message.address {
        Address::Device(device) => device,
        Address::BroadcastSearch => return Ok(DecodedMessage::Unknown(message)),
    };

    match message.function {
        0x00 => Ok(DecodedMessage::SearchResponse {
            device,
            payload: message.data,
        }),
        0x04 => Ok(DecodedMessage::PingResponse {
            device,
            payload: message.data,
        }),
        0x10 => {
            if message.data.len() < 6 {
                return Err(ProtocolError::MalformedDump(message.data.len()));
            }
            let part = match message.data[5] {
                0 => DumpPart::Part0,
                1 => DumpPart::Part1,
                other => return Err(ProtocolError::InvalidDumpPart(other)),
            };
            Ok(DecodedMessage::DumpResponse {
                device,
                part,
                payload: message.data,
            })
        }
        0x20 => decode_direct(device, &message.data),
        _ => Ok(DecodedMessage::Unknown(message)),
    }
}

fn decode_direct(device: DeviceId, data: &[u8]) -> Result<DecodedMessage, ProtocolError> {
    let Some((&count, tuples)) = data.split_first() else {
        return Err(ProtocolError::MalformedDirect {
            declared: 0,
            actual_bytes: 0,
        });
    };
    let expected = usize::from(count) * 4;
    if tuples.len() != expected {
        return Err(ProtocolError::MalformedDirect {
            declared: count,
            actual_bytes: tuples.len(),
        });
    }
    let mut changes = Vec::with_capacity(usize::from(count));
    let (tuples, remainder) = tuples.as_chunks::<4>();
    debug_assert!(remainder.is_empty());
    for tuple in tuples {
        if tuple[0] > 10 {
            return Err(ProtocolError::InvalidParameterChannel(tuple[0]));
        }
        changes.push(ParameterChange {
            channel: tuple[0],
            parameter: tuple[1],
            value: u16::from(tuple[2]) * 128 + u16::from(tuple[3]),
        });
    }
    Ok(DecodedMessage::DirectParameters { device, changes })
}

/// Incremental bounded frame decoder for arbitrary byte streams.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
    collecting: bool,
}

impl FrameDecoder {
    /// Feed one byte. Noise before a start byte is discarded. A nested start
    /// byte resynchronizes to the newest candidate frame.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError`] if a candidate frame exceeds the hard bound
    /// or terminates with an invalid envelope.
    pub fn push(&mut self, byte: u8) -> Result<Option<Message>, ProtocolError> {
        if byte == START {
            self.buffer.clear();
            self.buffer.push(byte);
            self.collecting = true;
            return Ok(None);
        }
        if !self.collecting {
            return Ok(None);
        }
        self.buffer.push(byte);
        if self.buffer.len() > MAX_FRAME_LEN {
            self.buffer.clear();
            self.collecting = false;
            return Err(ProtocolError::FrameTooLong(MAX_FRAME_LEN + 1));
        }
        if byte == END {
            self.collecting = false;
            return parse_frame(&self.buffer).map(Some);
        }
        Ok(None)
    }
}

/// Protocol validation failures.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// Frame shorter than the fixed envelope.
    #[error("frame is too short: {0} bytes")]
    FrameTooShort(usize),
    /// Frame exceeded the parser's hard bound.
    #[error("frame is too long: {0} bytes")]
    FrameTooLong(usize),
    /// First byte was not `0xf0`.
    #[error("frame does not start with 0xf0: found {0:#04x}")]
    MissingStart(u8),
    /// Final byte was not `0xf7`.
    #[error("frame does not end with 0xf7: found {0:#04x}")]
    MissingTerminator(u8),
    /// Manufacturer tuple was not `00 20 32`.
    #[error("unexpected manufacturer bytes: {0:02x?}")]
    WrongManufacturer([u8; 3]),
    /// Product/model byte was not `0x0e`.
    #[error("unexpected model byte: {0:#04x}")]
    WrongModel(u8),
    /// Interior bytes must fit seven bits.
    #[error("non-seven-bit byte at index {index}: {value:#04x}")]
    NonSevenBitData { index: usize, value: u8 },
    /// Device IDs are limited to 0 through 15.
    #[error("invalid device id: {0}")]
    InvalidDeviceId(u8),
    /// Dump payload was too short to contain the described part field.
    #[error("malformed dump payload: {0} bytes")]
    MalformedDump(usize),
    /// Dump part must be zero or one.
    #[error("invalid dump part: {0}")]
    InvalidDumpPart(u8),
    /// Direct-parameter tuple count and payload disagreed.
    #[error("direct message declares {declared} tuples but has {actual_bytes} tuple bytes")]
    MalformedDirect { declared: u8, actual_bytes: usize },
    /// Parameter channel was outside setup/input/output range.
    #[error("invalid parameter channel: {0}")]
    InvalidParameterChannel(u8),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_safe_queries() {
        assert_eq!(
            Query::Search.encode().unwrap(),
            [0xf0, 0x00, 0x20, 0x32, 0x20, 0x0e, 0x40, 0xf7]
        );
        let id = DeviceId::new(3).unwrap();
        assert_eq!(
            Query::Ping(id).encode().unwrap(),
            [0xf0, 0, 0x20, 0x32, 3, 0x0e, 0x44, 0, 0, 0xf7]
        );
        assert_eq!(
            Query::Dump {
                device: id,
                part: DumpPart::Part1
            }
            .encode()
            .unwrap(),
            [0xf0, 0, 0x20, 0x32, 3, 0x0e, 0x50, 1, 0, 1, 0xf7]
        );
    }

    #[test]
    fn direct_tuple_is_checked_and_decoded() {
        let message = Message {
            address: Address::Device(DeviceId::new(0).unwrap()),
            function: 0x20,
            data: vec![1, 1, 2, 1, 82],
        };
        assert_eq!(
            decode(message).unwrap(),
            DecodedMessage::DirectParameters {
                device: DeviceId::new(0).unwrap(),
                changes: vec![ParameterChange {
                    channel: 1,
                    parameter: 2,
                    value: 210,
                }],
            }
        );
    }

    #[test]
    fn stream_resynchronizes_at_nested_start() {
        let mut decoder = FrameDecoder::default();
        let mut got = None;
        for byte in [
            0xaa, 0xf0, 1, 2, 0xf0, 0, 0x20, 0x32, 0x20, 0x0e, 0x40, 0xf7,
        ] {
            got = decoder.push(byte).unwrap().or(got);
        }
        assert_eq!(got.unwrap().function(), 0x40);
    }
}
