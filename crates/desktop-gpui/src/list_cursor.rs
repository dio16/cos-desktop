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

/// Whether the active descendant should be published for this list.
///
/// It is published only when the list **holds keyboard focus** and the active row **still exists**.
/// Selection is a different thing and is not consulted here: a row can be selected while focus is
/// elsewhere, and a row can be active without being selected.
pub(crate) fn publishes_active_descendant(
    focused: bool,
    active: Option<usize>,
    len: usize,
) -> bool {
    focused && active.is_some_and(|index| index < len)
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
        assert!(publishes_active_descendant(true, Some(1), 4));
        assert!(publishes_active_descendant(true, Some(0), 4));
        // With focus elsewhere nothing is published, whatever the active index says.
        assert!(!publishes_active_descendant(false, Some(1), 4));
        assert!(!publishes_active_descendant(false, None, 4));
        // Focus with no active row publishes nothing.
        assert!(!publishes_active_descendant(true, None, 4));
    }

    #[test]
    fn the_published_row_is_the_one_the_render_loop_mounted() {
        // The render loop knows its row index and the list length; the gate must agree with what
        // the tree actually contains, which is what these two calls give it.
        let rows = rows();
        for (index, key) in rows.iter().enumerate() {
            let mounted = position_where(rows.len(), |row_index| row_index == index);
            assert_eq!(mounted, Some(index));
            // The mounted row publishes only while the list holds focus.
            assert!(publishes_active_descendant(true, mounted, rows.len()));
            assert!(!publishes_active_descendant(false, mounted, rows.len()));
            // And the row it publishes is the row at that index, not a fixed one.
            assert_eq!(rows[index], *key);
        }
        // A filter shrinks the list under a kept index: nothing is published for the old index.
        let filtered = ["a"];
        assert_eq!(
            position_where(filtered.len(), |row_index| row_index == 3),
            None
        );
        assert!(!publishes_active_descendant(
            true,
            position_where(filtered.len(), |row_index| row_index == 3),
            filtered.len()
        ));
    }

    #[test]
    fn a_removed_or_filtered_row_is_not_published() {
        let rows = rows();
        // The row the user was on disappears from the list: the stale key no longer matches.
        let stale = active_index(Some(&"z"), &rows);
        assert_eq!(stale, None);
        assert!(!publishes_active_descendant(true, stale, rows.len()));

        // An index past the end - the list shrank under a kept index - is not published either.
        assert!(!publishes_active_descendant(true, Some(9), rows.len()));
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
