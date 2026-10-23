//! MP-COR2 / DC-MP-COR2 — graph membership validation through public seams.
//!
//! Snapshot cases exercise `FixtureScenario::validate`; live cases exercise
//! transaction ingestion and its atomic recovery boundary. Expected messages
//! are the externally compatible error identities, not copies of the validator.

mod common;

use common::*;
use cos_client::{ClientError, ClientEvent, ConnectionState, FixtureScenario, MockCosClient};
use desktop_protocol::{
    ArtifactDigest, Counter, Cursor, DeltaTransaction, EdgeId, EpochId, GraphEdge, GraphId,
    GraphNode, GraphRef, GraphSnapshot, NodeId, Projection, ProjectionMutation, TransactionId,
    integrity_digest_of,
};

fn fixture_scenario() -> FixtureScenario {
    FixtureScenario::from_file(fixture_path()).expect("committed fixture loads")
}

fn node(local: &str) -> GraphNode {
    GraphNode {
        node_ref: NodeId::fixture(local),
        label: local.to_owned(),
        layer: None,
    }
}

fn edge(local: &str, from: &str, to: &str) -> GraphEdge {
    GraphEdge {
        edge_ref: EdgeId::fixture(local),
        from: NodeId::fixture(from),
        to: NodeId::fixture(to),
        kind: None,
    }
}

fn graph(local: &str, node_ids: &[&str], edges: &[(&str, &str, &str)]) -> GraphSnapshot {
    GraphSnapshot {
        graph_ref: GraphRef {
            graph_id: GraphId::fixture(format!("graph/{local}")),
            version: 1,
            digest: ArtifactDigest::fixture(format!("digest/{local}")),
        },
        plan_refs: Vec::new(),
        nodes: node_ids.iter().map(|id| node(id)).collect(),
        edges: edges
            .iter()
            .map(|(id, from, to)| edge(id, from, to))
            .collect(),
        provenance_refs: Vec::new(),
        refinement_refs: Vec::new(),
        projection_revision: Counter::new("1").expect("counter"),
    }
}

fn assert_snapshot_error(graphs: Vec<GraphSnapshot>, expected_integrity_error: &str) {
    let mut scenario = fixture_scenario();
    scenario.initial_state.graphs = graphs;
    let expected = format!(
        "fixture scenario is invalid: initial_state is inconsistent: {expected_integrity_error}"
    );

    let first = scenario
        .validate()
        .expect_err("invalid graph is rejected")
        .to_string();
    let repeated = scenario
        .validate()
        .expect_err("the same invalid graph remains rejected")
        .to_string();
    assert_eq!(first, expected);
    assert_eq!(repeated, expected);
    assert_eq!(
        first, repeated,
        "the public error identity is deterministic"
    );
}

fn next_counter(counter: &Counter) -> Counter {
    let next = counter
        .as_str()
        .parse::<u64>()
        .expect("fixture counter is numeric")
        + 1;
    Counter::new(next.to_string()).expect("next counter")
}

fn advance_graph_revision(graph: &mut GraphSnapshot) {
    graph.graph_ref.version += 1;
    graph.graph_ref.digest = ArtifactDigest::fixture("digest/graph-cor2-rewire");
}

fn graph_upsert_transaction(
    epoch: EpochId,
    transaction_id: &str,
    cursor: &str,
    predecessor: Cursor,
    base_revision: Counter,
    graph: GraphSnapshot,
) -> DeltaTransaction {
    let mutations = vec![ProjectionMutation::Upsert {
        base_revision: Some(base_revision),
        projection: Projection::Graph(graph),
    }];
    DeltaTransaction {
        transaction_id: TransactionId::fixture(transaction_id),
        epoch,
        cursor: Cursor::fixture(cursor),
        predecessor: Some(predecessor),
        mutation_count: u32::try_from(mutations.len()).expect("small transaction"),
        integrity_digest: integrity_digest_of(&mutations).expect("transaction digest"),
        mutations,
    }
}

#[test]
fn snapshot_validation_accepts_unsorted_directed_self_loop_and_empty_graphs() {
    // Catches: replacing ordered membership lookup with a rule that requires
    // sorted node input, reciprocal edges, or at least one graph node.
    // Oracle: DC-MP-COR2 preserves valid graph meaning; these cases are valid
    // because every directed endpoint exists in its graph, including the loop.
    let cases = [
        (
            "unsorted nodes, directed edge, and self-loop",
            graph(
                "valid-unsorted",
                &["node/z", "node/a", "node/m"],
                &[
                    ("edge/z-to-a", "node/z", "node/a"),
                    ("edge/m-loop", "node/m", "node/m"),
                ],
            ),
        ),
        ("empty graph", graph("valid-empty", &[], &[])),
    ];

    for (name, graph) in cases {
        let mut scenario = fixture_scenario();
        scenario.initial_state.graphs = vec![graph];
        assert!(scenario.validate().is_ok(), "valid case rejected: {name}");
    }
}

#[test]
fn snapshot_membership_is_graph_local_and_duplicate_nodes_win_before_edges() {
    // Catches: a global node index allowing another graph's key to satisfy a
    // local edge, or endpoint checks running before duplicate-node rejection.
    // Oracle: each GraphSnapshot owns its node set; duplicate nodes have the
    // established error precedence and exact public identity.
    assert_snapshot_error(
        vec![
            graph(
                "a-local",
                &["node/a"],
                &[("edge-cross-graph", "node/shared", "node/a")],
            ),
            graph("b-owner", &["node/shared"], &[]),
        ],
        "graph fixture:graph/a-local edge references absent source node",
    );

    assert_snapshot_error(
        vec![graph(
            "duplicate-before-edge",
            &["node/repeated", "node/repeated"],
            &[("edge-invalid", "node/missing-source", "node/missing-target")],
        )],
        "graph fixture:graph/duplicate-before-edge contains duplicate inspection nodes",
    );
}

#[test]
fn snapshot_validation_preserves_first_edge_and_source_before_target_order() {
    // Catches: hash/index iteration order leaking into validation or checking a
    // target before its source. The first case's first edge has an invalid
    // target and precedes a different edge's invalid source.
    assert_snapshot_error(
        vec![graph(
            "edge-order",
            &["node/present"],
            &[
                ("edge/first", "node/present", "node/missing-target"),
                ("edge/second", "node/missing-source", "node/present"),
            ],
        )],
        "graph fixture:graph/edge-order edge references absent target node",
    );

    // Both endpoints of this first edge are absent; its source must be
    // reported before its target.
    assert_snapshot_error(
        vec![graph(
            "source-before-target",
            &["node/present"],
            &[(
                "edge/both-missing",
                "node/missing-source",
                "node/missing-target",
            )],
        )],
        "graph fixture:graph/source-before-target edge references absent source node",
    );
}

fn reject_missing_graph_endpoint_and_recover() -> ClientError {
    let mut mock = live_mock();
    let before = mock.snapshot();
    let cursor_before = mock
        .client()
        .last_seen_cursor()
        .cloned()
        .expect("live fixture cursor");
    let graph_id = GraphId::fixture("graph/Task-184");
    let mut invalid_graph = before.graphs.get(&graph_id).expect("fixture graph").clone();
    let base_revision = invalid_graph.projection_revision.clone();
    invalid_graph.edges[0].to = NodeId::fixture("node/missing-delta-target");
    invalid_graph.projection_revision = next_counter(&base_revision);
    advance_graph_revision(&mut invalid_graph);

    let epoch = fixture_scenario().epoch;
    let transaction = graph_upsert_transaction(
        epoch,
        "transaction/cor2-missing-endpoint",
        "cursor/cor2-missing-endpoint",
        cursor_before.clone(),
        base_revision,
        invalid_graph,
    );
    mock.mock_mut().inject_raw(delta_frame(transaction));
    let events = mock.pump(0);

    let error = events
        .iter()
        .find_map(|event| match event {
            ClientEvent::Error(error) => Some(error.clone()),
            _ => None,
        })
        .expect("invalid graph delta surfaces a public error");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ClientEvent::DeltaApplied { .. }))
    );
    assert!(
        matches!(mock.connection(), ConnectionState::Reconnecting { .. }),
        "invalid graph delta must schedule bounded recovery"
    );
    assert!(mock.is_stale());
    assert_eq!(mock.snapshot().as_ref(), before.as_ref());
    assert_eq!(mock.client().last_seen_cursor(), Some(&cursor_before));

    let recovery_events = mock.pump(1_000_000);
    assert!(
        mock.connection().is_live(),
        "bounded recovery returns to Live"
    );
    assert!(!has_error(&recovery_events));
    assert_eq!(mock.snapshot().as_ref(), before.as_ref());

    error
}

#[test]
fn invalid_graph_delta_is_atomic_recovers_and_has_deterministic_error_identity() {
    // Catches: a failed graph-membership check publishing partial state or
    // advancing the cursor, or emitting nondeterministic diagnostics.
    // Oracle: DC-MP-COR1 recovery invariants and DC-MP-COR2 error identity;
    // this mock covers cos-client's protocol/transport boundary, not Runtime.
    let expected = ClientError::Protocol(
        "effective state rejected: graph fixture:graph/Task-184 edge references absent target node"
            .to_owned(),
    );
    let first = reject_missing_graph_endpoint_and_recover();
    let repeated = reject_missing_graph_endpoint_and_recover();
    assert_eq!(first, expected);
    assert_eq!(repeated, expected);
    assert_eq!(first, repeated);
}

#[test]
fn valid_graph_delta_replaces_nodes_and_edges_with_a_legal_rewire() {
    // Catches: false rejection of a legal transaction-local graph rewire or
    // accidental retention/reordering of the previous graph contents.
    // Oracle: a graph with unsorted nodes, a directed edge, and a self-loop is
    // accepted and published exactly as supplied when all endpoints exist.
    let mut mock: MockCosClient = live_mock();
    let before = mock.snapshot();
    let graph_id = GraphId::fixture("graph/Task-184");
    let original = before.graphs.get(&graph_id).expect("fixture graph");
    let base_revision = original.projection_revision.clone();
    let mut rewired = original.clone();
    rewired.nodes = vec![node("node/z"), node("node/a"), node("node/m")];
    rewired.edges = vec![
        edge("edge/z-to-a", "node/z", "node/a"),
        edge("edge/m-loop", "node/m", "node/m"),
    ];
    rewired.projection_revision = next_counter(&base_revision);
    advance_graph_revision(&mut rewired);

    let transaction_id = TransactionId::fixture("transaction/cor2-valid-rewire");
    let cursor = Cursor::fixture("cursor/cor2-valid-rewire");
    let transaction = graph_upsert_transaction(
        fixture_scenario().epoch,
        "transaction/cor2-valid-rewire",
        "cursor/cor2-valid-rewire",
        mock.client()
            .last_seen_cursor()
            .cloned()
            .expect("live fixture cursor"),
        base_revision,
        rewired.clone(),
    );
    mock.mock_mut().inject_raw(delta_frame(transaction));
    let events = mock.pump(0);

    assert!(
        events.iter().any(|event| matches!(
            event,
            ClientEvent::DeltaApplied { transaction_id: applied, cursor: applied_cursor }
                if applied == &transaction_id && applied_cursor == &cursor
        )),
        "valid graph rewire must publish its delta"
    );
    assert!(!has_error(&events));
    assert_eq!(mock.snapshot().graphs.get(&graph_id), Some(&rewired));
}
