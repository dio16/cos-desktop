//! MP-PUB1 acceptance for automatic read reconciliation (A20) at the client's
//! public seam.
//!
//! Every scenario is driven through the scripted fixture transcript. The
//! expected lookups come from the contract's 64-pending budget numbers, never
//! from the production drain loop.

use std::collections::BTreeSet;

use cos_client::{ClientError, CommandLookup, CommandState, MockCosClient, ReceiptState};
use desktop_protocol::{
    AuthorizedCommandReceipt, ClientFrame, ClientMessage, ClientWire, CommandEnvelope, CommandId,
    CommandOperation, CommandPayload, CommandReadDecision, CommandReceipt, CommandTarget,
    StopTaskCommand, TaskId,
};

const FIXTURE: &str = include_str!("../fixtures/task-184.json");

/// The committed fixture with the scripted receipt reads left unanswered.
fn fixture_without_lookup_answers() -> String {
    FIXTURE.replace(
        "\"grant_on_open\": true",
        "\"grant_on_open\": true, \"answer_lookups\": false",
    )
}

fn task() -> TaskId {
    TaskId::fixture("task/Task-184")
}

fn envelope(id: &CommandId) -> CommandEnvelope {
    CommandEnvelope {
        command_id: id.clone(),
        target: CommandTarget::Task { task_id: task() },
        expected_revision: None,
        epoch: None,
        payload: CommandPayload::StopTask(StopTaskCommand {
            task_id: task(),
            reason: None,
        }),
    }
}

fn live_from(fixture: &str) -> MockCosClient {
    let mut client = MockCosClient::from_fixture_str(fixture).expect("fixture loads");
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

fn live() -> MockCosClient {
    live_from(FIXTURE)
}

fn retained_state(client: &MockCosClient, id: &CommandId) -> Option<CommandState> {
    match client.lookup_command(id) {
        CommandLookup::Found(record) => Some(record.state().clone()),
        CommandLookup::NotRetained => None,
    }
}

/// Inject one readable receipt under the current binding.
fn inject_receipt(client: &MockCosClient, id: &CommandId, state: ReceiptState) {
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
                    readable: true,
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
                    message: Some("fixture receipt".to_owned()),
                },
            },
        ));
}

/// Reopen the same scope while declaring no retention interest.
///
/// This is a legitimate client request: the owner may observe the scope without
/// reconciling any retained identity. Every retained record therefore keeps the
/// decision of the former binding and now has no evidence for the current one,
/// which is exactly the "missing current evidence" condition the drain owns.
fn rebind_without_retention(client: &mut MockCosClient) {
    let scope = client.mock().scenario().observation_request();
    let mut request = client.client().observation_request(scope);
    request.retained_commands = Vec::new();
    client
        .begin_authorized_observation(request)
        .expect("rebind");
    for _ in 0..6 {
        client.pump(0);
        if client.connection().is_live() {
            break;
        }
    }
    assert!(client.connection().is_live(), "the rebound session is live");
}

#[test]
fn a20_a_server_derived_record_without_current_evidence_is_looked_up_automatically() {
    let mut client = live();
    // A tracked, server-derived, still-unsettled record under the first binding.
    let id = CommandId::fixture("command/needs-read");
    client
        .client_mut()
        .prepare_command(envelope(&id))
        .expect("prepare");
    inject_receipt(
        &client,
        &id,
        ReceiptState::Unknown("FUTURE_STATE".to_owned()),
    );
    client.pump(0);
    assert!(
        matches!(
            retained_state(&client, &id),
            Some(CommandState::ResultUnknown { .. })
        ),
        "the tracked record keeps its genuine unsettled state"
    );
    let lookups_before = client.control().lookups_seen();

    // A fresh binding that carries no decision for the record: the outcome may
    // only be reconciled by an automatic read, never by an invented state.
    rebind_without_retention(&mut client);
    for _ in 0..4 {
        client.pump(0);
    }
    assert_eq!(
        client.control().lookups_seen(),
        lookups_before + 1,
        "the drain spends exactly one bounded authorized lookup with no UI call"
    );
    // The automatic reply supplies the current evidence and preserves the record.
    let binding = client.control().active_stamp().expect("binding");
    match client.lookup_command(&id) {
        CommandLookup::Found(record) => assert_eq!(
            record.permission(&binding),
            Some(true),
            "the automatic reply installs current positive evidence"
        ),
        CommandLookup::NotRetained => panic!("the record stays retained"),
    }
    assert!(
        matches!(
            retained_state(&client, &id),
            Some(CommandState::ResultUnknown { .. })
        ),
        "the automatic read never fabricates a settled outcome"
    );
}

/// Receipt-lookup identities the client has actually written to the wire, in
/// the order it sent them.
///
/// This is the client's outward request stream at the public transport seam, so
/// it observes which identities the drain asks for, not an internal counter.
fn issued_lookups(client: &MockCosClient) -> Vec<CommandId> {
    client
        .mock()
        .sent_frames()
        .iter()
        .filter_map(|frame| match serde_json::from_slice::<ClientWire>(frame) {
            Ok(ClientWire::Frame(ClientFrame {
                message: ClientMessage::LookupReceipt(lookup),
                ..
            })) => Some(lookup.command_id),
            _ => None,
        })
        .collect()
}

/// Answer the provider's outstanding lookup with an allowed authoritative
/// receipt, exactly as the fixture answers a real lookup.
///
/// The reply carries the binding stamp of the outstanding lookup, so it frees
/// one pending slot without inventing authorization evidence.
fn answer_authoritatively(client: &MockCosClient, id: &CommandId) {
    client.control().answer_pending_lookup(Some(CommandReceipt {
        command_id: id.clone(),
        operation: CommandOperation::StopTask,
        state: ReceiptState::Accepted,
        task_id: Some(task()),
        accepted_at: None,
        reject_reason: None,
        effect_refs: Vec::new(),
        dedupe_expires_at: None,
        message: Some("authoritative fixture reply".to_owned()),
    }));
}

#[test]
fn a20_more_than_sixty_four_candidates_drain_only_within_the_pending_budget() {
    let mut client = live_from(&fixture_without_lookup_answers());
    // 65 unsettled server-derived identities: the first 64 take the ordinary
    // class, the 65th a tombstone slot, all still lacking current evidence once
    // the binding is replaced.
    for index in 0..65 {
        let id = CommandId::fixture(format!("command/candidate-{index}"));
        inject_receipt(&client, &id, ReceiptState::Accepted);
        client.pump(0);
    }
    let candidates: BTreeSet<CommandId> = (0..65)
        .map(|index| CommandId::fixture(format!("command/candidate-{index}")))
        .collect();
    let lookups_before = client.control().lookups_seen();
    rebind_without_retention(&mut client);
    for _ in 0..4 {
        client.pump(0);
    }
    // The drain admits exactly the 64-pending budget and leaves one candidate
    // waiting, never requesting any identity twice.
    let first_wave = issued_lookups(&client);
    assert_eq!(
        client.control().lookups_seen() - lookups_before,
        64,
        "the drain admits exactly the 64-pending budget, never a 65th request"
    );
    assert_eq!(
        first_wave.len(),
        64,
        "all 64 admitted lookups are on the wire"
    );
    let requested: BTreeSet<CommandId> = first_wave.iter().cloned().collect();
    assert_eq!(
        requested.len(),
        64,
        "the first wave requests every admitted candidate at most once"
    );
    let mut waiting: Vec<CommandId> = candidates.difference(&requested).cloned().collect();
    assert_eq!(
        waiting.len(),
        1,
        "exactly one candidate waits for a pending slot: {waiting:?}"
    );
    let waiting = waiting.pop().expect("the waiting candidate");

    // Further pumping cannot exceed the budget: unanswered reads stay in flight.
    let after_budget = client.control().lookups_seen();
    for _ in 0..6 {
        client.pump(0);
    }
    assert_eq!(
        client.control().lookups_seen(),
        after_budget,
        "no candidate is drained beyond the admitted pending budget"
    );

    // A single legitimate authoritative reply frees one pending slot...
    let answered = first_wave
        .last()
        .cloned()
        .expect("the last admitted lookup");
    answer_authoritatively(&client, &answered);
    for _ in 0..4 {
        client.pump(0);
    }

    // ...and the fair bounded drain immediately admits the waiting candidate,
    // exactly once, without any over-lookup.
    let second_wave = issued_lookups(&client);
    assert_eq!(
        second_wave.len(),
        65,
        "the freed slot admits exactly one more request, not more"
    );
    assert_eq!(
        second_wave.iter().filter(|id| **id == waiting).count(),
        1,
        "the 65th candidate is requested exactly once after the slot frees: {waiting}"
    );
    let unique: BTreeSet<CommandId> = second_wave.iter().cloned().collect();
    assert_eq!(
        unique.len(),
        65,
        "no candidate is ever requested twice across the whole drain"
    );

    // No retry loop: additional pumping issues nothing new.
    for _ in 0..8 {
        client.pump(0);
    }
    assert_eq!(
        issued_lookups(&client).len(),
        65,
        "the drain converges without polling the same identity again"
    );

    // No protected identity leaks: the drain emits no diagnostic at all.
    assert!(
        client.last_error().is_none(),
        "the finite drain discloses no error: {:?}",
        client.last_error()
    );
}

#[test]
fn a20_held_references_delay_automatic_reads_without_expanding_the_bound() {
    let mut client = live_from(&fixture_without_lookup_answers());
    // `referenced` sorts before `unreferenced`, so a bound-expanding bug would
    // abort the drain on the referenced candidate before reaching the other.
    let referenced = CommandId::fixture("command/referenced-candidate");
    let unreferenced = CommandId::fixture("command/unreferenced-candidate");
    inject_receipt(&client, &referenced, ReceiptState::Accepted);
    client.pump(0);
    inject_receipt(&client, &unreferenced, ReceiptState::Accepted);
    client.pump(0);
    // A surface legitimately holds the whole 64-reference budget on one record.
    for reference in 0..64 {
        client
            .client_mut()
            .retain_command(&referenced)
            .unwrap_or_else(|error| panic!("reference {reference} must fit: {error:?}"));
    }

    // The referenced candidate has no current evidence, but the exhausted
    // reference budget delays its read without aborting the fair drain: the
    // unreferenced candidate is still reconciled first.
    rebind_without_retention(&mut client);
    for _ in 0..4 {
        client.pump(0);
    }
    assert_eq!(
        client.control().lookups_seen(),
        1,
        "referenced reads are delayed while others proceed within the budget"
    );

    // Releasing the references admits the delayed automatic read.
    for _ in 0..64 {
        client.client_mut().release_command(&referenced);
    }
    for _ in 0..4 {
        client.pump(0);
    }
    assert_eq!(
        client.control().lookups_seen(),
        2,
        "the delayed automatic read proceeds once the budget is released"
    );
}

#[test]
fn a20_a_read_timeout_preserves_unknown_without_retry_or_disclosure() {
    let mut client = live_from(&fixture_without_lookup_answers());
    let id = CommandId::fixture("command/timeout-unknown");
    client
        .client_mut()
        .prepare_command(envelope(&id))
        .expect("prepare");
    inject_receipt(
        &client,
        &id,
        ReceiptState::Unknown("FUTURE_STATE".to_owned()),
    );
    client.pump(0);
    assert!(matches!(
        retained_state(&client, &id),
        Some(CommandState::ResultUnknown { .. })
    ));

    rebind_without_retention(&mut client);
    for _ in 0..4 {
        client.pump(0);
    }
    assert_eq!(
        client.control().lookups_seen(),
        1,
        "the automatic read is in flight"
    );

    // Past the read deadline the pending read times out: the outcome stays
    // unknown, a safe diagnostic is emitted and no protected identity leaks.
    let events = client.pump(30_001);
    let timed_out = events.iter().find_map(|event| match event {
        cos_client::ClientEvent::Error(error) => Some(error.clone()),
        _ => None,
    });
    let timed_out = timed_out.expect("a timeout diagnostic is emitted");
    let text = format!("{timed_out:?}");
    assert!(
        matches!(timed_out, ClientError::Protocol(_)) && text.contains("timed out"),
        "the timeout is a soft protocol diagnostic: {text}"
    );
    assert!(
        !text.contains(id.as_str())
            && !text.contains("FUTURE_STATE")
            && !text.contains("fixture:command"),
        "the diagnostic discloses no hidden identity or protected receipt: {text}"
    );
    assert!(
        matches!(
            retained_state(&client, &id),
            Some(CommandState::ResultUnknown { .. })
        ),
        "the timeout preserves the unsettled unknown instead of settling it"
    );

    // No silent retry: the stalled identity is never polled again.
    let lookups_at_timeout = client.control().lookups_seen();
    for _ in 0..8 {
        client.pump(60_000);
    }
    assert_eq!(
        client.control().lookups_seen(),
        lookups_at_timeout,
        "a timed-out read is not retried in a loop"
    );
}
