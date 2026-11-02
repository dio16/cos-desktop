//! Flow's non-canvas inspection model.
//!
//! Rows are pure views over the authoritative graph display. Filtering and
//! keyboard cursors never add, remove or redirect edges.

use std::collections::HashSet;
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

/// The revision-wide match sets the diagram uses for emphasis, built from the admitted
/// inspection rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Matched {
    pub nodes: HashSet<(GraphRef, NodeId)>,
    pub edges: HashSet<(GraphRef, EdgeId)>,
}

/// Bounded (single-entry) cache of the match sets, owned by the shell.
///
/// It is keyed by the **complete** [`AdmissionKey`] — session generation, publication
/// version, epoch, revision and query — so an authorization or session change can never
/// reuse a stale set. It never grows past one entry; a miss replaces it. The cache holds
/// only the match sets: the diagram itself is rebuilt from them.
#[derive(Debug, Default)]
pub struct MatchCache {
    entry: Option<(AdmissionKey, Arc<Matched>)>,
}

impl MatchCache {
    /// Return the cached sets for `key`, or build them once with `build`.
    ///
    /// The returned flag is `true` for a hit. A hit never calls `build`.
    pub fn get_or_build(
        &mut self,
        key: &AdmissionKey,
        build: impl FnOnce() -> Matched,
    ) -> (Arc<Matched>, bool) {
        if let Some((cached, matched)) = self.entry.as_ref()
            && cached == key
        {
            return (Arc::clone(matched), true);
        }
        let matched = Arc::new(build());
        self.entry = Some((key.clone(), Arc::clone(&matched)));
        (matched, false)
    }

    /// Drop the cached entry: authorization loss, scope switch or revision discard.
    pub fn clear(&mut self) {
        self.entry = None;
    }

    /// Number of cached entries; always 0 or 1.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        usize::from(self.entry.is_some())
    }

    /// Whether the cache holds no entry.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entry.is_none()
    }
}

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
    matched_nodes: &HashSet<(GraphRef, NodeId)>,
    matched_edges: &HashSet<(GraphRef, EdgeId)>,
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
        AdmissionKey, MAX_DIAGRAM_NODES, MatchCache, Matched, RevisionKey, SAME_LAYER_HEADROOM,
        admit, connector, contains_edge, contains_node, diagram, layers, revision_key, rows,
        rows_cancellable,
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
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
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
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
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
        let mut matched: std::collections::HashSet<(GraphRef, desktop_protocol::NodeId)> =
            std::collections::HashSet::new();
        matched.insert((
            wide.graph_ref.clone(),
            desktop_protocol::NodeId::fixture("node/second"),
        ));
        let drawn = diagram(
            &wide,
            &layers(&wide),
            &matched,
            &std::collections::HashSet::new(),
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
        let mut matched: std::collections::HashSet<(GraphRef, desktop_protocol::NodeId)> =
            std::collections::HashSet::new();
        matched.insert((
            chain.graph_ref.clone(),
            desktop_protocol::NodeId::fixture(format!("node/{}", count - 1)),
        ));
        let drawn = diagram(
            &chain,
            &layers(&chain),
            &matched,
            &std::collections::HashSet::new(),
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
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
        );
        assert!(drawn.cyclic, "a cycle is reported instead of being hidden");
    }

    #[test]
    fn emphasis_is_scoped_to_the_exact_graph_revision() {
        let graph = graph(3, "digest/three", 8);
        let mut matched: std::collections::HashSet<(GraphRef, desktop_protocol::NodeId)> =
            std::collections::HashSet::new();
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
            &std::collections::HashSet::new(),
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
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
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
    fn matched_from_rows(rows: &super::Rows) -> Matched {
        Matched {
            nodes: rows
                .nodes
                .iter()
                .map(|row| (row.graph.graph_ref.clone(), row.node().identity.clone()))
                .collect(),
            edges: rows
                .edges
                .iter()
                .map(|row| (row.graph.graph_ref.clone(), row.edge().identity.clone()))
                .collect(),
        }
    }

    fn cache_key(
        session_generation: u64,
        publication_version: u64,
        epoch: Option<EpochId>,
        revision: RevisionKey,
        query: &str,
    ) -> AdmissionKey {
        AdmissionKey {
            session_generation,
            publication_version,
            epoch,
            revision,
            query: query.to_string(),
        }
    }

    /// Stage C: the match-set cache must hit only on the complete admission identity,
    /// and every key component must invalidate it.
    #[test]
    fn the_match_cache_invalidates_on_every_admission_key_component() {
        let source_graph = graph(1, "digest/one", 4);
        let graphs = [Arc::clone(&source_graph)];
        let rows = rows(&graphs, "");
        let base = cache_key(1, 2, None, revision_key(&graphs), "");
        let builds = std::cell::Cell::new(0usize);
        let mut cache = MatchCache::default();

        let (first, hit) = cache.get_or_build(&base, || {
            builds.set(builds.get() + 1);
            matched_from_rows(&rows)
        });
        assert!(!hit, "the first request is a miss");
        assert_eq!(cache.len(), 1);
        let (cached, hit) = cache.get_or_build(&base, || panic!("a hit must not rebuild"));
        assert!(hit);
        assert_eq!(first, cached);
        assert_eq!(builds.get(), 1);

        let other_graph = graph(1, "digest/two", 4);
        let other_revision = revision_key(&[Arc::clone(&other_graph)]);
        let variants = [
            cache_key(2, 2, None, base.revision.clone(), ""),
            cache_key(1, 3, None, base.revision.clone(), ""),
            cache_key(
                1,
                2,
                Some(EpochId::fixture("epoch/one")),
                base.revision.clone(),
                "",
            ),
            cache_key(1, 2, None, other_revision, ""),
            cache_key(1, 2, None, base.revision.clone(), "alpha"),
        ];
        for (index, variant) in variants.iter().enumerate() {
            assert_ne!(variant, &base, "variant {index} must actually differ");
            let (_, hit) = cache.get_or_build(variant, || {
                builds.set(builds.get() + 1);
                matched_from_rows(&rows)
            });
            assert!(!hit, "variant {index} must invalidate the cache");
            assert_eq!(cache.len(), 1, "the cache never grows past one entry");
        }
        assert_eq!(builds.get(), 1 + variants.len());
    }

    /// Stage C: a hit feeds `diagram` exactly the same structure as an uncached build.
    #[test]
    fn a_cached_match_set_draws_the_same_diagram_as_an_uncached_build() {
        let source_graph = graph(1, "digest/one", 4);
        let graphs = [Arc::clone(&source_graph)];
        let rows = rows(&graphs, "");
        let key = cache_key(1, 1, None, revision_key(&graphs), "");
        let cached_layers = layers(&source_graph);
        let mut cache = MatchCache::default();
        let (cached, _) = cache.get_or_build(&key, || matched_from_rows(&rows));
        let fresh = matched_from_rows(&rows);
        let cached_diagram = diagram(&source_graph, &cached_layers, &cached.nodes, &cached.edges);
        let fresh_diagram = diagram(&source_graph, &cached_layers, &fresh.nodes, &fresh.edges);
        assert_eq!(cached_diagram, fresh_diagram);
        assert_eq!(cached_diagram.nodes.len(), fresh_diagram.nodes.len());
        assert_eq!(cached_diagram.depth_count, fresh_diagram.depth_count);
        assert_eq!(cached_diagram.lane_count, fresh_diagram.lane_count);
    }

    /// Stage C: clearing (authorization loss / scope switch) forces a rebuild even for the
    /// same key, and the cache refills to one entry.
    #[test]
    fn the_match_cache_rebuilds_after_clear() {
        let source_graph = graph(1, "digest/one", 4);
        let graphs = [Arc::clone(&source_graph)];
        let rows = rows(&graphs, "");
        let key = cache_key(1, 1, None, revision_key(&graphs), "");
        let mut cache = MatchCache::default();
        let (_, hit) = cache.get_or_build(&key, || matched_from_rows(&rows));
        assert!(!hit);
        let (_, hit) = cache.get_or_build(&key, || panic!("a hit must not rebuild"));
        assert!(hit);
        cache.clear();
        assert_eq!(cache.len(), 0, "clearing drops the retained identities");
        let builds = std::cell::Cell::new(0usize);
        let (_, hit) = cache.get_or_build(&key, || {
            builds.set(builds.get() + 1);
            matched_from_rows(&rows)
        });
        assert!(!hit, "a cleared cache rebuilds for the same key");
        assert_eq!(builds.get(), 1);
        assert_eq!(cache.len(), 1);
    }

    /// Stage C: boundedness under many distinct keys.
    #[test]
    fn the_match_cache_stays_bounded_under_many_keys() {
        let source_graph = graph(1, "digest/one", 4);
        let graphs = [Arc::clone(&source_graph)];
        let rows = rows(&graphs, "");
        let mut cache = MatchCache::default();
        assert!(cache.is_empty());
        for generation in 0..16u64 {
            cache.get_or_build(
                &cache_key(generation, 1, None, revision_key(&graphs), ""),
                || matched_from_rows(&rows),
            );
            assert_eq!(cache.len(), 1);
        }
        assert!(!cache.is_empty());
    }

    // ---------------------------------------------------------------------------------
    // MP-FLOW2: 10,000-node measurement (explicit run only).
    // ---------------------------------------------------------------------------------

    /// Build a deterministic 100-layer × 100-node synthetic DAG: every node has two
    /// edges into the next layer, so the longest-path depth equals the construction
    /// layer and the expected reference is known without running the implementation.
    fn ten_thousand_node_graph(layer_count: usize, width: usize) -> GraphDisplay {
        let node_count = layer_count * width;
        let mut nodes = Vec::with_capacity(node_count);
        for layer in 0..layer_count {
            for lane in 0..width {
                let index = layer * width + lane;
                nodes.push(Arc::new(GraphNodeDisplay {
                    identity: desktop_protocol::NodeId::fixture(format!("node/{index}")),
                    label: Arc::from(format!("Node {index}")),
                    layer: Some(Arc::from(format!("L{layer}"))),
                }));
            }
        }
        let mut edges = Vec::with_capacity((layer_count - 1) * width * 2);
        for layer in 0..(layer_count - 1) {
            for lane in 0..width {
                let from = layer * width + lane;
                for step in 0..2 {
                    let target_lane = (lane * 7 + step * 13 + layer * 3) % width;
                    let to = (layer + 1) * width + target_lane;
                    edges.push(Arc::new(GraphEdgeDisplay {
                        identity: desktop_protocol::EdgeId::fixture(format!("edge/{from}-{to}")),
                        from: desktop_protocol::NodeId::fixture(format!("node/{from}")),
                        to: desktop_protocol::NodeId::fixture(format!("node/{to}")),
                        from_label: Arc::from(format!("Node {from}")),
                        to_label: Arc::from(format!("Node {to}")),
                        kind: Some(Arc::from("FLOW")),
                    }));
                }
            }
        }
        GraphDisplay {
            graph_ref: GraphRef {
                graph_id: GraphId::fixture("graph/flow-bench"),
                version: 1,
                digest: ArtifactDigest::fixture("digest/flow-bench"),
            },
            source_revision: Counter::from_u128(1),
            nodes: nodes.into(),
            edges: edges.into(),
        }
    }

    /// Independent longest-path reference (Kahn), used only for the equivalence check.
    fn reference_depths(graph: &GraphDisplay) -> Vec<usize> {
        use std::collections::{HashMap, VecDeque};
        let index_of: HashMap<desktop_protocol::NodeId, usize> = graph
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.identity.clone(), index))
            .collect();
        let count = graph.nodes.len();
        let mut indegree = vec![0usize; count];
        let mut successors: Vec<Vec<usize>> = vec![Vec::new(); count];
        for edge in graph.edges.iter() {
            let (Some(&from), Some(&to)) = (index_of.get(&edge.from), index_of.get(&edge.to))
            else {
                continue;
            };
            successors[from].push(to);
            indegree[to] += 1;
        }
        let mut depth = vec![0usize; count];
        let mut queue: VecDeque<usize> = (0..count).filter(|index| indegree[*index] == 0).collect();
        while let Some(node) = queue.pop_front() {
            let next_depth = depth[node] + 1;
            for &next in &successors[node] {
                if depth[next] < next_depth {
                    depth[next] = next_depth;
                }
                indegree[next] -= 1;
                if indegree[next] == 0 {
                    queue.push_back(next);
                }
            }
        }
        depth
    }

    fn percentile(sorted: &[u128], percentile: f64) -> u128 {
        let index = ((sorted.len() as f64 * percentile).ceil() as usize).saturating_sub(1);
        sorted[index.min(sorted.len() - 1)]
    }

    /// One connector pass over the drawn subset, mirroring the render call site.
    fn flow_geometry(drawn: &super::Diagram) -> usize {
        let center_x = |lane: usize| lane as f32 * 150.0;
        let top_y = |depth: usize| depth as f32 * 80.0;
        let mut segments = 0usize;
        for edge in drawn.edges.iter() {
            let (Some(from), Some(to)) = (
                drawn.nodes.iter().find(|node| node.node == edge.from),
                drawn.nodes.iter().find(|node| node.node == edge.to),
            ) else {
                continue;
            };
            let connection = connector(
                center_x(from.lane),
                top_y(from.depth),
                center_x(to.lane),
                top_y(to.depth),
                from.node == to.node,
                136.0 / 2.0,
                2.0,
            );
            segments += connection.segments.len();
        }
        segments
    }

    /// MP-FLOW2 bottleneck measurement at 10,000 nodes. Ignored by default so the gate
    /// suite stays fast; run explicitly:
    /// `cargo test -p desktop-gpui --lib ten_thousand -- --ignored --nocapture`.
    ///
    /// Scope: CPU phases only (row admission, layers, diagram construction, connector
    /// geometry). Paint/submit/GPU/Present are NOT measured here. Debug build, one
    /// machine: the numbers are a bounded local observation, not an SLO claim.
    #[test]
    #[ignore = "MP-FLOW2 10,000-node measurement; run explicitly for evidence"]
    fn ten_thousand_node_flow_phases_stay_within_the_frame_budget() {
        use std::time::Instant;

        const LAYER_COUNT: usize = 100;
        const WIDTH: usize = 100;
        const NODES: usize = LAYER_COUNT * WIDTH;
        const ITERATIONS: usize = 20;
        const WARMUP: usize = 3;

        let graph = Arc::new(ten_thousand_node_graph(LAYER_COUNT, WIDTH));
        assert_eq!(graph.nodes.len(), NODES);
        assert_eq!(graph.edges.len(), (LAYER_COUNT - 1) * WIDTH * 2);

        // Equivalence: construction depth and the independent Kahn reference agree with
        // the implementation's layers, and the reference does not read the implementation.
        let expected: Vec<usize> = (0..NODES).map(|index| index / WIDTH).collect();
        let reference = reference_depths(&graph);
        let reference_layers = layers(&graph);
        assert_eq!(
            reference, expected,
            "the construction reference is the known depth"
        );
        assert_eq!(
            reference_layers.depth, expected,
            "the implementation's layers equal the independent reference"
        );
        assert!(!reference_layers.cyclic);

        // Rows equivalence: every node and edge is admitted in authoritative order.
        let rows_once = rows_cancellable(std::slice::from_ref(&graph), "", || false)
            .expect("an uncancelled admission returns rows");
        assert_eq!(rows_once.nodes.len(), NODES);
        assert_eq!(rows_once.edges.len(), graph.edges.len());
        for (index, row) in rows_once.nodes.iter().enumerate() {
            assert_eq!(row.node().identity, graph.nodes[index].identity);
        }
        for (index, row) in rows_once.edges.iter().enumerate() {
            assert_eq!(row.edge().identity, graph.edges[index].identity);
        }

        // The product builds the match sets from the admitted rows on every render, so the
        // measurement and the equivalence check use the same populated sets.
        let matched_nodes: std::collections::HashSet<(GraphRef, desktop_protocol::NodeId)> =
            rows_once
                .nodes
                .iter()
                .map(|row| (row.graph.graph_ref.clone(), row.node().identity.clone()))
                .collect();
        let matched_edges: std::collections::HashSet<(GraphRef, desktop_protocol::EdgeId)> =
            rows_once
                .edges
                .iter()
                .map(|row| (row.graph.graph_ref.clone(), row.edge().identity.clone()))
                .collect();
        assert_eq!(matched_nodes.len(), NODES);
        assert_eq!(matched_edges.len(), graph.edges.len());

        // Diagram equivalence: the cap draws the first 120 nodes in authoritative order,
        // every drawn depth equals the reference and the skipped-edge count accounts for
        // the full topology.
        let drawn_once = diagram(&graph, &reference_layers, &matched_nodes, &matched_edges);
        assert_eq!(drawn_once.nodes.len(), MAX_DIAGRAM_NODES);
        assert!(drawn_once.truncated);
        for (drawn_index, node) in drawn_once.nodes.iter().enumerate() {
            assert_eq!(node.node, graph.nodes[drawn_index].identity);
            assert_eq!(node.depth, expected[drawn_index]);
        }
        assert_eq!(
            drawn_once.edges.len() + drawn_once.skipped_edges,
            graph.edges.len(),
            "every edge is either drawn or explicitly skipped"
        );

        // Phase reference series and the per-iteration totals the decision uses.
        let mut rows_us = Vec::with_capacity(ITERATIONS);
        let mut layers_us = Vec::with_capacity(ITERATIONS);
        let mut matches_us = Vec::with_capacity(ITERATIONS);
        let mut diagram_us = Vec::with_capacity(ITERATIONS);
        let mut geometry_us = Vec::with_capacity(ITERATIONS);
        let mut frame_us = Vec::with_capacity(ITERATIONS);
        let mut full_us = Vec::with_capacity(ITERATIONS);
        let mut checksum = 0u64;
        for iteration in 0..(WARMUP + ITERATIONS) {
            let iteration_started = Instant::now();
            let started = Instant::now();
            let rows = rows_cancellable(std::slice::from_ref(&graph), "", || false).expect("rows");
            let rows_elapsed = started.elapsed().as_micros();
            let started = Instant::now();
            let computed = layers(&graph);
            let layers_elapsed = started.elapsed().as_micros();

            // The real per-render path: rebuild both match sets from the admitted rows,
            // then draw. This mirrors flow_diagram_region on every render.
            let frame_started = Instant::now();
            let started = Instant::now();
            let matched_nodes: std::collections::HashSet<(GraphRef, desktop_protocol::NodeId)> =
                rows.nodes
                    .iter()
                    .map(|row| (row.graph.graph_ref.clone(), row.node().identity.clone()))
                    .collect();
            let matched_edges: std::collections::HashSet<(GraphRef, desktop_protocol::EdgeId)> =
                rows.edges
                    .iter()
                    .map(|row| (row.graph.graph_ref.clone(), row.edge().identity.clone()))
                    .collect();
            let matches_elapsed = started.elapsed().as_micros();
            let started = Instant::now();
            let drawn = diagram(&graph, &computed, &matched_nodes, &matched_edges);
            let diagram_elapsed = started.elapsed().as_micros();
            let frame_elapsed = frame_started.elapsed().as_micros();
            let started = Instant::now();
            let segment_count = flow_geometry(&drawn);
            let geometry_elapsed = started.elapsed().as_micros();
            let full_elapsed = iteration_started.elapsed().as_micros();
            assert!(
                segment_count > 0,
                "the drawn subset produces connector segments"
            );
            assert_eq!(rows.nodes.len(), NODES);
            assert_eq!(computed.depth, expected);
            assert_eq!(drawn.nodes.len(), MAX_DIAGRAM_NODES);
            if iteration >= WARMUP {
                for (index, row) in rows.nodes.iter().enumerate() {
                    for byte in row.node().identity.as_str().bytes() {
                        checksum ^= u64::from(byte);
                        checksum = checksum.wrapping_mul(0x100_0000_01b3);
                    }
                    checksum ^= computed.depth[index] as u64;
                    checksum = checksum.wrapping_mul(0x100_0000_01b3);
                }
                rows_us.push(rows_elapsed);
                layers_us.push(layers_elapsed);
                matches_us.push(matches_elapsed);
                diagram_us.push(diagram_elapsed);
                geometry_us.push(geometry_elapsed);
                frame_us.push(frame_elapsed);
                full_us.push(full_elapsed);
            }
        }

        let stats = |mut values: Vec<u128>| {
            values.sort_unstable();
            let count = values.len();
            // Even counts: the median is the mean of the two middle values; odd counts use
            // the single middle value. p95 keeps the nearest-rank definition.
            let median = if count % 2 == 1 {
                values[count / 2]
            } else {
                (values[count / 2 - 1] + values[count / 2]) / 2
            };
            let p95 = percentile(&values, 0.95);
            (median, p95, values[count - 1])
        };
        // Decision series: the measured per-iteration totals, not a sum of phase medians.
        let (frame_median, frame_p95, frame_max) = stats(frame_us);
        let (full_median, full_p95, full_max) = stats(full_us);
        // Phase references (they are not added together for the decision).
        let (rows_median, rows_p95, _) = stats(rows_us);
        let (layers_median, layers_p95, _) = stats(layers_us);
        let (matches_median, matches_p95, _) = stats(matches_us);
        let (diagram_median, diagram_p95, _) = stats(diagram_us);
        let (geometry_median, geometry_p95, _) = stats(geometry_us);

        println!(
            "MP-FLOW2 ten_thousand nodes={NODES} edges={} iterations={ITERATIONS} warmup={WARMUP} checksum=0x{checksum:016x}",
            graph.edges.len()
        );
        println!("MP-FLOW2 phase rows        median={rows_median}us p95={rows_p95}us");
        println!("MP-FLOW2 phase layers      median={layers_median}us p95={layers_p95}us");
        println!("MP-FLOW2 phase matches     median={matches_median}us p95={matches_p95}us");
        println!("MP-FLOW2 phase diagram     median={diagram_median}us p95={diagram_p95}us");
        println!("MP-FLOW2 phase geometry    median={geometry_median}us p95={geometry_p95}us");
        println!(
            "MP-FLOW2 frame_path matches+diagram median={frame_median}us p95={frame_p95}us max={frame_max}us"
        );
        println!(
            "MP-FLOW2 full_baseline rows+layers+matches+diagram+geometry median={full_median}us p95={full_p95}us max={full_max}us"
        );
        println!(
            "MP-FLOW2 budget 60Hz=16670us ui_p95_target=8330us frame_median={frame_median}us frame_p95={frame_p95}us frame_within_ui_p95={} frame_within_60hz={}",
            frame_p95 <= 8_330,
            frame_median <= 16_670
        );
        println!(
            "MP-FLOW2 steady (layers cached per revision) = frame_path; full_baseline is the recompute path"
        );

        // The budget verdict is recorded, not asserted: a timing assertion here would make
        // the evidence run depend on machine load. The structural guarantees (equivalence,
        // determinism, complete raster) are asserted above.
        // Stage C: the first frame builds the match sets (miss); steady frames hit the
        // bounded single-entry cache keyed by the complete admission identity.
        let admission = AdmissionKey {
            session_generation: 1,
            publication_version: 1,
            epoch: None,
            revision: revision_key(std::slice::from_ref(&graph)),
            query: String::new(),
        };
        let mut cache = MatchCache::default();
        let miss_started = Instant::now();
        let (matched_miss, miss_hit) =
            cache.get_or_build(&admission, || matched_from_rows(&rows_once));
        let drawn_miss = diagram(
            &graph,
            &reference_layers,
            &matched_miss.nodes,
            &matched_miss.edges,
        );
        let miss_us = miss_started.elapsed().as_micros();
        assert!(!miss_hit, "the first frame is a miss");
        assert_eq!(cache.len(), 1);
        assert_eq!(
            drawn_miss, drawn_once,
            "the cached path draws the same diagram as the uncached build"
        );
        let mut hit_us = Vec::with_capacity(ITERATIONS);
        for iteration in 0..(WARMUP + ITERATIONS) {
            let started = Instant::now();
            let (matched, hit) = cache.get_or_build(&admission, || {
                panic!("a steady frame must hit the match cache")
            });
            let drawn = diagram(&graph, &reference_layers, &matched.nodes, &matched.edges);
            let elapsed = started.elapsed().as_micros();
            assert!(hit);
            assert_eq!(drawn, drawn_miss, "a hit draws the identical diagram");
            if iteration >= WARMUP {
                hit_us.push(elapsed);
            }
        }
        assert_eq!(cache.len(), 1, "the cache stays bounded");
        let (hit_median, hit_p95, hit_max) = stats(hit_us);
        let delta_median = frame_median as i128 - hit_median as i128;
        println!(
            "MP-FLOW2 cache miss_first_frame={miss_us}us hit_steady median={hit_median}us p95={hit_p95}us max={hit_max}us delta_median_vs_uncached={delta_median}us"
        );
        println!(
            "MP-FLOW2 cache hit_within_ui_p95={} hit_within_60hz={} hit_ui_headroom_us={}",
            hit_p95 <= 8_330,
            hit_p95 <= 16_670,
            8_330_i128 - hit_p95 as i128
        );
        println!(
            "MP-FLOW2 verdict frame_within_ui_p95={} frame_within_60hz={} full_within_60hz={} (paint/GPU NOT_MEASURED)",
            frame_p95 <= 8_330,
            frame_p95 <= 16_670,
            full_p95 <= 16_670
        );
        if cfg!(debug_assertions) {
            println!(
                "MP-FLOW2 note: debug build; the release profile is the evidence run for the budget decision"
            );
        }
    }

    /// MP-FLOW2 Stage C: separate 20+ iteration **miss** and **hit** series, plus
    /// admission-key invalidation and miss/hit diagram equivalence. Release-only
    /// evidence run; paint/GPU/Present remain NOT_MEASURED.
    ///
    /// `cargo test --release -p desktop-gpui --lib ten_thousand_cache -- --ignored --nocapture`
    #[test]
    #[ignore = "MP-FLOW2 Stage C cache measurement; run explicitly with --release"]
    fn ten_thousand_cache_miss_and_hit_series() {
        use std::collections::BTreeSet;
        use std::time::Instant;

        const LAYER_COUNT: usize = 100;
        const WIDTH: usize = 100;
        const NODES: usize = LAYER_COUNT * WIDTH;
        const ITERATIONS: usize = 21;
        const WARMUP: usize = 3;

        type BTreeNodes = BTreeSet<(GraphRef, desktop_protocol::NodeId)>;
        type BTreeEdges = BTreeSet<(GraphRef, desktop_protocol::EdgeId)>;

        println!(
            "MP-FLOW2 A/B conditions release={} warmup={WARMUP} iterations={ITERATIONS} nodes={NODES} edges={} order=alternating",
            !cfg!(debug_assertions),
            (LAYER_COUNT - 1) * WIDTH * 2
        );
        println!(
            "MP-FLOW2 A/B interval build=set construction only; probe=contains() over every admitted node and edge; product miss=clear/replace outside timer then get_or_build+diagram inside timer; hit=lookup+diagram"
        );
        println!(
            "MP-FLOW2 A/B aggregation n={ITERATIONS} median=odd:middle/even:mean(two middle) p95=nearest-rank ceil(n*0.95)-1 max=largest"
        );

        fn build_btree(rows: &super::Rows) -> (BTreeNodes, BTreeEdges) {
            (
                rows.nodes
                    .iter()
                    .map(|row| (row.graph.graph_ref.clone(), row.node().identity.clone()))
                    .collect(),
                rows.edges
                    .iter()
                    .map(|row| (row.graph.graph_ref.clone(), row.edge().identity.clone()))
                    .collect(),
            )
        }

        fn probe_hash(rows: &super::Rows, matched: &super::Matched) -> usize {
            let mut found = 0;
            for row in rows.nodes.iter() {
                if matched
                    .nodes
                    .contains(&(row.graph.graph_ref.clone(), row.node().identity.clone()))
                {
                    found += 1;
                }
            }
            for row in rows.edges.iter() {
                if matched
                    .edges
                    .contains(&(row.graph.graph_ref.clone(), row.edge().identity.clone()))
                {
                    found += 1;
                }
            }
            found
        }

        fn probe_btree(rows: &super::Rows, nodes: &BTreeNodes, edges: &BTreeEdges) -> usize {
            let mut found = 0;
            for row in rows.nodes.iter() {
                if nodes.contains(&(row.graph.graph_ref.clone(), row.node().identity.clone())) {
                    found += 1;
                }
            }
            for row in rows.edges.iter() {
                if edges.contains(&(row.graph.graph_ref.clone(), row.edge().identity.clone())) {
                    found += 1;
                }
            }
            found
        }

        let graph = Arc::new(ten_thousand_node_graph(LAYER_COUNT, WIDTH));
        assert_eq!(graph.nodes.len(), NODES);
        let rows = rows_cancellable(std::slice::from_ref(&graph), "", || false)
            .expect("uncancelled admission");
        assert_eq!(rows.nodes.len(), NODES);
        let computed = layers(&graph);
        let key = AdmissionKey {
            session_generation: 7,
            publication_version: 11,
            epoch: Some(EpochId::fixture("epoch/measure")),
            revision: revision_key(std::slice::from_ref(&graph)),
            query: String::new(),
        };
        let other = AdmissionKey {
            query: "replacement".to_owned(),
            ..key.clone()
        };
        let reference = {
            let mut cache = MatchCache::default();
            let (matched, hit) = cache.get_or_build(&key, || matched_from_rows(&rows));
            assert!(!hit);
            diagram(&graph, &computed, &matched.nodes, &matched.edges)
        };

        // A/B construction and existence-check series (alternating order).
        let mut btree_build = Vec::with_capacity(ITERATIONS);
        let mut btree_probe = Vec::with_capacity(ITERATIONS);
        let mut hash_build = Vec::with_capacity(ITERATIONS);
        let mut hash_probe = Vec::with_capacity(ITERATIONS);
        let mut btree_pair = None;
        let mut hash_matched = None;
        for iteration in 0..(WARMUP + ITERATIONS) {
            let btree_first = iteration % 2 == 0;
            let measure_btree = || {
                let started = Instant::now();
                let pair = build_btree(&rows);
                let build_us = started.elapsed().as_micros();
                let started = Instant::now();
                let found = probe_btree(&rows, &pair.0, &pair.1);
                let probe_us = started.elapsed().as_micros();
                (pair, build_us, probe_us, found)
            };
            let measure_hash = || {
                let started = Instant::now();
                let matched = matched_from_rows(&rows);
                let build_us = started.elapsed().as_micros();
                let started = Instant::now();
                let found = probe_hash(&rows, &matched);
                let probe_us = started.elapsed().as_micros();
                (matched, build_us, probe_us, found)
            };
            let (b, h) = if btree_first {
                let b = measure_btree();
                let h = measure_hash();
                (b, h)
            } else {
                let h = measure_hash();
                let b = measure_btree();
                (b, h)
            };
            assert_eq!(b.3, h.3, "both sets find every admitted row");
            assert_eq!(b.3, NODES + rows.edges.len());
            btree_pair = Some(b.0);
            hash_matched = Some(h.0);
            if iteration >= WARMUP {
                let index = iteration - WARMUP;
                println!(
                    "ab_sample kind=btree iter={index} build_us={} probe_us={}",
                    b.1, b.2
                );
                println!(
                    "ab_sample kind=hash iter={index} build_us={} probe_us={}",
                    h.1, h.2
                );
                btree_build.push(b.1);
                btree_probe.push(b.2);
                hash_build.push(h.1);
                hash_probe.push(h.2);
            }
        }
        let (btree_nodes, btree_edges) = btree_pair.expect("btree pair");
        let hash_ref = hash_matched.expect("hash sets");
        assert_eq!(
            hash_ref.nodes,
            btree_nodes
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>(),
            "node match sets agree"
        );
        assert_eq!(
            hash_ref.edges,
            btree_edges
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>(),
            "edge match sets agree"
        );
        let btree_drawn = diagram(
            &graph,
            &computed,
            &btree_nodes
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>(),
            &btree_edges
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>(),
        );
        // Draw from each implementation's own set: btree-derived, hash-derived and the
        // cached reference. Three independent diagrams, not two.
        let hash_drawn = diagram(&graph, &computed, &hash_ref.nodes, &hash_ref.edges);
        assert_eq!(
            btree_drawn, hash_drawn,
            "btree and hash sets draw the same diagram"
        );
        assert_eq!(
            hash_drawn, reference,
            "the A/B hash diagram equals the cached reference diagram"
        );
        let btree_checksum = diagram_checksum(&btree_drawn, &rows, &computed);
        let hash_checksum = diagram_checksum(&hash_drawn, &rows, &computed);
        let reference_checksum = diagram_checksum(&reference, &rows, &computed);
        assert_eq!(
            btree_checksum, hash_checksum,
            "btree and hash diagrams share the deterministic checksum"
        );
        assert_eq!(
            hash_checksum, reference_checksum,
            "the A/B checksum equals the cached reference checksum"
        );
        println!(
            "MP-FLOW2 A/B checksums btree=0x{btree_checksum:016x} hash=0x{hash_checksum:016x} reference=0x{reference_checksum:016x}"
        );

        // Product paths with the product (hash) cache, alternating.
        let mut cache = MatchCache::default();
        let mut clear_samples: Vec<(u128, u128, u128)> = Vec::new();
        let mut replace_samples: Vec<(u128, u128, u128)> = Vec::new();
        let mut hit_samples: Vec<(u128, u128, u128)> = Vec::new();
        for iteration in 0..(WARMUP + ITERATIONS) {
            let _ = cache.get_or_build(&other, || matched_from_rows(&rows));
            let miss_first = iteration % 2 == 0;
            if miss_first {
                replace_samples.push(measure_replace(
                    &mut cache, &key, &other, &rows, &graph, &computed, &reference,
                ));
                hit_samples.push(measure_hit(
                    &mut cache, &key, &rows, &graph, &computed, &reference,
                ));
                clear_samples.push(measure_clear(
                    &mut cache, &key, &rows, &graph, &computed, &reference,
                ));
            } else {
                clear_samples.push(measure_clear(
                    &mut cache, &key, &rows, &graph, &computed, &reference,
                ));
                hit_samples.push(measure_hit(
                    &mut cache, &key, &rows, &graph, &computed, &reference,
                ));
                replace_samples.push(measure_replace(
                    &mut cache, &key, &other, &rows, &graph, &computed, &reference,
                ));
            }
            if iteration >= WARMUP {
                let index = iteration - WARMUP;
                let (match_us, diagram_us, total_us) = replace_samples.last().copied().unwrap();
                println!(
                    "sample series=miss_replace iter={index} match_us={match_us} diagram_us={diagram_us} total_us={total_us}"
                );
                let (match_us, diagram_us, total_us) = clear_samples.last().copied().unwrap();
                println!(
                    "sample series=miss_clear iter={index} match_us={match_us} diagram_us={diagram_us} total_us={total_us}"
                );
                let (match_us, diagram_us, total_us) = hit_samples.last().copied().unwrap();
                println!(
                    "sample series=hit iter={index} match_us={match_us} diagram_us={diagram_us} total_us={total_us}"
                );
            }
        }

        // Every admission-key component alone forces a miss; the untouched key hits.
        let mut probe = MatchCache::default();
        let _ = probe.get_or_build(&key, || matched_from_rows(&rows));
        let changes: [(&str, AdmissionKey); 5] = [
            (
                "session_generation",
                AdmissionKey {
                    session_generation: 8,
                    ..key.clone()
                },
            ),
            (
                "publication_version",
                AdmissionKey {
                    publication_version: 12,
                    ..key.clone()
                },
            ),
            (
                "epoch",
                AdmissionKey {
                    epoch: Some(EpochId::fixture("epoch/other")),
                    ..key.clone()
                },
            ),
            (
                "revision",
                AdmissionKey {
                    revision: revision_key(&[]),
                    ..key.clone()
                },
            ),
            (
                "query",
                AdmissionKey {
                    query: "goal".to_owned(),
                    ..key.clone()
                },
            ),
        ];
        for (name, changed) in &changes {
            let (_, found) = probe.get_or_build(changed, || matched_from_rows(&rows));
            assert!(!found, "{name} changes must miss");
            let (_, found_again) =
                probe.get_or_build(changed, || panic!("the unchanged key must hit"));
            assert!(found_again, "{name} unchanged must hit");
            println!("MP-FLOW2 key_invalidation {name}=miss_then_hit");
        }

        let stats = |mut values: Vec<u128>| {
            values.sort_unstable();
            let count = values.len();
            let median = if count % 2 == 1 {
                values[count / 2]
            } else {
                (values[count / 2 - 1] + values[count / 2]) / 2
            };
            let p95 = percentile(&values, 0.95);
            (median, p95, values[count - 1])
        };
        for (name, values) in [
            ("btree_build", btree_build),
            ("btree_probe", btree_probe),
            ("hash_build", hash_build),
            ("hash_probe", hash_probe),
        ] {
            let (median, p95, max) = stats(values);
            println!(
                "ab_summary kind={name} n={ITERATIONS} median_us={median} p95_us={p95} max_us={max}"
            );
        }
        for (name, samples) in [
            ("miss_clear", &clear_samples),
            ("miss_replace", &replace_samples),
            ("hit", &hit_samples),
        ] {
            let measured: Vec<(u128, u128, u128)> = samples[WARMUP..].to_vec();
            let total = measured.iter().map(|sample| sample.2).collect();
            let match_only = measured.iter().map(|sample| sample.0).collect();
            let diagram_only = measured.iter().map(|sample| sample.1).collect();
            let (median, p95, max) = stats(total);
            let (match_median, match_p95, match_max) = stats(match_only);
            let (diagram_median, diagram_p95, diagram_max) = stats(diagram_only);
            println!(
                "summary series={name} n={ITERATIONS} median_us={median} p95_us={p95} max_us={max} match_median_us={match_median} match_p95_us={match_p95} match_max_us={match_max} diagram_median_us={diagram_median} diagram_p95_us={diagram_p95} diagram_max_us={diagram_max}"
            );
        }
        println!("MP-FLOW2 equivalence btree==hash==reference diagram_equal=true");
        println!("MP-FLOW2 NOT_MEASURED paint/GPU/present");
    }

    /// Deterministic checksum over the drawn diagram and the admitted rows, so an
    /// implementation change can be compared numerically as well as structurally.
    fn diagram_checksum(
        drawn: &super::Diagram,
        rows: &super::Rows,
        computed: &super::Layers,
    ) -> u64 {
        let mut checksum = 0u64;
        for (index, row) in rows.nodes.iter().enumerate() {
            for byte in row.node().identity.as_str().bytes() {
                checksum ^= u64::from(byte);
                checksum = checksum.wrapping_mul(0x100_0000_01b3);
            }
            checksum ^= computed.depth[index] as u64;
            checksum = checksum.wrapping_mul(0x100_0000_01b3);
        }
        checksum ^= drawn.nodes.len() as u64;
        checksum = checksum.wrapping_mul(0x100_0000_01b3);
        checksum ^= drawn.edges.len() as u64;
        checksum
    }

    /// Measure one replacement miss (an existing other-key entry is replaced) with the
    /// diagram build inside the same timed interval.
    fn measure_replace(
        cache: &mut MatchCache,
        key: &AdmissionKey,
        other: &AdmissionKey,
        rows: &super::Rows,
        graph: &Arc<GraphDisplay>,
        layers: &super::Layers,
        reference: &super::Diagram,
    ) -> (u128, u128, u128) {
        use std::time::Instant;
        // Ensure the cache holds the *other* key before timing the replacement.
        let _ = cache.get_or_build(other, || matched_from_rows(rows));
        let started = Instant::now();
        let match_started = Instant::now();
        let (matched, hit) = cache.get_or_build(key, || matched_from_rows(rows));
        let match_us = match_started.elapsed().as_micros();
        let diagram_started = Instant::now();
        let drawn = diagram(graph, layers, &matched.nodes, &matched.edges);
        let diagram_us = diagram_started.elapsed().as_micros();
        let total_us = started.elapsed().as_micros();
        assert!(!hit, "an other-key entry must be replaced");
        assert_eq!(&drawn, reference);
        (match_us, diagram_us, total_us)
    }

    /// Measure one clear-then-rebuild miss with the diagram build inside the interval.
    fn measure_clear(
        cache: &mut MatchCache,
        key: &AdmissionKey,
        rows: &super::Rows,
        graph: &Arc<GraphDisplay>,
        layers: &super::Layers,
        reference: &super::Diagram,
    ) -> (u128, u128, u128) {
        use std::time::Instant;
        cache.clear();
        let started = Instant::now();
        let match_started = Instant::now();
        let (matched, hit) = cache.get_or_build(key, || matched_from_rows(rows));
        let match_us = match_started.elapsed().as_micros();
        let diagram_started = Instant::now();
        let drawn = diagram(graph, layers, &matched.nodes, &matched.edges);
        let diagram_us = diagram_started.elapsed().as_micros();
        let total_us = started.elapsed().as_micros();
        assert!(!hit, "a cleared cache is a miss");
        assert_eq!(&drawn, reference);
        (match_us, diagram_us, total_us)
    }

    /// Measure one cache hit with the diagram build inside the interval.
    fn measure_hit(
        cache: &mut MatchCache,
        key: &AdmissionKey,
        rows: &super::Rows,
        graph: &Arc<GraphDisplay>,
        layers: &super::Layers,
        reference: &super::Diagram,
    ) -> (u128, u128, u128) {
        use std::time::Instant;
        let _ = cache.get_or_build(key, || matched_from_rows(rows));
        let started = Instant::now();
        let match_started = Instant::now();
        let (matched, found) =
            cache.get_or_build(key, || panic!("a steady frame must hit the match cache"));
        let match_us = match_started.elapsed().as_micros();
        let diagram_started = Instant::now();
        let drawn = diagram(graph, layers, &matched.nodes, &matched.edges);
        let diagram_us = diagram_started.elapsed().as_micros();
        let total_us = started.elapsed().as_micros();
        assert!(found);
        assert_eq!(&drawn, reference);
        (match_us, diagram_us, total_us)
    }
}
