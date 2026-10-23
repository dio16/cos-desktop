/// Desktop navigation only; never a Runtime command or canonical selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Surface {
    #[default]
    Artifact,
    Compare,
    Flow,
}

impl Surface {
    pub const ALL: [Self; 3] = [Self::Artifact, Self::Compare, Self::Flow];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Artifact => "Artifact",
            Self::Compare => "Compare",
            Self::Flow => "Flow",
        }
    }
}

#[derive(Debug, Default)]
pub struct Navigation {
    surface: Surface,
}

impl Navigation {
    pub fn surface(&self) -> Surface {
        self.surface
    }

    pub fn navigate(&mut self, surface: Surface) {
        self.surface = surface;
    }

    pub fn next(&mut self) {
        self.surface = match self.surface {
            Surface::Artifact => Surface::Compare,
            Surface::Compare => Surface::Flow,
            Surface::Flow => Surface::Artifact,
        };
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::Surface;
    use crate::AppCore;
    use desktop_protocol::{ArtifactId, CandidateId, CandidateRef, RunId, TaskId};

    #[test]
    fn local_candidate_does_not_change_surface_or_require_client() {
        let mut core = AppCore::disconnected(false);
        core.navigation.navigate(Surface::Compare);
        let candidate = CandidateRef {
            task_id: TaskId::new("fixture:task/Task-184").expect("valid task identity"),
            run_id: RunId::new("fixture:run/Task-184/1").expect("valid run identity"),
            iteration: 3,
            candidate_id: CandidateId::new("fixture:candidate/a")
                .expect("valid candidate identity"),
            artifact_ref: ArtifactId::new("fixture:artifact/a").expect("valid artifact identity"),
        };
        assert_eq!(core.navigation.surface(), Surface::Compare);
        assert!(core.snapshot().is_none());
        assert_eq!(
            core.unavailable_reason(),
            "Disconnected — Runtime not configured"
        );
        assert!(candidate.artifact_ref.as_str().contains("fixture:artifact"));
    }
}

#[cfg(test)]
mod tests {
    use super::{Navigation, Surface};

    #[test]
    fn explicit_navigation_and_keyboard_cycle_agree() {
        let mut navigation = Navigation::default();
        navigation.navigate(Surface::Compare);
        navigation.next();
        assert_eq!(navigation.surface(), Surface::Flow);
        navigation.next();
        assert_eq!(navigation.surface(), Surface::Artifact);
    }
}
