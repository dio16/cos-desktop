//! Command palette inventory and matching (DC-MP-NAV5).
//!
//! The inventory is closed: this module names only commands that an existing
//! approved surface already implements. Availability is supplied by the caller
//! from the current authorized publication; nothing here invents a capability or
//! mutates canonical state. Matching is case-folded substring over the stable id,
//! title and category in the contract's frozen rank order, and the result list is
//! bounded.

use crate::simple_casefold;

/// Maximum number of ranked rows the palette keeps.
pub const MAX_RESULTS: usize = 64;

/// One approved palette command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Command {
    /// Show the Artifact surface.
    ShowArtifact,
    /// Show the Compare surface.
    ShowCompare,
    /// Show the Flow surface.
    ShowFlow,
    /// Advance to the next surface.
    NextSurface,
    /// Open the active Task in the rail.
    OpenActiveTask,
    /// Open (view) the active candidate.
    OpenActiveCandidate,
    /// Show the inspector's Evaluation section.
    ShowEvaluation,
    /// Show the inspector's Evidence section.
    ShowEvidence,
    /// Toggle the diagnostics panel.
    ToggleDiagnostics,
    /// Request Stop for the active Task.
    StopTask,
    /// Check the recorded Stop receipt.
    CheckStopReceipt,
}

impl Command {
    /// The command's frozen inventory descriptor.
    pub fn descriptor(self) -> &'static Descriptor {
        INVENTORY
            .iter()
            .find(|descriptor| descriptor.command == self)
            .expect("every command has exactly one descriptor")
    }
}

/// A typed command descriptor: stable identity plus searchable text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Descriptor {
    /// The command this descriptor names.
    pub command: Command,
    /// Stable id, never translated.
    pub id: &'static str,
    /// Human title.
    pub title: &'static str,
    /// Grouping category.
    pub category: &'static str,
}

/// The frozen inventory: every entry is backed by an existing surface action.
pub const INVENTORY: &[Descriptor] = &[
    Descriptor {
        command: Command::ShowArtifact,
        id: "surface.artifact",
        title: "成果物を表示",
        category: "移動",
    },
    Descriptor {
        command: Command::ShowCompare,
        id: "surface.compare",
        title: "候補を比較",
        category: "移動",
    },
    Descriptor {
        command: Command::ShowFlow,
        id: "surface.flow",
        title: "処理の流れを表示",
        category: "移動",
    },
    Descriptor {
        command: Command::NextSurface,
        id: "surface.next",
        title: "次の画面へ",
        category: "移動",
    },
    Descriptor {
        command: Command::OpenActiveTask,
        id: "task.open-active",
        title: "選択中のタスクを開く",
        category: "タスク",
    },
    Descriptor {
        command: Command::OpenActiveCandidate,
        id: "candidate.open-active",
        title: "選択中の候補を開く",
        category: "候補",
    },
    Descriptor {
        command: Command::ShowEvaluation,
        id: "inspector.evaluation",
        title: "評価を表示",
        category: "確認",
    },
    Descriptor {
        command: Command::ShowEvidence,
        id: "inspector.evidence",
        title: "判断の根拠を表示",
        category: "確認",
    },
    Descriptor {
        command: Command::ToggleDiagnostics,
        id: "diagnostics.toggle",
        title: "診断情報の表示を切り替え",
        category: "診断",
    },
    Descriptor {
        command: Command::StopTask,
        id: "stop.task",
        title: "タスクを停止",
        category: "タスク",
    },
    Descriptor {
        command: Command::CheckStopReceipt,
        id: "stop.check-receipt",
        title: "停止結果を確認",
        category: "タスク",
    },
];

/// One inventory row with its current availability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The inventory descriptor this row lists.
    pub descriptor: &'a Descriptor,
    /// Why the command cannot run now; `None` means available.
    pub unavailable: Option<&'a str>,
}

impl Entry<'_> {
    /// Whether this row can be invoked.
    pub fn is_available(&self) -> bool {
        self.unavailable.is_none()
    }
}

/// Frozen match tier, lowest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    /// Query equals the stable id.
    ExactId,
    /// Title starts with the query.
    TitlePrefix,
    /// Title or id contains the query.
    Substring,
    /// Only the category contains the query.
    Category,
    /// Empty query: inventory order.
    List,
}

/// One ranked row, identified by its slot in the caller's entry list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    /// Index into the entries the caller passed.
    pub slot: usize,
    /// Why it matched, for stable ordering.
    pub rank: Rank,
}

/// The palette's result state; empty states stay distinct.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Ranked rows, best first, at most [`MAX_RESULTS`].
    Hits(Vec<Hit>),
    /// A non-empty query matched nothing.
    NoMatch,
    /// No command can run in the current capability state.
    NoAvailable,
}

/// Rank the entries for `query`.
///
/// A non-empty query is folded with the same simple case folding as the rail
/// search, and each field is folded once. Ties keep inventory order, so the list
/// never reorders between identical inputs.
pub fn search(query: &str, entries: &[Entry<'_>]) -> Outcome {
    if !entries.iter().any(Entry::is_available) {
        return Outcome::NoAvailable;
    }
    let folded_query = fold(query);
    let mut hits = Vec::new();
    for (slot, entry) in entries.iter().enumerate() {
        let Some(rank) = rank(entry.descriptor, &folded_query) else {
            continue;
        };
        hits.push(Hit { slot, rank });
    }
    if hits.is_empty() {
        return Outcome::NoMatch;
    }
    hits.sort_by_key(|hit| (hit.rank, hit.slot));
    hits.truncate(MAX_RESULTS);
    Outcome::Hits(hits)
}

/// Move the active row by `delta`, clamped to the available rows.
pub fn step(active: Option<usize>, len: usize, delta: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let last = len - 1;
    match active {
        None if delta < 0 => Some(last),
        None => Some(0),
        Some(current) => {
            let current = current.min(last);
            Some(current.saturating_add_signed(delta).min(last))
        }
    }
}

fn rank(descriptor: &Descriptor, folded_query: &str) -> Option<Rank> {
    if folded_query.is_empty() {
        return Some(Rank::List);
    }
    let id = fold(descriptor.id);
    if id == folded_query {
        return Some(Rank::ExactId);
    }
    let title = fold(descriptor.title);
    if title.starts_with(folded_query) {
        return Some(Rank::TitlePrefix);
    }
    if title.contains(folded_query) || id.contains(folded_query) {
        return Some(Rank::Substring);
    }
    fold(descriptor.category)
        .contains(folded_query)
        .then_some(Rank::Category)
}

fn fold(value: &str) -> String {
    value.chars().map(simple_casefold::fold_scalar).collect()
}

#[cfg(test)]
mod tests {
    use super::{Command, Entry, INVENTORY, MAX_RESULTS, Outcome, Rank, search, step};

    fn entries() -> Vec<Entry<'static>> {
        INVENTORY
            .iter()
            .map(|descriptor| Entry {
                descriptor,
                unavailable: None,
            })
            .collect()
    }

    fn commands(outcome: Outcome, entries: &[Entry<'_>]) -> Vec<Command> {
        match outcome {
            Outcome::Hits(hits) => hits
                .iter()
                .map(|hit| entries[hit.slot].descriptor.command)
                .collect(),
            other => panic!("expected hits, got {other:?}"),
        }
    }

    #[test]
    fn every_command_has_one_descriptor_and_unique_ids() {
        assert_eq!(
            INVENTORY.len(),
            Command::CheckStopReceipt.descriptor().command as usize + 1
        );
        let mut ids: Vec<&str> = INVENTORY.iter().map(|entry| entry.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), INVENTORY.len(), "ids must be unique");
        for descriptor in INVENTORY {
            assert_eq!(descriptor.command.descriptor().id, descriptor.id);
        }
    }

    #[test]
    fn matching_contract_uses_stable_ids_and_independent_title_category_fixtures() {
        let entries = entries();
        let exact = commands(search("SURFACE.FLOW", &entries), &entries);
        assert_eq!(exact.first(), Some(&Command::ShowFlow));
        assert!(matches!(
            search("SURFACE.FLOW", &entries),
            Outcome::Hits(hits) if hits[0].rank == Rank::ExactId
        ));
        let mut title_probe = INVENTORY[0];
        title_probe.title = "needle title";
        let title_entry = [Entry {
            descriptor: &title_probe,
            unavailable: None,
        }];
        assert!(matches!(
            search("needle", &title_entry),
            Outcome::Hits(hits) if hits[0].rank == Rank::TitlePrefix
        ));
        let mut category_probe = INVENTORY[0];
        category_probe.category = "needle category";
        let category_entry = [Entry {
            descriptor: &category_probe,
            unavailable: None,
        }];
        assert!(matches!(
            search("needle", &category_entry),
            Outcome::Hits(hits) if hits[0].rank == Rank::Category
        ));
    }

    #[test]
    fn matching_folds_case_for_ascii_and_unicode_ids() {
        let entries = entries();
        let lower = commands(search("surface.flow", &entries), &entries);
        let upper = commands(search("SURFACE.FLOW", &entries), &entries);
        assert_eq!(lower, upper);
        assert_eq!(lower.first(), Some(&Command::ShowFlow));
        let mut descriptor = INVENTORY[0];
        descriptor.title = "Überblick";
        let custom = [Entry {
            descriptor: &descriptor,
            unavailable: None,
        }];
        assert_eq!(
            commands(search("über", &custom), &custom),
            vec![Command::ShowArtifact]
        );
        assert_eq!(
            commands(search("ÜBER", &custom), &custom),
            vec![Command::ShowArtifact]
        );
    }

    #[test]
    fn a_category_only_match_ranks_after_title_and_id_matches() {
        let mut descriptor = INVENTORY[0];
        descriptor.category = "needle category";
        let custom = [Entry {
            descriptor: &descriptor,
            unavailable: None,
        }];
        assert!(matches!(
            search("needle", &custom),
            Outcome::Hits(hits) if hits.len() == 1 && hits[0].rank == Rank::Category
        ));
    }

    #[test]
    fn a_matching_query_with_no_row_is_no_match_and_echoes_nothing_else() {
        let entries = entries();
        assert_eq!(search("zzzz", &entries), Outcome::NoMatch);
        assert_eq!(search("qq", &entries), Outcome::NoMatch);
    }

    #[test]
    fn an_empty_query_lists_the_whole_inventory_in_inventory_order() {
        let entries = entries();
        let listed = commands(search("", &entries), &entries);
        let expected: Vec<Command> = INVENTORY.iter().map(|entry| entry.command).collect();
        assert_eq!(listed, expected);
        assert!(matches!(
            search("", &entries),
            Outcome::Hits(hits) if hits.iter().all(|hit| hit.rank == Rank::List)
        ));
    }

    #[test]
    fn an_inventory_with_no_available_command_is_no_available_even_with_a_query() {
        let all_blocked: Vec<Entry<'static>> = INVENTORY
            .iter()
            .map(|descriptor| Entry {
                descriptor,
                unavailable: Some("capability unavailable"),
            })
            .collect();
        assert_eq!(search("", &all_blocked), Outcome::NoAvailable);
        assert_eq!(search("stop", &all_blocked), Outcome::NoAvailable);
    }

    #[test]
    fn an_unavailable_row_stays_listed_and_keeps_its_reason() {
        let mut entries = entries();
        let slot = entries
            .iter()
            .position(|entry| entry.descriptor.command == Command::StopTask)
            .expect("stop row");
        let reason = "fixture unavailable reason";
        entries[slot].unavailable = Some(reason);
        let listed = commands(search("", &entries), &entries);
        assert!(listed.contains(&Command::StopTask));
        assert!(!entries[slot].is_available());
        assert_eq!(entries[slot].unavailable, Some(reason));
    }

    #[test]
    fn results_are_bounded_and_ties_keep_inventory_order() {
        assert_eq!(
            search("", &entries()).into_hit_count(),
            INVENTORY.len().min(MAX_RESULTS)
        );
        // Same rank for all, so the order is the inventory order.
        let listed = commands(search("", &entries()), &entries());
        let expected: Vec<Command> = INVENTORY.iter().map(|entry| entry.command).collect();
        assert_eq!(listed, expected);
    }

    #[test]
    fn the_cursor_clamps_and_moves_without_wrapping() {
        assert_eq!(step(None, 0, 1), None);
        assert_eq!(step(None, 3, 1), Some(0));
        assert_eq!(step(None, 3, -1), Some(2));
        assert_eq!(step(Some(0), 3, -1), Some(0));
        assert_eq!(step(Some(2), 3, 1), Some(2));
        assert_eq!(step(Some(1), 3, 1), Some(2));
        assert_eq!(step(Some(9), 3, 1), Some(2));
    }

    trait HitCount {
        fn into_hit_count(self) -> usize;
    }

    impl HitCount for Outcome {
        fn into_hit_count(self) -> usize {
            match self {
                Outcome::Hits(hits) => hits.len(),
                _ => 0,
            }
        }
    }
}
