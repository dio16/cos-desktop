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
    /// Authoritative connection kind, copied from the projection.
    pub kind: Option<Arc<str>>,
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

/// One orthogonal connector segment in drawing coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

/// Layer and cycle result for one complete graph revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layers {
    /// Longest-path depth per node, in authoritative node order.
    pub depth: Vec<usize>,
    /// The topology has a cycle; depth stays a hint rather than a rank.
    pub cyclic: bool,
}

/// Compute layers for the complete graph in linear time.
///
/// Strongly connected components are condensed iteratively (no recursion), so a
/// cycle never leaves a node without a layer and the whole pass is `O(V + E)`
/// using expected-constant lookups and stamp-based edge deduplication.
/// Members of one component share a layer, which is the honest answer for a
/// cyclic region.
pub fn layers(graph: &GraphDisplay) -> Layers {
    let count = graph.nodes.len();
    let index_of: std::collections::HashMap<NodeId, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.identity.clone(), index))
        .collect();
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut self_loop = false;
    for edge in graph.edges.iter() {
        let (Some(&from), Some(&to)) = (index_of.get(&edge.from), index_of.get(&edge.to)) else {
            continue;
        };
        if from == to {
            self_loop = true;
        }
        successors[from].push(to);
    }

    // Iterative Tarjan: one frame per node, no host-stack recursion.
    const UNVISITED: usize = usize::MAX;
    let mut index = vec![UNVISITED; count];
    let mut low = vec![0usize; count];
    let mut on_stack = vec![false; count];
    let mut stack: Vec<usize> = Vec::new();
    let mut component = vec![UNVISITED; count];
    let mut component_sizes: Vec<usize> = Vec::new();
    let mut next_index = 0usize;
    for root in 0..count {
        if index[root] != UNVISITED {
            continue;
        }
        index[root] = next_index;
        low[root] = next_index;
        next_index += 1;
        stack.push(root);
        on_stack[root] = true;
        let mut frames: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some(&(node, child)) = frames.last() {
            if child < successors[node].len() {
                let target = successors[node][child];
                frames.last_mut().expect("frame").1 += 1;
                if index[target] == UNVISITED {
                    index[target] = next_index;
                    low[target] = next_index;
                    next_index += 1;
                    stack.push(target);
                    on_stack[target] = true;
                    frames.push((target, 0));
                } else if on_stack[target] {
                    low[node] = low[node].min(index[target]);
                }
                continue;
            }
            frames.pop();
            if let Some(&(parent, _)) = frames.last() {
                low[parent] = low[parent].min(low[node]);
            }
            if low[node] == index[node] {
                let id = component_sizes.len();
                let mut size = 0usize;
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    component[member] = id;
                    size += 1;
                    if member == node {
                        break;
                    }
                }
                component_sizes.push(size);
            }
        }
    }

    // Condense and take the longest path over the component DAG.
    let components = component_sizes.len();
    // Parallel condensation edges are skipped with a per-target stamp, so the
    // whole pass stays O(V + E) with no ordered-set insertion.
    let mut component_successors: Vec<Vec<usize>> = vec![Vec::new(); components];
    let mut last_source = vec![0usize; components];
    let mut indegree = vec![0usize; components];
    for from in 0..count {
        for &to in &successors[from] {
            let (a, b) = (component[from], component[to]);
            if a != b && last_source[b] != a + 1 {
                last_source[b] = a + 1;
                component_successors[a].push(b);
                indegree[b] += 1;
            }
        }
    }
    let mut depth_component = vec![0usize; components];
    let mut queue: std::collections::VecDeque<usize> =
        (0..components).filter(|id| indegree[*id] == 0).collect();
    while let Some(id) = queue.pop_front() {
        let next_depth = depth_component[id] + 1;
        for &next in &component_successors[id] {
            depth_component[next] = depth_component[next].max(next_depth);
            indegree[next] -= 1;
            if indegree[next] == 0 {
                queue.push_back(next);
            }
        }
    }
    let cyclic = self_loop || component_sizes.iter().any(|size| *size > 1);
    let depth = (0..count)
        .map(|node| depth_component[component[node]])
        .collect();
    Layers { depth, cyclic }
}

/// Vertical room reserved above the first card row when a drawing has
/// same-layer or self-loop connectors, so their lines and markers stay inside
/// the canvas instead of using negative coordinates.
pub const SAME_LAYER_HEADROOM: f32 = 26.0;

/// Whether a drawing contains a self loop, which routes to the right of its card.
pub fn has_self_loop(edges: &[DiagramEdge]) -> bool {
    edges.iter().any(|edge| edge.from == edge.to)
}

/// One connector plus the marker that points at the projected target.
#[derive(Clone, Debug, PartialEq)]
pub struct Connector {
    pub segments: Vec<Segment>,
    /// Marker box top-left.
    pub marker: (f32, f32),
    pub glyph: &'static str,
}

/// Route an orthogonal connector between two node cards.
///
/// Downward and upward edges use a vertical channel between the cards. Edges
/// between cards on the same layer and self loops are routed above the cards, so
/// the line and its marker are never hidden by a node box.
pub fn connector(
    from_center_x: f32,
    from_top: f32,
    to_center_x: f32,
    to_top: f32,
    same_node: bool,
    card_half_width: f32,
    thickness: f32,
) -> Connector {
    let segment = |left: f32, top: f32, width: f32, height: f32| Segment {
        left,
        top,
        width: width.max(thickness),
        height: height.max(thickness),
    };
    if same_node {
        // Leave the same card, run down the outside channel and return to the card
        // just above its top edge, so the marker touches the projected target (the
        // node itself) while the line never crosses the card box.
        let channel_x = from_center_x + card_half_width + 12.0;
        let channel_top = from_top - 12.0;
        let return_x = from_center_x + 18.0;
        let return_y = from_top - 2.0;
        return Connector {
            segments: vec![
                segment(
                    from_center_x - thickness / 2.0,
                    channel_top,
                    thickness,
                    12.0,
                ),
                segment(
                    from_center_x,
                    channel_top,
                    channel_x - from_center_x,
                    thickness,
                ),
                segment(
                    channel_x - thickness / 2.0,
                    channel_top,
                    thickness,
                    return_y - channel_top,
                ),
                segment(
                    return_x,
                    return_y - thickness / 2.0,
                    channel_x - return_x,
                    thickness,
                ),
            ],
            marker: (return_x - 5.0, from_top - 11.0),
            glyph: "▼",
        };
    }
    if (from_top - to_top).abs() < 0.5 {
        let channel_top = from_top - 12.0;
        return Connector {
            segments: vec![
                segment(
                    from_center_x - thickness / 2.0,
                    channel_top,
                    thickness,
                    12.0,
                ),
                segment(
                    from_center_x.min(to_center_x),
                    channel_top,
                    (from_center_x - to_center_x).abs(),
                    thickness,
                ),
                segment(to_center_x - thickness / 2.0, channel_top, thickness, 12.0),
            ],
            marker: (to_center_x - 5.0, to_top - 11.0),
            glyph: "▼",
        };
    }
    let target_above = to_top < from_top;
    let (marker, glyph) = if target_above {
        ((to_center_x - 5.0, to_top + 1.0), "▲")
    } else {
        ((to_center_x - 5.0, to_top - 11.0), "▼")
    };
    if (from_center_x - to_center_x).abs() < 0.5 {
        let top = from_top.min(to_top);
        return Connector {
            segments: vec![segment(
                from_center_x - thickness / 2.0,
                top,
                thickness,
                (to_top - from_top).abs(),
            )],
            marker,
            glyph,
        };
    }
    let mid = (from_top + to_top) / 2.0;
    Connector {
        segments: vec![
            segment(
                from_center_x - thickness / 2.0,
                from_top.min(mid),
                thickness,
                (mid - from_top).abs(),
            ),
            segment(
                from_center_x.min(to_center_x),
                mid - thickness / 2.0,
                (from_center_x - to_center_x).abs(),
                thickness,
            ),
            segment(
                to_center_x - thickness / 2.0,
                mid.min(to_top),
                thickness,
                (to_top - mid).abs(),
            ),
        ],
        marker,
        glyph,
    }
}

/// Lay out one authoritative graph without inventing topology or labels.
///
/// Depth comes from the complete graph's `layers`, so truncating the drawing
/// never moves a drawn node to a wrong layer and a cycle through an omitted node
/// is still reported. Matching identities only decide which nodes are drawn
/// first when the cap applies; the drawn set is then restored to authoritative
/// order, so search emphasis never reorders a layer. Every label, layer, kind and
/// identity is copied from the projection.
pub fn diagram(
    graph: &GraphDisplay,
    layers: &Layers,
    matched_nodes: &std::collections::BTreeSet<(GraphRef, NodeId)>,
    matched_edges: &std::collections::BTreeSet<(GraphRef, EdgeId)>,
) -> Diagram {
    let is_matched_node =
        |identity: &NodeId| matched_nodes.contains(&(graph.graph_ref.clone(), identity.clone()));
    let mut selected: Vec<usize> = (0..graph.nodes.len())
        .filter(|index| is_matched_node(&graph.nodes[*index].identity))
        .chain(
            (0..graph.nodes.len()).filter(|index| !is_matched_node(&graph.nodes[*index].identity)),
        )
        .take(MAX_DIAGRAM_NODES)
        .collect();
    selected.sort_unstable();
    let truncated = selected.len() < graph.nodes.len();
    let drawn: std::collections::BTreeMap<NodeId, usize> = selected
        .iter()
        .enumerate()
        .map(|(position, index)| (graph.nodes[*index].identity.clone(), position))
        .collect();

    let mut drawn_edges = Vec::new();
    let mut skipped_edges = 0;
    for edge in graph.edges.iter() {
        if !drawn.contains_key(&edge.from) || !drawn.contains_key(&edge.to) {
            skipped_edges += 1;
            continue;
        }
        drawn_edges.push(DiagramEdge {
            edge: edge.identity.clone(),
            from: edge.from.clone(),
            to: edge.to.clone(),
            kind: edge.kind.clone(),
            matched: matched_edges.contains(&(graph.graph_ref.clone(), edge.identity.clone())),
        });
    }

    let mut lanes: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    let mut depth_count = 0usize;
    let nodes = selected
        .iter()
        .map(|index| {
            let node = &graph.nodes[*index];
            let node_depth = layers.depth.get(*index).copied().unwrap_or(0);
            depth_count = depth_count.max(node_depth + 1);
            let lane = lanes.entry(node_depth).or_insert(0);
            let assigned = *lane;
            *lane += 1;
            DiagramNode {
                node: node.identity.clone(),
                label: Arc::clone(&node.label),
                layer: node.layer.clone(),
                depth: node_depth,
                lane: assigned,
                matched: is_matched_node(&node.identity),
            }
        })
        .collect();
    let lane_count = lanes.values().copied().max().unwrap_or(0);

    Diagram {
        nodes,
        edges: drawn_edges,
        depth_count,
        lane_count,
        truncated,
        skipped_edges,
        cyclic: layers.cyclic,
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
        AdmissionKey, MAX_DIAGRAM_NODES, SAME_LAYER_HEADROOM, admit, connector, contains_edge,
        contains_node, diagram, layers, revision_key, rows, rows_cancellable,
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
        let drawn = diagram(
            &graph,
            &layers(&graph),
            &std::collections::BTreeSet::new(),
            &std::collections::BTreeSet::new(),
        );
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
        assert_eq!(
            drawn.edges[0].kind.as_deref(),
            Some("FLOW"),
            "the projected connection kind is carried into the drawing"
        );
    }

    #[test]
    fn the_diagram_keeps_projection_labels_and_never_invents_one() {
        let graph = graph(3, "digest/three", 8);
        let drawn = diagram(
            &graph,
            &layers(&graph),
            &std::collections::BTreeSet::new(),
            &std::collections::BTreeSet::new(),
        );
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
    fn a_search_match_marks_without_reordering_a_layer() {
        let base = graph(3, "digest/three", 8);
        let mut wide = (*base).clone();
        wide.nodes = vec![
            Arc::new(GraphNodeDisplay {
                identity: desktop_protocol::NodeId::fixture("node/first"),
                label: "First".into(),
                layer: None,
            }),
            Arc::new(GraphNodeDisplay {
                identity: desktop_protocol::NodeId::fixture("node/second"),
                label: "Second".into(),
                layer: None,
            }),
        ]
        .into();
        wide.edges = Arc::from([]);
        let mut matched: std::collections::BTreeSet<(GraphRef, desktop_protocol::NodeId)> =
            std::collections::BTreeSet::new();
        matched.insert((
            wide.graph_ref.clone(),
            desktop_protocol::NodeId::fixture("node/second"),
        ));
        let drawn = diagram(
            &wide,
            &layers(&wide),
            &matched,
            &std::collections::BTreeSet::new(),
        );
        assert_eq!(
            drawn
                .nodes
                .iter()
                .map(|node| node.node.as_str())
                .collect::<Vec<_>>(),
            vec![
                desktop_protocol::NodeId::fixture("node/first").as_str(),
                desktop_protocol::NodeId::fixture("node/second").as_str()
            ],
            "the layer keeps authoritative order even when the second node matches"
        );
        assert!(!drawn.nodes[0].matched);
        assert!(drawn.nodes[1].matched);
    }

    #[test]
    fn truncation_keeps_full_graph_depth_and_cycle_reporting() {
        let base = graph(3, "digest/three", 8);
        let mut chain = (*base).clone();
        let count = MAX_DIAGRAM_NODES + 2;
        chain.nodes = (0..count)
            .map(|index| {
                Arc::new(GraphNodeDisplay {
                    identity: desktop_protocol::NodeId::fixture(format!("node/{index}")),
                    label: format!("Node {index}").into(),
                    layer: None,
                })
            })
            .collect::<Vec<_>>()
            .into();
        chain.edges = (1..count)
            .map(|index| {
                Arc::new(GraphEdgeDisplay {
                    identity: desktop_protocol::EdgeId::fixture(format!("edge/{index}")),
                    from: desktop_protocol::NodeId::fixture(format!("node/{}", index - 1)),
                    to: desktop_protocol::NodeId::fixture(format!("node/{index}")),
                    from_label: format!("Node {}", index - 1).into(),
                    to_label: format!("Node {index}").into(),
                    kind: Some("FLOW".into()),
                })
            })
            .collect::<Vec<_>>()
            .into();
        let mut matched: std::collections::BTreeSet<(GraphRef, desktop_protocol::NodeId)> =
            std::collections::BTreeSet::new();
        matched.insert((
            chain.graph_ref.clone(),
            desktop_protocol::NodeId::fixture(format!("node/{}", count - 1)),
        ));
        let drawn = diagram(
            &chain,
            &layers(&chain),
            &matched,
            &std::collections::BTreeSet::new(),
        );
        assert!(drawn.truncated);
        let last = drawn
            .nodes
            .iter()
            .find(|node| node.matched)
            .expect("the matched tail is drawn");
        assert_eq!(
            last.depth,
            count - 1,
            "a drawn node keeps the depth computed over the complete chain"
        );

        let mut cycle = (*base).clone();
        cycle.nodes = vec![
            Arc::new(GraphNodeDisplay {
                identity: desktop_protocol::NodeId::fixture("node/a"),
                label: "Alpha".into(),
                layer: None,
            }),
            Arc::new(GraphNodeDisplay {
                identity: desktop_protocol::NodeId::fixture("node/b"),
                label: "Beta".into(),
                layer: None,
            }),
        ]
        .into();
        cycle.edges = vec![
            Arc::new(GraphEdgeDisplay {
                identity: desktop_protocol::EdgeId::fixture("edge/a-b"),
                from: desktop_protocol::NodeId::fixture("node/a"),
                to: desktop_protocol::NodeId::fixture("node/b"),
                from_label: "Alpha".into(),
                to_label: "Beta".into(),
                kind: Some("FLOW".into()),
            }),
            Arc::new(GraphEdgeDisplay {
                identity: desktop_protocol::EdgeId::fixture("edge/b-a"),
                from: desktop_protocol::NodeId::fixture("node/b"),
                to: desktop_protocol::NodeId::fixture("node/a"),
                from_label: "Beta".into(),
                to_label: "Alpha".into(),
                kind: Some("FLOW".into()),
            }),
        ]
        .into();
        let drawn = diagram(
            &cycle,
            &layers(&cycle),
            &std::collections::BTreeSet::new(),
            &std::collections::BTreeSet::new(),
        );
        assert!(drawn.cyclic, "a cycle is reported instead of being hidden");
    }

    #[test]
    fn emphasis_is_scoped_to_the_exact_graph_revision() {
        let graph = graph(3, "digest/three", 8);
        let mut matched: std::collections::BTreeSet<(GraphRef, desktop_protocol::NodeId)> =
            std::collections::BTreeSet::new();
        matched.insert((
            GraphRef {
                graph_id: GraphId::fixture("graph/other"),
                version: 3,
                digest: ArtifactDigest::fixture("digest/three"),
            },
            desktop_protocol::NodeId::fixture("node/a"),
        ));
        let drawn = diagram(
            &graph,
            &layers(&graph),
            &matched,
            &std::collections::BTreeSet::new(),
        );
        assert!(
            drawn.nodes.iter().all(|node| !node.matched),
            "a match in another revision never highlights this drawing"
        );
    }

    #[test]
    fn the_diagram_states_missing_endpoints_and_the_cap() {
        let base = graph(3, "digest/three", 8);
        let mut missing = (*base).clone();
        missing.edges = vec![Arc::new(GraphEdgeDisplay {
            identity: desktop_protocol::EdgeId::fixture("edge/absent"),
            from: desktop_protocol::NodeId::fixture("node/z"),
            to: desktop_protocol::NodeId::fixture("node/absent"),
            from_label: "Zeta".into(),
            to_label: "Absent".into(),
            kind: None,
        })]
        .into();
        let drawn = diagram(
            &missing,
            &layers(&missing),
            &std::collections::BTreeSet::new(),
            &std::collections::BTreeSet::new(),
        );
        assert_eq!(drawn.skipped_edges, 1);
        assert!(drawn.edges.is_empty());
    }

    #[test]
    fn layers_run_in_linear_time_on_a_large_chain() {
        let base = graph(3, "digest/three", 8);
        let count = 10_000usize;
        let mut chain = (*base).clone();
        chain.nodes = (0..count)
            .map(|index| {
                Arc::new(GraphNodeDisplay {
                    identity: desktop_protocol::NodeId::fixture(format!("node/{index}")),
                    label: format!("Node {index}").into(),
                    layer: None,
                })
            })
            .collect::<Vec<_>>()
            .into();
        chain.edges = (1..count)
            .map(|index| {
                Arc::new(GraphEdgeDisplay {
                    identity: desktop_protocol::EdgeId::fixture(format!("edge/{index}")),
                    from: desktop_protocol::NodeId::fixture(format!("node/{}", index - 1)),
                    to: desktop_protocol::NodeId::fixture(format!("node/{index}")),
                    from_label: format!("Node {}", index - 1).into(),
                    to_label: format!("Node {index}").into(),
                    kind: Some("FLOW".into()),
                })
            })
            .collect::<Vec<_>>()
            .into();
        let result = layers(&chain);
        assert!(!result.cyclic);
        assert_eq!(result.depth.last().copied(), Some(count - 1));
        assert_eq!(result.depth[0], 0);
    }

    #[test]
    fn layers_give_cycle_members_one_shared_layer() {
        let base = graph(3, "digest/three", 8);
        let mut cycle = (*base).clone();
        cycle.nodes = vec![
            Arc::new(GraphNodeDisplay {
                identity: desktop_protocol::NodeId::fixture("node/a"),
                label: "Alpha".into(),
                layer: None,
            }),
            Arc::new(GraphNodeDisplay {
                identity: desktop_protocol::NodeId::fixture("node/b"),
                label: "Beta".into(),
                layer: None,
            }),
        ]
        .into();
        cycle.edges = vec![
            Arc::new(GraphEdgeDisplay {
                identity: desktop_protocol::EdgeId::fixture("edge/a-b"),
                from: desktop_protocol::NodeId::fixture("node/a"),
                to: desktop_protocol::NodeId::fixture("node/b"),
                from_label: "Alpha".into(),
                to_label: "Beta".into(),
                kind: Some("FLOW".into()),
            }),
            Arc::new(GraphEdgeDisplay {
                identity: desktop_protocol::EdgeId::fixture("edge/b-a"),
                from: desktop_protocol::NodeId::fixture("node/b"),
                to: desktop_protocol::NodeId::fixture("node/a"),
                from_label: "Beta".into(),
                to_label: "Alpha".into(),
                kind: Some("FLOW".into()),
            }),
        ]
        .into();
        let result = layers(&cycle);
        assert!(result.cyclic);
        assert_eq!(result.depth, vec![0, 0]);

        let mut self_loop = (*base).clone();
        self_loop.edges = vec![Arc::new(GraphEdgeDisplay {
            identity: desktop_protocol::EdgeId::fixture("edge/z-z"),
            from: desktop_protocol::NodeId::fixture("node/z"),
            to: desktop_protocol::NodeId::fixture("node/z"),
            from_label: "Zeta".into(),
            to_label: "Zeta".into(),
            kind: None,
        })]
        .into();
        assert!(layers(&self_loop).cyclic);
    }

    #[test]
    fn connectors_stay_valid_and_visible_in_every_direction() {
        let card_top = 100.0;
        let half = 68.0;
        let down = connector(10.0, 0.0, 10.0, card_top, false, half, 2.0);
        assert_eq!(down.glyph, "▼");
        assert_eq!(down.segments.len(), 1);
        assert!(down.segments[0].height > 0.0);

        let up = connector(10.0, 160.0, 10.0, card_top, false, half, 2.0);
        assert_eq!(up.glyph, "▲");
        assert_eq!(up.segments.len(), 1);
        assert!(up.segments[0].height > 0.0 && up.segments[0].top == card_top);

        let same_layer = connector(10.0, card_top, 90.0, card_top, false, half, 2.0);
        assert_eq!(same_layer.glyph, "▼");
        for segment in &same_layer.segments {
            assert!(
                segment.top + segment.height <= card_top,
                "a same-layer edge stays above the node cards"
            );
        }

        let self_loop = connector(10.0, card_top, 10.0, card_top, true, half, 2.0);
        assert_eq!(self_loop.glyph, "▼");
        for segment in &self_loop.segments {
            assert!(
                segment.top + segment.height <= card_top,
                "the loop never crosses the card box"
            );
        }
        let channel = self_loop
            .segments
            .iter()
            .map(|segment| segment.left)
            .fold(f32::MIN, f32::max);
        assert!(
            channel >= 10.0 + half,
            "the loop channel sits outside the card: {channel}"
        );
        let return_leg = self_loop.segments.last().expect("return leg");
        assert!(
            return_leg.left < 10.0 + half && return_leg.left + return_leg.width > 10.0,
            "the loop returns over the same card"
        );
        let marker_center = self_loop.marker.0 + 5.0;
        assert!(
            (marker_center - (10.0 + 18.0)).abs() < 0.5,
            "the marker points at the same card's top edge"
        );
        assert!(self_loop.marker.1 + 11.0 <= card_top);

        let bent = connector(10.0, 0.0, 90.0, 60.0, false, half, 2.0);
        assert_eq!(bent.segments.len(), 3);
        for segment in &bent.segments {
            assert!(segment.width > 0.0 && segment.height > 0.0);
        }
    }

    #[test]
    fn the_same_layer_route_fits_the_reserved_headroom() {
        // A depth-0 card sits at the headroom offset, so every part of the
        // same-layer route must stay at or below zero-free canvas coordinates.
        let card_top = std::hint::black_box(SAME_LAYER_HEADROOM);
        let half = 68.0;
        let route = connector(10.0, card_top, 90.0, card_top, false, half, 2.0);
        for segment in &route.segments {
            assert!(segment.top >= 0.0);
            assert!(segment.top + segment.height <= card_top);
        }
        assert!(route.marker.1 >= 0.0 && route.marker.1 + 11.0 <= card_top);
        assert!(card_top >= 20.0, "the kind label fits too");

        let self_route = connector(10.0, card_top, 10.0, card_top, true, half, 2.0);
        for segment in &self_route.segments {
            assert!(segment.top >= 0.0);
        }
        assert!(self_route.marker.1 >= 0.0);
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
