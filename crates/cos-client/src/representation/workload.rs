//! Bounded deterministic synthetic workload for the MP-PUB4 representation experiment.
//!
//! Fixed seed, fixed sizes and a fixed transaction shape: entity updates, one
//! insertion pair (candidate + owning iteration), one removal, bursts, and both
//! held and promptly-dropped snapshot handles. Nothing here reads the clock,
//! the filesystem or the environment.

use std::collections::{BTreeMap, BTreeSet};

use desktop_protocol::{
    ArtifactId, CandidateId, CandidateRef, CandidateSnapshot, CapabilitiesId, CapabilityScope,
    Counter, CreativeCapabilities, EpochId, FormalStatus, IterationId, IterationSnapshot,
    OutcomeId, OutcomeSnapshot, PhaseLabel, Projection, ProjectionMutation, RemoveTarget, RunId,
    SupportState, TaskId, TaskSnapshot, TaskStatus,
};

use super::ExperimentError;
use crate::ProjectionKey;

/// Snapshot handle action taken after a transaction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HandlesAfter {
    /// No handle change.
    #[default]
    None,
    /// Retain a snapshot handle across subsequent transactions.
    Hold,
    /// Drop the retained snapshot handle.
    Release,
}

/// One transaction of the synthetic stream.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamTransaction {
    /// Mutations applied atomically.
    pub mutations: Vec<ProjectionMutation>,
    /// Snapshot-handle action after the transaction.
    pub handles_after: HandlesAfter,
}

/// Bounded deterministic workload configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkloadConfig {
    /// Deterministic generator seed.
    pub seed: u64,
    /// Target entity count (tasks plus their candidates/iterations/outcomes/capabilities).
    pub entities: usize,
    /// Number of delta transactions.
    pub transactions: usize,
    /// Maximum extra mutations in one burst transaction.
    pub burst: usize,
    /// Retain a snapshot handle every `hold_every` transactions (0 disables).
    pub hold_every: usize,
}

impl Default for WorkloadConfig {
    fn default() -> Self {
        Self {
            seed: 0x5eed_2026_0000_0001,
            entities: 96,
            transactions: 12,
            burst: 2,
            hold_every: 4,
        }
    }
}

impl WorkloadConfig {
    /// Configuration for one comparison size.
    pub const fn size(entities: usize, transactions: usize, seed: u64) -> Self {
        Self {
            seed,
            entities,
            transactions,
            burst: 2,
            hold_every: 4,
        }
    }

    /// Sizes covered by the fast documented summary.
    pub const SUMMARY_SIZES: [usize; 3] = [100, 1_000, 10_000];

    /// The largest DC-MP-PUB4 curve, reproducible through [`Self::extended`] but
    /// kept out of the fast documented summary so the workspace test budget stays
    /// bounded.
    pub const EXTENDED_SIZES: [usize; 1] = [100_000];

    /// Configurations used by the fast documented exploratory summary.
    pub fn summary() -> Vec<Self> {
        Self::SUMMARY_SIZES
            .iter()
            .enumerate()
            .map(|(index, entities)| {
                let transactions = if *entities >= 10_000 { 8 } else { 24 };
                Self::size(
                    *entities,
                    transactions,
                    0x5eed_2026_0000_0100 + index as u64,
                )
            })
            .collect()
    }

    /// Extended configuration that records the 100k curve on demand.
    pub fn extended() -> Vec<Self> {
        Self::EXTENDED_SIZES
            .iter()
            .map(|entities| Self::size(*entities, 8, 0x5eed_2026_0000_0200))
            .collect()
    }
}

/// Deterministic synthetic stream plus the snapshot-creation bundle it starts from.
#[derive(Clone, Debug, PartialEq)]
pub struct Workload {
    /// Runtime epoch of the synthetic stream.
    pub epoch: EpochId,
    /// Snapshot creations (one `Upsert` with no base revision per entity).
    pub initial: Vec<ProjectionMutation>,
    /// Delta transactions applied in order.
    pub transactions: Vec<StreamTransaction>,
    /// Actual entity count produced for the configured target.
    pub entity_count: usize,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    fn pick(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next() % bound as u64) as usize
        }
    }
}

fn counter(value: u64) -> Result<Counter, ExperimentError> {
    Counter::new(value.to_string()).map_err(|_| ExperimentError::Workload("invalid counter"))
}

fn phase() -> Result<PhaseLabel, ExperimentError> {
    PhaseLabel::new("REFINE").map_err(|_| ExperimentError::Workload("invalid phase label"))
}

fn upsert(base: Option<Counter>, projection: Projection) -> ProjectionMutation {
    ProjectionMutation::Upsert {
        base_revision: base,
        projection,
    }
}

fn task_id(index: usize) -> TaskId {
    TaskId::fixture(format!("r2/task/{index}"))
}

fn run_id(index: usize) -> RunId {
    RunId::fixture(format!("r2/run/{index}"))
}

fn iteration_id(task: usize, iteration: usize) -> IterationId {
    IterationId::fixture(format!("r2/iteration/{task}/{iteration}"))
}

fn candidate_id(task: usize, slot: usize) -> CandidateId {
    CandidateId::fixture(format!("r2/candidate/{task}/{slot}"))
}

fn outcome_id(task: usize) -> OutcomeId {
    OutcomeId::fixture(format!("r2/outcome/{task}"))
}

fn capabilities_id(task: usize) -> CapabilitiesId {
    CapabilitiesId::fixture(format!("r2/capabilities/{task}"))
}

fn candidate_ref(task: usize, slot: usize) -> CandidateRef {
    CandidateRef {
        task_id: task_id(task),
        run_id: run_id(task),
        iteration: (slot / 2) as u32,
        candidate_id: candidate_id(task, slot),
        artifact_ref: ArtifactId::fixture(format!("r2/artifact/{task}/{slot}")),
    }
}

struct TaskShape {
    task: TaskSnapshot,
    capabilities: CreativeCapabilities,
    iterations: Vec<IterationSnapshot>,
    candidates: Vec<CandidateSnapshot>,
    outcome: OutcomeSnapshot,
}

fn task_shape(task: usize, has_formal_outcome: bool) -> Result<TaskShape, ExperimentError> {
    let task_ref = task_id(task);
    let mut iterations = Vec::with_capacity(2);
    let mut candidates = Vec::with_capacity(4);
    for iteration in 0..2 {
        let mut refs = Vec::with_capacity(2);
        for slot in 0..2 {
            let slot = iteration * 2 + slot;
            let reference = candidate_ref(task, slot);
            candidates.push(CandidateSnapshot {
                candidate_ref: reference.clone(),
                task_ref: task_ref.clone(),
                iteration_ref: iteration_id(task, iteration),
                artifact_refs: Vec::new(),
                evaluation_refs: Vec::new(),
                display_label: format!("L{slot}"),
                projection_revision: counter(1)?,
            });
            refs.push(reference);
        }
        iterations.push(IterationSnapshot {
            iteration_ref: iteration_id(task, iteration),
            task_ref: task_ref.clone(),
            index: iteration as u32,
            candidate_refs: refs,
            refinement_refs: Vec::new(),
            evidence_refs: Vec::new(),
            projection_revision: counter(1)?,
        });
    }
    let outcome_ref = outcome_id(task);
    let outcome = OutcomeSnapshot {
        outcome_ref: outcome_ref.clone(),
        task_ref: task_ref.clone(),
        formal_status: FormalStatus::Completed,
        selected_refs: Vec::new(),
        stop_reason: None,
        provenance_refs: Vec::new(),
        evidence_refs: Vec::new(),
        projection_revision: counter(1)?,
    };
    let task_snapshot = TaskSnapshot {
        task_ref: task_ref.clone(),
        display_id: format!("R2-{task}"),
        title: format!("representation entity {task}"),
        task_status: TaskStatus::Active,
        inner_phase: phase()?,
        iteration_ref: Some(iteration_id(task, 0)),
        iteration_index: Some(0),
        best_candidate_ref: Some(candidate_ref(task, 0)),
        formal_outcome_ref: has_formal_outcome.then_some(outcome_ref),
        accepted_brief_ref: None,
        capabilities_ref: capabilities_id(task),
        projection_revision: counter(1)?,
    };
    let capabilities = CreativeCapabilities {
        capabilities_ref: capabilities_id(task),
        scope: CapabilityScope::Task { task_id: task_ref },
        commands: Vec::new(),
        media: Vec::new(),
        features: Vec::new(),
        constraints: Vec::new(),
        availability: SupportState::Supported,
        reason: None,
        projection_revision: counter(1)?,
    };
    Ok(TaskShape {
        task: task_snapshot,
        capabilities,
        iterations,
        candidates,
        outcome,
    })
}

/// Next revision of a key and the base revision a valid upsert must declare.
fn advance(
    revisions: &mut BTreeMap<ProjectionKey, Counter>,
    key: &ProjectionKey,
) -> Result<(Option<Counter>, Counter), ExperimentError> {
    let base = revisions.get(key).cloned();
    let current = base
        .as_ref()
        .map_or(0, |counter| counter.as_str().parse::<u64>().unwrap_or(0));
    let next = counter(current + 1)?;
    revisions.insert(key.clone(), next.clone());
    Ok((base, next))
}

/// Generate the deterministic synthetic stream.
pub(crate) fn generate(config: &WorkloadConfig) -> Result<Workload, ExperimentError> {
    if config.transactions == 0 || !(16..=1_000_000).contains(&config.entities) {
        return Err(ExperimentError::Workload("unsupported workload size"));
    }
    let tasks = (config.entities / 8).max(2);
    let mut initial = Vec::new();
    let mut shapes = Vec::with_capacity(tasks);
    let mut entity_count = 0_usize;
    for task in 0..tasks {
        let shape = task_shape(task, task % 2 == 0)?;
        entity_count += 9;
        initial.push(upsert(
            None,
            Projection::Capabilities(shape.capabilities.clone()),
        ));
        for iteration in &shape.iterations {
            initial.push(upsert(None, Projection::Iteration(iteration.clone())));
        }
        for candidate in &shape.candidates {
            initial.push(upsert(None, Projection::Candidate(candidate.clone())));
        }
        initial.push(upsert(None, Projection::Outcome(shape.outcome.clone())));
        initial.push(upsert(None, Projection::Task(shape.task.clone())));
        shapes.push(shape);
    }

    // Revisions of every created entity start at one. Every mutation below takes
    // its base revision from this map and touches distinct keys, so the product's
    // own validation cannot see a stale base, a double mutation or an
    // unresolvable reference.
    let mut revisions: BTreeMap<ProjectionKey, Counter> = BTreeMap::new();
    for mutation in &initial {
        if let ProjectionMutation::Upsert { projection, .. } = mutation {
            revisions.insert(ProjectionKey::of_projection(projection), counter(1)?);
        }
    }
    let mut removed_outcomes: BTreeSet<usize> = BTreeSet::new();
    let mut rng = Rng(config.seed);
    let mut transactions = Vec::with_capacity(config.transactions);
    let mut holding = false;
    for step in 0..config.transactions {
        let mut mutations: Vec<ProjectionMutation> = Vec::new();
        let mut touched: BTreeSet<ProjectionKey> = BTreeSet::new();
        let primary = rng.pick(tasks);
        match step % 5 {
            0 => {
                if let Some((base, next)) = push_task_update(
                    &mut mutations,
                    &mut touched,
                    &mut revisions,
                    &mut shapes,
                    primary,
                    step,
                )? {
                    debug_assert!(base.is_some());
                    debug_assert!(next.as_str() != "0");
                }
            }
            1 => {
                let slot = rng.pick(4);
                let reference = candidate_ref(primary, slot);
                let key = ProjectionKey::Candidate(reference.clone());
                if touched.insert(key.clone()) {
                    let (base, next) = advance(&mut revisions, &key)?;
                    let mut candidate = shapes[primary].candidates[slot].clone();
                    candidate.display_label = format!("L{slot}-v{step}");
                    candidate.projection_revision = next;
                    mutations.push(upsert(base, Projection::Candidate(candidate)));
                }
            }
            2 => {
                let iteration = rng.pick(2);
                let slot = 4 + rng.pick(2);
                let reference = candidate_ref(primary, slot);
                let candidate_key = ProjectionKey::Candidate(reference.clone());
                let iteration_key = ProjectionKey::Iteration(iteration_id(primary, iteration));
                if !revisions.contains_key(&candidate_key) && touched.insert(iteration_key.clone())
                {
                    // Insertion pair: the new candidate and the owning iteration in
                    // one atomic transaction, so no partial family is published.
                    let (iteration_base, iteration_next) = advance(&mut revisions, &iteration_key)?;
                    let mut snapshot = shapes[primary].iterations[iteration].clone();
                    if !snapshot.candidate_refs.contains(&reference) {
                        snapshot.candidate_refs.push(reference.clone());
                    }
                    snapshot.projection_revision = iteration_next;
                    mutations.push(upsert(iteration_base, Projection::Iteration(snapshot)));
                    let created = counter(1)?;
                    revisions.insert(candidate_key, created.clone());
                    mutations.push(upsert(
                        None,
                        Projection::Candidate(CandidateSnapshot {
                            candidate_ref: reference,
                            task_ref: task_id(primary),
                            iteration_ref: iteration_id(primary, iteration),
                            artifact_refs: Vec::new(),
                            evaluation_refs: Vec::new(),
                            display_label: format!("L{slot}-new{step}"),
                            projection_revision: created,
                        }),
                    ));
                } else if let Some((_, _)) = push_task_update(
                    &mut mutations,
                    &mut touched,
                    &mut revisions,
                    &mut shapes,
                    primary,
                    step + 101,
                )? {
                }
            }
            _ => {
                let victim =
                    (0..tasks).find(|task| task % 2 == 1 && !removed_outcomes.contains(task));
                match victim {
                    Some(victim) => {
                        let key = ProjectionKey::Outcome(outcome_id(victim));
                        if touched.insert(key.clone()) {
                            let base = revisions
                                .get(&key)
                                .cloned()
                                .ok_or(ExperimentError::Workload("missing outcome revision"))?;
                            mutations.push(ProjectionMutation::Remove {
                                base_revision: base,
                                target: RemoveTarget::Outcome {
                                    outcome_id: outcome_id(victim),
                                },
                            });
                            revisions.remove(&key);
                            removed_outcomes.insert(victim);
                        }
                    }
                    None => {
                        let _ = push_task_update(
                            &mut mutations,
                            &mut touched,
                            &mut revisions,
                            &mut shapes,
                            primary,
                            step,
                        )?;
                    }
                }
            }
        }
        // Burst: independent updates of other tasks in the same transaction.
        for extra in 1..=config.burst.min(3) {
            let task = (primary + extra) % tasks;
            if task == primary {
                continue;
            }
            let _ = push_task_update(
                &mut mutations,
                &mut touched,
                &mut revisions,
                &mut shapes,
                task,
                step + extra * 7,
            )?;
        }
        let handles_after = if config.hold_every == 0 {
            HandlesAfter::None
        } else if !holding && step > 0 && step % config.hold_every == 0 {
            holding = true;
            HandlesAfter::Hold
        } else if holding {
            holding = false;
            HandlesAfter::Release
        } else {
            HandlesAfter::None
        };
        transactions.push(StreamTransaction {
            mutations,
            handles_after,
        });
    }
    Ok(Workload {
        epoch: EpochId::fixture("r2/epoch/1"),
        initial,
        transactions,
        entity_count,
    })
}

fn push_task_update(
    mutations: &mut Vec<ProjectionMutation>,
    touched: &mut BTreeSet<ProjectionKey>,
    revisions: &mut BTreeMap<ProjectionKey, Counter>,
    shapes: &mut [TaskShape],
    task: usize,
    marker: usize,
) -> Result<Option<(Option<Counter>, Counter)>, ExperimentError> {
    let key = ProjectionKey::Task(task_id(task));
    if !touched.insert(key.clone()) {
        return Ok(None);
    }
    let (base, next) = advance(revisions, &key)?;
    let mut snapshot = shapes[task].task.clone();
    snapshot.title = format!("representation entity {task} marker {marker}");
    snapshot.projection_revision = next.clone();
    shapes[task].task = snapshot.clone();
    mutations.push(upsert(base.clone(), Projection::Task(snapshot)));
    Ok(Some((base, next)))
}
