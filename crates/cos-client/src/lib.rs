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
pub mod transport;

pub use client::{
    ClientConfig, ClientEvent, ClientSource, CommandStatus, ConnectionState, CosClient,
    DesktopClient, SnapshotHandle, backoff_ms,
};
pub use error::ClientError;
pub use fixture::{
    CommandDefault, CommandPolicy, FixtureAction, FixtureDelta, FixtureError, FixtureReceipt,
    FixtureRuntime, FixtureScenario, FixtureState, FixtureStep, ObservationMode, SnapshotBundle,
    Trigger, parse_trigger,
};
pub use mock::{MockCosClient, MockTransport};
pub use projection::{ProjectionIntegrityError, ProjectionKey, ProjectionState};
pub use transport::{Transport, TransportError};
