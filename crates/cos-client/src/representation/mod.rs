//! MP-PUB4 R2 representation experiment (COW / index / cache / sharing).
//!
//! This is a development-only exploratory harness, not a product API. No product
//! module consults the switch, so the experiment cannot change observable
//! behaviour: with [`RepresentationSwitch::default`] (`disabled`) the harness
//! reads exactly the values the current `Arc<ProjectionState>` +
//! `Arc::make_mut` representation produces.
//!
//! The harness replays one identical, already-valid transaction stream into each
//! candidate representation and records structural facts only: explicit
//! logical-byte capacity counters (no allocator hook), operation counts,
//! retained-handle copy-on-write events and collection traversals. It measures no
//! wall-clock time, makes no SLO claim and selects no design; the emitted summary
//! is [`SUMMARY_STATUS`] and must never be read as a numeric gate. Adoption
//! remains a separate owner decision (`DD-02b`).

use desktop_protocol::{EpochId, TaskId};

use crate::ProjectionKey;

#[cfg(test)]
mod tests;
mod store;
mod summary;
mod workload;

pub use summary::{ExperimentReport, SizeFacts, StrategyFacts};
pub use workload::{HandlesAfter, StreamTransaction, Workload, WorkloadConfig};

/// Stable experiment identity recorded in the summary.
pub const EXPERIMENT_ID: &str = "mp-pub4-r2-representation";
/// Version of the experiment contract and summary schema.
pub const EXPERIMENT_VERSION: u32 = 1;
/// The summary is exploratory; it is never a numeric gate or an adoption claim.
pub const SUMMARY_STATUS: &str = "EXPLORATORY_NO_NUMERIC_GATE";

/// Candidate representation strategies compared by the experiment.
///
/// `BaselineArcMakeMut` is the representation the product uses today; the other
/// variants are experiment-only candidates from DC-MP-PUB4 (b), (c) and (d).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RepresentationStrategy {
    /// Current product baseline: `Arc<ProjectionState>` + `Arc::make_mut`.
    #[default]
    BaselineArcMakeMut,
    /// Family-scoped immutable ownership: one `Arc` map per projection family.
    FamilyScopedArc,
    /// Family-scoped maps plus task-owned candidate/iteration secondary indexes.
    EntityIndex,
    /// Family-scoped maps plus revision-keyed derived display caches.
    RevisionCache,
    /// Test-only structurally impossible strategy: retains every published
    /// version forever, so the declared memory bound must fail.
    #[cfg(test)]
    UnboundedGrowth,
}

impl RepresentationStrategy {
    /// Strategies compared by the experiment, in summary order.
    pub const COMPARED: [Self; 4] = [
        Self::BaselineArcMakeMut,
        Self::FamilyScopedArc,
        Self::EntityIndex,
        Self::RevisionCache,
    ];

    /// Stable identifier used by the machine-readable summary.
    pub fn id(self) -> &'static str {
        match self {
            Self::BaselineArcMakeMut => "baseline-arc-make-mut",
            Self::FamilyScopedArc => "family-scoped-arc",
            Self::EntityIndex => "entity-index",
            Self::RevisionCache => "revision-cache",
            #[cfg(test)]
            Self::UnboundedGrowth => "unbounded-growth",
        }
    }
}

/// Explicit experiment switch.
///
/// Constructing the default disables the experiment; an enabled switch names one
/// candidate representation. The switch is only ever passed to this module by
/// experiment code and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RepresentationSwitch {
    enabled: bool,
    strategy: RepresentationStrategy,
}

impl RepresentationSwitch {
    /// Disabled switch: the baseline representation is observed.
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            strategy: RepresentationStrategy::BaselineArcMakeMut,
        }
    }

    /// Enabled switch naming one candidate representation.
    pub const fn enabled(strategy: RepresentationStrategy) -> Self {
        Self {
            enabled: true,
            strategy,
        }
    }

    /// Whether the experiment is enabled.
    pub const fn is_enabled(self) -> bool {
        self.enabled
    }

    /// Strategy named by the switch; ignored while disabled.
    pub const fn strategy(self) -> RepresentationStrategy {
        self.strategy
    }

    /// Test helper: a disabled switch that still names a strategy, proving that
    /// the strategy is ignored while the experiment is off.
    #[cfg(test)]
    pub(crate) const fn disabled_naming(strategy: RepresentationStrategy) -> Self {
        Self {
            enabled: false,
            strategy,
        }
    }
}

/// Explicit logical-byte capacity counters of one representation.
///
/// The accounting method is deterministic and structural: entities are counted
/// with their in-memory field sizes, copied map entries with the same rule, and
/// no allocator is instrumented.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExperimentCounters {
    /// Allocation units: copied map entries plus inserted/updated/removed entries.
    pub allocations: u64,
    /// Logical bytes duplicated by copy-on-write clones and value clones.
    pub bytes_copied: u64,
    /// Logical bytes held by the current representation version.
    pub bytes_held: u64,
    /// High-water mark of `bytes_held` plus bytes retained by held handles.
    pub peak_live_bytes: u64,
    /// Copy-on-write clones forced by a retained snapshot handle.
    pub cow_clones: u64,
    /// Stale snapshot handles discarded after the state advanced.
    pub stale_cancels: u64,
    /// Entries visited while deriving observations.
    pub collection_traversals: u64,
    /// Task derivations actually computed (cache misses for the cache strategy).
    pub derivations: u64,
    /// Transactions applied.
    pub operations_applied: u64,
    /// Observations produced.
    pub observations: u64,
}

/// One observed projection revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedEntry {
    /// Projection key.
    pub key: ProjectionKey,
    /// Authoritative revision of the projection.
    pub revision: desktop_protocol::Counter,
}

/// Entity presence per observed family.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FamilyCounts {
    /// Tasks present.
    pub tasks: usize,
    /// Candidates present.
    pub candidates: usize,
    /// Iterations present.
    pub iterations: usize,
    /// Outcomes present.
    pub outcomes: usize,
    /// Capabilities present.
    pub capabilities: usize,
}

/// Derived per-task observation facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskDerivation {
    /// Task the derivation belongs to.
    pub task_id: TaskId,
    /// Candidates of the task.
    pub candidates: usize,
    /// Iterations of the task.
    pub iterations: usize,
    /// Whether the task projects a formal outcome.
    pub formal_outcome: bool,
}

/// Observable values produced from one representation version.
///
/// Equality of these values is the equivalence criterion: representations may
/// differ internally, never in what the observation path reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObservationValues {
    /// Runtime epoch, when the stream started from a snapshot.
    pub epoch: Option<EpochId>,
    /// Revisions sorted by projection key.
    pub entries: Vec<ObservedEntry>,
    /// Entity presence per family.
    pub counts: FamilyCounts,
    /// Per-task derivations sorted by task identity.
    pub derivations: Vec<TaskDerivation>,
}

/// Failure of the experiment harness; never a product error.
#[derive(Debug, thiserror::Error)]
pub enum ExperimentError {
    /// The synthetic stream was rejected by the product's own validation.
    #[error("projection integrity rejected the stream: {0}")]
    Integrity(#[from] crate::ProjectionIntegrityError),
    /// A representation diverged from the baseline observation.
    #[error("{strategy} diverged from the baseline at transaction {transaction}")]
    Divergence {
        /// Strategy identifier.
        strategy: &'static str,
        /// Zero-based transaction index.
        transaction: usize,
    },
    /// A representation exceeded its declared memory bound.
    #[error("{strategy} exceeded the {bound_bytes} byte bound with peak {peak_bytes} bytes")]
    MemoryBoundExceeded {
        /// Strategy identifier.
        strategy: &'static str,
        /// Declared bound in logical bytes.
        bound_bytes: u64,
        /// Observed peak in logical bytes.
        peak_bytes: u64,
        /// Transaction at which the bound was exceeded.
        transaction: usize,
    },
    /// The synthetic workload could not be produced.
    #[error("workload rejected: {0}")]
    Workload(&'static str),
}

/// Replay the same stream with the given switch and return the final observation.
///
/// A disabled switch always observes the baseline representation, whatever
/// strategy it names. An enabled switch observes exactly its strategy. The
/// stream is validated once with the product's own transaction validation.
pub fn observe_with_switch(
    switch: RepresentationSwitch,
    workload: &Workload,
) -> Result<ObservationValues, ExperimentError> {
    let expected = expected_observations(workload)?;
    let strategy = if switch.enabled {
        switch.strategy
    } else {
        RepresentationStrategy::BaselineArcMakeMut
    };
    let outcome = run_strategy_with_expected(workload, strategy, &expected)?;
    let final_expected = expected
        .last()
        .ok_or(ExperimentError::Workload("the stream produced no observation"))?;
    if outcome.observation != *final_expected {
        return Err(ExperimentError::Divergence {
            strategy: strategy.id(),
            transaction: workload.transactions.len(),
        });
    }
    Ok(outcome.observation)
}

/// Product-semantics replay of the synthetic stream.
///
/// Each transaction is validated with [`crate::ProjectionState::validate_mutation`]
/// and [`crate::ProjectionState::validate_effective_state`] and applied to an
/// `Arc<ProjectionState>`, exactly like the client's delta path. The returned
/// vector holds the observation after the snapshot and after every transaction.
pub(crate) fn expected_observations(
    workload: &Workload,
) -> Result<Vec<ObservationValues>, ExperimentError> {
    let mut state = crate::ProjectionState::from_snapshot(&workload.epoch, &workload.initial)?;
    let mut values = Vec::with_capacity(workload.transactions.len() + 1);
    values.push(store::observe_state(&state));
    for transaction in &workload.transactions {
        store::validate_transaction(&state, &transaction.mutations)?;
        state.apply(&transaction.mutations);
        values.push(store::observe_state(&state));
    }
    Ok(values)
}

/// Replay the stream into one strategy and return its facts.
#[cfg(test)]
pub(crate) fn run_strategy(
    workload: &Workload,
    strategy: RepresentationStrategy,
) -> Result<StrategyFact, ExperimentError> {
    let expected = expected_observations(workload)?;
    run_strategy_with_expected(workload, strategy, &expected)
}

/// Replay one strategy against precomputed baseline observations.
pub(crate) fn run_strategy_with_expected(
    workload: &Workload,
    strategy: RepresentationStrategy,
    expected: &[ObservationValues],
) -> Result<StrategyFact, ExperimentError> {
    let initial = crate::ProjectionState::from_snapshot(&workload.epoch, &workload.initial)?;
    let bound = summary::memory_bound(strategy, workload.entity_count);
    let mut representation = store::Representation::new(strategy, &initial);
    let mut last_observation = representation.observe();
    for (index, transaction) in workload.transactions.iter().enumerate() {
        representation.apply(&transaction.mutations);
        match transaction.handles_after {
            HandlesAfter::None => {}
            HandlesAfter::Hold => representation.hold(),
            HandlesAfter::Release => representation.release(),
        }
        last_observation = representation.observe();
        match expected.get(index + 1) {
            Some(values) if *values == last_observation => {}
            Some(_) => {
                return Err(ExperimentError::Divergence {
                    strategy: strategy.id(),
                    transaction: index,
                });
            }
            None => return Err(ExperimentError::Workload("baseline observation is missing")),
        }
        if representation.counters().peak_live_bytes > bound {
            return Err(ExperimentError::MemoryBoundExceeded {
                strategy: strategy.id(),
                bound_bytes: bound,
                peak_bytes: representation.counters().peak_live_bytes,
                transaction: index,
            });
        }
    }
    let counters = representation.counters();
    Ok(StrategyFact {
        strategy,
        counters,
        bound_bytes: bound,
        within_bound: counters.peak_live_bytes <= bound,
        observation: last_observation,
    })
}

/// Run the comparison over several bounded synthetic sizes.
pub fn run_experiment(configs: &[WorkloadConfig]) -> Result<ExperimentReport, ExperimentError> {
    let mut sizes = Vec::with_capacity(configs.len());
    for config in configs {
        let workload = workload::generate(config)?;
        let expected = expected_observations(&workload)?;
        let mut strategies = Vec::with_capacity(RepresentationStrategy::COMPARED.len());
        for strategy in RepresentationStrategy::COMPARED {
            let facts = run_strategy_with_expected(&workload, strategy, &expected)?;
            strategies.push(StrategyFacts {
                strategy: facts.strategy,
                counters: facts.counters,
                bound_bytes: facts.bound_bytes,
                within_bound: facts.within_bound,
            });
        }
        sizes.push(SizeFacts {
            entities: workload.entity_count,
            transactions: workload.transactions.len(),
            strategies,
        });
    }
    Ok(ExperimentReport::new(configs, sizes))
}

/// Observation values of a projection state; used by tests to prove the default
/// product path is what the disabled switch reads.
pub fn observe_projection_state(
    state: &crate::ProjectionState,
) -> Result<ObservationValues, ExperimentError> {
    Ok(store::observe_state(state))
}

/// Internally used strategy facts including the observed values.
pub(crate) struct StrategyFact {
    strategy: RepresentationStrategy,
    counters: ExperimentCounters,
    bound_bytes: u64,
    within_bound: bool,
    observation: ObservationValues,
}
