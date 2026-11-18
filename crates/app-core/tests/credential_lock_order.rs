//! The credential-close callback must never run under the transport state lock.
//!
//! The owner closes the publication from inside its command gate
//! (`VisibilityFence::enter_command_boundary`), while a dispatch may hold that same
//! gate and then need the transport state lock. Holding the transport lock across
//! the callback is the ABBA deadlock this test fails on: the owner-side read below
//! would never return. The close must still complete before any result is exposed,
//! so the read is answered as a timeout, never with a document from the replaced
//! authorization context.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use app_core::fence::VisibilityFence;
use cos_client::{
    CATALOG_PATH, CredentialSlot, HttpMethod, HttpRequest, HttpResponse, ObservationControl,
    ProtocolHttp, ProtocolTransport, ProtocolTransportConfig, Transport,
};
use desktop_protocol::{ProtocolVersions, Version};

/// Minimal authenticated catalog document for the credential-binding route.
const CATALOG: &str = r#"{
    "schema_version": 1,
    "server": {"server_identity": "cos-runtime-orchestrator", "epoch": "epoch-1"},
    "catalog": {"schema_version": 1, "can_submit": true, "reason_codes": [], "recipes": []}
}"#;

struct CatalogStub;

impl ProtocolHttp for CatalogStub {
    fn execute(&mut self, request: HttpRequest) -> Result<HttpResponse, String> {
        if request.method == HttpMethod::Get && request.path == CATALOG_PATH {
            return Ok(HttpResponse {
                status: 200,
                body: CATALOG.as_bytes().to_vec(),
            });
        }
        Err(format!("unexpected route {}", request.path))
    }
}

fn versions() -> ProtocolVersions {
    ProtocolVersions {
        protocol: Version::new(2, 0),
        schema: Version::new(2, 0),
    }
}

#[test]
fn the_credential_close_never_runs_under_the_transport_state_lock() {
    let slot: CredentialSlot = Arc::new(Mutex::new(Some("first-token".to_owned())));
    let config = ProtocolTransportConfig {
        credential_slot: Some(Arc::clone(&slot)),
        ..ProtocolTransportConfig::default()
    };
    let mut transport = ProtocolTransport::new(CatalogStub, config, versions());

    // The real owner boundary: gate first, publication close second.
    let fence = VisibilityFence::new();
    {
        let fence = Arc::clone(&fence);
        transport.bind_observation_control(ObservationControl {
            wake: std::task::Waker::noop().clone(),
            close_visibility: Arc::new(move || {
                let _gate = fence.enter_command_boundary();
                fence.close_visibility()
            }),
        });
    }
    let epoch_before = fence.authorization_epoch();

    // The owner holds its command gate while a bounded reauthorization replaces the
    // credential; an owner-side read performed under that gate needs the transport
    // state lock, exactly as command dispatch does.
    let gate = fence.enter_command_boundary();
    *slot.lock().expect("credential slot") = Some("second-token".to_owned());

    let (result_tx, result_rx) = mpsc::channel();
    let owner_read = thread::spawn(move || {
        let outcome = transport.fetch_catalog();
        result_tx.send((outcome, transport)).ok();
    });

    let (outcome, mut transport) = result_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the owner-side read returns instead of deadlocking on the transport lock");
    assert!(
        outcome.is_err(),
        "the replaced context never delivers a catalog document to the consumer"
    );

    drop(gate);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && fence.authorization_epoch() == epoch_before {
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        fence.authorization_epoch() > epoch_before,
        "the credential close completes once the owner releases its gate"
    );

    // The worker is not stuck: the next read over the replacement credential is
    // answered, and it is asked to close only for a further replacement.
    let document = transport.fetch_catalog().expect("catalog after the close");
    assert_eq!(document.server.server_identity, "cos-runtime-orchestrator");
    assert_eq!(
        fence.authorization_epoch(),
        epoch_before + 1,
        "one replacement produces one close"
    );
    owner_read.join().expect("the owner read thread ends");
}
