//! Where a keyboard list's **active descendant** is, as one pure rule.
//!
//! Three lists in the shell (the task rail, the flow node list and the flow edge list) move one
//! active row with Up/Down and Home/End, and publish that row to the accessibility tree only while
//! the list itself holds focus. The rule was written three times, so it now lives here and both the
//! movement code and the tests read it from the same place.
//!
//! Nothing here describes what a screen reader announces; that remains unmeasured.

/// The row the keyboard is on, given the rows that exist right now.
///
/// A stale key - one whose row was filtered out or removed - resolves to `None`, which is what
/// keeps a removed row from staying published.
#[cfg(test)]
pub(crate) fn active_index<K: PartialEq>(active: Option<&K>, rows: &[K]) -> Option<usize> {
    let active = active?;
    rows.iter().position(|row| row == active)
}

/// The same rule for a caller whose rows are keyed individually, which cannot hand out a slice of
/// keys and should not build one just to search it: the predicate is asked about each index in
/// order and the search stops at the first match.
pub(crate) fn position_where(len: usize, mut matches: impl FnMut(usize) -> bool) -> Option<usize> {
    (0..len).find(|index| matches(*index))
}

/// The index a Up/Down move lands on.
///
/// With no current row the move starts at the first one, which is what pressing Down in an
/// unfocused-but-actived list should do. The result is clamped into the list, so holding Down at
/// the end stays there instead of wrapping or overflowing.
pub(crate) fn stepped_index(current: Option<usize>, len: usize, delta: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let last = len - 1;
    Some(match current {
        None => 0,
        Some(current) => {
            let moved = current as isize + delta;
            moved.clamp(0, last as isize) as usize
        }
    })
}

/// Whether **this rendered row** publishes the list's active descendant.
///
/// The single decision the three lists share: the list must hold focus, the row being drawn must be
/// the active one, and the active row must still exist in this list. A row that is merely mounted,
/// or an active key whose row was filtered out, publishes nothing.
///
/// The caller passes what its render loop already knows - its index and the list length - so this
/// costs nothing to call and cannot disagree with which row the tree contains.
pub(crate) fn publishes_row<K: PartialEq>(
    focused: bool,
    active_key: Option<&K>,
    row_index: usize,
    row_key: &K,
    list_len: usize,
) -> bool {
    focused && active_key.is_some_and(|active| active == row_key) && row_index < list_len
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list of stable keys, as the shell keeps them.
    fn rows() -> Vec<&'static str> {
        vec!["a", "b", "c", "d"]
    }

    #[test]
    fn down_and_up_move_one_row_and_stop_at_the_ends() {
        let rows = rows();
        assert_eq!(
            stepped_index(active_index(Some(&"a"), &rows), rows.len(), 1),
            Some(1)
        );
        assert_eq!(
            stepped_index(active_index(Some(&"c"), &rows), rows.len(), 1),
            Some(3)
        );
        // At the end, Down stays; at the start, Up stays.
        assert_eq!(
            stepped_index(active_index(Some(&"d"), &rows), rows.len(), 1),
            Some(3)
        );
        assert_eq!(
            stepped_index(active_index(Some(&"a"), &rows), rows.len(), -1),
            Some(0)
        );
        // No current row: the move starts at the first.
        assert_eq!(stepped_index(None, rows.len(), 1), Some(0));
        assert_eq!(stepped_index(None, rows.len(), -1), Some(0));
        // An empty list has nowhere to go.
        assert_eq!(stepped_index(None, 0, 1), None);
    }

    #[test]
    fn home_and_end_land_on_the_first_and_last_row() {
        assert_eq!(crate::rail_edge_index(4, crate::RailEdge::First), Some(0));
        assert_eq!(crate::rail_edge_index(4, crate::RailEdge::Last), Some(3));
        assert_eq!(crate::rail_edge_index(0, crate::RailEdge::First), None);
        assert_eq!(crate::rail_edge_index(0, crate::RailEdge::Last), None);
    }

    #[test]
    fn the_index_form_finds_the_same_row_without_a_copy() {
        // The flow lists key their rows individually, so they search by predicate instead of
        // building a key vector for every keystroke.
        let keys = vec!["a", "b", "c", "d"];
        assert_eq!(
            position_where(keys.len(), |index| keys[index] == "c"),
            active_index(Some(&"c"), &keys)
        );
        assert_eq!(position_where(keys.len(), |index| keys[index] == "z"), None);
        // The search stops at the first match rather than scanning on.
        let mut visited = Vec::new();
        let found = position_where(keys.len(), |index| {
            visited.push(index);
            keys[index] == "b"
        });
        assert_eq!(found, Some(1));
        assert_eq!(visited, vec![0, 1]);
    }

    #[test]
    fn focus_is_not_selection() {
        // The active descendant follows focus, not the selected row: a list can hold focus on one
        // row while a different row is selected.
        assert!(publishes_row(true, Some(&1usize), 1, &1usize, 4));
        assert!(!publishes_row(true, Some(&1usize), 0, &0usize, 4));
        // With focus elsewhere nothing is published, whatever the active key says.
        assert!(!publishes_row(false, Some(&1usize), 1, &1usize, 4));
        assert!(!publishes_row(false, None, 1, &1usize, 4));
        // Focus with no active row publishes nothing.
        assert!(!publishes_row(true, None, 1, &1usize, 4));
    }

    #[test]
    fn only_the_active_row_is_published() {
        // The product calls `publishes_row` from its render loop with the row's own index and key,
        // so this test calls the same function rather than restating it.
        let rows = rows();
        let active_key = "c";
        for (index, key) in rows.iter().enumerate() {
            assert_eq!(
                publishes_row(true, Some(&active_key), index, key, rows.len()),
                *key == active_key,
                "row {key} at {index}"
            );
        }

        // With focus elsewhere nothing publishes, not even the active row.
        for (index, key) in rows.iter().enumerate() {
            assert!(!publishes_row(
                false,
                Some(&active_key),
                index,
                key,
                rows.len()
            ));
        }

        // A filter that removes the active row publishes nothing: no mounted row matches the key.
        let filtered = ["a", "b"];
        for (index, key) in filtered.iter().enumerate() {
            assert!(!publishes_row(
                true,
                Some(&active_key),
                index,
                key,
                filtered.len()
            ));
        }

        // An index past the end is not published even when the key matches.
        assert!(!publishes_row(
            true,
            Some(&active_key),
            9,
            &active_key,
            rows.len()
        ));
    }

    #[test]
    fn a_removed_or_filtered_row_is_not_published() {
        let rows = rows();
        // The row the user was on disappears from the list: the stale key no longer matches.
        let stale = active_index(Some(&"z"), &rows);
        assert_eq!(stale, None);
        // No mounted row matches the stale key.
        assert!(rows.iter().enumerate().all(|(index, key)| !publishes_row(
            true,
            Some(&"z"),
            index,
            key,
            rows.len()
        )));

        // An index past the end - the list shrank under a kept index - is not published either.
        assert!(!publishes_row(true, Some(&"a"), 9, &"a", rows.len()));
    }

    #[test]
    fn a_stale_key_restarts_at_the_first_row_instead_of_jumping() {
        let rows = rows();
        let stale = active_index(Some(&"z"), &rows);
        // Moving from a stale key lands on the first row, not on wherever the old index was.
        assert_eq!(stepped_index(stale, rows.len(), 1), Some(0));
        assert_eq!(stepped_index(stale, rows.len(), -1), Some(0));
    }

    #[test]
    fn a_reordered_list_moves_from_the_rows_current_position() {
        let before = rows();
        let index = active_index(Some(&"c"), &before);
        assert_eq!(index, Some(2));
        // The same keys come back in a different order: the active row is looked up by key again.
        let after = vec!["d", "c", "b", "a"];
        let moved = active_index(Some(&"c"), &after);
        assert_eq!(moved, Some(1));
        assert_eq!(stepped_index(moved, after.len(), 1), Some(2));
        assert_eq!(after[2], "b");
    }
}
