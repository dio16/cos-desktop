//! Deterministic machine-readable summary of the representation experiment.
//!
//! The summary is exploratory. It records structural facts for the same bounded
//! stream per representation and never states a gate, an SLO or an adoption
//! decision; identical inputs produce byte-identical output.

use serde_json::{json, Value};

use super::{
    ExperimentCounters, RepresentationStrategy, SUMMARY_STATUS, WorkloadConfig, EXPERIMENT_ID,
    EXPERIMENT_VERSION,
};

/// Accounting method recorded in the summary.
pub(crate) const SUMMARY_ACCOUNTING: &str = "explicit logical-byte capacity counters (no allocator hook, no wall-clock measurement)";

/// Fixed logical-byte overhead allowed for every strategy.
const FIXED_OVERHEAD_BYTES: u64 = 64 * 1024;
/// Logical bytes allowed per entity (fields, maps and one retained version).
const PER_ENTITY_BYTES: u64 = 1024;
/// Additional logical bytes per entity allowed for a secondary index.
const INDEX_PER_ENTITY_BYTES: u64 = 96;
/// Additional logical bytes per entity allowed for a revision-keyed cache.
const CACHE_PER_ENTITY_BYTES: u64 = 128;

/// Declared logical-byte bound for one strategy and entity count.
///
/// The bound is a structural consequence of the representation, not a measured
/// threshold: it exists so that a structurally impossible strategy (unbounded
/// version retention) is rejected instead of silently polluting the comparison.
pub(crate) fn memory_bound(strategy: RepresentationStrategy, entities: usize) -> u64 {
    let per_entity = match strategy {
        RepresentationStrategy::EntityIndex => PER_ENTITY_BYTES + INDEX_PER_ENTITY_BYTES,
        RepresentationStrategy::RevisionCache => PER_ENTITY_BYTES + CACHE_PER_ENTITY_BYTES,
        #[cfg(test)]
        RepresentationStrategy::UnboundedGrowth => PER_ENTITY_BYTES,
        _ => PER_ENTITY_BYTES,
    };
    FIXED_OVERHEAD_BYTES + entities as u64 * per_entity
}

/// Facts for one strategy at one size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrategyFacts {
    /// Compared strategy.
    pub strategy: RepresentationStrategy,
    /// Recorded counters.
    pub counters: ExperimentCounters,
    /// Declared logical-byte bound.
    pub bound_bytes: u64,
    /// Whether the peak stayed within the bound.
    pub within_bound: bool,
}

/// Facts for every strategy at one synthetic size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SizeFacts {
    /// Entities in the synthetic stream.
    pub entities: usize,
    /// Transactions in the synthetic stream.
    pub transactions: usize,
    /// Per-strategy facts in comparison order.
    pub strategies: Vec<StrategyFacts>,
}

/// Complete exploratory report: facts plus the deterministic JSON summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExperimentReport {
    sizes: Vec<SizeFacts>,
    summary: Value,
    json: String,
}

impl ExperimentReport {
    pub(crate) fn new(configs: &[WorkloadConfig], sizes: Vec<SizeFacts>) -> Self {
        let strategies: Vec<&str> = RepresentationStrategy::COMPARED
            .iter()
            .map(|strategy| strategy.id())
            .collect();
        let workloads: Vec<Value> = sizes
            .iter()
            .enumerate()
            .map(|(index, size)| {
                let config = configs.get(index);
                json!({
                    "entities": size.entities,
                    "transactions": size.transactions,
                    "seed": config.map(|config| config.seed),
                    "burst": config.map(|config| config.burst),
                    "hold_every": config.map(|config| config.hold_every),
                })
            })
            .collect();
        let size_values: Vec<Value> = sizes
            .iter()
            .map(|size| {
                let strategies: Vec<Value> = size
                    .strategies
                    .iter()
                    .map(|facts| {
                        let counters = facts.counters;
                        json!({
                            "strategy": facts.strategy.id(),
                            "allocations": counters.allocations,
                            "bytes_copied": counters.bytes_copied,
                            "bytes_held": counters.bytes_held,
                            "peak_live_bytes": counters.peak_live_bytes,
                            "cow_clones": counters.cow_clones,
                            "stale_cancels": counters.stale_cancels,
                            "collection_traversals": counters.collection_traversals,
                            "derivations": counters.derivations,
                            "operations_applied": counters.operations_applied,
                            "observations": counters.observations,
                            "bound_bytes": facts.bound_bytes,
                            "within_bound": facts.within_bound,
                        })
                    })
                    .collect();
                json!({
                    "entities": size.entities,
                    "transactions": size.transactions,
                    "strategies": strategies,
                })
            })
            .collect();
        let summary = json!({
            "experiment": EXPERIMENT_ID,
            "version": EXPERIMENT_VERSION,
            "status": SUMMARY_STATUS,
            "accounting": SUMMARY_ACCOUNTING,
            "strategies": strategies,
            "workloads": workloads,
            "sizes": size_values,
            "notes": [
                "Exploratory representation facts only; no numeric gate, SLO or adoption claim.",
                "Representation selection remains a separate owner decision (DD-02b after EXPERIMENT_COMPLETED).",
                "Wall-clock latency is intentionally not measured; counters are structural.",
            ],
        });
        let json = serde_json::to_string_pretty(&summary).unwrap_or_default();
        Self {
            sizes,
            summary,
            json,
        }
    }

    /// Per-size facts in comparison order.
    pub fn sizes(&self) -> &[SizeFacts] {
        &self.sizes
    }

    /// Deterministic JSON summary.
    pub fn summary_json(&self) -> &str {
        &self.json
    }

    /// Structured summary value.
    pub fn summary(&self) -> &Value {
        &self.summary
    }
}
