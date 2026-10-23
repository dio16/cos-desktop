//! MP-PUB1 acceptance for registry bounds (A08, A15) at the client's public seam.
//!
//! Every boundary is driven through a scripted fixture transcript: the expected
//! values come from the contract numbers, never from the production gate.

use cos_client::{ClientError, CommandLookup, CommandState, MockCosClient, ReceiptState};
use desktop_protocol::{
    AuthorizedCommandReceipt, CommandEnvelope, CommandId, CommandPayload, CommandReadDecision,
    CommandReceipt, CommandTarget, StopTaskCommand, TaskId,
};

const FIXTURE: &str = include_str!("../fixtures/task-184.json");

fn task() -> TaskId {
    TaskId::fixture("task/Task-184")
}

fn envelope(index: usize) -> CommandEnvelope {
    CommandEnvelope {
        command_id: CommandId::fixture(format!("command/ordinary-{index}")),
        target: CommandTarget::Task { task_id: task() },
        expected_revision: None,
        epoch: None,
        payload: CommandPayload::StopTask(StopTaskCommand {
            task_id: task(),
            reason: None,
        }),
    }
}

fn live() -> MockCosClient {
    let mut client = MockCosClient::from_fixture_str(FIXTURE).expect("fixture loads");
    client.connect(0).expect("connect");
    for _ in 0..4 {
        client.pump(0);
        if client.connection().is_live() {
            break;
        }
    }
    assert!(client.connection().is_live(), "fixture session is live");
    client
}

fn retained_state(client: &MockCosClient, id: &CommandId) -> Option<CommandState> {
    match client.lookup_command(id) {
        CommandLookup::Found(record) => Some(record.state().clone()),
        CommandLookup::NotRetained => None,
    }
}

/// Whether this state is one of the contract's ordinary unsettled classes.
///
/// `CommandStatus::is_settled()` is deliberately not the oracle here: the
/// contract forbids it for this classification because
/// Accepted-not-canonically-reflected is capacity-unsettled even though
/// `is_settled` reports it settled.
fn is_capacity_unsettled(state: &CommandState) -> bool {
    matches!(
        state,
        CommandState::Prepared
            | CommandState::Pending { .. }
            | CommandState::Accepted(_)
            | CommandState::ResultUnknown { .. }
    )
}

/// Number of retained records occupying an unsettled capacity class.
fn unsettled_count(client: &MockCosClient) -> usize {
    client
        .command_statuses()
        .values()
        .filter(|record| is_capacity_unsettled(record.state()))
        .count()
}

fn tombstone_count(client: &MockCosClient) -> usize {
    client
        .command_statuses()
        .values()
        .filter(|record| {
            matches!(
                record.state(),
                CommandState::ResultUnknown { reason, .. } if reason == "CommandReceiptTombstone"
            )
        })
        .count()
}

fn inject_receipt(client: &MockCosClient, id: &CommandId, state: ReceiptState, readable: bool) {
    client
        .control()
        .inject_message(desktop_protocol::ServerMessage::Receipt(
            AuthorizedCommandReceipt {
                stamp: client
                    .control()
                    .active_stamp()
                    .expect("fixture binding is open"),
                permission: CommandReadDecision {
                    command_id: id.clone(),
                    readable,
                },
                receipt: CommandReceipt {
                    command_id: id.clone(),
                    operation: desktop_protocol::CommandOperation::StopTask,
                    state,
                    task_id: Some(task()),
                    accepted_at: None,
                    reject_reason: None,
                    effect_refs: Vec::new(),
                    dedupe_expires_at: None,
                    message: None,
                },
            },
        ));
}

#[test]
fn b18_a_receipt_read_result_with_a_conflicting_envelope_stamp_is_refused() {
    // The transcript leaves the lookup unanswered, so the reply is supplied here.
    let fixture = FIXTURE.replace(
        "\"grant_on_open\": true",
        "\"grant_on_open\": true, \"answer_lookups\": false",
    );
    let mut client = MockCosClient::from_fixture_str(&fixture).expect("fixture loads");
    client.connect(0).expect("connect");
    for _ in 0..6 {
        client.pump(0);
    }
    assert!(client.connection().is_live(), "fixture session is live");

    let binding = client.control().active_stamp().expect("binding");
    let stale = desktop_protocol::ReadStamp {
        read_revision: desktop_protocol::ReadRevision::new(
            desktop_protocol::Counter::new("7").expect("counter"),
        ),
        ..binding.clone()
    };
    let id = CommandId::fixture("command/read-result");
    let reply = |stamp: desktop_protocol::ReadStamp| desktop_protocol::ReceiptReadResult {
        lookup_id: desktop_protocol::ReceiptReadId::new("fixture:lookup/1").expect("lookup id"),
        stamp,
        command_id: id.clone(),
        outcome: desktop_protocol::ReceiptReadOutcome::Allowed {
            receipt: Some(CommandReceipt {
                command_id: id.clone(),
                operation: desktop_protocol::CommandOperation::StopTask,
                state: ReceiptState::Accepted,
                task_id: Some(task()),
                accepted_at: None,
                reject_reason: None,
                effect_refs: Vec::new(),
                dedupe_expires_at: None,
                message: None,
            }),
        },
    };

    // Envelope stamp stale, payload stamp current: the contradiction is invalid.
    client
        .lookup_receipt_authorized(desktop_protocol::AuthorizedReceiptLookup {
            lookup_id: desktop_protocol::ReceiptReadId::new("fixture:lookup/1").expect("lookup id"),
            stamp: binding.clone(),
            command_id: id.clone(),
        })
        .expect("lookup is sent");
    for _ in 0..4 {
        client.pump(0);
    }
    let pending = client.lookup_receipt_authorized(desktop_protocol::AuthorizedReceiptLookup {
        lookup_id: desktop_protocol::ReceiptReadId::new("fixture:lookup/2").expect("lookup id"),
        stamp: binding.clone(),
        command_id: id.clone(),
    });
    assert!(pending.is_ok(), "the identity stays pending: {pending:?}");

    client.control().inject_message_with_stamp(
        desktop_protocol::ServerMessage::ReceiptReadResult(reply(binding.clone())),
        stale.clone(),
    );
    client.pump(0);
    assert!(
        client
            .last_error()
            .is_some_and(|error| format!("{error:?}").contains("conflicting envelope stamp")),
        "the contradictory envelope stamp is refused: {:?}",
        client.last_error()
    );
    assert_eq!(
        retained_state(&client, &id),
        None,
        "no receipt is applied from a frame whose envelope and payload stamps disagree"
    );
}

#[test]
fn a10_an_explicitly_denied_identity_is_not_queried_again_automatically() {
    let mut client = live();
    // A server-derived, unsettled identity that a later binding explicitly denies:
    // the denial is a settled answer, so the automatic drain never spends a lookup.
    let denied = CommandId::fixture("command/denied-auto");
    inject_receipt(&client, &denied, ReceiptState::Accepted, true);
    client.pump(0);
    inject_receipt(
        &client,
        &denied,
        ReceiptState::Unknown("FUTURE_STATE".to_owned()),
        true,
    );
    client.pump(0);
    assert!(
        matches!(
            retained_state(&client, &denied),
            Some(CommandState::ResultUnknown { .. })
        ),
        "the identity stays server-derived and unsettled"
    );

    client.control().deny_command(denied.as_str().to_owned());
    for _ in 0..2 {
        client.pump(0);
    }
    assert!(
        client.client().transport().read_denies(&denied),
        "the fixture context denies this identity"
    );
    // Rebind with the current registry: the new grant denies the identity.
    let lookups_before = client.control().lookups_seen();
    client.connect(0).expect("rebind");
    for _ in 0..6 {
        client.pump(0);
    }
    assert!(client.connection().is_live(), "the rebound session is live");
    let binding = client.control().active_stamp().expect("binding");
    assert!(
        matches!(
            client.lookup_command(&denied),
            CommandLookup::Found(record) if record.permission(&binding) == Some(false)
        ),
        "the new binding explicitly denies this identity"
    );
    assert_eq!(
        client.control().lookups_seen(),
        lookups_before,
        "a grant-level denial is a settled answer, so no lookup is spent on it"
    );

    let settled_frames = client.control().sent_frames();
    for _ in 0..12 {
        client.pump(0);
    }
    assert_eq!(
        client.control().sent_frames(),
        settled_frames,
        "the denied identity stays unqueried"
    );
}

#[test]
fn a15_ordinary_capacity_is_63_64_65() {
    let mut client = live();
    for index in 0..64 {
        client
            .client_mut()
            .prepare_command(envelope(index))
            .unwrap_or_else(|error| panic!("ordinary slot {index} must fit: {error:?}"));
    }
    let overflow = client.client_mut().prepare_command(envelope(64));
    assert!(
        matches!(&overflow, Err(ClientError::InvalidCommand(message)) if message == "CommandCapacityExceeded"),
        "the 65th ordinary intent is rejected before preparation: {overflow:?}"
    );
    assert_eq!(
        unsettled_count(&client),
        64,
        "a rejected intent never borrows a slot"
    );
    assert!(
        !client
            .command_statuses()
            .contains_key(&CommandId::fixture("command/ordinary-64")),
        "a rejected intent is not retained"
    );
}

#[test]
fn a15_tombstones_are_63_64_65_and_the_combined_maximum_is_128() {
    let mut client = live();
    for index in 0..64 {
        client
            .client_mut()
            .prepare_command(envelope(index))
            .expect("ordinary slot");
    }
    for index in 0..64 {
        let id = CommandId::fixture(format!("command/tombstone-{index}"));
        inject_receipt(
            &client,
            &id,
            ReceiptState::Unknown("FUTURE_STATE".to_owned()),
            true,
        );
        client.pump(0);
        let state = retained_state(&client, &id);
        assert!(
            matches!(&state, Some(CommandState::ResultUnknown { reason, .. }) if reason == "CommandReceiptTombstone"),
            "an untracked unsettled receipt with no ordinary slot becomes an actual tombstone"
        );
    }
    let extra = CommandId::fixture("command/tombstone-64");
    inject_receipt(
        &client,
        &extra,
        ReceiptState::Unknown("FUTURE_STATE".to_owned()),
        true,
    );
    client.pump(0);
    assert_eq!(
        retained_state(&client, &extra),
        None,
        "at both caps the tombstone is refused without overwriting tracked state"
    );
    assert!(
        client
            .last_error()
            .is_some_and(|error| matches!(error, ClientError::InvalidCommand(message) if message == "CommandCapacityExceeded")),
        "the refusal is the existing capacity error"
    );
    assert_eq!(
        unsettled_count(&client),
        128,
        "64 ordinary plus 64 tombstone slots are occupied, the 129th is refused"
    );
}

#[test]
fn a15_settled_retention_evicts_only_zero_reference_oldest_first() {
    let mut client = live();
    let referenced = CommandId::fixture("command/settled-referenced");
    inject_receipt(&client, &referenced, ReceiptState::Rejected, true);
    client.pump(0);
    client
        .client_mut()
        .retain_command(&referenced)
        .expect("retain");

    // 256 further settled records fill the bounded settled window.
    for index in 0..256 {
        let id = CommandId::fixture(format!("command/settled-{index}"));
        inject_receipt(&client, &id, ReceiptState::Rejected, true);
        client.pump(0);
    }
    // The 257th settled insertion evicts the oldest zero-reference record.
    let newest = CommandId::fixture("command/settled-newest");
    inject_receipt(&client, &newest, ReceiptState::Rejected, true);
    client.pump(0);
    assert!(
        retained_state(&client, &newest).is_some(),
        "the newest settled record is retained"
    );
    assert!(
        retained_state(&client, &referenced).is_some(),
        "a referenced settled record is never evicted"
    );
    let oldest = CommandId::fixture("command/settled-0");
    assert_eq!(
        retained_state(&client, &oldest),
        None,
        "the deterministic oldest zero-reference settled record is evicted first"
    );
}

#[test]
fn a15_references_are_63_64_65_and_capacity_recovers_only_genuinely() {
    let mut client = live();
    let id = CommandId::fixture("command/reference-target");
    client
        .client_mut()
        .prepare_command(CommandEnvelope {
            command_id: id.clone(),
            ..envelope(0)
        })
        .expect("prepare");
    for reference in 0..64 {
        client
            .client_mut()
            .retain_command(&id)
            .unwrap_or_else(|error| panic!("reference {reference} must fit: {error:?}"));
    }
    let overflow = client.client_mut().retain_command(&id);
    assert!(
        matches!(&overflow, Err(ClientError::InvalidCommand(message)) if message == "CommandReferenceLimitExceeded"),
        "the 65th reference is refused: {overflow:?}"
    );
    for _ in 0..64 {
        client.client_mut().release_command(&id);
    }

    // Fill the ordinary capacity, then recover it by genuine settlement only.
    for index in 0..63 {
        client
            .client_mut()
            .prepare_command(envelope(index + 100))
            .expect("ordinary slot");
    }
    let blocked = client.client_mut().prepare_command(envelope(999));
    assert!(blocked.is_err(), "the ordinary capacity is exhausted");

    // Filtering (a denied read) must not free capacity.
    client.control().deny_command(id.as_str().to_owned());
    client.pump(0);
    let still_blocked = client.client_mut().prepare_command(envelope(998));
    assert!(
        still_blocked.is_err(),
        "read filtering never releases command capacity"
    );

    // A genuine settlement releases the ordinary slot it occupied.
    let settled = CommandId::fixture("command/ordinary-100");
    inject_receipt(&client, &settled, ReceiptState::CanonicalReflected, true);
    client.pump(0);
    let recovered = client.client_mut().prepare_command(envelope(997));
    assert!(
        recovered.is_ok(),
        "genuine settlement recovers ordinary capacity: {recovered:?}"
    );
}

#[test]
fn b06_identity_read_evidence_is_bounded() {
    let mut client = live();
    let stamp = client.control().active_stamp().expect("binding");
    let absent = CommandId::fixture("command/absent");
    client.control().deny_command(absent.as_str().to_owned());

    // Distinct denied identities accumulate evidence; the bounded set must keep
    // only the most recent entries, never grow with the request count.
    for index in 0..700 {
        let id = CommandId::fixture(format!("command/denied-{index}"));
        client.control().deny_command(id.as_str().to_owned());
        client
            .lookup_receipt_authorized(desktop_protocol::AuthorizedReceiptLookup {
                lookup_id: desktop_protocol::ReceiptReadId::new(format!(
                    "receipt-read/bounded-{index}"
                ))
                .expect("valid lookup identity"),
                stamp: stamp.clone(),
                command_id: id,
            })
            .expect("lookup is admitted");
        client.pump(0);
        client.pump(0);
    }
    let evidence = client.read_evidence_len();
    assert!(
        evidence <= cos_client::MAX_READ_EVIDENCE,
        "identity read evidence is bounded: {evidence}"
    );
    assert!(evidence > 0, "recent evidence is retained");
    // The most recent identity keeps its decision, the oldest was evicted.
    assert!(client.read_evidence_has(&CommandId::fixture("command/denied-699")));
    assert!(!client.read_evidence_has(&CommandId::fixture("command/denied-0")));
}

#[test]
fn b15_a_grant_whose_stamp_names_another_scope_never_opens_a_binding() {
    // The transcript's grant is valid in every respect except the stamp's scope
    // identity, so only the stamp/scope contradiction can refuse it.
    let mut client = MockCosClient::from_fixture_str(FIXTURE).expect("fixture loads");
    client.control().grant_stamp_scope("scope/other");
    client.connect(0).expect("connect");
    for _ in 0..6 {
        client.pump(0);
    }
    assert!(
        !client.connection().is_live(),
        "a grant whose stamp names another scope identity must not open a binding"
    );
    assert!(
        matches!(
            client.publication_access(),
            cos_client::PublicationAccess::Withheld { .. }
        ),
        "no data is admitted without a consistent grant"
    );
    assert!(
        client.last_error().is_some_and(
            |error| matches!(error, ClientError::Protocol(message) if message.contains("stamp"))
        ),
        "the refusal names the stamp contradiction: {:?}",
        client.last_error()
    );
}

#[test]
fn b15_a_receipt_with_another_authorization_stamp_changes_no_state() {
    // The outer frame stamp is the current binding, the receipt's own stamp is not:
    // state must not change under a stale authorization context.
    let mut client = live();
    let binding = client.control().active_stamp().expect("binding");
    let stale = desktop_protocol::ReadStamp {
        read_revision: desktop_protocol::ReadRevision::new(
            desktop_protocol::Counter::new("7").expect("counter"),
        ),
        ..binding.clone()
    };
    let id = CommandId::fixture("command/stale-authorization");
    client.control().inject_message_with_stamp(
        desktop_protocol::ServerMessage::Receipt(AuthorizedCommandReceipt {
            stamp: stale,
            permission: CommandReadDecision {
                command_id: id.clone(),
                readable: true,
            },
            receipt: CommandReceipt {
                command_id: id.clone(),
                operation: desktop_protocol::CommandOperation::StopTask,
                state: ReceiptState::Accepted,
                task_id: Some(task()),
                accepted_at: None,
                reject_reason: None,
                effect_refs: Vec::new(),
                dedupe_expires_at: None,
                message: None,
            },
        }),
        binding,
    );
    client.pump(0);
    assert_eq!(
        retained_state(&client, &id),
        None,
        "a receipt carrying another authorization stamp is not applied"
    );
    assert!(
        client
            .last_error()
            .is_some_and(|error| format!("{error:?}").contains("stale authorization stamp")),
        "the refusal names the stale authorization stamp: {:?}",
        client.last_error()
    );
}

#[test]
fn b16_settled_records_keep_distinct_settlement_sequences() {
    // Arrival order is the reverse of the identity order, so eviction can only
    // follow the settlement sequence the client assigned.
    let mut client = live();
    for index in (0..257).rev() {
        let id = CommandId::fixture(format!("command/sequence-{index:03}"));
        inject_receipt(&client, &id, ReceiptState::Rejected, true);
        client.pump(0);
    }
    assert_eq!(
        retained_state(&client, &CommandId::fixture("command/sequence-256")),
        None,
        "the first settled record is evicted first, not the lowest identity"
    );
    assert!(
        retained_state(&client, &CommandId::fixture("command/sequence-255")).is_some(),
        "the following settled record survives: eviction followed settlement order"
    );
    assert!(
        retained_state(&client, &CommandId::fixture("command/sequence-000")).is_some(),
        "the last settled record is still retained inside the cap"
    );
}

#[test]
fn b17_an_untracked_acknowledgement_falls_back_to_a_tombstone_slot() {
    let mut client = live();
    for index in 0..64 {
        client
            .client_mut()
            .prepare_command(envelope(index))
            .expect("ordinary slot");
    }
    assert_eq!(
        unsettled_count(&client),
        64,
        "64 unsettled ordinary intents occupy the ordinary capacity"
    );

    // An actual new untracked unsettled receipt: ordinary is full, so it takes a
    // tombstone slot holding its actual state, and the ordinary class is untouched.
    let untracked = CommandId::fixture("command/untracked-ack");
    inject_receipt(&client, &untracked, ReceiptState::Accepted, true);
    client.pump(0);
    assert_eq!(
        tombstone_count(&client),
        1,
        "exactly one record uses a tombstone slot"
    );
    for index in 0..64 {
        let id = CommandId::fixture(format!("command/ordinary-{index}"));
        assert!(
            retained_state(&client, &id)
                .as_ref()
                .is_some_and(is_capacity_unsettled),
            "ordinary intent {index} keeps its own unsettled state"
        );
    }
    assert_eq!(
        unsettled_count(&client),
        65,
        "the 64 ordinary intents plus the one tombstone-class record"
    );
    let state = retained_state(&client, &untracked);
    assert!(
        matches!(&state, Some(CommandState::ResultUnknown { reason, receipt }) if reason == "CommandReceiptTombstone" && receipt.is_some()),
        "the untracked acknowledgement is retained as an actual tombstone state: {state:?}"
    );
}

#[test]
fn a08_unresolved_stop_stays_capacity_unsettled_across_lifecycle_events() {
    let mut client = live();
    let pending = client.stop_task(&task()).expect("stop sends");
    assert!(matches!(
        retained_state(&client, &pending),
        Some(CommandState::Pending { .. })
    ));
    assert!(
        !unsettled_count(&client).eq(&0),
        "a pending Stop occupies an ordinary unsettled slot"
    );

    // Transport loss: the unresolved intent survives as capacity-unsettled and
    // is never resent.
    client.mock_mut().simulate_disconnect();
    client.pump(0);
    assert!(
        retained_state(&client, &pending)
            .as_ref()
            .is_some_and(is_capacity_unsettled),
        "transport loss keeps the Stop capacity-unsettled"
    );

    // Revocation: read access closes, the registry keeps its unsettled record.
    let stamp = client.control().active_stamp();
    if let Some(stamp) = stamp {
        client.control().inject_unstamped_message(
            desktop_protocol::ServerMessage::ReadInvalidated(desktop_protocol::ReadInvalidated {
                request_id: stamp.request_id.clone(),
                context_id: stamp.context_id.clone(),
                read_revision: desktop_protocol::ReadRevision::new(
                    desktop_protocol::Counter::new("77").expect("revision"),
                ),
                reason: desktop_protocol::ReadInvalidationReason::GrantRevoked,
            }),
        );
    }
    client.pump(1_000_000);
    assert!(
        retained_state(&client, &pending)
            .as_ref()
            .is_some_and(is_capacity_unsettled),
        "revocation keeps the Stop capacity-unsettled"
    );

    // Surface close releases references only; the record stays capacity-unsettled.
    client.client_mut().release_command(&pending);
    assert!(
        retained_state(&client, &pending)
            .as_ref()
            .is_some_and(is_capacity_unsettled),
        "a surface close keeps the Stop capacity-unsettled"
    );
    assert!(
        !matches!(
            retained_state(&client, &pending),
            Some(CommandState::Failed { .. } | CommandState::Unsupported { .. })
        ),
        "acceptance or disconnection is never completion"
    );
}

#[test]
fn a08_an_accepted_record_keeps_its_ordinary_slot_until_canonical_reflection() {
    let mut client = live();
    // Fill the ordinary capacity so occupancy is directly observable.
    for index in 0..63 {
        client
            .client_mut()
            .prepare_command(envelope(index))
            .expect("ordinary slot");
    }
    let accepted = CommandId::fixture("command/accepted-unsettled");
    client
        .client_mut()
        .prepare_command(CommandEnvelope {
            command_id: accepted.clone(),
            ..envelope(0)
        })
        .expect("ordinary slot");
    assert_eq!(
        unsettled_count(&client),
        64,
        "64 ordinary unsettled intents fill the capacity"
    );
    let overflow = client.client_mut().prepare_command(envelope(64));
    assert!(
        matches!(&overflow, Err(ClientError::InvalidCommand(message)) if message == "CommandCapacityExceeded"),
        "the 65th ordinary intent is refused before preparation: {overflow:?}"
    );

    // A readable acknowledgement is durable registration, not canonical
    // reflection: the record keeps its ordinary unsettled slot.
    inject_receipt(&client, &accepted, ReceiptState::Accepted, true);
    client.pump(0);
    let state = retained_state(&client, &accepted);
    assert!(
        matches!(state, Some(CommandState::Accepted(_))),
        "the acknowledgement is retained verbatim: {state:?}"
    );
    assert!(
        state.as_ref().is_some_and(is_capacity_unsettled),
        "Accepted-not-canonically-reflected is capacity-unsettled"
    );
    assert_eq!(
        unsettled_count(&client),
        64,
        "acceptance keeps occupying the ordinary slot it was prepared into"
    );

    // A Live authorization genuinely exists: the read fence is validated and the
    // provider holds the exact stamp a real invalidation must be bound to.
    assert!(
        client.connection().is_live(),
        "the session is live, so a delivered invalidation is processed while bound"
    );
    assert!(
        matches!(
            client.publication_access(),
            cos_client::PublicationAccess::Validated { .. }
        ),
        "the live binding publishes under validated authorization"
    );

    // Identity-level evidence that an authorization closure must not leave
    // disclosable.
    let absorbed = CommandId::fixture("command/absorbed-read-evidence");
    client.control().deny_command(absorbed.as_str().to_owned());
    client.pump(0);
    let binding = client
        .control()
        .active_stamp()
        .expect("a live binding supplies its stamp");
    client
        .lookup_receipt_authorized(desktop_protocol::AuthorizedReceiptLookup {
            lookup_id: desktop_protocol::ReceiptReadId::new("receipt-read/a08-absorbed")
                .expect("valid lookup identity"),
            stamp: binding.clone(),
            command_id: absorbed.clone(),
        })
        .expect("the authorized lookup is admitted");
    client.pump(0);
    client.pump(0);
    assert!(
        client.read_evidence_has(&absorbed),
        "the session holds identity-level read evidence before the closure"
    );

    // The provider revokes read authorization while the binding is live. The
    // notification is bound to that exact binding and is never an optional,
    // silently skipped injection: an unstampable binding would panic here.
    client
        .control()
        .inject_unstamped_message(desktop_protocol::ServerMessage::ReadInvalidated(
            desktop_protocol::ReadInvalidated {
                request_id: binding.request_id.clone(),
                context_id: binding.context_id.clone(),
                read_revision: desktop_protocol::ReadRevision::new(
                    desktop_protocol::Counter::new("77").expect("counter"),
                ),
                reason: desktop_protocol::ReadInvalidationReason::GrantRevoked,
            },
        ));
    let events = client.pump(1_000_000);
    // A binding-matched invalidation is trusted evidence: the client closes the
    // fence and drives recovery. A mismatch would instead be a non-disclosing
    // protocol error that leaves recovery untouched, so this proves the
    // invalidation was genuinely processed and not merely delivered.
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, cos_client::ClientEvent::Error(_))),
        "a binding-matched invalidation is trusted, not a protocol error: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            cos_client::ClientEvent::ResyncRequested {
                reason: desktop_protocol::ResyncReason::Internal
            }
        )),
        "the trusted invalidation actually drove recovery: {events:?}"
    );
    assert!(
        !client.connection().is_live(),
        "a revoked session stops publishing: {:?}",
        client.connection()
    );

    // Authorization closure and non-disclosure: the fence is withheld and the
    // accumulated identity evidence is not left disclosable.
    assert!(
        matches!(
            client.publication_access(),
            cos_client::PublicationAccess::Withheld { .. }
        ),
        "a revoked binding discloses nothing"
    );
    assert!(
        !client.read_evidence_has(&absorbed),
        "the closure drops the identity-level read evidence"
    );
    assert_eq!(
        client.read_evidence_len(),
        0,
        "no held read evidence survives the closure"
    );

    // The true command state and its ordinary capacity slot survive the closure:
    // revocation settles no acknowledgement and frees no slot.
    assert!(
        matches!(
            retained_state(&client, &accepted),
            Some(CommandState::Accepted(_))
        ),
        "revocation never settles an acknowledgement"
    );
    assert_eq!(unsettled_count(&client), 64);
    assert!(
        client.client_mut().prepare_command(envelope(64)).is_err(),
        "the acknowledged record still blocks the 65th ordinary intent after revocation"
    );

    // Surface close only releases a reference; the record stays unsettled.
    client.client_mut().release_command(&accepted);
    assert!(
        matches!(
            retained_state(&client, &accepted),
            Some(CommandState::Accepted(_))
        ),
        "a surface close never settles an acknowledgement"
    );
    assert_eq!(unsettled_count(&client), 64);

    // Fresh reauthorization under a strictly newer read revision (the floor
    // advanced past the revoked revision), then a genuine canonical reflection:
    // only real settlement releases the ordinary slot.
    client
        .mock_mut()
        .set_read_revision(desktop_protocol::ReadRevision::new(
            desktop_protocol::Counter::new("78").expect("counter"),
        ));
    client.connect(0).expect("reauthorize");
    for _ in 0..6 {
        client.pump(0);
        if client.connection().is_live() {
            break;
        }
    }
    assert!(
        client.connection().is_live(),
        "the reauthorized session is live"
    );
    assert!(
        matches!(
            client.publication_access(),
            cos_client::PublicationAccess::Validated { .. }
        ),
        "the fresh authorization reopens validated publication"
    );
    assert_eq!(
        unsettled_count(&client),
        64,
        "reauthorization alone never releases the ordinary slot"
    );
    inject_receipt(&client, &accepted, ReceiptState::CanonicalReflected, true);
    client.pump(0);
    assert!(
        matches!(
            retained_state(&client, &accepted),
            Some(CommandState::CanonicalReflected(_))
        ),
        "the genuine settlement is retained"
    );
    assert!(
        unsettled_count(&client) < 64,
        "genuine settlement releases the ordinary unsettled slot"
    );
    assert!(
        client.client_mut().prepare_command(envelope(64)).is_ok(),
        "the recovered ordinary capacity admits the blocked intent"
    );
}

/// Retain two ordinary intents under a live binding, then re-handshake.
fn prepare_two_ordinary_intents(client: &mut MockCosClient) {
    client
        .client_mut()
        .prepare_command(envelope(0))
        .expect("prepare the first retained intent");
    client
        .client_mut()
        .prepare_command(envelope(1))
        .expect("prepare the second retained intent");
}

/// The flawed grant must leave the client synchronizing-but-withheld.
fn assert_grant_refused_while_synchronizing(client: &MockCosClient, needle: &str) {
    assert!(
        !client.connection().is_live(),
        "a flawed grant never opens a live session"
    );
    assert!(
        matches!(
            client.publication_access(),
            cos_client::PublicationAccess::Withheld { .. }
        ),
        "no binding or projection data is admitted from a flawed grant"
    );
    let error = client.last_error().expect("the flawed grant is refused");
    let text = format!("{error:?}");
    assert!(
        matches!(error, ClientError::Protocol(_)) && text.contains(needle),
        "the refusal names the contradiction {needle:?}: {text}"
    );
    for secret in ["ordinary-", "unrequested", "fixture:", "receipt", "FUTURE"] {
        assert!(
            !text.contains(secret),
            "the refusal leaks no hidden identity or receipt ({secret}): {text}"
        );
    }
}

#[test]
fn a17_a_grant_missing_a_requested_decision_is_refused_while_synchronizing() {
    let mut client = live();
    prepare_two_ordinary_intents(&mut client);
    client
        .control()
        .grant_flaw(cos_client::mock::GrantFlaw::MissingDecision);
    client.connect(0).expect("re-handshake");
    client.pump(0);
    assert_grant_refused_while_synchronizing(
        &client,
        "read decisions do not match the requested retained identities",
    );
}

#[test]
fn a17_a_grant_answering_an_unrequested_identity_is_refused_while_synchronizing() {
    let mut client = live();
    prepare_two_ordinary_intents(&mut client);
    client
        .control()
        .grant_flaw(cos_client::mock::GrantFlaw::ExtraDecision);
    client.connect(0).expect("re-handshake");
    client.pump(0);
    assert_grant_refused_while_synchronizing(
        &client,
        "read decisions do not match the requested retained identities",
    );
}

#[test]
fn a17_a_grant_repeating_a_decision_is_refused_while_synchronizing() {
    let mut client = live();
    prepare_two_ordinary_intents(&mut client);
    client
        .control()
        .grant_flaw(cos_client::mock::GrantFlaw::DuplicateDecision);
    client.connect(0).expect("re-handshake");
    client.pump(0);
    assert_grant_refused_while_synchronizing(&client, "duplicate read decision");
}

#[test]
fn a17_a_grant_stamped_with_a_foreign_server_is_refused_while_synchronizing() {
    let mut client = live();
    prepare_two_ordinary_intents(&mut client);
    client
        .control()
        .grant_flaw(cos_client::mock::GrantFlaw::ForeignServer);
    client.connect(0).expect("re-handshake");
    client.pump(0);
    assert_grant_refused_while_synchronizing(&client, "read grant binding mismatch");
}

#[test]
fn a17_a_grant_older_than_the_closed_floor_is_refused_while_synchronizing() {
    let mut client = live();
    prepare_two_ordinary_intents(&mut client);
    // A trusted invalidation advances the floor far beyond the scripted read
    // revision, so the reconnection grant the provider issues is strictly older.
    let stamp = client.control().active_stamp().expect("binding");
    client
        .control()
        .inject_unstamped_message(desktop_protocol::ServerMessage::ReadInvalidated(
            desktop_protocol::ReadInvalidated {
                request_id: stamp.request_id.clone(),
                context_id: stamp.context_id.clone(),
                read_revision: desktop_protocol::ReadRevision::new(
                    desktop_protocol::Counter::new("77").expect("counter"),
                ),
                reason: desktop_protocol::ReadInvalidationReason::GrantRevoked,
            },
        ));
    client.pump(1_000_000);
    client.connect(0).expect("re-handshake");
    client.pump(0);
    assert_grant_refused_while_synchronizing(&client, "late or equal read grant rejected");
}

#[test]
fn a17_a_grant_equal_to_the_closed_floor_is_refused_while_synchronizing() {
    let mut client = live();
    prepare_two_ordinary_intents(&mut client);
    let stamp = client.control().active_stamp().expect("binding");
    client
        .control()
        .inject_unstamped_message(desktop_protocol::ServerMessage::ReadInvalidated(
            desktop_protocol::ReadInvalidated {
                request_id: stamp.request_id.clone(),
                context_id: stamp.context_id.clone(),
                read_revision: desktop_protocol::ReadRevision::new(
                    desktop_protocol::Counter::new("77").expect("counter"),
                ),
                reason: desktop_protocol::ReadInvalidationReason::GrantRevoked,
            },
        ));
    client.pump(1_000_000);
    // The provider now scripts exactly the closed floor revision.
    client
        .mock_mut()
        .set_read_revision(desktop_protocol::ReadRevision::new(
            desktop_protocol::Counter::new("77").expect("counter"),
        ));
    client.connect(0).expect("re-handshake");
    client.pump(0);
    assert_grant_refused_while_synchronizing(&client, "late or equal read grant rejected");
}
