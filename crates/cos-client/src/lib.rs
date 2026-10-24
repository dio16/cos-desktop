//! # cos-client
//!
//! Transport-independent COS client and explicit scripted fixture provider.
//!
//! The client owns atomic snapshot staging, delta validation, duplicate/gap
//! detection, bounded recovery and honest command receipts. It depends on
//! neither GPUI, UI model nor COS internals: consumers render
//! [`ProjectionState`] and drive the session through [`DesktopClient`].
//!
//! Fixture mode is explicit: [`MockCosClient`] reports
//! [`ClientSource::Fixture`] and never falls back from a real connection.

pub mod client;
pub mod error;
pub mod fixture;
pub mod mock;
pub mod projection;
pub mod read;
/// Development-only MP-PUB4 representation experiment (off by default; not a
/// product API and never consulted by product modules).
#[doc(hidden)]
pub mod representation;
pub mod transport;

pub use client::{
    ClientConfig, ClientEvent, ClientSource, CommandState, ConnectionState, CosClient,
    DesktopClient, PumpReport, SnapshotHandle, backoff_ms,
};
pub use desktop_protocol::ReceiptState;
pub use error::ClientError;
pub use fixture::{
    CommandDefault, CommandPolicy, FixtureAction, FixtureDelta, FixtureError,
    FixtureReadAuthorization, FixtureReceipt, FixtureReceiptOutcome, FixtureReceiptRead,
    FixtureRuntime, FixtureScenario, FixtureState, FixtureStep, ObservationMode, SnapshotBundle,
    Trigger, parse_trigger,
};
pub use mock::{MockControl, MockCosClient, MockTransport};
pub use projection::{ProjectionIntegrityError, ProjectionKey, ProjectionState};
pub use read::{
    CommandLookup, CommandStatus, MAX_READ_EVIDENCE, ObservationControl, PublicationAccess,
    ReadEvidence, ReadUnavailable, ReadableCommands, next_read_request_id, next_receipt_read_id,
};
pub use transport::{Transport, TransportError};
