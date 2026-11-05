//! The two different things "stale" can mean, and which one a disconnect is.
//!
//! They are easy to conflate and the difference decides what a user sees:
//!
//! * a projection whose **authorization binding is gone** must be **withheld** - the desktop may not
//!   show data it is no longer authorized to read, so the disconnect path publishes no snapshot at
//!   all (`a01` in `observation_publication.rs`).
//! * a publication that is still authorized but **older than the current revision** keeps its
//!   snapshot and is marked stale; a delayed read from it is rejected when it tries to derive or
//!   render (`a13`).
//!
//! `§19.275` described the second case as if it were the first. This file pins the distinction so the
//! plan and the tests cannot drift apart again.

use app_core::observation::{ObservationPublication, PublicationConnection};
use cos_client::ProjectionState;
use std::sync::Arc;

#[test]
fn a_publication_can_be_stale_and_still_hold_data_while_authorized() {
    // Authorized but superseded: the snapshot stays, `stale` says it is not current, and the
    // connection is not live. This is the case where withholding would lose information for no
    // authorization reason.
    let snapshot = Arc::new(ProjectionState::default());
    let stale_but_authorized = ObservationPublication {
        snapshot: Some(Arc::clone(&snapshot)),
        revision: None,
        connection: PublicationConnection::Recovering,
        source: app_core::observation::PublicationSource::Runtime,
        stale: true,
        ..ObservationPublication::default()
    };
    assert!(stale_but_authorized.stale);
    let carried = stale_but_authorized
        .snapshot
        .as_ref()
        .expect("an authorized publication keeps its snapshot");
    assert!(Arc::ptr_eq(carried, &snapshot));
}

#[test]
fn withholding_is_expressed_by_an_absent_snapshot_not_by_a_stale_flag() {
    // The fail-closed case: no authorization, so no snapshot and no command data - and `stale` is
    // not what carries that meaning. A reader that only checked `stale` would show an empty
    // projection as if it were merely old, which is the wrong story to tell.
    let withheld = ObservationPublication {
        snapshot: None,
        revision: None,
        connection: PublicationConnection::Disconnected,
        source: app_core::observation::PublicationSource::Unavailable,
        stale: true,
        ..ObservationPublication::default()
    };
    assert!(withheld.snapshot.is_none());
    assert!(withheld.command_statuses.is_empty());
    assert_eq!(withheld.connection, PublicationConnection::Disconnected);
    // Both cases are `stale`, so `stale` alone cannot distinguish them; the snapshot's presence and
    // the connection category do.
    assert!(withheld.stale);
}
