//! Regression tests for the MP-PUB4 representation experiment.
//!
//! These tests prove the experiment contract only: switch default, on/off
//! equivalence, determinism, the memory bound (including a structurally
//! impossible strategy), and that nothing leaks into default product behaviour.

use super::workload;
use super::{
    ExperimentError, HandlesAfter, RepresentationStrategy, RepresentationSwitch, WorkloadConfig,
    expected_observations, observe_projection_state, observe_with_switch, run_experiment,
    run_strategy, run_strategy_with_expected,
};

fn small() -> WorkloadConfig {
    WorkloadConfig {
        seed: 7,
        entities: 64,
        transactions: 8,
        burst: 1,
        hold_every: 3,
    }
}

fn generated(config: &WorkloadConfig) -> workload::Workload {
    workload::generate(config).expect("the bounded workload is generated")
}

#[test]
fn switch_defaults_to_disabled_baseline() {
    let switch = RepresentationSwitch::default();
    assert!(!switch.is_enabled());
    assert_eq!(
        switch.strategy(),
        RepresentationStrategy::BaselineArcMakeMut
    );
    assert!(!RepresentationSwitch::disabled().is_enabled());
    assert!(RepresentationSwitch::enabled(RepresentationStrategy::EntityIndex).is_enabled());
}

#[test]
fn on_and_off_switch_produce_equal_observation_values() {
    let workload = generated(&small());
    let disabled = observe_with_switch(RepresentationSwitch::disabled(), &workload)
        .expect("the disabled switch observes the product baseline");
    for strategy in RepresentationStrategy::COMPARED {
        let enabled = observe_with_switch(RepresentationSwitch::enabled(strategy), &workload)
            .expect("the candidate replay matches the product baseline");
        assert_eq!(
            enabled,
            disabled,
            "{} changed the observed values",
            strategy.id()
        );
    }
}

#[test]
fn summary_is_deterministic_for_identical_inputs() {
    let configs = [small()];
    let first = run_experiment(&configs).expect("the comparison runs");
    let second = run_experiment(&configs).expect("the comparison is repeatable");
    assert_eq!(first.summary_json(), second.summary_json());
    assert_eq!(first.sizes(), second.sizes());
    assert_eq!(
        first.summary()["status"],
        serde_json::json!("EXPLORATORY_NO_NUMERIC_GATE")
    );
    assert_eq!(
        first.summary()["strategies"],
        serde_json::json!([
            "baseline-arc-make-mut",
            "family-scoped-arc",
            "entity-index",
            "revision-cache",
        ])
    );
}

#[test]
fn every_compared_strategy_stays_within_its_memory_bound() {
    let report = run_experiment(&[small()]).expect("the comparison runs");
    for size in report.sizes() {
        for facts in &size.strategies {
            assert!(
                facts.within_bound,
                "{} exceeded {} bytes at {} entities",
                facts.strategy.id(),
                facts.bound_bytes,
                size.entities
            );
            assert!(facts.counters.peak_live_bytes <= facts.bound_bytes);
            assert!(facts.counters.operations_applied >= size.transactions as u64);
        }
    }
}

#[test]
fn structurally_unbounded_strategy_fails_the_memory_bound() {
    let workload = generated(&small());
    let expected = expected_observations(&workload).expect("the stream is valid");
    let outcome = run_strategy_with_expected(
        &workload,
        RepresentationStrategy::UnboundedGrowth,
        &expected,
    );
    assert!(
        matches!(outcome, Err(ExperimentError::MemoryBoundExceeded { .. })),
        "an unbounded version-retaining strategy must fail the declared bound"
    );
}

#[test]
fn experiment_cannot_leak_into_default_product_behaviour() {
    let workload = generated(&small());
    let product = observe_with_switch(RepresentationSwitch::disabled(), &workload)
        .expect("the disabled path observes the product store");
    // A disabled switch ignores whatever strategy it names.
    for strategy in RepresentationStrategy::COMPARED {
        let disabled =
            observe_with_switch(RepresentationSwitch::disabled_naming(strategy), &workload)
                .expect("the disabled switch ignores the named strategy");
        assert_eq!(disabled, product);
    }
    // The product store itself produces exactly the disabled observation.
    let state = crate::ProjectionState::from_snapshot(&workload.epoch, &workload.initial)
        .expect("the snapshot is valid");
    let observed = observe_projection_state(&state).expect("the product store is observed");
    assert_eq!(
        Some(observed),
        expected_observations(&workload)
            .expect("the stream is valid")
            .first()
            .map(|(_, values)| values.clone())
    );
}

#[test]
fn workload_is_deterministic_and_mixes_insert_update_and_remove() {
    let config = small();
    let first = generated(&config);
    let second = generated(&config);
    assert_eq!(first, second);
    assert!(first.transactions.len() == config.transactions);
    let mutations: usize = first
        .transactions
        .iter()
        .map(|transaction| transaction.mutations.len())
        .sum();
    assert!(mutations > first.transactions.len());
    assert!(
        first
            .transactions
            .iter()
            .any(|transaction| transaction.mutations.len() > 1),
        "the workload must contain a burst"
    );
    assert!(
        first
            .transactions
            .iter()
            .any(|transaction| transaction.handles_after == HandlesAfter::Hold),
        "the workload must retain a snapshot handle"
    );
    assert!(
        first
            .transactions
            .iter()
            .any(|transaction| transaction.handles_after == HandlesAfter::Release),
        "the workload must drop a snapshot handle"
    );
    let has_removal = first.transactions.iter().any(|transaction| {
        transaction.mutations.iter().any(|mutation| {
            matches!(
                mutation,
                desktop_protocol::ProjectionMutation::Remove { .. }
            )
        })
    });
    assert!(has_removal, "the workload must remove an entity");
    let baseline = run_strategy(&first, RepresentationStrategy::BaselineArcMakeMut)
        .expect("baseline replay over the mixed workload");
    assert!(
        baseline.counters.cow_clones > 0,
        "the retained-handle path must copy at least once"
    );
}

#[test]
fn held_handle_covers_the_insertion_path() {
    let workload = workload::held_insertion_workload().expect("the held-insertion stream is valid");
    assert_eq!(super::observation_stride(workload.entity_count), 1);
    let expected = expected_observations(&workload).expect("the insertion stream is valid");
    let first = &expected.first().expect("initial observation").1;
    let last = &expected.last().expect("final observation").1;
    assert_eq!(
        last.derivations[0].candidates,
        first.derivations[0].candidates + 1,
        "the insertion must be observed"
    );
    for strategy in RepresentationStrategy::COMPARED {
        let facts = run_strategy_with_expected(&workload, strategy, &expected)
            .unwrap_or_else(|error| panic!("{} diverged: {error}", strategy.id()));
        assert_eq!(facts.observation, *last);
    }
    let baseline = run_strategy(&workload, RepresentationStrategy::BaselineArcMakeMut)
        .expect("baseline replay over the held insertion");
    assert!(
        baseline.counters.cow_clones > 0,
        "the insertion must run behind a retained handle copy-on-write"
    );
}

#[test]
fn revision_cache_derives_fewer_tasks_than_the_baseline() {
    let workload = generated(&small());
    let baseline = run_strategy(&workload, RepresentationStrategy::BaselineArcMakeMut)
        .expect("baseline replay");
    let cache =
        run_strategy(&workload, RepresentationStrategy::RevisionCache).expect("cache replay");
    assert!(
        cache.counters.derivations < baseline.counters.derivations,
        "cache derivations {} must stay below baseline {}",
        cache.counters.derivations,
        baseline.counters.derivations
    );
    // One family scan per observation with misses, never one per missed task.
    assert!(
        cache.counters.collection_traversals * 2 < baseline.counters.collection_traversals,
        "cache traversals {} must stay far below baseline {}",
        cache.counters.collection_traversals,
        baseline.counters.collection_traversals
    );
    let family =
        run_strategy(&workload, RepresentationStrategy::FamilyScopedArc).expect("family replay");
    let tasks = expected_observations(&workload)
        .expect("the stream is valid")
        .first()
        .expect("the initial observation exists")
        .1
        .derivations
        .len() as u64;
    // The final observation refills every task, so bytes_held must include exactly
    // one cache entry per task on top of the family maps: this proves the cache
    // refreshed size accounting instead of merely being non-decreasing.
    assert_eq!(
        cache.counters.bytes_held,
        family.counters.bytes_held + tasks * super::store::CACHE_ENTRY_BYTES,
        "bytes_held must charge the {tasks} cached derivations"
    );
    assert!(cache.counters.bytes_held > baseline.counters.bytes_held);
    assert!(cache.counters.peak_live_bytes >= cache.counters.bytes_held);
    assert!(cache.counters.bytes_copied > 0);
}

#[test]
fn entity_index_traverses_fewer_entries_than_the_baseline() {
    let workload = generated(&small());
    let baseline = run_strategy(&workload, RepresentationStrategy::BaselineArcMakeMut)
        .expect("baseline replay");
    let index = run_strategy(&workload, RepresentationStrategy::EntityIndex).expect("index replay");
    assert!(
        index.counters.collection_traversals < baseline.counters.collection_traversals,
        "index traversals {} must stay below baseline {}",
        index.counters.collection_traversals,
        baseline.counters.collection_traversals
    );
}

#[test]
fn child_removals_are_observed_identically_by_every_strategy() {
    let workload = workload::child_removal_workload().expect("the child-removal stream is valid");
    assert_eq!(
        super::observation_stride(workload.entity_count),
        1,
        "the full-equality case must not be checkpointed"
    );
    let expected = expected_observations(&workload).expect("the product accepts the removals");
    assert_eq!(
        expected.len(),
        workload.transactions.len() + 1,
        "stride 1 must observe every transaction, so the cache is populated before the candidate removal"
    );
    let initial = &expected
        .first()
        .expect("the stream has an initial observation")
        .1;
    let final_values = &expected
        .last()
        .expect("the stream has a final observation")
        .1;
    assert!(
        initial.derivations[0].candidates > final_values.derivations[0].candidates,
        "the candidate removal must change the derived candidate count"
    );
    assert!(
        initial.derivations[0].iterations > final_values.derivations[0].iterations,
        "the iteration removal must change the derived iteration count"
    );
    for strategy in RepresentationStrategy::COMPARED {
        let facts = run_strategy_with_expected(&workload, strategy, &expected)
            .unwrap_or_else(|error| panic!("{} diverged: {error}", strategy.id()));
        assert_eq!(facts.observation, *final_values);
    }
}

#[test]
fn index_maintenance_scan_is_counted_in_traversals() {
    let workload = workload::child_removal_workload().expect("the child-removal stream is valid");
    let index = run_strategy(&workload, RepresentationStrategy::EntityIndex)
        .expect("index replay over removals");
    // Exact expectation for this stride-1 workload: three observations visit
    // 5+3, 4+3 and 4+2 index entries (18 + 16 + 14 = 48), and the two removals
    // rebuild the index, scanning 4+3 and 4+2 entries (7 + 6 = 13). Total 61.
    assert_eq!(
        index.counters.collection_traversals, 61,
        "3 observations (48) + 2 maintenance rebuild scans (13) must be counted exactly"
    );
    // Maintenance also allocates index entries; the exact traversal assertion
    // above is the load-bearing check for the scan count.
    assert!(index.counters.allocations > 0);
}

#[test]
fn child_removals_leave_no_stale_cache_accounting() {
    let workload = workload::child_removal_workload().expect("the child-removal stream is valid");
    let expected = expected_observations(&workload).expect("the product accepts the removals");
    let cache =
        run_strategy_with_expected(&workload, RepresentationStrategy::RevisionCache, &expected)
            .expect("cache replay over removals");
    // Exact expectation: the candidate removal invalidates the cached task, the
    // iteration removal invalidates it again, and the release of the handle held
    // across the second removal cancels one stale handle. Deleting either removal
    // invalidation (or the candidate-owner branch) drops this to two or fewer.
    assert_eq!(
        cache.counters.stale_cancels, 3,
        "2 removal invalidations + 1 stale-handle release"
    );
    assert!(cache.counters.peak_live_bytes >= cache.counters.bytes_held);
    assert_eq!(cache.observation, expected[expected.len() - 1].1);
}

#[test]
fn documented_summary_covers_every_contract_size_within_bounds() {
    let report = run_experiment(&WorkloadConfig::summary()).expect("the documented summary runs");
    assert_eq!(report.sizes().len(), WorkloadConfig::SUMMARY_SIZES.len());
    assert_eq!(
        report.summary()["status"],
        serde_json::json!("EXPLORATORY_NO_NUMERIC_GATE")
    );
    for size in report.sizes() {
        assert_eq!(
            size.strategies.len(),
            RepresentationStrategy::COMPARED.len()
        );
        for facts in &size.strategies {
            assert!(
                facts.within_bound,
                "{} exceeded its bound at {} entities",
                facts.strategy.id(),
                size.entities
            );
        }
    }
}

/// The 100k curve is reproducible on demand, not part of the fast test budget:
/// `cargo test --locked -p cos-client --lib -- --ignored extended_summary`.
#[test]
#[ignore = "records the 100k exploratory curve on demand"]
fn extended_summary_records_the_100k_curve() {
    let report = run_experiment(&WorkloadConfig::extended()).expect("the extended summary runs");
    assert_eq!(report.sizes().len(), WorkloadConfig::EXTENDED_SIZES.len());
    for size in report.sizes() {
        assert_eq!(size.entities, 112_500);
        for facts in &size.strategies {
            assert!(
                facts.within_bound,
                "{} exceeded its bound",
                facts.strategy.id()
            );
            assert!(facts.counters.peak_live_bytes > 0);
        }
    }
}

#[test]
fn product_validation_rejects_an_invalid_transaction() {
    let config = small();
    let mut workload = generated(&config);
    // Break the first mutation's base revision: the product's own validation must
    // reject the stream before any representation records facts.
    if let Some(desktop_protocol::ProjectionMutation::Upsert { base_revision, .. }) =
        workload.transactions[0].mutations.first_mut()
    {
        *base_revision = desktop_protocol::Counter::new("999").ok();
    }
    let outcome = expected_observations(&workload);
    assert!(
        matches!(outcome, Err(ExperimentError::Integrity(_))),
        "an invalid stream must be rejected, got {outcome:?}"
    );
}
