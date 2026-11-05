//! Read-only state information exposed to the accessibility tree.
//!
//! The owner decision of 2026-10-31 (recorded in the implementation plan, §19.104) says a task's
//! state and the current work surface are published as **read-only text** with three parts: a
//! fixed `name`, a mutable `value`, and a fixed `description` that says what the information means
//! and where it comes from. When the value changes, the element's **id, role, name and description
//! stay put** and only the value changes; a task switch or stale information never republishes the
//! previous task's value.
//!
//! Nothing here describes what a screen reader announces; that remains unmeasured.

/// One read-only piece of state, ready to be attached to an element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StateText {
    /// Stable element id: unchanged while the same information is shown.
    pub id: String,
    /// Fixed label for the kind of information.
    pub name: &'static str,
    /// The current value. This is the only part that changes.
    pub value: String,
    /// Fixed explanation of what the value means and where it comes from.
    pub description: &'static str,
}

/// How a value that is not known is shown.
///
/// A task that has just been selected, an information source that has gone stale, and a field the
/// origin never sent all read as "not determined" rather than as the previous value.
pub(crate) const NOT_DETERMINED: &str = "未確定";
/// How a value the origin reported as unavailable is shown.
pub(crate) const UNAVAILABLE: &str = "利用不可";

/// The phase text the presentation model uses when the origin projected no phase.
///
/// It is a presentation placeholder, not a phase, so it must be published as "not determined"
/// rather than as a value the origin reported.
pub(crate) const PHASE_NOT_PROJECTED: &str = "不明";

/// A field of the origin's task state, or a reason it cannot be shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StateValue {
    /// The origin reported this value.
    Reported(String),
    /// The origin has not reported it (yet).
    NotDetermined,
    /// The origin reported the information as unavailable.
    Unavailable,
}

impl StateValue {
    fn text(&self) -> String {
        match self {
            StateValue::Reported(value) => value.clone(),
            StateValue::NotDetermined => NOT_DETERMINED.to_string(),
            StateValue::Unavailable => UNAVAILABLE.to_string(),
        }
    }
}

/// The task-state rows a rail row publishes.
///
/// The ids are built from the task identity, so updating the same task keeps every id and only the
/// values move; switching tasks yields different ids, and the previous task's values cannot leak
/// through because each row is built from the state handed in here.
pub(crate) fn task_state_texts(
    task_id: &str,
    status: &StateValue,
    phase: &StateValue,
    iteration: &StateValue,
) -> Vec<StateText> {
    [
        (
            "task-state-task-status",
            "タスクの状態",
            status,
            "タスク全体の状態です。処理段階や成果物の利用状態とは別に通知されます。",
        ),
        (
            "task-state-inner-phase",
            "処理段階",
            phase,
            "実行元が通知した処理段階の表記です。未知の表記を、この画面で別の意味に置き換えることはありません。",
        ),
        (
            "task-state-iteration",
            "反復回数",
            iteration,
            "実行元が通知した現在の反復回数です。候補を閲覧しても変わりません。",
        ),
    ]
    .into_iter()
    .map(|(stem, name, value, description)| StateText {
        id: format!("{stem}-{task_id}"),
        name,
        value: value.text(),
        description,
    })
    .collect()
}

/// A rail row's state, from the values the presentation model carries.
///
/// A phase the origin never projected is `NotDetermined`, and a task the model reports as having no
/// current value is `Unavailable`: neither case repeats a previous value.
pub(crate) fn rail_row_state_texts(
    task_id: &str,
    status: &str,
    phase: Option<&str>,
    iteration: Option<u32>,
    unavailable: bool,
) -> Vec<StateText> {
    let mut texts = task_state_texts(
        task_id,
        &StateValue::Reported(status.to_string()),
        &phase
            .filter(|phase| *phase != PHASE_NOT_PROJECTED)
            .map_or(StateValue::NotDetermined, |phase| {
                StateValue::Reported(phase.to_string())
            }),
        &iteration.map_or(StateValue::NotDetermined, |iteration| {
            StateValue::Reported(iteration.to_string())
        }),
    );
    // A missing candidate reference is **its own** fact about this row's presentation: it says
    // nothing about the task's own status, which the origin still reports.
    if unavailable {
        texts.push(StateText {
            id: format!("task-state-reference-{task_id}"),
            name: "候補の参照",
            value: StateValue::Unavailable.text(),
            description: "この行が参照している候補を、いまは取得できていないことを示します。タスクの状態とは別の情報です。",
        });
    }
    texts
}

/// The work-surface rows the current surface publishes.
///
/// The id depends on the surface, not on its state, so switching what the surface is showing
/// changes the value while the element identity follows the surface itself.
pub(crate) fn surface_state_texts(
    surface_slug: &str,
    surface_name: &str,
    state: &StateValue,
) -> Vec<StateText> {
    let _ = surface_name;
    vec![
        StateText {
            id: format!("surface-state-name-{surface_slug}"),
            name: "表示中の画面",
            value: surface_name.to_string(),
            description: "いま表示している画面の名前です。閲覧中のタスクとは別に管理されます。",
        },
        StateText {
            id: format!("surface-state-state-{surface_slug}"),
            name: "画面の表示",
            value: state.text(),
            description: "この画面がいま表示されているというローカルの事実です。実行元が通知するタスクの状態とは別の軸で、利用可能性の判定ではありません。",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(texts: &[StateText]) -> Vec<(&'static str, String)> {
        texts
            .iter()
            .map(|text| (text.name, text.value.clone()))
            .collect()
    }

    #[test]
    fn an_update_keeps_every_identity_and_moves_only_the_value() {
        let before = task_state_texts(
            "task-1",
            &StateValue::Reported("実行中".into()),
            &StateValue::Reported("生成".into()),
            &StateValue::Reported("2".into()),
        );
        let after = task_state_texts(
            "task-1",
            &StateValue::Reported("完了".into()),
            &StateValue::Reported("生成".into()),
            &StateValue::Reported("3".into()),
        );
        for (before, after) in before.iter().zip(&after) {
            assert_eq!(before.id, after.id, "the element id must not move");
            assert_eq!(before.name, after.name);
            assert_eq!(before.description, after.description);
        }
        assert_ne!(values(&before), values(&after), "the values must change");
        assert_eq!(before[0].value, "実行中");
        assert_eq!(after[0].value, "完了");
        assert_eq!(after[2].value, "3");
    }

    #[test]
    fn a_different_task_gets_different_ids() {
        let first = task_state_texts(
            "task-1",
            &StateValue::Reported("実行中".into()),
            &StateValue::NotDetermined,
            &StateValue::NotDetermined,
        );
        let second = task_state_texts(
            "task-2",
            &StateValue::Reported("実行中".into()),
            &StateValue::NotDetermined,
            &StateValue::NotDetermined,
        );
        for (a, b) in first.iter().zip(&second) {
            assert_ne!(
                a.id, b.id,
                "a different task must not reuse the same element"
            );
        }
    }

    #[test]
    fn undetermined_and_unavailable_never_repeat_the_old_value() {
        // A task switch, a stale source or a field the origin never sent reads as "not determined".
        let switched = task_state_texts(
            "task-1",
            &StateValue::NotDetermined,
            &StateValue::NotDetermined,
            &StateValue::NotDetermined,
        );
        for text in &switched {
            assert_eq!(text.value, NOT_DETERMINED);
            assert_ne!(text.value, "実行中");
        }
        // An origin that reported the information as unavailable says exactly that.
        let unavailable = task_state_texts(
            "task-1",
            &StateValue::Unavailable,
            &StateValue::Unavailable,
            &StateValue::Unavailable,
        );
        for text in &unavailable {
            assert_eq!(text.value, UNAVAILABLE);
        }
    }

    #[test]
    fn a_row_without_a_projected_phase_says_so_instead_of_inventing_one() {
        let texts = rail_row_state_texts("task-1", "実行中", None, None, false);
        assert_eq!(texts[0].value, "実行中");
        assert_eq!(texts[1].value, NOT_DETERMINED);
        assert_eq!(texts[2].value, NOT_DETERMINED);

        let reported = rail_row_state_texts("task-1", "実行中", Some("生成"), Some(2), false);
        assert_eq!(reported[1].value, "生成");
        assert_eq!(reported[2].value, "2");
        // The identity does not move between the two states.
        for (a, b) in texts.iter().zip(&reported) {
            assert_eq!(a.id, b.id);
        }
    }

    #[test]
    fn a_missing_reference_is_its_own_fact_and_leaves_the_task_state_alone() {
        let texts = rail_row_state_texts("task-1", "実行中", Some("生成"), Some(2), true);
        // The task's own reported state is still published as reported.
        assert_eq!(texts[0].value, "実行中");
        assert_eq!(texts[1].value, "生成");
        assert_eq!(texts[2].value, "2");
        // The missing reference is a separate row, with its own identity.
        let reference = texts
            .iter()
            .find(|text| text.name == "候補の参照")
            .expect("a missing reference is published");
        assert_eq!(reference.value, UNAVAILABLE);
        assert!(reference.id.ends_with("task-1"));

        // Without the condition that row does not exist at all.
        let reported = rail_row_state_texts("task-1", "実行中", Some("生成"), Some(2), false);
        assert!(reported.iter().all(|text| text.name != "候補の参照"));
        // The task's own rows keep their identity across the two states.
        for (a, b) in texts.iter().zip(&reported) {
            assert_eq!(a.id, b.id);
        }
    }

    #[test]
    fn a_surface_switch_keeps_the_element_ids_of_the_information_rows() {
        // The ids follow the surface, so each surface keeps its own elements and a state change on
        // the same surface moves only the value.
        let idle =
            surface_state_texts("artifact", "成果物", &StateValue::Reported("表示中".into()));
        let busy = surface_state_texts(
            "artifact",
            "成果物",
            &StateValue::Reported("読み込み中".into()),
        );
        for (a, b) in idle.iter().zip(&busy) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.name, b.name);
            assert_eq!(a.description, b.description);
        }
        assert_eq!(idle[0].name, "表示中の画面");
        assert_eq!(idle[0].value, "成果物");
        assert_ne!(idle[1].value, busy[1].value);

        // A different surface is a different element, not a reused one.
        let other =
            surface_state_texts("flow", "処理の流れ", &StateValue::Reported("表示中".into()));
        assert_ne!(idle[0].id, other[0].id);
        assert_eq!(other[0].value, "処理の流れ");
    }
}
