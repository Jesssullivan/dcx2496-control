//! Pure, deterministic building blocks for DCX2496 control.
//!
//! This crate deliberately performs no serial, MIDI, filesystem, network, or
//! clock I/O. Callers must supply bytes, profiles, timestamps, and entropy.

pub mod discovery;
pub mod feedback;
pub mod layout;
pub mod peq_bank;
pub mod profile;
pub mod protocol;
pub mod rew;
pub mod routing;
pub mod snapshot;

pub use profile::{LabProfileV1, ProfileBinding, ProfileDiff, ProfileError};
pub use protocol::{
    DecodedMessage, DirectParameterAction, DirectParameterCommand, Message, ProtocolError, Query,
    RemoteMode, RemoteModeCommand, SearchResponse26,
};
pub use rew::{
    DirectPeqSlotPlanV1, RewImportReportV1, RewMappingError, RewParseError,
    map_filter_to_output_slot,
};
pub use snapshot::{
    ApplyPlanError, ApplyPlanV1, ApplyTransactionError, ApplyTransactionState, ApplyTransactionV1,
    ByteChangeV1, PlanCarrierError, ReadbackVerificationV1, RollbackPlanV1, SnapshotDiffError,
    SnapshotDiffV1, SnapshotError, SnapshotProjectionError, SnapshotSection, SnapshotSectionChange,
    SnapshotV1,
};
