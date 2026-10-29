use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use app_core::{ObservationPublication, PublicationNotice, PublicationSource, SnapshotHandle};
use desktop_protocol::{EpochId, TaskId, TaskStatus};

/// Canonical task fields plus Desktop-local attention, with no formatted row text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRow {
    pub identity: TaskId,
    pub display_id: Arc<str>,
    pub title: Arc<str>,
    pub task_status: TaskStatus,
    pub inner_phase: Arc<str>,
    pub iteration_index: Option<u32>,
    pub attention: LocalAttention,
}

/// Presentation-only attention. These flags never become Runtime task facts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalAttention {
    pub stale_connection: bool,
    pub unsettled_command: bool,
    pub unavailable_reference: bool,
}

/// A complete raw task projection collection in the frozen unfiltered order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRailCollection {
    pub rows: Arc<[Arc<TaskRow>]>,
    pub epoch: Option<EpochId>,
    /// True only for unsettled commands that cannot be bound to an observed task.
    pub scope_has_unsettled_commands: bool,
}

/// Search results refer to rows without copying or formatting their fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRailSearch {
    pub query: Arc<str>,
    pub indices: Arc<[usize]>,
}

struct CachedTaskRow {
    revision: desktop_protocol::Counter,
    attention: LocalAttention,
    row: Arc<TaskRow>,
}

/// Incremental per-consumer task projection cache; membership never outlives a snapshot.
#[derive(Default)]
pub struct TaskRailCache {
    scope: Option<(u64, Option<EpochId>)>,
    rows: BTreeMap<TaskId, CachedTaskRow>,
}

impl TaskRailCache {
    /// Drop all reusable task rows and their observation scope.
    pub fn reset(&mut self) {
        self.scope = None;
        self.rows.clear();
    }

    /// Derive raw task rows from one complete, currently usable publication.
    ///
    /// No snapshot handle is retained. Cancellation or an unavailable publication
    /// discards all reusable rows before returning.
    pub fn derive(
        &mut self,
        publication: &ObservationPublication,
        cancelled: impl Fn() -> bool,
    ) -> Option<Arc<TaskRailCollection>> {
        if cancelled() {
            self.reset();
            return None;
        }
        if publication.source == PublicationSource::Unavailable || publication.revision.is_none() {
            self.reset();
            return None;
        }
        let Some(snapshot) = publication.snapshot.as_ref() else {
            self.reset();
            return None;
        };

        let scope = (publication.session_generation, snapshot.epoch.clone());
        if self.scope.as_ref() != Some(&scope) {
            self.reset();
            self.scope = Some(scope);
        }

        let mut unsettled_targets = BTreeSet::new();
        let mut scope_has_unsettled_commands = false;
        for view in publication.command_statuses.values() {
            if cancelled() {
                self.reset();
                return None;
            }
            if !command_is_unsettled(view) {
                continue;
            }
            let target = view
                .receipt
                .as_ref()
                .and_then(|receipt| receipt.task_id.as_ref());
            if let Some(target) = target.filter(|id| snapshot.tasks.contains_key(*id)) {
                unsettled_targets.insert(target.clone());
            } else {
                // A receipt-less/unobserved command has no safe task identity.
                scope_has_unsettled_commands = true;
            }
        }

        let mut canceled_during_prune = false;
        self.rows.retain(|identity, _| {
            if cancelled() {
                canceled_during_prune = true;
                true
            } else {
                snapshot.tasks.contains_key(identity)
            }
        });
        if canceled_during_prune || cancelled() {
            self.reset();
            return None;
        }

        let mut rows = Vec::with_capacity(snapshot.tasks.len());
        for (identity, task) in &snapshot.tasks {
            if cancelled() {
                self.reset();
                return None;
            }
            if identity != &task.task_ref {
                self.reset();
                return None;
            }
            let attention = LocalAttention {
                stale_connection: publication.stale,
                unsettled_command: unsettled_targets.contains(identity),
                unavailable_reference: has_unavailable_reference(task, snapshot),
            };
            let cached = self.rows.get(identity).filter(|cached| {
                cached.revision == task.projection_revision && cached.attention == attention
            });
            let row = if let Some(cached) = cached {
                Arc::clone(&cached.row)
            } else {
                let row = Arc::new(TaskRow {
                    identity: task.task_ref.clone(),
                    display_id: Arc::from(task.display_id.as_str()),
                    title: Arc::from(task.title.as_str()),
                    task_status: task.task_status.clone(),
                    inner_phase: Arc::from(task.inner_phase.as_str()),
                    iteration_index: task.iteration_index,
                    attention,
                });
                self.rows.insert(
                    identity.clone(),
                    CachedTaskRow {
                        revision: task.projection_revision.clone(),
                        attention,
                        row: Arc::clone(&row),
                    },
                );
                row
            };
            rows.push(row);
        }

        if !cancellable_sort_by(&mut rows, &cancelled, |left, right| {
            compare_rows(left.as_ref(), right.as_ref())
        }) || cancelled()
        {
            self.reset();
            return None;
        }

        Some(Arc::new(TaskRailCollection {
            rows: rows.into(),
            epoch: snapshot.epoch.clone(),
            scope_has_unsettled_commands,
        }))
    }
}

impl TaskRailCollection {
    /// Trim and simple-case-fold a query, then return matching row indices in
    /// frozen rank/status/iteration/display-id order.
    pub fn search(&self, query: &str, cancelled: impl Fn() -> bool) -> Option<TaskRailSearch> {
        if cancelled() {
            return None;
        }
        let query = query.trim();
        let visible_query: Arc<str> = Arc::from(query);
        if query.is_empty() {
            let mut indices = Vec::with_capacity(self.rows.len());
            for index in 0..self.rows.len() {
                if cancelled() {
                    return None;
                }
                indices.push(index);
            }
            return (!cancelled()).then(|| TaskRailSearch {
                query: visible_query,
                indices: indices.into(),
            });
        }

        let folded_query = query
            .chars()
            .map(crate::simple_casefold::fold_scalar)
            .collect::<Vec<_>>();
        let prefix = query_prefix_table(&folded_query, &cancelled)?;
        let mut matches = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            if cancelled() {
                return None;
            }
            let display = folded_field_match(&row.display_id, &folded_query, &prefix, &cancelled)?;
            let tier = if display.exact {
                Some(0)
            } else if display.prefix {
                Some(1)
            } else {
                let title = folded_field_match(&row.title, &folded_query, &prefix, &cancelled)?;
                if title.prefix {
                    Some(2)
                } else if title.contains || display.contains {
                    // Title and display-id substrings share the same final tier.
                    Some(3)
                } else {
                    None
                }
            };
            if let Some(tier) = tier {
                matches.push(SearchIndex { tier, index });
            }
        }

        // The collection is already in the frozen within-tier order. Partition
        // matches into the four tiers without sorting the same rows a second time.
        let mut indices = Vec::with_capacity(matches.len());
        for tier in 0..4 {
            for result in &matches {
                if cancelled() {
                    return None;
                }
                if result.tier == tier {
                    indices.push(result.index);
                }
            }
        }
        (!cancelled()).then(|| TaskRailSearch {
            query: visible_query,
            indices: indices.into(),
        })
    }
}

#[derive(Clone, Copy)]
struct SearchIndex {
    tier: u8,
    index: usize,
}

#[derive(Clone, Copy)]
struct FieldMatch {
    exact: bool,
    prefix: bool,
    contains: bool,
}

/// Per-window query, selection and exact-publication result admission state.
#[derive(Default)]
pub struct TaskRailState {
    query: String,
    query_generation: u64,
    selected: Option<TaskId>,
    owner_generation: Option<u64>,
    epoch: Option<EpochId>,
    notice: Option<PublicationNotice>,
    collection: Option<Arc<TaskRailCollection>>,
    results: Option<TaskRailSearch>,
}

impl TaskRailState {
    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn query_generation(&self) -> u64 {
        self.query_generation
    }

    /// An actual edit advances the generation and hides prior search results.
    pub fn set_query(&mut self, query: &str) -> bool {
        if self.query == query {
            return false;
        }
        let Some(next_generation) = self.query_generation.checked_add(1) else {
            return false;
        };
        self.query.clear();
        self.query.push_str(query);
        self.query_generation = next_generation;
        self.results = None;
        true
    }

    /// The local selected identity, including one no longer present in the collection.
    pub fn selected(&self) -> Option<&TaskId> {
        self.selected.as_ref()
    }

    /// Select only an identity present in the exact currently authorized collection.
    pub fn select(&mut self, identity: &TaskId, live: Option<PublicationNotice>) -> bool {
        if !self.revalidate(live) {
            return false;
        }
        let Some(collection) = self.collection.as_ref() else {
            return false;
        };
        if !collection.rows.iter().any(|row| &row.identity == identity) {
            return false;
        }
        self.selected = Some(identity.clone());
        true
    }

    /// Whether the rail could currently activate this identity: the derived
    /// search results and the collection they index must both be present and
    /// the identity must appear in those results. This is the same admission
    /// the rail's Enter path uses, exposed for callers that gate a command.
    pub fn selectable(&self, identity: &TaskId) -> bool {
        let (Some(results), Some(collection)) = (self.results.as_ref(), self.collection.as_ref())
        else {
            return false;
        };
        results
            .indices
            .iter()
            .any(|index| collection.rows[*index].identity == *identity)
    }

    pub fn collection(&self) -> Option<&Arc<TaskRailCollection>> {
        self.collection.as_ref()
    }

    pub fn results(&self) -> Option<&TaskRailSearch> {
        self.results.as_ref()
    }

    /// Withhold collection/results whenever the exact publication lease changes.
    pub fn revalidate(&mut self, live: Option<PublicationNotice>) -> bool {
        if let Some(live_notice) = live {
            if self
                .owner_generation
                .is_some_and(|generation| generation != live_notice.session_generation)
            {
                self.selected = None;
            }
            self.owner_generation = Some(live_notice.session_generation);
        }
        let authorized = self.collection.is_some() && self.notice == live;
        if !authorized {
            self.notice = None;
            self.collection = None;
            self.results = None;
        }
        authorized
    }

    /// Install only the exact live publication and current query completion.
    pub fn install(
        &mut self,
        notice: PublicationNotice,
        query_generation: u64,
        collection: Arc<TaskRailCollection>,
        result: TaskRailSearch,
        live: Option<PublicationNotice>,
    ) -> bool {
        self.revalidate(live);
        if live != Some(notice)
            || query_generation != self.query_generation
            || result.query.as_ref() != self.query.trim()
            || result
                .indices
                .iter()
                .any(|index| *index >= collection.rows.len())
        {
            return false;
        }

        let owner_changed = self
            .owner_generation
            .is_some_and(|generation| generation != notice.session_generation);
        let epoch_changed = self.owner_generation == Some(notice.session_generation)
            && self.epoch.as_ref() != collection.epoch.as_ref();
        if owner_changed || epoch_changed {
            self.selected = None;
        }
        self.owner_generation = Some(notice.session_generation);
        self.epoch = collection.epoch.clone();
        self.notice = Some(notice);
        self.collection = Some(collection);
        self.results = Some(result);
        true
    }
}

fn command_is_unsettled(view: &app_core::CommandReadView) -> bool {
    use app_core::CommandPresentation as Presentation;
    use desktop_protocol::ReceiptState;

    let receipt_state = view.receipt.as_ref().map(|receipt| &receipt.state);
    match view.presentation {
        Presentation::Prepared
        | Presentation::AwaitingResponse
        | Presentation::OutcomeUnconfirmed => true,
        Presentation::ReceiptAvailable => match receipt_state {
            Some(ReceiptState::Rejected | ReceiptState::CanonicalReflected) => false,
            Some(ReceiptState::Accepted | ReceiptState::Unknown(_)) | None => true,
        },
        Presentation::CanonicalEffectObserved
        | Presentation::Unsupported
        | Presentation::Failed => false,
    }
}

fn has_unavailable_reference(
    task: &desktop_protocol::TaskSnapshot,
    snapshot: &SnapshotHandle,
) -> bool {
    let capabilities_missing = !snapshot.capabilities.contains_key(&task.capabilities_ref);
    let iteration_missing = task.iteration_ref.as_ref().is_some_and(|identity| {
        !matches!(
            snapshot.iterations.get(identity),
            Some(iteration) if iteration.task_ref == task.task_ref
        )
    });
    let candidate_missing = task.best_candidate_ref.as_ref().is_some_and(|identity| {
        !matches!(
            snapshot.candidates.get(identity),
            Some(candidate) if candidate.task_ref == task.task_ref
        )
    });
    let outcome_missing = task.formal_outcome_ref.as_ref().is_some_and(|identity| {
        !matches!(
            snapshot.outcomes.get(identity),
            Some(outcome) if outcome.task_ref == task.task_ref
        )
    });
    let brief_missing = task
        .accepted_brief_ref
        .as_ref()
        .is_some_and(|identity| !snapshot.artifacts.contains_key(identity));

    capabilities_missing
        || iteration_missing
        || candidate_missing
        || outcome_missing
        || brief_missing
}

fn query_prefix_table(query: &[char], cancelled: &impl Fn() -> bool) -> Option<Vec<usize>> {
    let mut prefix = vec![0; query.len()];
    let mut matched = 0;
    for index in 1..query.len() {
        if cancelled() {
            return None;
        }
        while matched > 0 && query[index] != query[matched] {
            if cancelled() {
                return None;
            }
            matched = prefix[matched - 1];
        }
        if query[index] == query[matched] {
            matched += 1;
        }
        prefix[index] = matched;
    }
    (!cancelled()).then_some(prefix)
}

fn folded_field_match(
    field: &str,
    query: &[char],
    prefix_table: &[usize],
    cancelled: &impl Fn() -> bool,
) -> Option<FieldMatch> {
    let mut matched = 0;
    let mut field_length = 0;
    let mut prefix = true;
    let mut contains = false;

    for character in field.chars() {
        if cancelled() {
            return None;
        }
        let folded = crate::simple_casefold::fold_scalar(character);
        if field_length < query.len() && folded != query[field_length] {
            prefix = false;
        }
        field_length += 1;

        while matched > 0 && folded != query[matched] {
            matched = prefix_table[matched - 1];
        }
        if folded == query[matched] {
            matched += 1;
            if matched == query.len() {
                contains = true;
                matched = prefix_table[matched - 1];
            }
        }
    }

    Some(FieldMatch {
        exact: prefix && field_length == query.len(),
        prefix: prefix && field_length >= query.len(),
        contains,
    })
}

fn compare_rows(left: &TaskRow, right: &TaskRow) -> Ordering {
    status_rank(&left.task_status)
        .cmp(&status_rank(&right.task_status))
        .then_with(|| right.iteration_index.cmp(&left.iteration_index))
        .then_with(|| left.display_id.cmp(&right.display_id))
        .then_with(|| left.identity.cmp(&right.identity))
}

fn status_rank(status: &TaskStatus) -> u8 {
    match status {
        TaskStatus::Active => 0,
        TaskStatus::Assigned => 1,
        TaskStatus::Completed => 2,
        TaskStatus::Cancelled => 3,
        TaskStatus::Unknown(_) => 4,
    }
}

/// In-place heapsort so long traversals and sorting remain cancellation-aware.
fn cancellable_sort_by<T>(
    items: &mut [T],
    cancelled: &impl Fn() -> bool,
    compare: impl Fn(&T, &T) -> Ordering,
) -> bool {
    if cancelled() {
        return false;
    }
    let length = items.len();
    for start in (0..length / 2).rev() {
        if !sift_down(items, start, length, cancelled, &compare) {
            return false;
        }
    }
    let mut end = length;
    while end > 1 {
        if cancelled() {
            return false;
        }
        items.swap(0, end - 1);
        end -= 1;
        if !sift_down(items, 0, end, cancelled, &compare) {
            return false;
        }
    }
    !cancelled()
}

fn sift_down<T>(
    items: &mut [T],
    mut root: usize,
    end: usize,
    cancelled: &impl Fn() -> bool,
    compare: &impl Fn(&T, &T) -> Ordering,
) -> bool {
    loop {
        if cancelled() {
            return false;
        }
        let left = root * 2 + 1;
        if left >= end {
            return true;
        }
        let right = left + 1;
        let mut greater = left;
        if right < end {
            if cancelled() {
                return false;
            }
            if compare(&items[left], &items[right]) == Ordering::Less {
                greater = right;
            }
        }
        if cancelled() {
            return false;
        }
        if compare(&items[root], &items[greater]) != Ordering::Less {
            return true;
        }
        items.swap(root, greater);
        root = greater;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::Arc;

    use app_core::{
        CommandPresentation, CommandReadView, ObservationPublication, PublicationConnection,
        PublicationNotice, PublicationSource, SnapshotHandle,
    };
    use desktop_protocol::{
        CapabilitiesId, CapabilityScope, CommandCapability, CommandId, CommandOperation, Counter,
        CreativeCapabilities, EpochId, PhaseLabel, ReceiptState, SupportState, TaskId,
        TaskSnapshot, TaskStatus,
    };

    use super::{TaskRailCache, TaskRailState};

    fn task(
        identity: &str,
        display_id: &str,
        title: &str,
        status: TaskStatus,
        iteration_index: Option<u32>,
    ) -> TaskSnapshot {
        let task_ref = TaskId::fixture(format!("task/{identity}"));
        TaskSnapshot {
            task_ref,
            display_id: display_id.to_owned(),
            title: title.to_owned(),
            task_status: status,
            inner_phase: PhaseLabel::new("BUILD").expect("valid phase"),
            iteration_ref: None,
            iteration_index,
            best_candidate_ref: None,
            formal_outcome_ref: None,
            accepted_brief_ref: None,
            capabilities_ref: CapabilitiesId::fixture(format!("capabilities/{identity}")),
            projection_revision: Counter::from_u128(1),
        }
    }

    fn publication(
        tasks: Vec<TaskSnapshot>,
        session_generation: u64,
        publication_version: u64,
        epoch: &str,
    ) -> ObservationPublication {
        let mut snapshot: SnapshotHandle = Arc::default();
        let state = Arc::get_mut(&mut snapshot).expect("snapshot has one owner");
        state.epoch = Some(EpochId::fixture(epoch));
        for task in tasks {
            let task_id = task.task_ref.clone();
            let capabilities_ref = task.capabilities_ref.clone();
            state.capabilities.insert(
                capabilities_ref.clone(),
                CreativeCapabilities {
                    capabilities_ref,
                    scope: CapabilityScope::Task {
                        task_id: task_id.clone(),
                    },
                    commands: vec![CommandCapability {
                        operation: CommandOperation::StopTask,
                        support: SupportState::Supported,
                        reason: None,
                    }],
                    media: Vec::new(),
                    features: Vec::new(),
                    constraints: Vec::new(),
                    availability: SupportState::Supported,
                    reason: None,
                    projection_revision: Counter::from_u128(1),
                },
            );
            state.tasks.insert(task_id, task);
        }
        ObservationPublication {
            snapshot: Some(snapshot),
            revision: Some(Counter::from_u128(publication_version.into())),
            connection: PublicationConnection::Observing,
            source: PublicationSource::Fixture,
            stale: false,
            session_generation,
            publication_version,
            ..ObservationPublication::default()
        }
    }

    fn notice(generation: u64, version: u64) -> PublicationNotice {
        PublicationNotice {
            session_generation: generation,
            publication_version: version,
        }
    }

    fn derive(
        cache: &mut TaskRailCache,
        publication: &ObservationPublication,
    ) -> Arc<super::TaskRailCollection> {
        cache
            .derive(publication, || false)
            .expect("authorized fixture publication derives")
    }

    fn task_ids(
        collection: &super::TaskRailCollection,
        search: &super::TaskRailSearch,
    ) -> Vec<TaskId> {
        search
            .indices
            .iter()
            .map(|index| collection.rows[*index].identity.clone())
            .collect()
    }

    #[test]
    fn selectable_follows_the_installed_search_results_only() {
        let publication = publication(
            vec![
                task("alpha", "A", "alpha", TaskStatus::Active, Some(1)),
                task("beta", "B", "beta", TaskStatus::Active, Some(1)),
            ],
            3,
            1,
            "epoch/1",
        );
        let collection = derive(&mut TaskRailCache::default(), &publication);
        let alpha = TaskId::fixture("task/alpha");
        let beta = TaskId::fixture("task/beta");
        let live = notice(3, 1);
        let mut state = TaskRailState::default();
        assert!(state.set_query("alpha"));
        let search = collection
            .search("alpha", || false)
            .expect("search completes");
        assert!(state.install(live, 1, Arc::clone(&collection), search, Some(live)));
        assert!(state.selectable(&alpha));
        assert!(!state.selectable(&beta));

        // An edited query hides the previous results before a new derivation lands.
        assert!(state.set_query("beta"));
        assert!(!state.selectable(&alpha));
        assert!(!state.selectable(&beta));

        let search = collection
            .search("beta", || false)
            .expect("search completes");
        assert!(state.install(live, 2, Arc::clone(&collection), search, Some(live)));
        assert!(state.selectable(&beta));
        assert!(!state.selectable(&alpha));
    }

    #[test]
    fn collection_and_search_order_follow_independent_frozen_tiers() {
        let publication = publication(
            vec![
                task(
                    "exact",
                    "NeEdLe",
                    "exact title",
                    TaskStatus::Unknown("FUTURE".into()),
                    Some(1),
                ),
                task("pfx-low", "needle-1", "other", TaskStatus::Active, Some(1)),
                task("pfx-high", "needle-9", "other", TaskStatus::Active, Some(9)),
                task("pfx-a", "Needle-A", "other", TaskStatus::Active, Some(9)),
                task(
                    "pfx-assigned",
                    "needle-assigned",
                    "other",
                    TaskStatus::Assigned,
                    Some(20),
                ),
                task(
                    "title-prefix",
                    "z-title",
                    "needle starts here",
                    TaskStatus::Completed,
                    None,
                ),
                task(
                    "id-substring",
                    "xneedle",
                    "other",
                    TaskStatus::Active,
                    Some(3),
                ),
                task(
                    "title-substring",
                    "Frag",
                    "some needle here",
                    TaskStatus::Active,
                    Some(3),
                ),
                task(
                    "cancelled-substring",
                    "cancel",
                    "contains needle",
                    TaskStatus::Cancelled,
                    Some(99),
                ),
            ],
            1,
            1,
            "epoch/1",
        );
        let collection = derive(&mut TaskRailCache::default(), &publication);
        let result = collection
            .search("  NEEDLE  ", || false)
            .expect("query search completes");
        assert_eq!(result.query.as_ref(), "NEEDLE");
        assert_eq!(
            task_ids(&collection, &result),
            [
                TaskId::fixture("task/exact"),
                TaskId::fixture("task/pfx-a"),
                TaskId::fixture("task/pfx-high"),
                TaskId::fixture("task/pfx-low"),
                TaskId::fixture("task/pfx-assigned"),
                TaskId::fixture("task/title-prefix"),
                TaskId::fixture("task/title-substring"),
                TaskId::fixture("task/id-substring"),
                TaskId::fixture("task/cancelled-substring"),
            ]
        );

        let unfiltered = collection
            .search(" \t ", || false)
            .expect("empty query completes");
        assert_eq!(unfiltered.query.as_ref(), "");
        assert_eq!(
            task_ids(&collection, &unfiltered),
            [
                TaskId::fixture("task/pfx-a"),
                TaskId::fixture("task/pfx-high"),
                TaskId::fixture("task/title-substring"),
                TaskId::fixture("task/id-substring"),
                TaskId::fixture("task/pfx-low"),
                TaskId::fixture("task/pfx-assigned"),
                TaskId::fixture("task/title-prefix"),
                TaskId::fixture("task/cancelled-substring"),
                TaskId::fixture("task/exact"),
            ]
        );
        assert_eq!(collection.rows[0].task_status, TaskStatus::Active);
        assert_eq!(
            collection
                .rows
                .iter()
                .find(|row| row.identity == TaskId::fixture("task/exact"))
                .expect("unknown task remains visible")
                .task_status,
            TaskStatus::Unknown("FUTURE".to_owned())
        );
    }

    #[test]
    fn unicode_simple_fold_matches_hand_authored_scalar_vectors_without_expansion() {
        let vectors = [
            ("sigma", "Σigma", "  ςIGMA  ", true),
            ("cherokee", "ᎠX", "ꭰx", true),
            ("dotted-i", "İ", "i", false),
            ("dotted-i-exact", "İ", "İ", true),
            ("sharp-s-full", "Straße", "STRASSE", false),
            ("sharp-s-simple", "Straße", "STRAẞE", true),
            ("supplementary", "𐐀X", "𐐨x", true),
            ("diacritic", "café", "cafe", false),
            ("diacritic-case", "café", "CAFÉ", true),
        ];
        for (identity, display_id, query, expected_match) in vectors {
            let publication = publication(
                vec![task(
                    identity,
                    display_id,
                    "vector",
                    TaskStatus::Active,
                    None,
                )],
                1,
                1,
                "epoch/1",
            );
            let collection = derive(&mut TaskRailCache::default(), &publication);
            let search = collection
                .search(query, || false)
                .expect("search completes");
            assert_eq!(
                !search.indices.is_empty(),
                expected_match,
                "unexpected simple-fold match for {display_id:?} / {query:?}"
            );
        }
    }

    #[test]
    fn empty_collection_and_no_match_remain_observably_distinct() {
        let empty_publication = publication(Vec::new(), 1, 1, "epoch/1");
        let empty_collection = derive(&mut TaskRailCache::default(), &empty_publication);
        let empty_search = empty_collection
            .search("", || false)
            .expect("empty search completes");
        assert!(empty_collection.rows.is_empty());
        assert!(empty_search.indices.is_empty());

        let populated_publication = publication(
            vec![task("one", "Task-1", "Observed", TaskStatus::Active, None)],
            1,
            2,
            "epoch/1",
        );
        let populated = derive(&mut TaskRailCache::default(), &populated_publication);
        let no_match = populated
            .search("absent", || false)
            .expect("no-match search completes");
        assert_eq!(populated.rows.len(), 1);
        assert!(no_match.indices.is_empty());
        assert_eq!(no_match.query.as_ref(), "absent");
    }

    #[test]
    fn only_explicitly_bound_unsettled_commands_mark_a_task_row() {
        let mut publication = publication(
            vec![
                task("bound", "Task-bound", "Bound", TaskStatus::Active, None),
                task("other", "Task-other", "Other", TaskStatus::Active, None),
            ],
            1,
            1,
            "epoch/1",
        );
        let bound_id = TaskId::fixture("task/bound");
        let command_id = CommandId::fixture("command/bound");
        publication.command_statuses.insert(
            command_id.clone(),
            CommandReadView {
                presentation: CommandPresentation::ReceiptAvailable,
                receipt: Some(desktop_protocol::CommandReceipt {
                    command_id,
                    operation: CommandOperation::StopTask,
                    state: ReceiptState::Accepted,
                    task_id: Some(bound_id.clone()),
                    accepted_at: None,
                    reject_reason: None,
                    effect_refs: Vec::new(),
                    dedupe_expires_at: None,
                    message: None,
                }),
                issue: None,
            },
        );
        publication.command_statuses.insert(
            CommandId::fixture("command/unbound"),
            CommandReadView {
                presentation: CommandPresentation::AwaitingResponse,
                receipt: None,
                issue: None,
            },
        );
        let collection = derive(&mut TaskRailCache::default(), &publication);
        assert!(collection.scope_has_unsettled_commands);
        assert!(
            collection
                .rows
                .iter()
                .find(|row| row.identity == bound_id)
                .expect("bound task is present")
                .attention
                .unsettled_command
        );
        assert!(
            collection
                .rows
                .iter()
                .filter(|row| row.identity != TaskId::fixture("task/bound"))
                .all(|row| !row.attention.unsettled_command)
        );
    }

    #[test]
    fn stale_and_unavailable_reference_attention_is_local_and_typed() {
        let mut missing = task(
            "missing-reference",
            "Task-missing",
            "Raw title",
            TaskStatus::Unknown("FUTURE_STATUS".to_owned()),
            None,
        );
        missing.formal_outcome_ref = Some(desktop_protocol::OutcomeId::fixture("outcome/missing"));
        let mut publication = publication(vec![missing], 1, 1, "epoch/1");
        publication.stale = true;
        let collection = derive(&mut TaskRailCache::default(), &publication);
        let row = &collection.rows[0];
        assert_eq!(row.display_id.as_ref(), "Task-missing");
        assert_eq!(row.title.as_ref(), "Raw title");
        assert_eq!(row.inner_phase.as_ref(), "BUILD");
        assert_eq!(row.iteration_index, None);
        assert_eq!(
            row.task_status,
            TaskStatus::Unknown("FUTURE_STATUS".to_owned())
        );
        assert!(row.attention.stale_connection);
        assert!(row.attention.unavailable_reference);
        assert!(!row.attention.unsettled_command);
    }

    #[test]
    fn canceled_derivation_drops_cached_rows_and_search_cancels_before_return() {
        let first_publication = publication(
            vec![task("old", "Old", "Old", TaskStatus::Active, None)],
            1,
            1,
            "epoch/1",
        );
        let current_publication = publication(
            vec![task(
                "current",
                "Current",
                "Current",
                TaskStatus::Active,
                None,
            )],
            1,
            2,
            "epoch/1",
        );
        let mut cache = TaskRailCache::default();
        let _old = derive(&mut cache, &first_publication);
        let cancel_after_start = Cell::new(false);
        assert!(
            cache
                .derive(&current_publication, || cancel_after_start.replace(true))
                .is_none()
        );
        let current = derive(&mut cache, &current_publication);
        assert_eq!(current.rows[0].identity, TaskId::fixture("task/current"));

        let cancel_search = Cell::new(false);
        assert!(
            current
                .search("Current", || cancel_search.replace(true))
                .is_none()
        );
    }

    #[test]
    fn query_generation_and_publication_notice_reject_late_completions() {
        let first_publication = publication(
            vec![task("alpha", "Alpha", "Alpha", TaskStatus::Active, None)],
            4,
            7,
            "epoch/1",
        );
        let second_publication = publication(
            vec![task("beta", "Beta", "Beta", TaskStatus::Active, None)],
            4,
            8,
            "epoch/1",
        );
        let mut cache = TaskRailCache::default();
        let first = derive(&mut cache, &first_publication);
        let first_notice = notice(4, 7);
        let mut state = TaskRailState::default();
        assert!(state.set_query("alpha"));
        let first_generation = state.query_generation();
        let first_result = first
            .search("alpha", || false)
            .expect("first search completes");
        assert!(state.install(
            first_notice,
            first_generation,
            Arc::clone(&first),
            first_result.clone(),
            Some(first_notice),
        ));

        assert!(state.set_query(" beta "));
        assert_eq!(state.query(), " beta ");
        let second_generation = state.query_generation();
        assert!(
            state.results().is_none(),
            "query edits hide previous results"
        );
        assert!(!state.install(
            first_notice,
            first_generation,
            Arc::clone(&first),
            first_result,
            Some(first_notice),
        ));
        assert!(state.revalidate(Some(first_notice)));

        let second = derive(&mut cache, &second_publication);
        let second_notice = notice(4, 8);
        let second_result = second
            .search(state.query(), || false)
            .expect("second search completes");
        assert!(state.install(
            second_notice,
            second_generation,
            Arc::clone(&second),
            second_result,
            Some(second_notice),
        ));
        assert!(
            !state.install(
                first_notice,
                first_generation,
                Arc::clone(&first),
                first
                    .search("alpha", || false)
                    .expect("late search completes"),
                Some(second_notice),
            )
        );
        assert_eq!(
            state
                .results()
                .expect("current result remains")
                .query
                .as_ref(),
            "beta"
        );
        assert_eq!(
            state.collection().expect("current rows remain").rows[0].identity,
            TaskId::fixture("task/beta")
        );
    }

    #[test]
    fn selection_stays_local_across_updates_and_clears_on_scope_changes() {
        let mut cache = TaskRailCache::default();
        let first_publication = publication(
            vec![
                task(
                    "selected",
                    "Task-selected",
                    "Selected",
                    TaskStatus::Active,
                    Some(1),
                ),
                task(
                    "other",
                    "Task-other",
                    "Other",
                    TaskStatus::Assigned,
                    Some(1),
                ),
            ],
            2,
            1,
            "epoch/1",
        );
        let first = derive(&mut cache, &first_publication);
        let first_notice = notice(2, 1);
        let mut state = TaskRailState::default();
        let first_result = first.search("", || false).expect("empty search completes");
        assert!(state.install(
            first_notice,
            0,
            Arc::clone(&first),
            first_result,
            Some(first_notice),
        ));
        let selected = TaskId::fixture("task/selected");
        assert!(state.select(&selected, Some(first_notice)));
        assert_eq!(state.selected(), Some(&selected));
        assert_eq!(
            first
                .rows
                .iter()
                .find(|row| row.identity == selected)
                .expect("selected row remains present")
                .task_status,
            TaskStatus::Active
        );

        let inserted = publication(
            vec![
                task(
                    "selected",
                    "Task-selected",
                    "Selected",
                    TaskStatus::Active,
                    Some(1),
                ),
                task(
                    "other",
                    "Task-other",
                    "Other",
                    TaskStatus::Assigned,
                    Some(1),
                ),
                task(
                    "inserted",
                    "Task-inserted",
                    "Inserted",
                    TaskStatus::Active,
                    Some(2),
                ),
            ],
            2,
            2,
            "epoch/1",
        );
        let inserted_collection = derive(&mut cache, &inserted);
        let inserted_notice = notice(2, 2);
        assert!(
            state.install(
                inserted_notice,
                0,
                Arc::clone(&inserted_collection),
                inserted_collection
                    .search("", || false)
                    .expect("empty search completes"),
                Some(inserted_notice),
            )
        );
        assert_eq!(
            state.selected(),
            Some(&selected),
            "insertions do not steal selection"
        );

        let update = publication(
            vec![task(
                "other",
                "Task-other",
                "Other",
                TaskStatus::Assigned,
                Some(2),
            )],
            2,
            3,
            "epoch/1",
        );
        let update_collection = derive(&mut cache, &update);
        let update_notice = notice(2, 3);
        let update_result = update_collection
            .search("", || false)
            .expect("empty search completes");
        assert!(state.install(
            update_notice,
            0,
            Arc::clone(&update_collection),
            update_result,
            Some(update_notice),
        ));
        assert_eq!(
            state.selected(),
            Some(&selected),
            "removed selection is not replaced"
        );
        assert!(
            !update_collection
                .rows
                .iter()
                .any(|row| row.identity == selected)
        );
        assert!(!state.select(&selected, Some(update_notice)));
        assert_eq!(state.selected(), Some(&selected));

        let new_owner = publication(
            vec![task(
                "other",
                "Task-other",
                "Other",
                TaskStatus::Assigned,
                Some(2),
            )],
            3,
            1,
            "epoch/1",
        );
        let new_owner_collection = derive(&mut cache, &new_owner);
        let new_owner_notice = notice(3, 1);
        assert!(
            state.install(
                new_owner_notice,
                0,
                Arc::clone(&new_owner_collection),
                new_owner_collection
                    .search("", || false)
                    .expect("empty search completes"),
                Some(new_owner_notice),
            )
        );
        assert_eq!(
            state.selected(),
            None,
            "owner change clears incompatible selection"
        );
        assert!(state.select(&TaskId::fixture("task/other"), Some(new_owner_notice)));

        let new_epoch = publication(
            vec![task(
                "other",
                "Task-other",
                "Other",
                TaskStatus::Assigned,
                Some(2),
            )],
            3,
            2,
            "epoch/2",
        );
        let new_epoch_collection = derive(&mut cache, &new_epoch);
        let new_epoch_notice = notice(3, 2);
        assert!(
            state.install(
                new_epoch_notice,
                0,
                Arc::clone(&new_epoch_collection),
                new_epoch_collection
                    .search("", || false)
                    .expect("empty search completes"),
                Some(new_epoch_notice),
            )
        );
        assert_eq!(
            state.selected(),
            None,
            "epoch change clears incompatible selection"
        );
    }

    #[test]
    fn selection_and_rows_are_withheld_when_live_publication_changes() {
        let mut cache = TaskRailCache::default();
        let publication = publication(
            vec![task("one", "Task-one", "One", TaskStatus::Active, None)],
            1,
            1,
            "epoch/1",
        );
        let collection = derive(&mut cache, &publication);
        let original_notice = notice(1, 1);
        let mut state = TaskRailState::default();
        assert!(
            state.install(
                original_notice,
                0,
                Arc::clone(&collection),
                collection
                    .search("", || false)
                    .expect("empty search completes"),
                Some(original_notice),
            )
        );
        let selected = TaskId::fixture("task/one");
        assert!(state.select(&selected, Some(original_notice)));

        let changed_notice = notice(1, 2);
        assert!(!state.revalidate(Some(changed_notice)));
        assert!(state.collection().is_none());
        assert!(state.results().is_none());
        assert!(!state.select(&selected, Some(original_notice)));
        assert!(!state.install(
            original_notice,
            0,
            collection,
            super::TaskRailSearch {
                query: Arc::from(""),
                indices: Arc::from([0]),
            },
            Some(changed_notice),
        ));
        assert!(state.collection().is_none());
        assert_eq!(state.selected(), Some(&selected));
    }

    #[test]
    fn authorized_install_rejects_malformed_indices_and_wrong_query_content() {
        let mut cache = TaskRailCache::default();
        let publication = publication(
            vec![task("one", "Task-one", "One", TaskStatus::Active, None)],
            1,
            1,
            "epoch/1",
        );
        let collection = derive(&mut cache, &publication);
        let current_notice = notice(1, 1);
        let mut state = TaskRailState::default();
        assert!(state.set_query("Task"));
        let generation = state.query_generation();
        assert!(!state.install(
            current_notice,
            generation,
            Arc::clone(&collection),
            super::TaskRailSearch {
                query: Arc::from("not Task"),
                indices: Arc::from([0]),
            },
            Some(current_notice),
        ));
        assert!(!state.install(
            current_notice,
            generation,
            Arc::clone(&collection),
            super::TaskRailSearch {
                query: Arc::from("Task"),
                indices: Arc::from([1]),
            },
            Some(current_notice),
        ));
        assert!(state.collection().is_none());
        assert!(state.results().is_none());
    }

    #[test]
    fn absent_optional_task_fields_are_not_synthesized() {
        let publication = publication(
            vec![task(
                "unknown",
                "Task-unknown",
                "Raw title",
                TaskStatus::Unknown("FUTURE".into()),
                None,
            )],
            1,
            1,
            "epoch/1",
        );
        let collection = derive(&mut TaskRailCache::default(), &publication);
        let row = &collection.rows[0];
        assert_eq!(row.iteration_index, None);
        assert_eq!(row.task_status.as_str(), "FUTURE");
        assert_eq!(row.title.as_ref(), "Raw title");
        assert_eq!(row.display_id.as_ref(), "Task-unknown");
        assert!(!row.attention.unavailable_reference);
    }
}
