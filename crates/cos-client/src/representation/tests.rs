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
