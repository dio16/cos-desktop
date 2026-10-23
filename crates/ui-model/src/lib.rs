//! Window-local interactions. Read-only Runtime projections stay in app-core.

pub mod presentation {
    /// A local reference to a candidate, not an artifact identity or formal selection.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct ViewedCandidate(pub desktop_protocol::CandidateRef);

    #[derive(Debug, Default)]
    pub struct Presentation {
        viewed: Option<ViewedCandidate>,
        compare: Option<[ViewedCandidate; 2]>,
        pub diagnostics_open: bool,
    }

    impl Presentation {
        pub fn view(&mut self, candidate: ViewedCandidate) {
            self.viewed = Some(candidate);
        }

        pub fn viewed(&self) -> Option<&ViewedCandidate> {
            self.viewed.as_ref()
        }

        pub fn compare(&mut self, pair: [ViewedCandidate; 2]) {
            self.compare = Some(pair);
        }

        pub fn compare_pair(&self) -> Option<&[ViewedCandidate; 2]> {
            self.compare.as_ref()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{Presentation, ViewedCandidate};
        use desktop_protocol::{ArtifactId, CandidateId, CandidateRef, RunId, TaskId};

        fn candidate(label: &str) -> ViewedCandidate {
            ViewedCandidate(CandidateRef {
                task_id: TaskId::new("fixture:task/Task-184").expect("valid task identity"),
                run_id: RunId::new("fixture:run/Task-184/1").expect("valid run identity"),
                iteration: 3,
                candidate_id: CandidateId::new(format!("fixture:candidate/{label}"))
                    .expect("valid candidate"),
                artifact_ref: ArtifactId::new(format!("fixture:artifact/{label}"))
                    .expect("valid artifact"),
            })
        }

        #[test]
        fn viewing_and_comparing_stay_independent() {
            let mut state = Presentation::default();
            let pair = [candidate("a"), candidate("b")];
            state.compare(pair.clone());
            state.view(candidate("c"));
            assert_eq!(state.compare_pair(), Some(&pair));
            assert_eq!(state.viewed(), Some(&candidate("c")));
        }
    }
}

pub struct Workbench {
    pub core: app_core::AppCore,
    pub presentation: presentation::Presentation,
    pub text: WorkbenchText,
}

/// Display-only text prepared from an immutable observation, not a second Domain model.
#[derive(Default)]
pub struct WorkbenchText {
    pub title: String,
    pub summary: String,
    pub connection: String,
    pub artifacts: Vec<(desktop_protocol::CandidateRef, String)>,
    pub evaluation: Vec<String>,
    pub graph: Vec<String>,
    pub diagnostics: String,
}

impl Workbench {
    pub fn new(core: app_core::AppCore) -> Self {
        let mut text = WorkbenchText {
            connection: core.connection_label(),
            title: "No Task observation".into(),
            diagnostics: format!(
                "Desktop {} · {} · Runtime version: not connected",
                env!("CARGO_PKG_VERSION"),
                core.source_label()
            ),
            ..Default::default()
        };
        if let Some(snapshot) = core.snapshot() {
            if let Some(task) = snapshot.tasks.values().next() {
                text.title = format!("{}  {}", task.display_id, task.title);
                text.summary = format!(
                    "{} · Presentation phase {} · Iteration {}",
                    task.task_status.as_str(),
                    task.inner_phase.as_str(),
                    task.iteration_index
                        .map_or_else(|| "unknown".into(), |index| index.to_string())
                );
                text.diagnostics = format!(
                    "Desktop {} · Source {} · Task projection revision {} · Runtime version: not connected",
                    env!("CARGO_PKG_VERSION"),
                    core.source_label(),
                    task.projection_revision.as_str()
                );
            }
            text.artifacts = snapshot
                .candidates
                .values()
                .map(|candidate| {
                    (
                        candidate.candidate_ref.clone(),
                        candidate.display_label.clone(),
                    )
                })
                .collect();
            for evaluation in snapshot.evaluations.values() {
                text.evaluation.push(format!(
                    "Decision: {} (fixture data, not evaluator output)",
                    evaluation.decision.as_str()
                ));
                for axis in &evaluation.axes {
                    text.evaluation.push(format!(
                        "{}: {} {}",
                        axis.name,
                        axis.value
                            .map_or_else(|| "unavailable".into(), |v| v.to_string()),
                        axis.unit.as_deref().unwrap_or("")
                    ));
                }
                for evidence in &evaluation.evidence_refs {
                    if let Some(summary) = &evidence.summary {
                        text.evaluation.push(format!("Evidence: {summary}"));
                    }
                    if let Some(detail) = &evidence.detail {
                        text.evaluation.push(detail.clone());
                    }
                }
            }
            text.evaluation.push(if snapshot.outcomes.is_empty() {
                "Formal Outcome: not provided by this observation".into()
            } else {
                let statuses = snapshot
                    .outcomes
                    .values()
                    .map(|outcome| outcome.formal_status.as_str())
                    .collect::<Vec<_>>();
                format!(
                    "Formal Outcome (Runtime projection): {}",
                    statuses.join(", ")
                )
            });
            for graph in snapshot.graphs.values() {
                text.graph
                    .extend(graph.nodes.iter().map(|node| node.label.clone()));
            }
        }
        Self {
            core,
            presentation: presentation::Presentation::default(),
            text,
        }
    }
}
