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
/// The entry is keyed by the **complete** [`AdmissionKey`]: an exact key is a hit, and a
/// different key replaces the entry, so no identity component is silently ignored. The
/// *rebuild* is incremental for a publication-version-only change: the admitted rows are
/// pinned by the exact projection revision counters and canonical [`GraphRef`] values, so a
/// new publication that changes neither cannot change which rows are admitted, and the
/// previously built set stays valid. A projection-revision change re-derives the display rows
/// (labels, layers and kinds may change), and any scope, query or graph identity change also
/// rebuilds from the new rows. It never grows past one entry; a miss replaces it. The cache
/// holds only the match sets: the diagram itself is rebuilt from them.
#[derive(Debug, Default)]
pub struct MatchCache {
    entry: Option<(AdmissionKey, Arc<Matched>)>,
}

/// How a [`MatchCache`] lookup was served.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchLookup {
    /// The complete admission key was already cached; the build closure did not run.
    Hit,
    /// The entry belonged to a different admission key with identical match content; the
    /// set was reused and the entry re-keyed without running the build closure.
    Reused,
    /// The match content identity changed (or the cache was empty); the closure rebuilt it.
    Rebuilt,
}

/// Whether two admission keys admit exactly the same match-set content.
///
/// The content is the set of `(GraphRef, identity)` pairs of the admitted rows, and those rows
/// are derived from the exact projection revision and canonical graph revision. A different
/// projection [`Counter`] or a different [`GraphRef`] can change labels, layers, kinds or
/// identities, so both are part of the content identity: a row is reused only when both match.
/// Only the publication version is a coordination value that cannot change which rows a
/// projection revision admits, so it alone may differ for a reuse.
fn same_match_content(left: &AdmissionKey, right: &AdmissionKey) -> bool {
    left.session_generation == right.session_generation
        && left.epoch == right.epoch
        && left.query == right.query
        && left.revision == right.revision
}

impl MatchCache {
    /// Return the match sets for `key`, rebuilding only when their content changed.
    ///
    /// This is [`Self::get_or_build`] with the lookup outcome exposed so tests and the
    /// measurement path can distinguish an exact hit, an incremental reuse and a rebuild.
    pub fn lookup(
        &mut self,
        key: &AdmissionKey,
        build: impl FnOnce() -> Matched,
    ) -> (Arc<Matched>, MatchLookup) {
        let mut reused = None;
        if let Some((cached, matched)) = self.entry.as_ref() {
            if cached == key {
                return (Arc::clone(matched), MatchLookup::Hit);
            }
            if same_match_content(cached, key) {
                reused = Some(Arc::clone(matched));
            }
        }
        if let Some(matched) = reused {
            self.entry = Some((key.clone(), Arc::clone(&matched)));
            return (matched, MatchLookup::Reused);
        }
        let matched = Arc::new(build());
        self.entry = Some((key.clone(), Arc::clone(&matched)));
        (matched, MatchLookup::Rebuilt)
    }

    /// Return the cached sets for `key`, or build them once with `build`.
    ///
    /// The returned flag is `true` only for an exact-key hit. A content-identical incremental
    /// reuse also avoids `build` but reports `false`, because the entry belongs to a different
    /// complete admission identity. A hit never calls `build`.
    pub fn get_or_build(
        &mut self,
        key: &AdmissionKey,
        build: impl FnOnce() -> Matched,
    ) -> (Arc<Matched>, bool) {
        let (matched, outcome) = self.lookup(key, build);
        (matched, outcome == MatchLookup::Hit)
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
    layers_inner(
        graph,
        AdjacencyMode::Baseline,
        #[cfg(test)]
        None,
        #[cfg(test)]
        None,
    )
}

/// Six-phase measurement of exactly this path; `None` in every product build.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AdjacencyMode {
    Baseline,
    #[cfg(test)]
    Preallocated,
}

fn layers_inner(
    graph: &GraphDisplay,
    adjacency: AdjacencyMode,
    #[cfg(test)] mut phases: Option<&mut [u128; 9]>,
    #[cfg(test)] mut structure: Option<&mut StructureStats>,
) -> Layers {
    // The clock is only created when a phase array is supplied; the non-instrumented
    // test path takes no timestamps at all.
    #[cfg(test)]
    let mut phase_start = phases.is_some().then(std::time::Instant::now);
    let count = graph.nodes.len();
    let index_of: std::collections::HashMap<NodeId, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.identity.clone(), index))
        .collect();
    #[cfg(test)]
    if let (Some(slots), Some(start)) = (phases.as_mut(), phase_start) {
        slots[0] = start.elapsed().as_micros();
        phase_start = Some(std::time::Instant::now());
    }
    let mut successors: Vec<Vec<usize>> = if adjacency == AdjacencyMode::Baseline {
        vec![Vec::new(); count]
    } else {
        Vec::new()
    };
    #[cfg(test)]
    let p2a_us = match phase_start {
        Some(start) => {
            let elapsed = start.elapsed().as_micros();
            phase_start = Some(std::time::Instant::now());
            elapsed
        }
        None => 0,
    };
    #[cfg(test)]
    if adjacency == AdjacencyMode::Preallocated {
        let started = std::time::Instant::now();
        let mut degrees = vec![0usize; count];
        for edge in graph.edges.iter() {
            if let Some(&from) = index_of.get(&edge.from) {
                degrees[from] = degrees[from].saturating_add(1);
            }
        }
        successors = degrees
            .into_iter()
            .map(|degree| {
                let bytes = degree.saturating_mul(std::mem::size_of::<usize>());
                if bytes <= isize::MAX as usize {
                    Vec::with_capacity(degree)
                } else {
                    Vec::new()
                }
            })
            .collect();
        let _ = started.elapsed().as_micros();
    }
    let mut self_loop = false;
    for edge in graph.edges.iter() {
        if let (Some(&from), Some(&to)) = (index_of.get(&edge.from), index_of.get(&edge.to)) {
            if from == to {
                self_loop = true;
            }
            #[cfg(test)]
            if let Some(stats) = structure.as_mut() {
                // Capacity is only read while the structural analysis runs, so the timed
                // paths (`layers`, `layers_phased`) keep the single push above.
                let before = successors[from].capacity();
                successors[from].push(to);
                let after = successors[from].capacity();
                stats.inserted_edges += 1;
                if before == 0 && after > 0 {
                    stats.initial_allocations += 1;
                } else if before > 0 && after > before {
                    stats.capacity_grows += 1;
                }
            } else {
                successors[from].push(to);
            }
            #[cfg(not(test))]
            {
                successors[from].push(to);
            }
        } else {
            #[cfg(test)]
            if let Some(stats) = structure.as_mut() {
                stats.skipped_edges += 1;
            }
        }
    }
    #[cfg(test)]
    if let Some(stats) = structure.as_mut() {
        stats.total_capacity = successors.iter().map(Vec::capacity).sum();
        stats.total_length = successors.iter().map(Vec::len).sum();
        assert!(
            successors.iter().all(|list| list.capacity() >= list.len()),
            "every adjacency list holds at least its elements"
        );
        stats.nonempty_nodes = successors.iter().filter(|list| !list.is_empty()).count();
        stats.max_outdegree = successors.iter().map(Vec::len).max().unwrap_or(0);
        stats.adjacency = successors.clone();
    }
    #[cfg(test)]
    if let (Some(slots), Some(start)) = (phases.as_mut(), phase_start) {
        let p2b_us = start.elapsed().as_micros();
        slots[7] = p2a_us;
        slots[8] = p2b_us;
        // P2 overall is exactly the two measured halves of the same pass.
        slots[1] = p2a_us + p2b_us;
        phase_start = Some(std::time::Instant::now());
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

    #[cfg(test)]
    if let (Some(slots), Some(start)) = (phases.as_mut(), phase_start) {
        slots[2] = start.elapsed().as_micros();
        phase_start = Some(std::time::Instant::now());
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
    #[cfg(test)]
    if let (Some(slots), Some(start)) = (phases.as_mut(), phase_start) {
        slots[3] = start.elapsed().as_micros();
        phase_start = Some(std::time::Instant::now());
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
    #[cfg(test)]
    if let (Some(slots), Some(start)) = (phases.as_mut(), phase_start) {
        slots[4] = start.elapsed().as_micros();
        phase_start = Some(std::time::Instant::now());
    }
    let cyclic = self_loop || component_sizes.iter().any(|size| *size > 1);
    let depth = (0..count)
        .map(|node| depth_component[component[node]])
        .collect();
    #[cfg(test)]
    if let (Some(slots), Some(start)) = (phases.as_mut(), phase_start) {
        slots[5] = start.elapsed().as_micros();
        phase_start = Some(std::time::Instant::now());
        // Explicit deallocation: the index map and adjacency are freed here, inside
        // the measured path, so their cost is not silently left outside the phases.
        drop(index_of);
        drop(successors);
        slots[6] = phase_start.map(|s| s.elapsed().as_micros()).unwrap_or(0);
    }
    Layers { depth, cyclic }
}

/// Test-only entry point: the same `layers_inner` path with seven phase durations and
/// the outer elapsed time (outer is not the sum of the phases).
#[cfg(test)]
pub(crate) fn layers_phased(graph: &GraphDisplay) -> (Layers, [u128; 9], u128) {
    let mut phases = [0u128; 9];
    let outer = std::time::Instant::now();
    let result = layers_inner(graph, AdjacencyMode::Baseline, Some(&mut phases), None);
    (result, phases, outer.elapsed().as_micros())
}

/// Test-only structural counters for the P2b edge pass.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct StructureStats {
    pub inserted_edges: usize,
    pub skipped_edges: usize,
    pub initial_allocations: usize,
    pub capacity_grows: usize,
    pub total_capacity: usize,
    pub total_length: usize,
    pub nonempty_nodes: usize,
    pub max_outdegree: usize,
    pub adjacency: Vec<Vec<usize>>,
}

/// Test-only entry point: same `layers_inner` path, P2b structural counters enabled.
#[cfg(test)]
pub(crate) fn layers_structural(graph: &GraphDisplay) -> (Layers, StructureStats) {
    let mut structure = StructureStats::default();
    let result = layers_inner(graph, AdjacencyMode::Baseline, None, Some(&mut structure));
    (result, structure)
}

/// Test-only comparison entries: identical algorithm, P2b adjacency only differs.
#[cfg(test)]
pub(crate) fn layers_preallocated(graph: &GraphDisplay) -> Layers {
    layers_inner(graph, AdjacencyMode::Preallocated, None, None)
}

#[cfg(test)]
pub(crate) fn layers_preallocated_phased(graph: &GraphDisplay) -> (Layers, [u128; 9], u128) {
    let mut phases = [0u128; 9];
    let outer = std::time::Instant::now();
    let result = layers_inner(graph, AdjacencyMode::Preallocated, Some(&mut phases), None);
    (result, phases, outer.elapsed().as_micros())
}

#[cfg(test)]
pub(crate) fn layers_preallocated_structural(graph: &GraphDisplay) -> (Layers, StructureStats) {
    let mut structure = StructureStats::default();
    let result = layers_inner(
        graph,
        AdjacencyMode::Preallocated,
        None,
        Some(&mut structure),
    );
    (result, structure)
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
        AdmissionKey, MAX_DIAGRAM_NODES, MatchCache, MatchLookup, Matched, RevisionKey,
        SAME_LAYER_HEADROOM, admit, connector, contains_edge, contains_node, diagram, layers,
        layers_preallocated, revision_key, rows, rows_cancellable,
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

    /// Stage C: every content-identity component must invalidate the cache and force a
    /// rebuild. Only a publication-version-only change reuses the built set; a projection
    /// revision change re-derives the display rows and rebuilds.
    #[test]
    fn the_match_cache_invalidates_on_every_content_identity_component() {
        let source_graph = graph(1, "digest/one", 4);
        let graphs = [Arc::clone(&source_graph)];
        let rows = rows(&graphs, "");
        let base = cache_key(1, 2, None, revision_key(&graphs), "");
        let builds = std::cell::Cell::new(0usize);
        let mut cache = MatchCache::default();

        let (first, outcome) = cache.lookup(&base, || {
            builds.set(builds.get() + 1);
            matched_from_rows(&rows)
        });
        assert_eq!(outcome, MatchLookup::Rebuilt, "the first request rebuilds");
        assert_eq!(cache.len(), 1);
        let (cached, outcome) = cache.lookup(&base, || panic!("a hit must not rebuild"));
        assert_eq!(outcome, MatchLookup::Hit);
        assert!(
            Arc::ptr_eq(&first, &cached),
            "a hit returns the cached instance"
        );
        assert_eq!(builds.get(), 1);

        // A publication-version-only change cannot change which rows a projection revision
        // admits: the set is reused and the entry re-keyed, so no rebuild happens and the next
        // exact lookup is a hit.
        let publication_only = cache_key(1, 3, None, base.revision.clone(), "");
        let (reused, outcome) = cache.lookup(&publication_only, || {
            panic!("a publication-version-only change must reuse the built set")
        });
        assert_eq!(outcome, MatchLookup::Reused);
        assert_eq!(*reused, matched_from_rows(&rows));
        let (again, outcome) =
            cache.lookup(&publication_only, || panic!("the re-keyed entry hits"));
        assert_eq!(outcome, MatchLookup::Hit);
        assert!(Arc::ptr_eq(&reused, &again));
        assert_eq!(cache.len(), 1);
        assert_eq!(builds.get(), 1, "publication reuse never rebuilds");

        let other_graph = graph(1, "digest/two", 4);
        let other_revision = revision_key(&[Arc::clone(&other_graph)]);
        let newer_graph = graph(2, "digest/three", 5);
        let two_graphs = revision_key(&[Arc::clone(&source_graph), Arc::clone(&newer_graph)]);
        let reversed = revision_key(&[Arc::clone(&newer_graph), Arc::clone(&source_graph)]);
        let projection_variant =
            cache_key(1, 2, None, revision_key(&[graph(1, "digest/one", 9)]), "");
        let session_variant = cache_key(2, 2, None, base.revision.clone(), "");
        let epoch_variant = cache_key(
            1,
            2,
            Some(EpochId::fixture("epoch/one")),
            base.revision.clone(),
            "",
        );
        let query_variant = cache_key(1, 2, None, base.revision.clone(), "alpha");
        let identity_variant = cache_key(1, 2, None, other_revision, "");
        let count_variant = cache_key(1, 2, None, two_graphs, "");
        let order_variant = cache_key(1, 2, None, reversed, "");
        let variants = [
            ("projection_revision", projection_variant),
            ("session_generation", session_variant),
            ("epoch", epoch_variant),
            ("query", query_variant),
            ("graph_identity", identity_variant),
            ("graph_count", count_variant),
            ("graph_order", order_variant),
        ];
        let variant_count = variants.len();
        for (name, variant) in &variants {
            assert_ne!(variant, &base, "{name} must actually differ");
            let (_, outcome) = cache.lookup(variant, || {
                builds.set(builds.get() + 1);
                matched_from_rows(&rows)
            });
            assert_eq!(
                outcome,
                MatchLookup::Rebuilt,
                "{name} changes must invalidate the cache"
            );
            assert_eq!(cache.len(), 1, "the cache never grows past one entry");
        }
        assert_eq!(builds.get(), 1 + variant_count);
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

    /// Stage C: a projection-revision change re-derives the display rows (labels, layers and
    /// kinds may change even when the canonical `GraphRef` does not), so the cached match set
    /// must be rebuilt rather than reused; the rebuilt set still equals a full rebuild.
    #[test]
    fn a_projection_counter_change_rebuilds_the_match_set() {
        let source_graph = graph(7, "digest/seven", 3);
        let graphs = [Arc::clone(&source_graph)];
        let rows = rows(&graphs, "a");
        let first_key = cache_key(1, 1, None, revision_key(&graphs), "a");
        let second_key = cache_key(
            1,
            1,
            None,
            revision_key(&[graph(7, "digest/seven", 4)]),
            "a",
        );
        assert_ne!(first_key, second_key);
        assert_eq!(first_key.revision[0].0, second_key.revision[0].0);
        let computed = layers(&source_graph);
        let mut cache = MatchCache::default();
        let (first, outcome) = cache.lookup(&first_key, || matched_from_rows(&rows));
        assert_eq!(outcome, MatchLookup::Rebuilt);
        let builds = std::cell::Cell::new(0usize);
        let (rebuilt, outcome) = cache.lookup(&second_key, || {
            builds.set(builds.get() + 1);
            matched_from_rows(&rows)
        });
        assert_eq!(
            outcome,
            MatchLookup::Rebuilt,
            "the projection revision is part of the content identity"
        );
        assert_eq!(builds.get(), 1, "the previous set is not reused");
        assert!(!Arc::ptr_eq(&first, &rebuilt));
        let fresh = matched_from_rows(&rows);
        assert_eq!(*rebuilt, fresh);
        let rebuilt_diagram = diagram(&source_graph, &computed, &rebuilt.nodes, &rebuilt.edges);
        let fresh_diagram = diagram(&source_graph, &computed, &fresh.nodes, &fresh.edges);
        assert_eq!(rebuilt_diagram, fresh_diagram);
        assert_eq!(
            diagram_checksum(&rebuilt_diagram, &rows, &computed),
            diagram_checksum(&fresh_diagram, &rows, &computed)
        );
    }

    /// Stage C incremental build: a publication-version change alone keeps the same canonical
    /// graph revision and reuses the built set while re-keying the entry.
    #[test]
    fn a_publication_version_change_reuses_the_built_match_set() {
        let source_graph = graph(2, "digest/two", 5);
        let graphs = [Arc::clone(&source_graph)];
        let rows = rows(&graphs, "");
        let base = cache_key(1, 4, None, revision_key(&graphs), "");
        let publication = cache_key(1, 5, None, revision_key(&graphs), "");
        let mut cache = MatchCache::default();
        let (built, outcome) = cache.lookup(&base, || matched_from_rows(&rows));
        assert_eq!(outcome, MatchLookup::Rebuilt);
        let builds = std::cell::Cell::new(0usize);
        let (reused, outcome) = cache.lookup(&publication, || {
            builds.set(builds.get() + 1);
            matched_from_rows(&rows)
        });
        assert_eq!(outcome, MatchLookup::Reused);
        assert_eq!(builds.get(), 0, "the publication change never rebuilds");
        assert!(Arc::ptr_eq(&built, &reused));
        let (again, outcome) = cache.lookup(&publication, || panic!("the re-keyed entry hits"));
        assert_eq!(outcome, MatchLookup::Hit);
        assert!(Arc::ptr_eq(&built, &again));
        assert_eq!(cache.len(), 1);
    }

    /// Stage C: a canonical revision change never reuses the previous revision's set; the
    /// rebuilt content only names the new revision and the old key cannot hit it.
    #[test]
    fn a_canonical_revision_change_never_reuses_stale_match_sets() {
        let first = graph(4, "digest/first", 2);
        let second = graph(5, "digest/second", 2);
        let rows_first = rows(&[Arc::clone(&first)], "");
        let rows_second = rows(&[Arc::clone(&second)], "");
        let key_first = cache_key(1, 7, None, revision_key(&[Arc::clone(&first)]), "");
        let key_second = cache_key(1, 7, None, revision_key(&[Arc::clone(&second)]), "");
        let mut cache = MatchCache::default();
        let (built_first, outcome) = cache.lookup(&key_first, || matched_from_rows(&rows_first));
        assert_eq!(outcome, MatchLookup::Rebuilt);
        let builds = std::cell::Cell::new(0usize);
        let (built_second, outcome) = cache.lookup(&key_second, || {
            builds.set(builds.get() + 1);
            matched_from_rows(&rows_second)
        });
        assert_eq!(
            outcome,
            MatchLookup::Rebuilt,
            "a graph identity change rebuilds"
        );
        assert_eq!(builds.get(), 1);
        assert!(!Arc::ptr_eq(&built_first, &built_second));
        assert!(
            built_second
                .nodes
                .iter()
                .all(|(graph_ref, _)| *graph_ref == second.graph_ref)
        );
        assert!(
            built_second
                .edges
                .iter()
                .all(|(graph_ref, _)| *graph_ref == second.graph_ref)
        );
        // The key that produced the stale content rebuilds again instead of hitting the
        // entry that now holds the other revision.
        let (again_first, outcome) = cache.lookup(&key_first, || matched_from_rows(&rows_first));
        assert_eq!(outcome, MatchLookup::Rebuilt);
        assert!(!Arc::ptr_eq(&again_first, &built_first));
        assert!(
            again_first
                .nodes
                .iter()
                .all(|(graph_ref, _)| *graph_ref == first.graph_ref)
        );
        assert_eq!(cache.len(), 1);
    }

    /// Stage C: the exact-key hit path is unchanged: no rebuild, the same instance, bounded.
    #[test]
    fn the_match_cache_hit_path_is_unchanged() {
        let source_graph = graph(1, "digest/one", 4);
        let graphs = [Arc::clone(&source_graph)];
        let rows = rows(&graphs, "");
        let key = cache_key(1, 1, None, revision_key(&graphs), "");
        let mut cache = MatchCache::default();
        let (first, outcome) = cache.lookup(&key, || matched_from_rows(&rows));
        assert_eq!(outcome, MatchLookup::Rebuilt);
        for _ in 0..8 {
            let (cached, outcome) = cache.lookup(&key, || panic!("a hit must not rebuild"));
            assert_eq!(outcome, MatchLookup::Hit);
            assert!(Arc::ptr_eq(&first, &cached));
            assert_eq!(cache.len(), 1);
        }
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

    /// Synthetic high-fan-out + chain DAG for the P2b out-degree distribution check:
    /// 10,000 nodes / 19,800 edges, hub `node/0` with 9,999 out-edges, chain
    /// `node/1 -> node/2 -> ... -> node/9802` (9,801 edges), acyclic, no duplicates.
    fn skewed_node_graph() -> GraphDisplay {
        let nodes: Vec<Arc<GraphNodeDisplay>> = (0..10_000usize)
            .map(|index| {
                Arc::new(GraphNodeDisplay {
                    identity: desktop_protocol::NodeId::fixture(format!("node/{index}")),
                    label: Arc::from(format!("Node {index}")),
                    layer: None,
                })
            })
            .collect();
        let mut edges = Vec::with_capacity(19_800);
        let mut push = |from: usize, to: usize| {
            edges.push(Arc::new(GraphEdgeDisplay {
                identity: desktop_protocol::EdgeId::fixture(format!("skewed-edge/{from}-{to}")),
                from: desktop_protocol::NodeId::fixture(format!("node/{from}")),
                to: desktop_protocol::NodeId::fixture(format!("node/{to}")),
                from_label: Arc::from(format!("Node {from}")),
                to_label: Arc::from(format!("Node {to}")),
                kind: Some(Arc::from("FLOW")),
            }));
        };
        for target in 1..=9_999 {
            push(0, target);
        }
        for source in 1..=9_801 {
            push(source, source + 1);
        }
        assert_eq!(edges.len(), 19_800);
        GraphDisplay {
            graph_ref: GraphRef {
                graph_id: GraphId::fixture("graph/flow-skewed"),
                version: 1,
                digest: ArtifactDigest::fixture("digest/flow-skewed"),
            },
            source_revision: Counter::from_u128(2),
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

    /// MP-FLOW2 Stage C incremental build: same-run A/B of the pre-incremental rebuild
    /// against the publication-version-only reuse, plus the projection-revision rebuild that
    /// the content contract requires, on the same 10,000-node / 19,800-edge graph.
    /// Release-only evidence run; paint/GPU/Present are NOT_MEASURED.
    ///
    /// `cargo test --release -p desktop-gpui --lib ten_thousand_incremental -- --ignored --nocapture`
    #[test]
    #[ignore = "MP-FLOW2 incremental-build measurement; run explicitly with --release"]
    fn ten_thousand_incremental_match_set_series() {
        use std::time::Instant;

        const LAYER_COUNT: usize = 100;
        const WIDTH: usize = 100;
        const NODES: usize = LAYER_COUNT * WIDTH;
        const EDGES: usize = (LAYER_COUNT - 1) * WIDTH * 2;
        const ITERATIONS: usize = 21;
        const WARMUP: usize = 3;

        println!(
            "MP-FLOW2 incremental conditions release={} warmup={WARMUP} iterations={ITERATIONS} nodes={NODES} edges={EDGES} order=alternating",
            !cfg!(debug_assertions)
        );
        println!(
            "MP-FLOW2 incremental interval baseline=rebuild match sets+diagram; publication=publication-version-only reuse+diagram; projection_rebuild=projection-counter-only rebuild+diagram; hit=exact-key lookup+diagram"
        );
        println!(
            "MP-FLOW2 incremental aggregation n={ITERATIONS} median=odd:middle/even:mean(two middle) p95=nearest-rank ceil(n*0.95)-1 max=largest"
        );

        let graph = Arc::new(ten_thousand_node_graph(LAYER_COUNT, WIDTH));
        assert_eq!(graph.nodes.len(), NODES);
        assert_eq!(graph.edges.len(), EDGES);
        let rows = rows_cancellable(std::slice::from_ref(&graph), "", || false)
            .expect("uncancelled admission");
        assert_eq!(rows.nodes.len(), NODES);
        assert_eq!(rows.edges.len(), EDGES);
        let computed = layers(&graph);
        let base_key = AdmissionKey {
            session_generation: 21,
            publication_version: 34,
            epoch: Some(EpochId::fixture("epoch/incremental")),
            revision: revision_key(std::slice::from_ref(&graph)),
            query: String::new(),
        };
        // A projection-revision-only change: the canonical GraphRef is identical and only
        // the independent projection counter differs. The projection revision is part of the
        // row content identity, so this key change must rebuild rather than reuse.
        let mut revision_only = base_key.clone();
        revision_only.revision[0].1 = Counter::from_u128(2);
        assert_ne!(revision_only, base_key);
        assert_eq!(revision_only.revision[0].0, base_key.revision[0].0);
        // A publication-version-only change: no other admission component differs.
        let publication_only = AdmissionKey {
            publication_version: base_key.publication_version + 1,
            ..base_key.clone()
        };
        let reference_matched = matched_from_rows(&rows);
        let reference = diagram(
            &graph,
            &computed,
            &reference_matched.nodes,
            &reference_matched.edges,
        );
        assert_eq!(reference.nodes.len(), MAX_DIAGRAM_NODES);
        assert!(reference.truncated);

        let mut cache = MatchCache::default();
        let mut baseline_us = Vec::with_capacity(ITERATIONS);
        let mut publication_us = Vec::with_capacity(ITERATIONS);
        let mut projection_rebuild_us = Vec::with_capacity(ITERATIONS);
        let mut hit_us = Vec::with_capacity(ITERATIONS);
        for iteration in 0..(WARMUP + ITERATIONS) {
            // Baseline: the pre-incremental path — rebuild both sets from the admitted rows,
            // then draw. This is exactly what the render build closure does.
            let started = Instant::now();
            let rebuilt = matched_from_rows(&rows);
            let drawn = diagram(&graph, &computed, &rebuilt.nodes, &rebuilt.edges);
            let baseline = started.elapsed().as_micros();
            assert_eq!(drawn, reference);

            // Seed the entry with the base key; the previous iteration may have left another.
            let _ = cache.get_or_build(&base_key, || matched_from_rows(&rows));

            // Publication-only: the projection revision and canonical GraphRef are unchanged,
            // so the built set is reused and the entry re-keyed.
            let started = Instant::now();
            let (reused, outcome) = cache.lookup(&publication_only, || {
                panic!("a publication-version-only change must reuse the built set")
            });
            let drawn_publication = diagram(&graph, &computed, &reused.nodes, &reused.edges);
            let publication = started.elapsed().as_micros();
            assert_eq!(outcome, MatchLookup::Reused);
            assert_eq!(drawn_publication, reference);

            // Projection-revision-only: the projection counters are row content identity, so
            // the set must be rebuilt rather than reused.
            let started = Instant::now();
            let (rebuilt_counter, outcome) =
                cache.lookup(&revision_only, || matched_from_rows(&rows));
            let drawn_projection = diagram(
                &graph,
                &computed,
                &rebuilt_counter.nodes,
                &rebuilt_counter.edges,
            );
            let projection_rebuild = started.elapsed().as_micros();
            assert_eq!(outcome, MatchLookup::Rebuilt);
            assert_eq!(drawn_projection, reference);

            // Exact hit stays the cheapest path.
            let started = Instant::now();
            let (cached, outcome) = cache.lookup(&revision_only, || {
                panic!("a steady frame must hit the match cache")
            });
            let drawn_hit = diagram(&graph, &computed, &cached.nodes, &cached.edges);
            let hit = started.elapsed().as_micros();
            assert_eq!(outcome, MatchLookup::Hit);
            assert_eq!(drawn_hit, reference);

            if iteration >= WARMUP {
                let index = iteration - WARMUP;
                println!(
                    "inc_sample iter={index} baseline_us={baseline} publication_us={publication} projection_rebuild_us={projection_rebuild} hit_us={hit}"
                );
                baseline_us.push(baseline);
                publication_us.push(publication);
                projection_rebuild_us.push(projection_rebuild);
                hit_us.push(hit);
            }
        }
        assert_eq!(baseline_us.len(), ITERATIONS);

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
        for (name, values) in [
            ("baseline_rebuild", baseline_us),
            ("incremental_publication", publication_us),
            ("projection_rebuild", projection_rebuild_us),
            ("hit", hit_us),
        ] {
            let (median, p95, max) = stats(values);
            println!(
                "inc_summary series={name} n={ITERATIONS} median_us={median} p95_us={p95} max_us={max}"
            );
        }
        let checksum = diagram_checksum(&reference, &rows, &computed);
        println!(
            "MP-FLOW2 incremental checksum=0x{checksum:016x} reuse_equals_reference=true projection_rebuild_equals_reference=true"
        );
        println!("MP-FLOW2 incremental NOT_MEASURED paint/GPU/present");
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

    /// P2b adjacency regression: input order, duplicate edges and missing endpoints.
    #[test]
    #[ignore = "MP-FLOW2 measurement: release-only A/B; not part of the regular suite"]
    fn p2b_preallocated_ab_measurement() {
        use super::{
            layers_phased, layers_preallocated_phased, layers_preallocated_structural,
            layers_structural,
        };
        struct Samples {
            a_p2: Vec<u128>,
            a_p2b: Vec<u128>,
            a_all: Vec<u128>,
            b_p2: Vec<u128>,
            b_p2b: Vec<u128>,
            b_all: Vec<u128>,
        }
        fn stats(samples: &mut [u128]) -> (u128, u128, u128) {
            samples.sort_unstable();
            let n = samples.len();
            let median = if n % 2 == 1 {
                samples[n / 2]
            } else {
                (samples[n / 2 - 1] + samples[n / 2]) / 2
            };
            let p95 = samples[(n * 95).div_ceil(100) - 1];
            let max = *samples.last().unwrap();
            (median, p95, max)
        }
        fn run(name: &str, graph: &GraphDisplay) -> Samples {
            let (mut a_p2, mut a_p2b, mut a_all) = (Vec::new(), Vec::new(), Vec::new());
            let (mut b_p2, mut b_p2b, mut b_all) = (Vec::new(), Vec::new(), Vec::new());
            for round in 0..3 {
                let baseline_first = round != 1;
                let measure_a = || {
                    let (_layers, phases, all) = layers_phased(graph);
                    (phases[1], phases[8], all)
                };
                let measure_b = || {
                    let (_layers, phases, all) = layers_preallocated_phased(graph);
                    (phases[1], phases[8], all)
                };
                for _ in 0..3 {
                    let _ = measure_a();
                    let _ = measure_b();
                }
                for iteration in 0..21 {
                    let order = if baseline_first { "A-first" } else { "B-first" };
                    let (a, b) = if baseline_first {
                        let a = measure_a();
                        let b = measure_b();
                        (a, b)
                    } else {
                        let b = measure_b();
                        let a = measure_a();
                        (a, b)
                    };
                    println!(
                        "P2B_SAMPLE graph={name} round={round} iteration={iteration} order={order} variant=baseline p2={} p2b={} all={}",
                        a.0, a.1, a.2
                    );
                    println!(
                        "P2B_SAMPLE graph={name} round={round} iteration={iteration} order={order} variant=candidate p2={} p2b={} all={}",
                        b.0, b.1, b.2
                    );
                    a_p2.push(a.0);
                    a_p2b.push(a.1);
                    a_all.push(a.2);
                    b_p2.push(b.0);
                    b_p2b.push(b.1);
                    b_all.push(b.2);
                }
            }
            Samples {
                a_p2,
                a_p2b,
                a_all,
                b_p2,
                b_p2b,
                b_all,
            }
        }
        for (name, graph) in [
            ("normal", ten_thousand_node_graph(100, 100)),
            ("skewed", skewed_node_graph()),
        ] {
            let Samples {
                mut a_p2,
                mut a_p2b,
                mut a_all,
                mut b_p2,
                mut b_p2b,
                mut b_all,
            } = run(name, &graph);
            let a = (stats(&mut a_p2), stats(&mut a_p2b), stats(&mut a_all));
            let b = (stats(&mut b_p2), stats(&mut b_p2b), stats(&mut b_all));
            let (_l, base) = layers_structural(&graph);
            let (_l2, cand) = layers_preallocated_structural(&graph);
            println!(
                "P2B_RAW graph={name} baseline_p2={:?} baseline_p2b={:?} baseline_all={:?} candidate_p2={:?} candidate_p2b={:?} candidate_all={:?}",
                a_p2, a_p2b, a_all, b_p2, b_p2b, b_all
            );
            println!(
                "P2B_AB graph={name} baseline p2={:?} p2b={:?} all={:?} candidate p2={:?} p2b={:?} all={:?} base_cap total={} length={} initial={} grows={} nonempty={} maxout={} cand_cap total={} length={} initial={} grows={} nonempty={} maxout={}",
                a.0,
                a.1,
                a.2,
                b.0,
                b.1,
                b.2,
                base.total_capacity,
                base.total_length,
                base.initial_allocations,
                base.capacity_grows,
                base.nonempty_nodes,
                base.max_outdegree,
                cand.total_capacity,
                cand.total_length,
                cand.initial_allocations,
                cand.capacity_grows,
                cand.nonempty_nodes,
                cand.max_outdegree
            );
        }
    }

    #[test]
    fn p2b_preallocated_adjacency_is_equivalent_on_dag_skew_duplicates_and_cycles() {
        fn adversarial() -> GraphDisplay {
            let nodes: Vec<Arc<GraphNodeDisplay>> = (0..=5)
                .map(|index| {
                    Arc::new(GraphNodeDisplay {
                        identity: desktop_protocol::NodeId::fixture(format!("node/{index}")),
                        label: Arc::from(format!("Node {index}")),
                        layer: None,
                    })
                })
                .collect();
            let mut edges = Vec::new();
            let mut push = |from: usize, to: usize| {
                edges.push(Arc::new(GraphEdgeDisplay {
                    identity: desktop_protocol::EdgeId::fixture(format!(
                        "adv/{from}-{to}-{}",
                        edges.len()
                    )),
                    from: desktop_protocol::NodeId::fixture(format!("node/{from}")),
                    to: desktop_protocol::NodeId::fixture(format!("node/{to}")),
                    from_label: Arc::from("from"),
                    to_label: Arc::from("to"),
                    kind: None,
                }))
            };
            push(0, 1);
            push(0, 1);
            push(0, 2);
            push(1, 2);
            push(2, 1);
            push(3, 4);
            push(4, 4);
            push(1, 3);
            push(5, 1);
            edges.push(Arc::new(GraphEdgeDisplay {
                identity: desktop_protocol::EdgeId::fixture("adv/missing-to"),
                from: desktop_protocol::NodeId::fixture("node/0"),
                to: desktop_protocol::NodeId::fixture("node/999"),
                from_label: Arc::from("from"),
                to_label: Arc::from("to"),
                kind: None,
            }));
            edges.push(Arc::new(GraphEdgeDisplay {
                identity: desktop_protocol::EdgeId::fixture("adv/missing-from"),
                from: desktop_protocol::NodeId::fixture("node/999"),
                to: desktop_protocol::NodeId::fixture("node/0"),
                from_label: Arc::from("from"),
                to_label: Arc::from("to"),
                kind: None,
            }));
            GraphDisplay {
                graph_ref: GraphRef {
                    graph_id: GraphId::fixture("graph/adversarial"),
                    version: 1,
                    digest: ArtifactDigest::fixture("digest/adversarial"),
                },
                source_revision: Counter::from_u128(7),
                nodes: nodes.into(),
                edges: edges.into(),
            }
        }
        for graph in [
            ten_thousand_node_graph(100, 100),
            skewed_node_graph(),
            adversarial(),
        ] {
            let baseline = layers(&graph);
            let candidate = layers_preallocated(&graph);
            assert_eq!(candidate.depth, baseline.depth);
            assert_eq!(candidate.cyclic, baseline.cyclic);
            let nodes: std::collections::HashSet<(GraphRef, desktop_protocol::NodeId)> =
                std::collections::HashSet::new();
            let edges: std::collections::HashSet<(GraphRef, desktop_protocol::EdgeId)> =
                std::collections::HashSet::new();
            let a = diagram(&graph, &baseline, &nodes, &edges);
            let b = diagram(&graph, &candidate, &nodes, &edges);
            assert_eq!(b, a, "diagram must be identical");
        }
    }

    #[test]
    fn the_p2b_adjacency_preserves_order_duplicates_and_skips_missing_endpoints() {
        let base = graph(3, "digest/three", 8);
        let edge = |identity: &str, from: &str, to: &str| {
            Arc::new(GraphEdgeDisplay {
                identity: desktop_protocol::EdgeId::fixture(identity),
                from: desktop_protocol::NodeId::fixture(from),
                to: desktop_protocol::NodeId::fixture(to),
                from_label: Arc::from(from),
                to_label: Arc::from(to),
                kind: Some(Arc::from("FLOW")),
            })
        };
        let display = Arc::new(GraphDisplay {
            graph_ref: GraphRef {
                graph_id: GraphId::fixture("graph/flow-p2b-order"),
                version: 1,
                digest: ArtifactDigest::fixture("digest/flow-p2b-order"),
            },
            source_revision: Counter::from_u128(1),
            nodes: base.nodes.clone(),
            edges: vec![
                edge("edge/1-z-m", "node/z", "node/m"),
                edge("edge/2-z-a", "node/z", "node/a"),
                // Same endpoints, distinct EdgeId: the duplicate must be kept.
                edge("edge/3-z-a-dup", "node/z", "node/a"),
                edge("edge/4-absent-m", "node/absent", "node/m"),
                edge("edge/5-a-m", "node/a", "node/m"),
                edge("edge/6-m-absent", "node/m", "node/absent"),
            ]
            .into(),
        });
        let (analyzed, stats) = super::layers_structural(&display);
        assert_eq!(
            analyzed,
            super::layers(&display),
            "analyzed and plain paths agree"
        );
        assert_eq!(analyzed.depth, reference_depths(&display));
        assert_eq!(analyzed.depth, vec![0, 1, 2]);
        assert!(!analyzed.cyclic);
        assert_eq!(
            stats.adjacency,
            vec![vec![2, 1, 1], vec![2], vec![]],
            "input order, duplicates and skipped endpoints all at once"
        );
        assert_eq!(stats.inserted_edges, 4);
        assert_eq!(stats.skipped_edges, 2);
        assert_eq!(stats.total_length, 4);
        assert_eq!(stats.nonempty_nodes, 2);
        assert_eq!(stats.max_outdegree, 3);
        assert_eq!(stats.inserted_edges + stats.skipped_edges, 6);
        assert!(
            stats
                .adjacency
                .iter()
                .all(|list| list.capacity() >= list.len())
        );
    }

    /// MP-FLOW2 revision change vs query-only: separate Rows (background), Layers
    /// (display), match-set build/replace and Diagram; release measurement only.
    ///
    /// `cargo test --release -p desktop-gpui --lib ten_thousand_revision -- --ignored --nocapture`
    #[test]
    #[ignore = "MP-FLOW2 revision-change measurement; run explicitly with --release"]
    fn ten_thousand_revision_change_series() {
        use std::time::Instant;

        const LAYER_COUNT: usize = 100;
        const WIDTH: usize = 100;
        const ITERATIONS: usize = 21;
        const WARMUP: usize = 3;

        println!(
            "MP-FLOW2 revision conditions release={} warmup={WARMUP} iterations={ITERATIONS} nodes={} edges={} order=alternating",
            !cfg!(debug_assertions),
            LAYER_COUNT * WIDTH,
            (LAYER_COUNT - 1) * WIDTH * 2
        );
        println!(
            "MP-FLOW2 revision interval rows=background re-derivation (separate thread class); layers=display recompute (query-only reuses the cached layers, reported as 0); match=HashSet build/replace; diagram=Diagram build; display_total=layers+match+diagram (do not read as frame latency)"
        );
        println!(
            "MP-FLOW2 revision aggregation n={ITERATIONS} median=odd:middle/even:mean(two middle) p95=nearest-rank ceil(n*0.95)-1 max=largest"
        );

        let graph = Arc::new(super::tests::ten_thousand_node_graph(LAYER_COUNT, WIDTH));
        let revision = Arc::new(GraphDisplay {
            graph_ref: GraphRef {
                graph_id: graph.graph_ref.graph_id.clone(),
                version: graph.graph_ref.version + 1,
                digest: desktop_protocol::ArtifactDigest::fixture("digest/flow-bench-r2"),
            },
            source_revision: Counter::from_u128(2),
            nodes: graph.nodes.clone(),
            edges: graph.edges.clone(),
        });
        let layers_first = layers(&graph);
        let rows_query =
            rows_cancellable(std::slice::from_ref(&graph), " ", || false).expect("query rows");
        let rows_base =
            rows_cancellable(std::slice::from_ref(&graph), "", || false).expect("base rows");
        let key_query_base = AdmissionKey {
            session_generation: 7,
            publication_version: 11,
            epoch: Some(EpochId::fixture("epoch/measure")),
            revision: revision_key(std::slice::from_ref(&graph)),
            query: String::new(),
        };
        let key_old = AdmissionKey {
            session_generation: 7,
            publication_version: 11,
            epoch: Some(EpochId::fixture("epoch/measure")),
            revision: revision_key(std::slice::from_ref(&graph)),
            query: String::new(),
        };
        let key_query = AdmissionKey {
            session_generation: 7,
            publication_version: 11,
            epoch: Some(EpochId::fixture("epoch/measure")),
            revision: revision_key(std::slice::from_ref(&graph)),
            query: " ".to_owned(),
        };
        let key_revision = AdmissionKey {
            session_generation: 7,
            publication_version: 12,
            epoch: Some(EpochId::fixture("epoch/measure")),
            revision: revision_key(std::slice::from_ref(&revision)),
            query: String::new(),
        };
        let mut cache = MatchCache::default();
        let query_started = Instant::now();
        let (query_matched, query_hit) =
            cache.get_or_build(&key_query, || matched_from_rows(&rows_query));
        let query_diagram = diagram(
            &graph,
            &layers_first,
            &query_matched.nodes,
            &query_matched.edges,
        );
        let query_match_us = query_started.elapsed().as_micros();
        // Query-only key: identical except `query`.
        assert_eq!(
            key_query.session_generation,
            key_query_base.session_generation
        );
        assert_eq!(
            key_query.publication_version,
            key_query_base.publication_version
        );
        assert_eq!(key_query.epoch, key_query_base.epoch);
        assert_eq!(key_query.revision, key_query_base.revision);
        assert_ne!(key_query.query, key_query_base.query);
        // Revision key: identical except revision and publication_version.
        assert_eq!(key_revision.session_generation, key_old.session_generation);
        assert_eq!(key_revision.epoch, key_old.epoch);
        assert_eq!(key_revision.query, key_old.query);
        assert_ne!(key_revision.revision, key_old.revision);
        assert_ne!(
            key_revision.publication_version,
            key_old.publication_version
        );
        let mut base_cache = MatchCache::default();
        let (base_matched, _) =
            base_cache.get_or_build(&key_query_base, || matched_from_rows(&rows_base));
        assert_eq!(base_matched.nodes.len(), query_matched.nodes.len());
        assert_eq!(base_matched.edges.len(), query_matched.edges.len());
        // Query-only keeps the revision and publication version: only the query text
        // changes. A whitespace-only query trims to the empty filter, so the admitted
        // rows and match-set cardinality stay identical.
        assert_eq!(rows_query.nodes.len(), LAYER_COUNT * WIDTH);
        assert_eq!(rows_query.edges.len(), (LAYER_COUNT - 1) * WIDTH * 2);
        assert_eq!(query_matched.nodes.len(), LAYER_COUNT * WIDTH);
        assert_eq!(query_matched.edges.len(), (LAYER_COUNT - 1) * WIDTH * 2);
        let checksum = |drawn: &super::Diagram| {
            let mut value = 0u64;
            for node in &drawn.nodes {
                for byte in node.node.as_str().bytes() {
                    value ^= u64::from(byte);
                    value = value.wrapping_mul(0x100_0000_01b3);
                }
                value ^= node.depth as u64;
                value = value.wrapping_mul(0x100_0000_01b3);
            }
            value ^= drawn.edges.len() as u64;
            value ^= (drawn.skipped_edges as u64) << 32;
            value ^= u64::from(drawn.truncated);
            value
        };
        // Revision-reference Diagram from the same topology with the new GraphRef.
        let rows_revision =
            rows_cancellable(std::slice::from_ref(&revision), "", || false).expect("revision rows");
        let layers_revision = layers(&revision);
        let mut reference_cache = MatchCache::default();
        let (reference_matched, _) =
            reference_cache.get_or_build(&key_revision, || matched_from_rows(&rows_revision));
        let revision_reference = diagram(
            &revision,
            &layers_revision,
            &reference_matched.nodes,
            &reference_matched.edges,
        );
        let revision_checksum = checksum(&revision_reference);
        assert_eq!(revision_reference.nodes.len(), super::MAX_DIAGRAM_NODES);
        assert!(revision_reference.truncated);
        assert_eq!(
            revision_reference.edges.len() + revision_reference.skipped_edges,
            (LAYER_COUNT - 1) * WIDTH * 2
        );
        // Pin the expected numbers directly instead of only comparing them across sides.
        assert_eq!(
            revision_reference.skipped_edges, 19_760,
            "the revision diagram skips the same 19,760 edges"
        );
        let revision_nodes: std::collections::HashSet<_> = reference_matched
            .nodes
            .iter()
            .map(|(graph_ref, _)| graph_ref)
            .collect();
        assert_eq!(
            revision_nodes.len(),
            1,
            "the revision match set holds a single graph reference"
        );
        assert!(
            revision_nodes.contains(&&revision.graph_ref),
            "the revision match set belongs to the new graph reference"
        );
        assert_eq!(
            reference_matched.nodes.len(),
            LAYER_COUNT * WIDTH,
            "the revision match set covers all 10,000 nodes"
        );
        assert_eq!(
            rows_revision.edges.len(),
            (LAYER_COUNT - 1) * WIDTH * 2,
            "the revision rows hold all 19,800 edges"
        );
        assert_eq!(
            reference_matched.edges.len(),
            (LAYER_COUNT - 1) * WIDTH * 2,
            "the revision match set covers all 19,800 edges"
        );
        assert!(!query_hit, "a query change is a cache miss");

        let mut query = Vec::new();
        let mut rev = Vec::new();
        for iteration in 0..(WARMUP + ITERATIONS) {
            let query_first = iteration % 2 == 0;
            let measure_query = |cache: &mut MatchCache| {
                let rows_started = Instant::now();
                let rows = rows_cancellable(std::slice::from_ref(&graph), " ", || false)
                    .expect("query rows");
                let rows_us = rows_started.elapsed().as_micros();
                // Query-only: the entry replaced is the same revision/publication with
                // the empty query, so only `query` changes.
                let _ = cache.get_or_build(&key_query_base, || matched_from_rows(&rows_base));
                let match_started = Instant::now();
                let (matched, hit) = cache.get_or_build(&key_query, || matched_from_rows(&rows));
                let match_us = match_started.elapsed().as_micros();
                assert!(!hit, "the other key replaces the entry");
                let diagram_started = Instant::now();
                let drawn = diagram(&graph, &layers_first, &matched.nodes, &matched.edges);
                let diagram_us = diagram_started.elapsed().as_micros();
                assert_eq!(drawn, query_diagram, "query-only draws the same diagram");
                (rows_us, 0_u128, match_us, diagram_us)
            };
            let measure_revision = |cache: &mut MatchCache| {
                // Revision: the entry replaced is the old revision with the same empty
                // query, so only revision and publication_version change.
                let _ = cache.get_or_build(&key_old, || matched_from_rows(&rows_base));
                let rows_started = Instant::now();
                let rows = rows_cancellable(std::slice::from_ref(&revision), "", || false)
                    .expect("revision rows");
                let rows_us = rows_started.elapsed().as_micros();
                let layers_started = Instant::now();
                let computed = layers(&revision);
                let layers_us = layers_started.elapsed().as_micros();
                let match_started = Instant::now();
                let (matched, hit) = cache.get_or_build(&key_revision, || matched_from_rows(&rows));
                let match_us = match_started.elapsed().as_micros();
                assert!(!hit, "a revision change is a cache miss");
                let diagram_started = Instant::now();
                let drawn = diagram(&revision, &computed, &matched.nodes, &matched.edges);
                let diagram_us = diagram_started.elapsed().as_micros();
                assert_eq!(computed.depth, layers_first.depth);
                assert_eq!(computed.cyclic, layers_first.cyclic);
                assert_eq!(drawn, revision_reference, "revision diagram is identical");
                assert_eq!(
                    checksum(&drawn),
                    revision_checksum,
                    "revision checksum is stable"
                );
                (rows_us, layers_us, match_us, diagram_us)
            };
            let (q, r) = if query_first {
                let q = measure_query(&mut cache);
                let r = measure_revision(&mut cache);
                (q, r)
            } else {
                let r = measure_revision(&mut cache);
                let q = measure_query(&mut cache);
                (q, r)
            };
            if iteration >= WARMUP {
                let index = iteration - WARMUP;
                println!(
                    "rev_sample condition=query_only iter={index} rows_us={} layers_us={} match_us={} diagram_us={}",
                    q.0, q.1, q.2, q.3
                );
                println!(
                    "rev_sample condition=revision iter={index} rows_us={} layers_us={} match_us={} diagram_us={}",
                    r.0, r.1, r.2, r.3
                );
                query.push(q);
                rev.push(r);
            }
        }
        assert_eq!(query.len(), ITERATIONS);
        assert_eq!(rev.len(), ITERATIONS);
        let _ = query_match_us;
        let _ = query_started;

        // Seven-phase decomposition of the same layers path (P1..P7), test-only.
        let mut phase_samples: Vec<([u128; 9], u128, u128)> = Vec::new();
        let expected_depth = reference_depths(&revision);
        for iteration in 0..(WARMUP + ITERATIONS) {
            let (phased, phases, outer_us) = super::layers_phased(&revision);
            assert_eq!(
                phased.depth, expected_depth,
                "the reference Kahn depths agree"
            );
            assert_eq!(
                phased,
                super::layers(&revision),
                "the phased path returns the same layers"
            );
            assert!(!phased.cyclic, "the synthetic DAG is acyclic");
            assert_eq!(phased.depth.len(), LAYER_COUNT * WIDTH);
            if iteration >= WARMUP {
                let index = iteration - WARMUP;
                // Phase sum uses P1..P7 exactly; P2a/P2b are subdivisions of P2.
                let sum_us: u128 = phases[0..=6].iter().sum();
                let unaccounted = outer_us.saturating_sub(sum_us);
                assert_eq!(phases[7] + phases[8], phases[1], "P2a+P2b equals P2");
                println!(
                    "layer_phase_sample iter={index} p1_us={} p2_us={} p2a_us={} p2b_us={} p3_us={} p4_us={} p5_us={} p6_us={} p7_drop_us={} sum_us={sum_us} outer_total_us={outer_us} unattributed_us={unaccounted}",
                    phases[0],
                    phases[1],
                    phases[7],
                    phases[8],
                    phases[2],
                    phases[3],
                    phases[4],
                    phases[5],
                    phases[6]
                );
                phase_samples.push((phases, outer_us, sum_us));
            }
        }
        assert_eq!(phase_samples.len(), ITERATIONS);
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
        let outer_values: Vec<u128> = phase_samples.iter().map(|sample| sample.1).collect();
        let sum_values: Vec<u128> = phase_samples.iter().map(|sample| sample.2).collect();
        let (outer_median, outer_p95, outer_max) = stats(outer_values);
        let (sum_median, sum_p95, sum_max) = stats(sum_values);
        let unattributed_values: Vec<u128> = phase_samples
            .iter()
            .map(|sample| sample.1.saturating_sub(sample.2))
            .collect();
        let (unattributed_median, unattributed_p95, unattributed_max) = stats(unattributed_values);
        for (index, name) in [
            "p1_index",
            "p2_successors",
            "p3_tarjan",
            "p4_condense",
            "p5_kahn",
            "p6_depth",
            "p7_deallocation",
        ]
        .iter()
        .enumerate()
        {
            let values: Vec<u128> = phase_samples.iter().map(|sample| sample.0[index]).collect();
            let (median, p95, max) = stats(values);
            let share = median as f64 * 100.0 / outer_median as f64;
            println!(
                "layer_phase_summary phase={name} n={ITERATIONS} median_us={median} p95_us={p95} max_us={max} share_of_outer_pct={share:.1}"
            );
        }
        for (name, index) in [
            ("p2_all", 1usize),
            ("p2a_outer_init", 7),
            ("p2b_edge_pass", 8),
        ] {
            let values: Vec<u128> = phase_samples.iter().map(|sample| sample.0[index]).collect();
            let (median, p95, max) = stats(values);
            let share = median as f64 * 100.0 / outer_median as f64;
            println!(
                "layer_phase_summary phase={name} n={ITERATIONS} median_us={median} p95_us={p95} max_us={max} share_of_outer_pct={share:.1}"
            );
        }
        for (name, median, p95, max) in [
            ("outer_total", outer_median, outer_p95, outer_max),
            ("phase_sum", sum_median, sum_p95, sum_max),
            (
                "unattributed",
                unattributed_median,
                unattributed_p95,
                unattributed_max,
            ),
        ] {
            println!(
                "layer_phase_summary phase={name} n={ITERATIONS} median_us={median} p95_us={p95} max_us={max}"
            );
        }

        // One full structural pass over the P2b edge loop (counters, not timing).
        let (structured, stats) = super::layers_structural(&revision);
        assert_eq!(
            structured,
            super::layers(&revision),
            "analyzed path returns the same layers"
        );
        assert_eq!(structured.depth, expected_depth);
        assert!(!structured.cyclic);
        assert_eq!(
            stats.inserted_edges + stats.skipped_edges,
            (LAYER_COUNT - 1) * WIDTH * 2,
            "every edge is inserted or skipped"
        );
        assert_eq!(
            stats.skipped_edges, 0,
            "the synthetic graph has no unknown endpoints"
        );
        assert_eq!(stats.total_length, stats.inserted_edges);
        assert!(stats.total_capacity >= stats.total_length);
        assert_eq!(
            stats.adjacency.iter().map(Vec::len).sum::<usize>(),
            stats.total_length
        );
        assert_eq!(stats.nonempty_nodes, (LAYER_COUNT - 1) * WIDTH);
        assert_eq!(
            stats
                .adjacency
                .iter()
                .filter(|list| list.is_empty())
                .count(),
            WIDTH
        );
        assert_eq!(stats.max_outdegree, 2);
        assert_eq!(stats.initial_allocations, stats.nonempty_nodes);
        assert!(stats.initial_allocations + stats.capacity_grows <= stats.inserted_edges);
        // Independent adjacency reference: same edges, same order.
        let mut reference_adjacency: Vec<Vec<usize>> = vec![Vec::new(); LAYER_COUNT * WIDTH];
        let mut index_of = std::collections::HashMap::new();
        for (index, node) in revision.nodes.iter().enumerate() {
            index_of.insert(node.identity.clone(), index);
        }
        for edge in revision.edges.iter() {
            let (Some(&from), Some(&to)) = (index_of.get(&edge.from), index_of.get(&edge.to))
            else {
                continue;
            };
            reference_adjacency[from].push(to);
        }
        assert_eq!(
            stats.adjacency, reference_adjacency,
            "adjacency content and order agree"
        );
        println!(
            "layer_structure condition=baseline inserted_edges={} skipped_edges={} initial_allocations={} capacity_grows={} total_capacity={} total_length={} nonempty_nodes={} empty_nodes={} max_outdegree={} unused_slots={} utilization_pct={:.1}",
            stats.inserted_edges,
            stats.skipped_edges,
            stats.initial_allocations,
            stats.capacity_grows,
            stats.total_capacity,
            stats.total_length,
            stats.nonempty_nodes,
            stats
                .adjacency
                .iter()
                .filter(|list| list.is_empty())
                .count(),
            stats.max_outdegree,
            stats.total_capacity - stats.total_length,
            stats.total_length as f64 * 100.0 / stats.total_capacity as f64,
        );

        // Same node/edge counts, different out-degree distribution (hub + chain).
        let skewed = super::tests::skewed_node_graph();
        let (skewed_layers, skewed_stats) = super::layers_structural(&skewed);
        assert_eq!(skewed_layers, super::layers(&skewed));
        assert_eq!(skewed_layers.depth, reference_depths(&skewed));
        assert!(!skewed_layers.cyclic);
        assert_eq!(skewed_stats.inserted_edges, 19_800);
        assert_eq!(skewed_stats.skipped_edges, 0);
        assert_eq!(skewed_stats.total_length, 19_800);
        assert_eq!(skewed_stats.nonempty_nodes, 9_802);
        assert_eq!(
            skewed_stats
                .adjacency
                .iter()
                .filter(|list| list.is_empty())
                .count(),
            198
        );
        assert_eq!(skewed_stats.max_outdegree, 9_999);
        assert!(skewed_stats.capacity_grows > 0);
        assert!(skewed_stats.total_capacity >= skewed_stats.total_length);
        assert!(
            skewed_stats
                .adjacency
                .iter()
                .all(|list| list.capacity() >= list.len())
        );
        let mut skewed_reference: Vec<Vec<usize>> = vec![Vec::new(); 10_000];
        let mut skewed_index = std::collections::HashMap::new();
        for (index, node) in skewed.nodes.iter().enumerate() {
            skewed_index.insert(node.identity.clone(), index);
        }
        for edge in skewed.edges.iter() {
            let (Some(&from), Some(&to)) =
                (skewed_index.get(&edge.from), skewed_index.get(&edge.to))
            else {
                continue;
            };
            skewed_reference[from].push(to);
        }
        assert_eq!(
            skewed_stats.adjacency, skewed_reference,
            "skewed adjacency content and order agree"
        );
        println!(
            "layer_structure condition=skewed inserted_edges={} skipped_edges={} initial_allocations={} capacity_grows={} total_capacity={} total_length={} nonempty_nodes={} empty_nodes={} max_outdegree={} unused_slots={} utilization_pct={:.1}",
            skewed_stats.inserted_edges,
            skewed_stats.skipped_edges,
            skewed_stats.initial_allocations,
            skewed_stats.capacity_grows,
            skewed_stats.total_capacity,
            skewed_stats.total_length,
            skewed_stats.nonempty_nodes,
            skewed_stats
                .adjacency
                .iter()
                .filter(|list| list.is_empty())
                .count(),
            skewed_stats.max_outdegree,
            skewed_stats.total_capacity - skewed_stats.total_length,
            skewed_stats.total_length as f64 * 100.0 / skewed_stats.total_capacity as f64,
        );

        // Different GraphRef must not reuse the other revision's match sets.
        let rows_revision =
            rows_cancellable(std::slice::from_ref(&revision), "", || false).expect("revision rows");
        cache.clear();
        let _ = cache.get_or_build(&key_revision, || matched_from_rows(&rows_revision));
        let (revision_matched, revision_hit) =
            cache.get_or_build(&key_revision, || panic!("the second lookup must hit"));
        assert!(revision_hit, "the key is now cached");
        assert!(
            revision_matched
                .nodes
                .iter()
                .all(|(graph_ref, _)| *graph_ref == revision.graph_ref)
        );
        assert!(
            revision_matched
                .edges
                .iter()
                .all(|(graph_ref, _)| *graph_ref == revision.graph_ref)
        );
        // The same semantic query before and after the revision change draws the same
        // diagram: identities, order, depths, cap, truncation, skipped edges, checksum.
        assert_eq!(query_diagram.nodes.len(), revision_reference.nodes.len());
        assert_eq!(query_diagram.nodes.len(), super::MAX_DIAGRAM_NODES);
        assert!(query_diagram.truncated);
        assert_eq!(query_diagram, revision_reference);
        assert_eq!(checksum(&query_diagram), checksum(&revision_reference));
        assert_eq!(
            query_diagram.edges.len() + query_diagram.skipped_edges,
            (LAYER_COUNT - 1) * WIDTH * 2
        );
        assert_eq!(
            query_diagram.skipped_edges,
            revision_reference.skipped_edges
        );
        let reference = diagram(
            &revision,
            &layers(&revision),
            &revision_matched.nodes,
            &revision_matched.edges,
        );
        println!(
            "MP-FLOW2 revision equivalence layers_equal=true diagram_nodes={} truncated={} skipped_edges={}",
            reference.nodes.len(),
            reference.truncated,
            reference.skipped_edges
        );
        println!("MP-FLOW2 revision NOT_MEASURED paint/GPU/present");

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
        for (name, samples) in [("query_only", &query), ("revision", &rev)] {
            let rows: Vec<u128> = samples.iter().map(|sample| sample.0).collect();
            let layers: Vec<u128> = samples.iter().map(|sample| sample.1).collect();
            let match_only: Vec<u128> = samples.iter().map(|sample| sample.2).collect();
            let diagram_only: Vec<u128> = samples.iter().map(|sample| sample.3).collect();
            let display_total: Vec<u128> = samples
                .iter()
                .map(|sample| sample.1 + sample.2 + sample.3)
                .collect();
            for (phase, values) in [
                ("rows_background", rows),
                ("layers_display", layers),
                ("match_display", match_only),
                ("diagram_display", diagram_only),
                ("display_total", display_total),
            ] {
                let (median, p95, max) = stats(values);
                println!(
                    "rev_summary condition={name} phase={phase} n={ITERATIONS} median_us={median} p95_us={p95} max_us={max}"
                );
            }
        }
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
