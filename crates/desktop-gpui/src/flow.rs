//! Flow's non-canvas inspection model.
//!
//! Rows are pure views over the authoritative graph display. Filtering and
//! keyboard cursors never add, remove or redirect edges.

use std::sync::Arc;

use desktop_protocol::{Counter, EdgeId, GraphRef, NodeId};
use ui_model::{GraphDisplay, GraphEdgeDisplay, GraphNodeDisplay};

/// Stable keyboard identity for one node in one exact graph revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeKey {
    pub graph_ref: GraphRef,
    pub node: NodeId,
}

/// Stable keyboard identity for one edge in one exact graph revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeKey {
    pub graph_ref: GraphRef,
    pub edge: EdgeId,
}

/// One node row bound to the graph revision it belongs to.
#[derive(Clone)]
pub struct NodeRow {
    pub graph: Arc<GraphDisplay>,
    pub index: usize,
}

impl NodeRow {
    pub fn node(&self) -> &Arc<GraphNodeDisplay> {
        &self.graph.nodes[self.index]
    }

    pub fn key(&self) -> NodeKey {
        NodeKey {
            graph_ref: self.graph.graph_ref.clone(),
            node: self.node().identity.clone(),
        }
    }
}

/// One edge row bound to the graph revision it belongs to.
#[derive(Clone)]
pub struct EdgeRow {
    pub graph: Arc<GraphDisplay>,
    pub index: usize,
}

impl EdgeRow {
    pub fn edge(&self) -> &Arc<GraphEdgeDisplay> {
        &self.graph.edges[self.index]
    }

    pub fn key(&self) -> EdgeKey {
        EdgeKey {
            graph_ref: self.graph.graph_ref.clone(),
            edge: self.edge().identity.clone(),
        }
    }
}

/// Filtered node/edge rows in authoritative order.
pub struct Rows {
    pub graphs: Arc<[Arc<GraphDisplay>]>,
    pub nodes: Arc<[NodeRow]>,
    pub edges: Arc<[EdgeRow]>,
}

/// Complete identity that must still hold when filtered rows are admitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmissionKey {
    pub session_generation: u64,
    pub publication_version: u64,
    pub epoch: Option<desktop_protocol::EpochId>,
    pub revision: RevisionKey,
    pub query: String,
}

/// One background filtering request.
#[derive(Clone)]
pub struct Job {
    pub generation: usize,
    pub key: AdmissionKey,
    pub graphs: Arc<[Arc<GraphDisplay>]>,
}

/// Build filtered inspection rows without changing graph identity or topology.
///
/// The cancellation predicate is checked before and during every graph,
/// node and edge traversal. A cancelled request returns no partial rows.
pub fn rows_cancellable(
    graphs: &[Arc<GraphDisplay>],
    query: &str,
    cancelled: impl Fn() -> bool,
) -> Option<Rows> {
    if cancelled() {
        return None;
    }
    let query = query.trim().to_lowercase();
    let matches = |fields: &[&str]| {
        query.is_empty()
            || fields
                .iter()
                .any(|field| field.to_lowercase().contains(&query))
    };
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for graph in graphs {
        if cancelled() {
            return None;
        }
        for (index, node) in graph.nodes.iter().enumerate() {
            if cancelled() {
                return None;
            }
            if matches(&[
                node.label.as_ref(),
                node.layer.as_deref().unwrap_or(""),
                node.identity.as_str(),
            ]) {
                nodes.push(NodeRow {
                    graph: Arc::clone(graph),
                    index,
                });
            }
        }
        for (index, edge) in graph.edges.iter().enumerate() {
            if cancelled() {
                return None;
            }
            if matches(&[
                edge.kind.as_deref().unwrap_or(""),
                edge.from_label.as_ref(),
                edge.to_label.as_ref(),
                edge.from.as_str(),
                edge.to.as_str(),
                edge.identity.as_str(),
            ]) {
                edges.push(EdgeRow {
                    graph: Arc::clone(graph),
                    index,
                });
            }
        }
    }
    if cancelled() {
        return None;
    }
    Some(Rows {
        graphs: graphs.into(),
        nodes: nodes.into(),
        edges: edges.into(),
    })
}

/// Build filtered inspection rows for uncancelled tests only.
#[cfg(test)]
pub fn rows(graphs: &[Arc<GraphDisplay>], query: &str) -> Rows {
    rows_cancellable(graphs, query, || false).expect("uncancelled derivation completes")
}

/// A completed job may be installed only when it is still the pending request
/// and every current admission dimension is identical.
pub fn admit(
    job: &AdmissionKey,
    pending: Option<&AdmissionKey>,
    current: Option<&AdmissionKey>,
) -> bool {
    pending == Some(job) && current == Some(job)
}

pub fn contains_node(rows: &Rows, key: &NodeKey) -> bool {
    rows.nodes.iter().any(|row| row.key() == *key)
}

pub fn contains_edge(rows: &Rows, key: &EdgeKey) -> bool {
    rows.edges.iter().any(|row| row.key() == *key)
}

/// Exact graph revisions and their independent projection revisions.
pub type RevisionKey = Vec<(GraphRef, Counter)>;

pub fn revision_key(graphs: &[Arc<GraphDisplay>]) -> RevisionKey {
    graphs
        .iter()
        .map(|graph| (graph.graph_ref.clone(), graph.source_revision.clone()))
        .collect()
}

/// Nodes the prototype diagram draws; the lists stay complete.
///
/// A bounded drawing keeps layout and paint cost predictable without ever
/// changing graph identity, topology or the accessible inspection rows.
pub const MAX_DIAGRAM_NODES: usize = 120;

/// One drawn node, positioned by depth in the authoritative topology.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagramNode {
    pub node: NodeId,
    pub label: Arc<str>,
    pub layer: Option<Arc<str>>,
    pub depth: usize,
    pub lane: usize,
    pub matched: bool,
}

/// One drawn connection between two drawn nodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagramEdge {
    pub edge: EdgeId,
    pub from: NodeId,
    pub to: NodeId,
    pub matched: bool,
}

/// A layered drawing of one authoritative graph revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagram {
    pub nodes: Vec<DiagramNode>,
    pub edges: Vec<DiagramEdge>,
    pub depth_count: usize,
    pub lane_count: usize,
    /// The graph has more nodes than the drawing cap.
    pub truncated: bool,
    /// Edges whose endpoint is absent from this drawing.
    pub skipped_edges: usize,
    /// The topology contains a cycle, so depth is a hint rather than a rank.
    pub cyclic: bool,
}

/// Lay out one authoritative graph without inventing topology or labels.
///
/// Depth is the longest path over the projected edges, so a node is drawn after
/// its predecessors. Matching nodes fill the drawing first, then the remaining
/// nodes in authoritative order. Every label, layer and identity is copied from
/// the projection; nothing is derived from the drawing position.
pub fn diagram(
    graph: &GraphDisplay,
    matched_nodes: &std::collections::BTreeSet<NodeId>,
    matched_edges: &std::collections::BTreeSet<EdgeId>,
) -> Diagram {
    let truncated = graph.nodes.len() > MAX_DIAGRAM_NODES;
    let ordered: Vec<usize> = (0..graph.nodes.len())
        .filter(|index| matched_nodes.contains(&graph.nodes[*index].identity))
        .chain(
            (0..graph.nodes.len())
                .filter(|index| !matched_nodes.contains(&graph.nodes[*index].identity)),
        )
        .take(MAX_DIAGRAM_NODES)
        .collect();
    let drawn: std::collections::BTreeMap<NodeId, usize> = ordered
        .iter()
        .enumerate()
        .map(|(position, index)| (graph.nodes[*index].identity.clone(), position))
        .collect();

    let mut successor: Vec<Vec<usize>> = vec![Vec::new(); ordered.len()];
    let mut depth: Vec<usize> = vec![0; ordered.len()];
    let mut drawn_edges: Vec<DiagramEdge> = Vec::new();
    let mut skipped_edges = 0;
    for edge in graph.edges.iter() {
        let (Some(&from), Some(&to)) = (drawn.get(&edge.from), drawn.get(&edge.to)) else {
            skipped_edges += 1;
            continue;
        };
        successor[from].push(to);
        drawn_edges.push(DiagramEdge {
            edge: edge.identity.clone(),
            from: edge.from.clone(),
            to: edge.to.clone(),
            matched: matched_edges.contains(&edge.identity),
        });
    }

    let mut cyclic = false;
    for _ in 0..ordered.len() {
        let mut changed = false;
        for from in 0..ordered.len() {
            for &to in &successor[from] {
                if depth[to] < depth[from] + 1 {
                    depth[to] = depth[from] + 1;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    // A relaxation that still changes after |V| passes means a cycle; keeping the
    // bounded pass count avoids an infinite loop and reports the ambiguity.
    if (0..ordered.len()).any(|from| {
        successor[from]
            .iter()
            .any(|&to| depth[to] < depth[from] + 1)
    }) {
        cyclic = true;
    }

    let depth_count = depth.iter().copied().max().map_or(0, |value| value + 1);
    let mut lanes = vec![0usize; depth_count];
    let nodes = ordered
        .iter()
        .enumerate()
        .map(|(position, index)| {
            let node = &graph.nodes[*index];
            let node_depth = depth[position];
            let lane = lanes[node_depth];
            lanes[node_depth] += 1;
            DiagramNode {
                node: node.identity.clone(),
                label: Arc::clone(&node.label),
                layer: node.layer.clone(),
                depth: node_depth,
                lane,
                matched: matched_nodes.contains(&node.identity),
            }
        })
        .collect();
    let lane_count = lanes.into_iter().max().unwrap_or(0);

    Diagram {
        nodes,
        edges: drawn_edges,
        depth_count,
        lane_count,
        truncated,
        skipped_edges,
        cyclic,
    }
}

pub fn revision_label(graph: &GraphDisplay) -> String {
    format!(
        "グラフ改訂 {} · バージョン {} · {}",
        graph.graph_ref.graph_id.as_str(),
        graph.graph_ref.version,
        graph.graph_ref.digest.as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::{
        AdmissionKey, MAX_DIAGRAM_NODES, admit, contains_edge, contains_node, diagram,
        revision_key, rows, rows_cancellable,
    };
    use desktop_protocol::{ArtifactDigest, Counter, EpochId, GraphId, GraphRef};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use ui_model::{GraphDisplay, GraphEdgeDisplay, GraphNodeDisplay};

    fn graph(version: u64, digest: &str, projection: u128) -> Arc<GraphDisplay> {
        let node = |identity: &str, label: &str, layer: Option<&str>| {
            Arc::new(GraphNodeDisplay {
                identity: desktop_protocol::NodeId::fixture(identity),
                label: label.into(),
                layer: layer.map(Into::into),
            })
        };
        let edge = |identity: &str, from: &str, to: &str, kind: &str| {
            fn label(identity: &str) -> &str {
                match identity {
                    "node/z" => "Zeta",
                    "node/a" => "Alpha",
                    "node/m" => "Middle",
                    _ => identity,
                }
            }
            Arc::new(GraphEdgeDisplay {
                identity: desktop_protocol::EdgeId::fixture(identity),
                from: desktop_protocol::NodeId::fixture(from),
                to: desktop_protocol::NodeId::fixture(to),
                from_label: label(from).into(),
                to_label: label(to).into(),
                kind: Some(kind.into()),
            })
        };
        Arc::new(GraphDisplay {
            graph_ref: GraphRef {
                graph_id: GraphId::fixture("graph/flow"),
                version,
                digest: ArtifactDigest::fixture(digest),
            },
            source_revision: Counter::from_u128(projection),
            nodes: vec![
                node("node/z", "Zeta", Some("GOAL")),
                node("node/a", "Alpha", None),
                node("node/m", "Middle", Some("FLOW")),
            ]
            .into(),
            edges: vec![
                edge("edge/z-a", "node/z", "node/a", "FLOW"),
                edge("edge/a-m", "node/a", "node/m", "FLOW"),
            ]
            .into(),
        })
    }

    #[test]
    fn the_diagram_layers_predecessors_before_successors() {
        let graph = graph(3, "digest/three", 8);
        let none: std::collections::BTreeSet<desktop_protocol::NodeId> =
            std::collections::BTreeSet::new();
        let no_edges: std::collections::BTreeSet<desktop_protocol::EdgeId> =
            std::collections::BTreeSet::new();
        let drawn = diagram(&graph, &none, &no_edges);
        assert_eq!(drawn.nodes.len(), 3);
        assert_eq!(drawn.edges.len(), 2);
        assert!(!drawn.cyclic);
        let depth = |identity: &str| {
            drawn
                .nodes
                .iter()
                .find(|node| node.node == desktop_protocol::NodeId::fixture(identity))
                .expect("drawn node")
                .depth
        };
        assert!(depth("node/z") < depth("node/a"));
        assert!(depth("node/a") < depth("node/m"));
        assert_eq!(drawn.depth_count, 3);
        assert_eq!(drawn.lane_count, 1);
    }

    #[test]
    fn the_diagram_keeps_projection_labels_and_never_invents_one() {
        let graph = graph(3, "digest/three", 8);
        let none: std::collections::BTreeSet<desktop_protocol::NodeId> =
            std::collections::BTreeSet::new();
        let no_edges: std::collections::BTreeSet<desktop_protocol::EdgeId> =
            std::collections::BTreeSet::new();
        let drawn = diagram(&graph, &none, &no_edges);
        let labels: Vec<&str> = drawn.nodes.iter().map(|node| node.label.as_ref()).collect();
        assert_eq!(labels, vec!["Zeta", "Alpha", "Middle"]);
        let layers: Vec<Option<&str>> = drawn
            .nodes
            .iter()
            .map(|node| node.layer.as_deref())
            .collect();
        assert_eq!(layers, vec![Some("GOAL"), None, Some("FLOW")]);
    }

    #[test]
    fn the_diagram_states_cycles_missing_endpoints_and_truncation() {
        let base = graph(3, "digest/three", 8);
        let mut cyclic = (*base).clone();
        cyclic.edges = vec![
            Arc::new(GraphEdgeDisplay {
                identity: desktop_protocol::EdgeId::fixture("edge/a-z"),
                from: desktop_protocol::NodeId::fixture("node/a"),
                to: desktop_protocol::NodeId::fixture("node/z"),
                from_label: "Alpha".into(),
                to_label: "Zeta".into(),
                kind: Some("FLOW".into()),
            }),
            Arc::new(GraphEdgeDisplay {
                identity: desktop_protocol::EdgeId::fixture("edge/z-a"),
                from: desktop_protocol::NodeId::fixture("node/z"),
                to: desktop_protocol::NodeId::fixture("node/a"),
                from_label: "Zeta".into(),
                to_label: "Alpha".into(),
                kind: Some("FLOW".into()),
            }),
            Arc::new(GraphEdgeDisplay {
                identity: desktop_protocol::EdgeId::fixture("edge/absent"),
                from: desktop_protocol::NodeId::fixture("node/z"),
                to: desktop_protocol::NodeId::fixture("node/absent"),
                from_label: "Zeta".into(),
                to_label: "Absent".into(),
                kind: None,
            }),
        ]
        .into();
        let none: std::collections::BTreeSet<desktop_protocol::NodeId> =
            std::collections::BTreeSet::new();
        let no_edges: std::collections::BTreeSet<desktop_protocol::EdgeId> =
            std::collections::BTreeSet::new();
        let drawn = diagram(&cyclic, &none, &no_edges);
        assert!(drawn.cyclic, "a cycle is reported instead of being hidden");
        assert_eq!(
            drawn.skipped_edges, 1,
            "an edge without a drawn endpoint is counted"
        );
        assert_eq!(drawn.edges.len(), 2);

        let mut many = (*base).clone();
        many.nodes = (0..MAX_DIAGRAM_NODES + 5)
            .map(|index| {
                Arc::new(GraphNodeDisplay {
                    identity: desktop_protocol::NodeId::fixture(format!("node/{index}")),
                    label: format!("Node {index}").into(),
                    layer: None,
                })
            })
            .collect::<Vec<_>>()
            .into();
        many.edges = Arc::from([]);
        let no_edges: std::collections::BTreeSet<desktop_protocol::EdgeId> =
            std::collections::BTreeSet::new();
        let drawn = diagram(&many, &none, &no_edges);
        assert!(drawn.truncated);
        assert_eq!(drawn.nodes.len(), MAX_DIAGRAM_NODES);
    }

    #[test]
    fn the_diagram_draws_matching_nodes_first() {
        let mut many = (*graph(3, "digest/three", 8)).clone();
        many.nodes = (0..MAX_DIAGRAM_NODES + 2)
            .map(|index| {
                Arc::new(GraphNodeDisplay {
                    identity: desktop_protocol::NodeId::fixture(format!("node/{index}")),
                    label: format!("Node {index}").into(),
                    layer: None,
                })
            })
            .collect::<Vec<_>>()
            .into();
        many.edges = Arc::from([]);
        let mut matched = std::collections::BTreeSet::new();
        matched.insert(desktop_protocol::NodeId::fixture("node/100"));
        let drawn = diagram(&many, &matched, &std::collections::BTreeSet::new());
        assert_eq!(
            drawn.nodes[0].node,
            desktop_protocol::NodeId::fixture("node/100")
        );
        assert!(drawn.nodes[0].matched);
    }

    #[test]
    fn filtering_preserves_authoritative_order_and_exact_identities() {
        let graphs = [graph(3, "digest/three", 8)];
        let all = rows(&graphs, "");
        assert_eq!(
            all.nodes
                .iter()
                .map(|row| row.node().identity.as_str())
                .collect::<Vec<_>>(),
            ["fixture:node/z", "fixture:node/a", "fixture:node/m"]
        );
        assert_eq!(
            all.edges
                .iter()
                .map(|row| row.edge().identity.as_str())
                .collect::<Vec<_>>(),
            ["fixture:edge/z-a", "fixture:edge/a-m"]
        );
        let filtered = rows(&graphs, "zeta");
        assert_eq!(filtered.nodes.len(), 1);
        assert_eq!(filtered.nodes[0].node().label.as_ref(), "Zeta");
        assert_eq!(filtered.nodes[0].graph.graph_ref, graphs[0].graph_ref);
        assert!(contains_node(&filtered, &filtered.nodes[0].key()));
        assert_eq!(filtered.edges.len(), 1);
        assert_eq!(
            filtered.edges[0].edge().identity.as_str(),
            "fixture:edge/z-a"
        );
        assert_eq!(filtered.edges[0].edge().from.as_str(), "fixture:node/z");

        let by_endpoint = rows(&graphs, "alpha");
        assert_eq!(by_endpoint.nodes.len(), 1);
        assert_eq!(by_endpoint.edges.len(), 2);
        assert_eq!(
            by_endpoint.edges[0].edge().identity.as_str(),
            "fixture:edge/z-a"
        );
        assert_eq!(by_endpoint.edges[0].edge().from.as_str(), "fixture:node/z");
        assert_eq!(by_endpoint.edges[0].edge().to.as_str(), "fixture:node/a");
        assert!(contains_edge(&by_endpoint, &by_endpoint.edges[0].key()));
    }

    #[test]
    fn search_is_case_insensitive_and_matches_layer_or_kind() {
        let graphs = [graph(1, "digest/one", 1)];
        assert_eq!(rows(&graphs, "GOAL").nodes.len(), 1);
        assert_eq!(rows(&graphs, "flow").nodes.len(), 1);
        assert_eq!(rows(&graphs, "flow").edges.len(), 2);
        assert_eq!(rows(&graphs, "fixture:edge/a-m").edges.len(), 1);
        assert_eq!(rows(&graphs, "absent").nodes.len(), 0);
        assert_eq!(rows(&graphs, "absent").edges.len(), 0);
    }

    #[test]
    fn revision_key_distinguishes_canonical_and_projection_revisions() {
        let first = graph(1, "digest/one", 2);
        let same = graph(1, "digest/one", 2);
        let projection_changed = graph(1, "digest/one", 3);
        let canonical_changed = graph(2, "digest/two", 3);
        assert_eq!(
            revision_key(std::slice::from_ref(&first)),
            revision_key(std::slice::from_ref(&same))
        );
        assert_ne!(
            revision_key(std::slice::from_ref(&first)),
            revision_key(&[projection_changed])
        );
        assert_ne!(revision_key(&[first]), revision_key(&[canonical_changed]));
    }

    #[test]
    fn reorder_keeps_node_and_edge_identities_keyed_by_exact_revision() {
        let first = graph(1, "digest/one", 1);
        let mut nodes = first.nodes.iter().cloned().collect::<Vec<_>>();
        nodes.reverse();
        let mut edges = first.edges.iter().cloned().collect::<Vec<_>>();
        edges.reverse();
        let reordered = GraphDisplay {
            graph_ref: first.graph_ref.clone(),
            source_revision: first.source_revision.clone(),
            nodes: nodes.into(),
            edges: edges.into(),
        };
        let before = rows(&[first], "");
        let after = rows(&[Arc::new(reordered)], "");
        let mut before_nodes = before.nodes.iter().map(|row| row.key()).collect::<Vec<_>>();
        let mut after_nodes = after.nodes.iter().map(|row| row.key()).collect::<Vec<_>>();
        before_nodes.sort_by_key(|key| key.node.as_str().to_owned());
        after_nodes.sort_by_key(|key| key.node.as_str().to_owned());
        assert_eq!(before_nodes, after_nodes);
        let mut before_edges = before.edges.iter().map(|row| row.key()).collect::<Vec<_>>();
        let mut after_edges = after.edges.iter().map(|row| row.key()).collect::<Vec<_>>();
        before_edges.sort_by_key(|key| key.edge.as_str().to_owned());
        after_edges.sort_by_key(|key| key.edge.as_str().to_owned());
        assert_eq!(before_edges, after_edges);
    }

    #[test]
    fn large_graph_rows_preserve_order_and_filter_bound() {
        let nodes = (0..10_000)
            .map(|index| {
                Arc::new(GraphNodeDisplay {
                    identity: desktop_protocol::NodeId::fixture(format!("node/{index:05}")),
                    label: format!("Node {index:05}").into(),
                    layer: None,
                })
            })
            .collect::<Vec<_>>();
        let graph = Arc::new(GraphDisplay {
            graph_ref: GraphRef {
                graph_id: GraphId::fixture("graph/large"),
                version: 1,
                digest: ArtifactDigest::fixture("digest/large"),
            },
            source_revision: Counter::from_u128(1),
            nodes: nodes.into(),
            edges: Vec::new().into(),
        });
        let all = rows(&[graph], "");
        assert_eq!(all.nodes.len(), 10_000);
        assert_eq!(all.nodes[0].node().identity.as_str(), "fixture:node/00000");
        assert_eq!(
            all.nodes[9_999].node().identity.as_str(),
            "fixture:node/09999"
        );
        let filtered = rows(&all.graphs, "Node 09999");
        assert_eq!(filtered.nodes.len(), 1);
        assert_eq!(
            filtered.nodes[0].node().identity.as_str(),
            "fixture:node/09999"
        );
    }

    #[test]
    fn cancellable_filter_stops_without_returning_partial_rows() {
        let nodes = (0..10_000)
            .map(|index| {
                Arc::new(GraphNodeDisplay {
                    identity: desktop_protocol::NodeId::fixture(format!("node/{index:05}")),
                    label: format!("Node {index:05}").into(),
                    layer: None,
                })
            })
            .collect::<Vec<_>>();
        let graph = Arc::new(GraphDisplay {
            graph_ref: GraphRef {
                graph_id: GraphId::fixture("graph/cancel"),
                version: 1,
                digest: ArtifactDigest::fixture("digest/cancel"),
            },
            source_revision: Counter::from_u128(1),
            nodes: nodes.into(),
            edges: Vec::new().into(),
        });
        let checks = AtomicUsize::new(0);
        let cancelled = rows_cancellable(&[Arc::clone(&graph)], "Node", || {
            checks.fetch_add(1, Ordering::AcqRel) > 64
        });
        assert!(cancelled.is_none());
        assert!(checks.load(Ordering::Acquire) > 64);
        let complete = rows_cancellable(&[graph], "Node 09999", || false)
            .expect("uncancelled filter completes");
        assert_eq!(complete.nodes.len(), 1);
    }

    #[test]
    fn another_thread_can_cancel_an_active_filter_mid_scan() {
        let nodes = (0..10_000)
            .map(|index| {
                Arc::new(GraphNodeDisplay {
                    identity: desktop_protocol::NodeId::fixture(format!("node/{index:05}")),
                    label: format!("Node {index:05}").into(),
                    layer: None,
                })
            })
            .collect::<Vec<_>>();
        let graphs = vec![Arc::new(GraphDisplay {
            graph_ref: GraphRef {
                graph_id: GraphId::fixture("graph/thread-cancel"),
                version: 1,
                digest: ArtifactDigest::fixture("digest/thread-cancel"),
            },
            source_revision: Counter::from_u128(1),
            nodes: nodes.into(),
            edges: Vec::new().into(),
        })];
        let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let checks = Arc::new(AtomicUsize::new(0));
        let worker = {
            let entered = Arc::clone(&entered);
            let cancel = Arc::clone(&cancel);
            let checks = Arc::clone(&checks);
            std::thread::spawn(move || {
                rows_cancellable(&graphs, "Node", || {
                    let count = checks.fetch_add(1, Ordering::AcqRel) + 1;
                    if count == 100 {
                        entered.store(true, Ordering::Release);
                        while !cancel.load(Ordering::Acquire) {
                            std::thread::yield_now();
                        }
                        return true;
                    }
                    false
                })
            })
        };
        while !entered.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        cancel.store(true, Ordering::Release);
        let result = worker.join().expect("filter thread completed");
        assert!(result.is_none());
        assert!(checks.load(Ordering::Acquire) >= 100);
    }

    #[test]
    fn admission_identity_rejects_every_superseded_dimension() {
        let graph = graph(3, "digest/three", 8);
        let key = AdmissionKey {
            session_generation: 2,
            publication_version: 9,
            epoch: Some(EpochId::fixture("epoch/one")),
            revision: revision_key(&[graph]),
            query: "goal".to_owned(),
        };
        for changed in [
            AdmissionKey {
                session_generation: 3,
                ..key.clone()
            },
            AdmissionKey {
                publication_version: 10,
                ..key.clone()
            },
            AdmissionKey {
                epoch: Some(EpochId::fixture("epoch/two")),
                ..key.clone()
            },
            AdmissionKey {
                query: "context".to_owned(),
                ..key.clone()
            },
        ] {
            assert_ne!(key, changed);
        }
        assert!(admit(&key, Some(&key), Some(&key)));
        assert!(!admit(&key, None, Some(&key)));
        assert!(!admit(&key, Some(&key), None));
        let newer = AdmissionKey {
            query: "newer".to_owned(),
            ..key.clone()
        };
        assert!(!admit(&key, Some(&newer), Some(&newer)));
        assert!(!admit(&newer, Some(&newer), Some(&key)));
    }

    #[test]
    fn duplicate_identities_in_distinct_graph_revisions_have_distinct_keys() {
        let first = graph(1, "digest/one", 1);
        let second = graph(2, "digest/two", 2);
        let all = rows(&[first, second], "");
        assert_eq!(all.nodes.len(), 6);
        assert_ne!(all.nodes[0].key(), all.nodes[3].key());
        assert_ne!(all.edges[0].key(), all.edges[2].key());
    }
}
