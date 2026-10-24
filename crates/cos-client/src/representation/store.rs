//! Representation stores behind the MP-PUB4 experiment switch.
//!
//! Every store replays the same already-validated transaction stream. Only the
//! representation differs: the product baseline keeps one `Arc<ProjectionState>`
//! and clones the whole state through `Arc::make_mut` while a snapshot handle is
//! retained; the candidates keep one `Arc` map per projection family (and an
//! optional task index or revision-keyed cache) so an applied transaction copies
//! only the families and auxiliary structures it touches.

use std::collections::{BTreeMap, BTreeSet};
use std::mem::size_of;
use std::sync::Arc;

use desktop_protocol::{
    CandidateRef, CandidateSnapshot, CapabilitiesId, Counter, CreativeCapabilities, EpochId,
    IterationId, IterationSnapshot, OutcomeId, OutcomeSnapshot, Projection, ProjectionMutation,
    RemoveTarget, TaskId, TaskSnapshot,
};

use crate::ProjectionKey;

use super::{
    ExperimentCounters, ExperimentError, FamilyCounts, ObservationValues, ObservedEntry,
    RepresentationStrategy, TaskDerivation,
};
use crate::ProjectionState;

fn task_bytes(snapshot: &TaskSnapshot) -> u64 {
    (size_of::<TaskSnapshot>() + snapshot.display_id.len() + snapshot.title.len()) as u64
}

fn candidate_bytes(snapshot: &CandidateSnapshot) -> u64 {
    (size_of::<CandidateSnapshot>()
        + snapshot.display_label.len()
        + snapshot.artifact_refs.len() * size_of::<desktop_protocol::ArtifactId>()
        + snapshot.evaluation_refs.len() * size_of::<desktop_protocol::EvaluationId>()) as u64
}

fn iteration_bytes(snapshot: &IterationSnapshot) -> u64 {
    (size_of::<IterationSnapshot>()
        + snapshot.candidate_refs.len() * size_of::<CandidateRef>()
        + snapshot.refinement_refs.len() * size_of::<desktop_protocol::RefinementId>()
        + snapshot.evidence_refs.len() * size_of::<desktop_protocol::EvidenceRef>()) as u64
}

fn outcome_bytes(snapshot: &OutcomeSnapshot) -> u64 {
    (size_of::<OutcomeSnapshot>()
        + snapshot.selected_refs.len() * size_of::<desktop_protocol::FormalSelectionRef>()
        + snapshot.provenance_refs.len() * size_of::<desktop_protocol::ProvenanceId>()
        + snapshot.evidence_refs.len() * size_of::<desktop_protocol::EvidenceRef>()) as u64
}

fn capabilities_bytes(snapshot: &CreativeCapabilities) -> u64 {
    (size_of::<CreativeCapabilities>()
        + snapshot.commands.len() * size_of::<desktop_protocol::CommandCapability>()
        + snapshot.media.len() * size_of::<desktop_protocol::MediaCapability>()
        + snapshot.features.len() * size_of::<desktop_protocol::FeatureCapability>()
        + snapshot.constraints.len() * size_of::<desktop_protocol::CapabilityConstraint>())
        as u64
}

fn projection_bytes(projection: &Projection) -> u64 {
    match projection {
        Projection::Task(snapshot) => task_bytes(snapshot),
        Projection::Candidate(snapshot) => candidate_bytes(snapshot),
        Projection::Artifact(snapshot) => {
            (size_of::<desktop_protocol::ArtifactSnapshot>()
                + snapshot.media_type.as_ref().map_or(0, String::len)) as u64
        }
        Projection::Evaluation(snapshot) => {
            (size_of::<desktop_protocol::EvaluationSnapshot>()
                + snapshot.axes.len() * size_of::<desktop_protocol::NamedScore>()
                + snapshot.evidence_refs.len() * size_of::<desktop_protocol::EvidenceRef>())
                as u64
        }
        Projection::Iteration(snapshot) => iteration_bytes(snapshot),
        Projection::Outcome(snapshot) => outcome_bytes(snapshot),
        Projection::Graph(snapshot) => {
            (size_of::<desktop_protocol::GraphSnapshot>()
                + snapshot.nodes.len() * size_of::<desktop_protocol::GraphNode>()
                + snapshot.edges.len() * size_of::<desktop_protocol::GraphEdge>())
                as u64
        }
        Projection::Capabilities(snapshot) => capabilities_bytes(snapshot),
    }
}

fn state_entries(state: &ProjectionState) -> u64 {
    (state.tasks.len()
        + state.candidates.len()
        + state.artifacts.len()
        + state.evaluations.len()
        + state.iterations.len()
        + state.outcomes.len()
        + state.graphs.len()
        + state.capabilities.len()) as u64
}

fn revision_bytes() -> u64 {
    (size_of::<ProjectionKey>() + size_of::<Counter>()) as u64
}

fn state_bytes(state: &ProjectionState) -> u64 {
    state.tasks.values().map(task_bytes).sum::<u64>()
        + state.candidates.values().map(candidate_bytes).sum::<u64>()
        + state.iterations.values().map(iteration_bytes).sum::<u64>()
        + state.outcomes.values().map(outcome_bytes).sum::<u64>()
        + state
            .capabilities
            .values()
            .map(capabilities_bytes)
            .sum::<u64>()
        + state.revisions.len() as u64 * revision_bytes()
}

fn observed_entries(revisions: &BTreeMap<ProjectionKey, Counter>) -> Vec<ObservedEntry> {
    revisions
        .iter()
        .map(|(key, revision)| ObservedEntry {
            key: key.clone(),
            revision: revision.clone(),
        })
        .collect()
}

/// Per-task candidate/iteration counts in one pass.
///
/// The counters record the derivation shape a representation pays; the harness
/// itself builds this bounded map so the synthetic sizes stay fast and the
/// comparison never depends on wall-clock time.
fn per_task_counts(
    candidates: &BTreeMap<CandidateRef, CandidateSnapshot>,
    iterations: &BTreeMap<IterationId, IterationSnapshot>,
) -> BTreeMap<TaskId, (usize, usize)> {
    let mut counts: BTreeMap<TaskId, (usize, usize)> = BTreeMap::new();
    for candidate in candidates.values() {
        counts.entry(candidate.task_ref.clone()).or_default().0 += 1;
    }
    for iteration in iterations.values() {
        counts.entry(iteration.task_ref.clone()).or_default().1 += 1;
    }
    counts
}

/// Product observation values: no counting, no representation bias.
pub(crate) fn observe_state(state: &ProjectionState) -> ObservationValues {
    let per_task = per_task_counts(&state.candidates, &state.iterations);
    let mut derivations = Vec::with_capacity(state.tasks.len());
    for task in state.tasks.values() {
        let (candidates, iterations) = per_task.get(&task.task_ref).copied().unwrap_or((0, 0));
        derivations.push(TaskDerivation {
            task_id: task.task_ref.clone(),
            candidates,
            iterations,
            formal_outcome: task.formal_outcome_ref.is_some(),
        });
    }
    ObservationValues {
        epoch: state.epoch.clone(),
        entries: observed_entries(&state.revisions),
        counts: FamilyCounts {
            tasks: state.tasks.len(),
            candidates: state.candidates.len(),
            iterations: state.iterations.len(),
            outcomes: state.outcomes.len(),
            capabilities: state.capabilities.len(),
        },
        derivations,
    }
}

/// Validate one transaction with the product's own rules.
pub(crate) fn validate_transaction(
    state: &ProjectionState,
    mutations: &[ProjectionMutation],
) -> Result<(), ExperimentError> {
    let mut upserts: Vec<Projection> = Vec::with_capacity(mutations.len());
    let mut removals: Vec<RemoveTarget> = Vec::with_capacity(mutations.len());
    for mutation in mutations {
        state.validate_mutation(mutation)?;
        match mutation {
            ProjectionMutation::Upsert { projection, .. } => upserts.push(projection.clone()),
            ProjectionMutation::Remove { target, .. } => removals.push(target.clone()),
        }
    }
    state.validate_effective_state(&upserts, &removals)?;
    Ok(())
}

/// Snapshot-handle bookkeeping shared by the strategies.
#[derive(Clone, Copy, Debug, Default)]
struct HandleState {
    held: u64,
    generation: u64,
    held_generation: Option<u64>,
    retained_bytes: u64,
}

impl HandleState {
    fn hold(&mut self) {
        self.held += 1;
        self.held_generation = Some(self.generation);
    }

    fn release(&mut self, counters: &mut ExperimentCounters) {
        if let Some(generation) = self.held_generation
            && generation < self.generation
        {
            counters.stale_cancels += 1;
        }
        self.held = self.held.saturating_sub(1);
        if self.held == 0 {
            self.held_generation = None;
            self.retained_bytes = 0;
        }
    }

    fn retained(&self) -> u64 {
        if self.held == 0 {
            0
        } else {
            self.retained_bytes
        }
    }
}

/// Baseline: one `Arc<ProjectionState>` with `Arc::make_mut` publication.
pub(crate) struct Baseline {
    state: Arc<ProjectionState>,
    counters: ExperimentCounters,
    handles: HandleState,
}

impl Baseline {
    fn new(initial: ProjectionState) -> Self {
        let state = Arc::new(initial);
        let mut counters = ExperimentCounters::default();
        counters.bytes_held = state_bytes(&state);
        counters.peak_live_bytes = counters.bytes_held;
        Self {
            state,
            counters,
            handles: HandleState::default(),
        }
    }

    fn apply(&mut self, mutations: &[ProjectionMutation]) {
        let before = state_bytes(&self.state);
        let changed: u64 = mutations
            .iter()
            .map(|mutation| match mutation {
                ProjectionMutation::Upsert { projection, .. } => projection_bytes(projection),
                ProjectionMutation::Remove { .. } => 0,
            })
            .sum();
        if self.handles.held > 0 {
            self.counters.cow_clones += 1;
            self.counters.bytes_copied += before;
            self.counters.allocations += state_entries(&self.state);
            self.handles.retained_bytes = before;
        }
        self.counters.bytes_copied += changed;
        self.counters.allocations += mutations.len() as u64;
        let state = Arc::make_mut(&mut self.state);
        state.apply(mutations);
        self.counters.operations_applied += 1;
        self.handles.generation += 1;
        self.counters.bytes_held = state_bytes(&self.state);
        self.counters.peak_live_bytes = self
            .counters
            .peak_live_bytes
            .max(self.counters.bytes_held + self.handles.retained());
    }

    fn observe(&mut self) -> ObservationValues {
        self.counters.observations += 1;
        let values = observe_state(&self.state);
        self.counters.derivations += values.derivations.len() as u64;
        // The baseline derives by scanning every candidate and iteration per task.
        self.counters.collection_traversals += values.derivations.len() as u64
            * (values.counts.candidates + values.counts.iterations) as u64;
        self.counters.collection_traversals += values.counts.candidates as u64
            + values.counts.iterations as u64
            + values.counts.outcomes as u64
            + values.counts.capabilities as u64;
        values
    }

    fn counters(&self) -> ExperimentCounters {
        self.counters
    }
}

/// Projection family held behind its own `Arc` map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Tasks = 0,
    Candidates = 1,
    Iterations = 2,
    Outcomes = 3,
    Capabilities = 4,
    Revisions = 5,
}

const FAMILY_COUNT: usize = 6;

#[derive(Clone, Debug, Default)]
struct FamilyMaps {
    epoch: Option<EpochId>,
    tasks: Arc<BTreeMap<TaskId, TaskSnapshot>>,
    candidates: Arc<BTreeMap<CandidateRef, CandidateSnapshot>>,
    iterations: Arc<BTreeMap<IterationId, IterationSnapshot>>,
    outcomes: Arc<BTreeMap<OutcomeId, OutcomeSnapshot>>,
    capabilities: Arc<BTreeMap<CapabilitiesId, CreativeCapabilities>>,
    revisions: Arc<BTreeMap<ProjectionKey, Counter>>,
}

impl FamilyMaps {
    fn from_state(state: &ProjectionState) -> Self {
        Self {
            epoch: state.epoch.clone(),
            tasks: Arc::new(state.tasks.clone()),
            candidates: Arc::new(state.candidates.clone()),
            iterations: Arc::new(state.iterations.clone()),
            outcomes: Arc::new(state.outcomes.clone()),
            capabilities: Arc::new(state.capabilities.clone()),
            revisions: Arc::new(state.revisions.clone()),
        }
    }

    fn totals(&self) -> [u64; FAMILY_COUNT] {
        [
            self.tasks.values().map(task_bytes).sum(),
            self.candidates.values().map(candidate_bytes).sum(),
            self.iterations.values().map(iteration_bytes).sum(),
            self.outcomes.values().map(outcome_bytes).sum(),
            self.capabilities.values().map(capabilities_bytes).sum(),
            self.revisions.len() as u64 * revision_bytes(),
        ]
    }

    fn entries(&self, family: Family) -> u64 {
        match family {
            Family::Tasks => self.tasks.len() as u64,
            Family::Candidates => self.candidates.len() as u64,
            Family::Iterations => self.iterations.len() as u64,
            Family::Outcomes => self.outcomes.len() as u64,
            Family::Capabilities => self.capabilities.len() as u64,
            Family::Revisions => self.revisions.len() as u64,
        }
    }

    fn upsert(&mut self, projection: &Projection) {
        let key = ProjectionKey::of_projection(projection);
        let revision = projection.revision().clone();
        match projection {
            Projection::Task(snapshot) => {
                Arc::make_mut(&mut self.tasks).insert(snapshot.task_ref.clone(), snapshot.clone());
            }
            Projection::Candidate(snapshot) => {
                Arc::make_mut(&mut self.candidates)
                    .insert(snapshot.candidate_ref.clone(), snapshot.clone());
            }
            Projection::Iteration(snapshot) => {
                Arc::make_mut(&mut self.iterations)
                    .insert(snapshot.iteration_ref.clone(), snapshot.clone());
            }
            Projection::Outcome(snapshot) => {
                Arc::make_mut(&mut self.outcomes)
                    .insert(snapshot.outcome_ref.clone(), snapshot.clone());
            }
            Projection::Capabilities(snapshot) => {
                Arc::make_mut(&mut self.capabilities)
                    .insert(snapshot.capabilities_ref.clone(), snapshot.clone());
            }
            Projection::Artifact(_) | Projection::Evaluation(_) | Projection::Graph(_) => {}
        }
        Arc::make_mut(&mut self.revisions).insert(key, revision);
    }

    fn remove(&mut self, key: &ProjectionKey) {
        match key {
            ProjectionKey::Task(task_id) => {
                Arc::make_mut(&mut self.tasks).remove(task_id);
            }
            ProjectionKey::Candidate(reference) => {
                Arc::make_mut(&mut self.candidates).remove(reference);
            }
            ProjectionKey::Iteration(iteration_id) => {
                Arc::make_mut(&mut self.iterations).remove(iteration_id);
            }
            ProjectionKey::Outcome(outcome_id) => {
                Arc::make_mut(&mut self.outcomes).remove(outcome_id);
            }
            ProjectionKey::Capabilities(capabilities_id) => {
                Arc::make_mut(&mut self.capabilities).remove(capabilities_id);
            }
            ProjectionKey::Artifact(_) | ProjectionKey::Evaluation(_) | ProjectionKey::Graph(_) => {
            }
        }
        Arc::make_mut(&mut self.revisions).remove(key);
    }
}

fn family_of_key(key: &ProjectionKey) -> Family {
    match key {
        ProjectionKey::Task(_) => Family::Tasks,
        ProjectionKey::Candidate(_) => Family::Candidates,
        ProjectionKey::Iteration(_) => Family::Iterations,
        ProjectionKey::Outcome(_) => Family::Outcomes,
        ProjectionKey::Capabilities(_) => Family::Capabilities,
        ProjectionKey::Artifact(_) | ProjectionKey::Evaluation(_) | ProjectionKey::Graph(_) => {
            Family::Revisions
        }
    }
}

fn owner_of_projection(projection: &Projection) -> Option<TaskId> {
    match projection {
        Projection::Task(snapshot) => Some(snapshot.task_ref.clone()),
        Projection::Candidate(snapshot) => Some(snapshot.task_ref.clone()),
        Projection::Iteration(snapshot) => Some(snapshot.task_ref.clone()),
        Projection::Outcome(snapshot) => Some(snapshot.task_ref.clone()),
        Projection::Capabilities(snapshot) => match &snapshot.scope {
            desktop_protocol::CapabilityScope::Task { task_id } => Some(task_id.clone()),
            _ => None,
        },
        Projection::Artifact(_) | Projection::Evaluation(_) | Projection::Graph(_) => None,
    }
}

/// Task that owns a removal target, resolved before the entry disappears.
fn owner_of_target(maps: &FamilyMaps, target: &RemoveTarget) -> Option<TaskId> {
    match target {
        // The stored snapshot is the same source the derivation and the secondary
        // index read; product validation keeps `candidate_ref.task_id == task_ref`
        // for every retained candidate, so the key and the snapshot agree, while a
        // mismatched-but-valid entry could never invalidate the wrong task here.
        RemoveTarget::Task { task_id } => maps.tasks.get(task_id).map(|task| task.task_ref.clone()),
        RemoveTarget::Candidate { candidate_ref } => maps
            .candidates
            .get(candidate_ref)
            .map(|candidate| candidate.task_ref.clone()),
        RemoveTarget::Iteration { iteration_id } => maps
            .iterations
            .get(iteration_id)
            .map(|iteration| iteration.task_ref.clone()),
        RemoveTarget::Outcome { outcome_id } => maps
            .outcomes
            .get(outcome_id)
            .map(|outcome| outcome.task_ref.clone()),
        RemoveTarget::Capabilities { capabilities_id } => maps
            .capabilities
            .get(capabilities_id)
            .and_then(|capabilities| match &capabilities.scope {
                desktop_protocol::CapabilityScope::Task { task_id } => Some(task_id.clone()),
                _ => None,
            }),
        RemoveTarget::Artifact { .. }
        | RemoveTarget::Evaluation { .. }
        | RemoveTarget::Graph { .. } => None,
    }
}

fn build_candidate_index(state: &ProjectionState) -> BTreeMap<TaskId, BTreeSet<CandidateRef>> {
    let mut index: BTreeMap<TaskId, BTreeSet<CandidateRef>> = BTreeMap::new();
    for candidate in state.candidates.values() {
        index
            .entry(candidate.task_ref.clone())
            .or_default()
            .insert(candidate.candidate_ref.clone());
    }
    index
}

fn build_iteration_index(state: &ProjectionState) -> BTreeMap<TaskId, BTreeSet<IterationId>> {
    let mut index: BTreeMap<TaskId, BTreeSet<IterationId>> = BTreeMap::new();
    for iteration in state.iterations.values() {
        index
            .entry(iteration.task_ref.clone())
            .or_default()
            .insert(iteration.iteration_ref.clone());
    }
    index
}

/// Fixed logical bytes of one secondary-index reference.
const INDEX_ENTRY_BYTES: u64 = 80;
/// Fixed logical bytes of one cached derivation.
pub(crate) const CACHE_ENTRY_BYTES: u64 = 120;

#[derive(Clone, Debug, Default)]
enum Extra {
    #[default]
    None,
    Index {
        candidates_by_task: Arc<BTreeMap<TaskId, BTreeSet<CandidateRef>>>,
        iterations_by_task: Arc<BTreeMap<TaskId, BTreeSet<IterationId>>>,
    },
    Cache {
        entries: Arc<BTreeMap<TaskId, TaskDerivation>>,
    },
}

/// Family-scoped representation with an optional index or cache layer.
pub(crate) struct FamilyStore {
    kind: RepresentationStrategy,
    maps: FamilyMaps,
    totals: [u64; FAMILY_COUNT],
    extra: Extra,
    extra_bytes: u64,
    counters: ExperimentCounters,
    handles: HandleState,
    retained_extra: bool,
}

impl FamilyStore {
    fn new(kind: RepresentationStrategy, initial: &ProjectionState) -> Self {
        let maps = FamilyMaps::from_state(initial);
        let totals = maps.totals();
        let (extra, extra_bytes) = match kind {
            RepresentationStrategy::EntityIndex => {
                let candidates_by_task = build_candidate_index(initial);
                let iterations_by_task = build_iteration_index(initial);
                let entries = candidates_by_task
                    .values()
                    .map(BTreeSet::len)
                    .sum::<usize>()
                    + iterations_by_task
                        .values()
                        .map(BTreeSet::len)
                        .sum::<usize>();
                (
                    Extra::Index {
                        candidates_by_task: Arc::new(candidates_by_task),
                        iterations_by_task: Arc::new(iterations_by_task),
                    },
                    entries as u64 * INDEX_ENTRY_BYTES,
                )
            }
            RepresentationStrategy::RevisionCache => (
                Extra::Cache {
                    entries: Arc::new(BTreeMap::new()),
                },
                0,
            ),
            _ => (Extra::None, 0),
        };
        let mut counters = ExperimentCounters::default();
        counters.bytes_held = totals.iter().sum::<u64>() + extra_bytes;
        counters.peak_live_bytes = counters.bytes_held;
        Self {
            kind,
            maps,
            totals,
            extra,
            extra_bytes,
            counters,
            handles: HandleState::default(),
            retained_extra: false,
        }
    }

    fn cow_family(&mut self, family: Family, touched: &mut [bool; FAMILY_COUNT]) {
        if self.handles.held == 0 || touched[family as usize] {
            return;
        }
        touched[family as usize] = true;
        self.counters.cow_clones += 1;
        self.counters.bytes_copied += self.totals[family as usize];
        self.counters.allocations += self.maps.entries(family);
        self.handles.retained_bytes += self.totals[family as usize];
    }

    fn cow_extra(&mut self) {
        if self.handles.held == 0 || self.retained_extra {
            return;
        }
        self.retained_extra = true;
        self.counters.cow_clones += 1;
        self.counters.bytes_copied += self.extra_bytes;
        self.counters.allocations += self.extra_entries();
        self.handles.retained_bytes += self.extra_bytes;
    }

    fn extra_entries(&self) -> u64 {
        match &self.extra {
            Extra::None => 0,
            Extra::Index {
                candidates_by_task,
                iterations_by_task,
            } => {
                (candidates_by_task
                    .values()
                    .map(BTreeSet::len)
                    .sum::<usize>()
                    + iterations_by_task
                        .values()
                        .map(BTreeSet::len)
                        .sum::<usize>()) as u64
            }
            Extra::Cache { entries } => entries.len() as u64,
        }
    }

    fn apply(&mut self, mutations: &[ProjectionMutation]) {
        let mut touched = [false; FAMILY_COUNT];
        let mut invalidated = 0_u64;
        let mut index_touched = false;
        for mutation in mutations {
            match mutation {
                ProjectionMutation::Upsert { projection, .. } => {
                    let family = family_of_projection(projection);
                    self.cow_family(Family::Revisions, &mut touched);
                    self.cow_family(family, &mut touched);
                    self.maps.upsert(projection);
                    index_touched |= matches!(family, Family::Candidates | Family::Iterations);
                    if let Some(owner) = owner_of_projection(projection) {
                        invalidated += self.invalidate_cache(&owner);
                    }
                }
                ProjectionMutation::Remove { target, .. } => {
                    let key = ProjectionKey::of_remove_target(target);
                    let family = family_of_key(&key);
                    // The owner must be resolved while the removed entry is still
                    // readable: a child removal invalidates that task's derivation.
                    let owner = owner_of_target(&self.maps, target);
                    self.cow_family(Family::Revisions, &mut touched);
                    if family != Family::Revisions {
                        self.cow_family(family, &mut touched);
                    }
                    self.maps.remove(&key);
                    index_touched |= matches!(family, Family::Candidates | Family::Iterations);
                    if let Some(owner) = owner {
                        invalidated += self.invalidate_cache(&owner);
                    }
                }
            }
        }
        self.counters.stale_cancels += invalidated;
        if index_touched && matches!(self.kind, RepresentationStrategy::EntityIndex) {
            self.rebuild_index();
        }
        self.counters.allocations += mutations.len() as u64;
        self.counters.bytes_copied += mutations
            .iter()
            .map(|mutation| match mutation {
                ProjectionMutation::Upsert { projection, .. } => projection_bytes(projection),
                ProjectionMutation::Remove { .. } => 0,
            })
            .sum::<u64>();
        self.counters.operations_applied += 1;
        self.handles.generation += 1;
        self.totals = self.maps.totals();
        self.refresh_sizes();
    }

    fn invalidate_cache(&mut self, task_id: &TaskId) -> u64 {
        if !matches!(self.extra, Extra::Cache { .. }) {
            return 0;
        }
        self.cow_extra();
        let Extra::Cache { entries } = &mut self.extra else {
            return 0;
        };
        let entries = Arc::make_mut(entries);
        let removed = entries.remove(task_id).is_some();
        self.extra_bytes = entries.len() as u64 * CACHE_ENTRY_BYTES;
        u64::from(removed)
    }

    fn rebuild_index(&mut self) {
        if !matches!(self.extra, Extra::Index { .. }) {
            return;
        }
        self.cow_extra();
        // The rebuild scans every indexed family member and allocates new entry
        // references; both are counted so the index cannot look cheaper than the
        // maintenance work it really performs.
        let scanned = (self.maps.candidates.len() + self.maps.iterations.len()) as u64;
        let candidates = build_candidate_index_from(&self.maps);
        let iterations = build_iteration_index_from(&self.maps);
        let entries = candidates.values().map(BTreeSet::len).sum::<usize>()
            + iterations.values().map(BTreeSet::len).sum::<usize>();
        self.counters.collection_traversals += scanned;
        self.counters.allocations += entries as u64;
        self.counters.bytes_copied += entries as u64 * INDEX_ENTRY_BYTES;
        let Extra::Index {
            candidates_by_task,
            iterations_by_task,
        } = &mut self.extra
        else {
            return;
        };
        Arc::make_mut(candidates_by_task).clone_from(&candidates);
        Arc::make_mut(iterations_by_task).clone_from(&iterations);
        self.extra_bytes = entries as u64 * INDEX_ENTRY_BYTES;
        self.refresh_sizes();
    }

    fn observe(&mut self) -> ObservationValues {
        self.counters.observations += 1;
        let counts = FamilyCounts {
            tasks: self.maps.tasks.len(),
            candidates: self.maps.candidates.len(),
            iterations: self.maps.iterations.len(),
            outcomes: self.maps.outcomes.len(),
            capabilities: self.maps.capabilities.len(),
        };
        let mut derivations = Vec::with_capacity(counts.tasks);
        // Built lazily: the index strategy never needs a family pass, and the
        // cache only needs one when it actually misses; whichever model charges a
        // scan is also the one that builds the map (the family model charges one
        // scan per task, the cache one per observation with misses).
        let mut per_task: Option<BTreeMap<TaskId, (usize, usize)>> = None;
        let tasks: Vec<(TaskId, bool)> = self
            .maps
            .tasks
            .values()
            .map(|task| (task.task_ref.clone(), task.formal_outcome_ref.is_some()))
            .collect();
        match self.kind {
            RepresentationStrategy::EntityIndex => {
                let Extra::Index {
                    candidates_by_task,
                    iterations_by_task,
                } = &self.extra
                else {
                    return self.observe_fallback(counts, tasks);
                };
                for (task_id, formal_outcome) in tasks {
                    let candidates = candidates_by_task.get(&task_id).map_or(0, BTreeSet::len);
                    let iterations = iterations_by_task.get(&task_id).map_or(0, BTreeSet::len);
                    self.counters.derivations += 1;
                    self.counters.collection_traversals += (candidates + iterations) as u64;
                    derivations.push(TaskDerivation {
                        task_id,
                        candidates,
                        iterations,
                        formal_outcome,
                    });
                }
            }
            RepresentationStrategy::RevisionCache => {
                let mut inserted = Vec::new();
                let mut counted_scan = false;
                for (task_id, formal_outcome) in tasks {
                    let cached = match &self.extra {
                        Extra::Cache { entries } => entries.get(&task_id).cloned(),
                        _ => None,
                    };
                    let derivation = match cached {
                        Some(derivation) => derivation,
                        None => {
                            let scanned =
                                (self.maps.candidates.len() + self.maps.iterations.len()) as u64;
                            let counts = per_task.get_or_insert_with(|| {
                                per_task_counts(&self.maps.candidates, &self.maps.iterations)
                            });
                            let (candidates, iterations) =
                                counts.get(&task_id).copied().unwrap_or((0, 0));
                            self.counters.derivations += 1;
                            // Every miss of one observation is derived from the
                            // same pass over the indexed families: count one scan.
                            if !counted_scan {
                                self.counters.collection_traversals += scanned;
                                counted_scan = true;
                            }
                            let derivation = TaskDerivation {
                                task_id: task_id.clone(),
                                candidates,
                                iterations,
                                formal_outcome,
                            };
                            inserted.push((task_id, derivation.clone()));
                            derivation
                        }
                    };
                    derivations.push(derivation);
                }
                if !inserted.is_empty() {
                    self.cow_extra();
                    if let Extra::Cache { entries } = &mut self.extra {
                        let entries = Arc::make_mut(entries);
                        for (task_id, derivation) in inserted {
                            entries.insert(task_id, derivation);
                        }
                        self.extra_bytes = entries.len() as u64 * CACHE_ENTRY_BYTES;
                    }
                }
            }
            _ => {
                let scanned = (self.maps.candidates.len() + self.maps.iterations.len()) as u64;
                let counts = per_task.get_or_insert_with(|| {
                    per_task_counts(&self.maps.candidates, &self.maps.iterations)
                });
                for (task_id, formal_outcome) in tasks {
                    let (candidates, iterations) = counts.get(&task_id).copied().unwrap_or((0, 0));
                    self.counters.derivations += 1;
                    self.counters.collection_traversals += scanned;
                    derivations.push(TaskDerivation {
                        task_id,
                        candidates,
                        iterations,
                        formal_outcome,
                    });
                }
            }
        }
        self.finish_observation(counts, derivations)
    }

    fn observe_fallback(
        &mut self,
        counts: FamilyCounts,
        tasks: Vec<(TaskId, bool)>,
    ) -> ObservationValues {
        let mut derivations = Vec::with_capacity(tasks.len());
        let scanned = (self.maps.candidates.len() + self.maps.iterations.len()) as u64;
        for (task_id, formal_outcome) in tasks {
            let candidates = self
                .maps
                .candidates
                .values()
                .filter(|candidate| candidate.task_ref == task_id)
                .count();
            let iterations = self
                .maps
                .iterations
                .values()
                .filter(|iteration| iteration.task_ref == task_id)
                .count();
            self.counters.derivations += 1;
            self.counters.collection_traversals += scanned;
            derivations.push(TaskDerivation {
                task_id,
                candidates,
                iterations,
                formal_outcome,
            });
        }
        self.finish_observation(counts, derivations)
    }

    fn finish_observation(
        &mut self,
        counts: FamilyCounts,
        derivations: Vec<TaskDerivation>,
    ) -> ObservationValues {
        self.counters.collection_traversals += counts.candidates as u64
            + counts.iterations as u64
            + counts.outcomes as u64
            + counts.capabilities as u64;
        self.refresh_sizes();
        ObservationValues {
            epoch: self.maps.epoch.clone(),
            entries: observed_entries(&self.maps.revisions),
            counts,
            derivations,
        }
    }

    /// Refresh `bytes_held`/`peak_live_bytes` after any size change (cache
    /// insertion or invalidation, index rebuild), so the accounting never goes stale.
    fn refresh_sizes(&mut self) {
        self.counters.bytes_held = self.totals.iter().sum::<u64>() + self.extra_bytes;
        self.counters.peak_live_bytes = self
            .counters
            .peak_live_bytes
            .max(self.counters.bytes_held + self.handles.retained());
    }

    fn hold(&mut self) {
        self.handles.hold();
        self.retained_extra = false;
    }

    fn release(&mut self) {
        self.handles.release(&mut self.counters);
        if self.handles.held == 0 {
            self.retained_extra = false;
        }
    }

    fn counters(&self) -> ExperimentCounters {
        self.counters
    }
}

fn family_of_projection(projection: &Projection) -> Family {
    match projection {
        Projection::Task(_) => Family::Tasks,
        Projection::Candidate(_) => Family::Candidates,
        Projection::Iteration(_) => Family::Iterations,
        Projection::Outcome(_) => Family::Outcomes,
        Projection::Capabilities(_) => Family::Capabilities,
        Projection::Artifact(_) | Projection::Evaluation(_) | Projection::Graph(_) => {
            Family::Revisions
        }
    }
}

fn build_candidate_index_from(maps: &FamilyMaps) -> BTreeMap<TaskId, BTreeSet<CandidateRef>> {
    let mut index: BTreeMap<TaskId, BTreeSet<CandidateRef>> = BTreeMap::new();
    for candidate in maps.candidates.values() {
        index
            .entry(candidate.task_ref.clone())
            .or_default()
            .insert(candidate.candidate_ref.clone());
    }
    index
}

fn build_iteration_index_from(maps: &FamilyMaps) -> BTreeMap<TaskId, BTreeSet<IterationId>> {
    let mut index: BTreeMap<TaskId, BTreeSet<IterationId>> = BTreeMap::new();
    for iteration in maps.iterations.values() {
        index
            .entry(iteration.task_ref.clone())
            .or_default()
            .insert(iteration.iteration_ref.clone());
    }
    index
}

/// Test-only structurally impossible strategy: retains every published version.
#[cfg(test)]
pub(crate) struct Unbounded {
    state: Arc<ProjectionState>,
    versions: Vec<Arc<ProjectionState>>,
    counters: ExperimentCounters,
}

#[cfg(test)]
impl Unbounded {
    fn new(initial: ProjectionState) -> Self {
        let state = Arc::new(initial);
        let mut counters = ExperimentCounters::default();
        counters.bytes_held = state_bytes(&state);
        counters.peak_live_bytes = counters.bytes_held;
        Self {
            state,
            versions: Vec::new(),
            counters,
        }
    }

    fn apply(&mut self, mutations: &[ProjectionMutation]) {
        self.versions.push(Arc::clone(&self.state));
        let state = Arc::make_mut(&mut self.state);
        state.apply(mutations);
        self.counters.operations_applied += 1;
        self.counters.bytes_held = state_bytes(&self.state);
        let retained = self.counters.bytes_held * self.versions.len() as u64;
        self.counters.bytes_copied += self.counters.bytes_held + retained;
        self.counters.allocations += state_entries(&self.state);
        self.counters.peak_live_bytes = self
            .counters
            .peak_live_bytes
            .max(self.counters.bytes_held + retained);
    }

    fn observe(&mut self) -> ObservationValues {
        self.counters.observations += 1;
        observe_state(&self.state)
    }

    fn counters(&self) -> ExperimentCounters {
        self.counters
    }
}

/// Strategy store used by the experiment.
pub(crate) enum Representation {
    Baseline(Baseline),
    Store(FamilyStore),
    #[cfg(test)]
    Unbounded(Unbounded),
}

impl Representation {
    pub(crate) fn new(strategy: RepresentationStrategy, initial: &ProjectionState) -> Self {
        match strategy {
            RepresentationStrategy::BaselineArcMakeMut => {
                Self::Baseline(Baseline::new(initial.clone()))
            }
            RepresentationStrategy::FamilyScopedArc
            | RepresentationStrategy::EntityIndex
            | RepresentationStrategy::RevisionCache => {
                Self::Store(FamilyStore::new(strategy, initial))
            }
            #[cfg(test)]
            RepresentationStrategy::UnboundedGrowth => {
                Self::Unbounded(Unbounded::new(initial.clone()))
            }
        }
    }

    pub(crate) fn apply(&mut self, mutations: &[ProjectionMutation]) {
        match self {
            Self::Baseline(baseline) => baseline.apply(mutations),
            Self::Store(store) => store.apply(mutations),
            #[cfg(test)]
            Self::Unbounded(unbounded) => unbounded.apply(mutations),
        }
    }

    pub(crate) fn observe(&mut self) -> ObservationValues {
        match self {
            Self::Baseline(baseline) => baseline.observe(),
            Self::Store(store) => store.observe(),
            #[cfg(test)]
            Self::Unbounded(unbounded) => unbounded.observe(),
        }
    }

    pub(crate) fn hold(&mut self) {
        match self {
            Self::Baseline(baseline) => baseline.handles.hold(),
            Self::Store(store) => store.hold(),
            #[cfg(test)]
            Self::Unbounded(_) => {}
        }
    }

    pub(crate) fn release(&mut self) {
        match self {
            Self::Baseline(baseline) => baseline.handles.release(&mut baseline.counters),
            Self::Store(store) => store.release(),
            #[cfg(test)]
            Self::Unbounded(_) => {}
        }
    }

    pub(crate) fn counters(&self) -> ExperimentCounters {
        match self {
            Self::Baseline(baseline) => baseline.counters(),
            Self::Store(store) => store.counters(),
            #[cfg(test)]
            Self::Unbounded(unbounded) => unbounded.counters(),
        }
    }
}
