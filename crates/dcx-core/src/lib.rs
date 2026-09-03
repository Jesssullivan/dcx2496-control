//! Pure, deterministic building blocks for DCX2496 control.
//!
//! This crate deliberately performs no serial, MIDI, filesystem, network, or
//! clock I/O. Callers must supply bytes, profiles, timestamps, and entropy.

pub mod discovery;
pub mod profile;
pub mod protocol;
pub mod rew;
pub mod state_machine;

pub use profile::{LabProfileV1, ProfileBinding, ProfileDiff, ProfileError};
pub use protocol::{DecodedMessage, Message, ProtocolError, Query, SearchResponse26};
pub use rew::{RewImportReportV1, RewParseError};
pub use state_machine::{ControllerMachine, ControllerState, Event, TransitionError};
