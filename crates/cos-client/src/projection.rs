//! Authoritative projection store.
//!
//! The store is immutable to consumers: it is published behind an
//! [`std::sync::Arc`] and replaced atomically. It carries only server-projected
//! state — there is no viewed/compare/selection field, and best-so-far or
//! formal selection is never derived from the candidate list.

use std::collections::BTreeMap;
use std::fmt;

use desktop_protocol::{
    ArtifactId, ArtifactSnapshot, CandidateRef, CandidateSnapshot, CapabilitiesId, Counter,
    CreativeCapabilities, EpochId, EvaluationId, EvaluationSnapshot, GraphId, GraphSnapshot,
    IterationId, IterationSnapshot, OutcomeId, OutcomeSnapshot, Projection, ProjectionKind,
    ProjectionMutation, RemoveTarget, TaskId, TaskSnapshot,
};

/// Stable key of an authoritative projection.
///
/// A projection key is `(entity kind, canonical entity id)`; the projection kind
/// and entity kind coincide for the bootstrap vocabulary.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProjectionKey {
    /// Task.
    Task(TaskId),
    /// Candidate tuple.
    Candidate(CandidateRef),
    /// Artifact.
    Artifact(ArtifactId),
    /// Evaluation.
    Evaluation(EvaluationId),
    /// Iteration.
    Iteration(IterationId),
    /// Outcome.
    Outcome(OutcomeId),
    /// Graph.
    Graph(GraphId),
    /// Capabilities.
    Capabilities(CapabilitiesId),
}

impl ProjectionKey {
    /// Projection kind of this key.
    pub fn kind(&self) -> ProjectionKind {
        match self {
            Self::Task(_) => ProjectionKind::Task,
            Self::Candidate(_) => ProjectionKind::Candidate,
            Self::Artifact(_) => ProjectionKind::Artifact,
            Self::Evaluation(_) => ProjectionKind::Evaluation,
            Self::Iteration(_) => ProjectionKind::Iteration,
            Self::Outcome(_) => ProjectionKind::Outcome,
            Self::Graph(_) => ProjectionKind::Graph,
            Self::Capabilities(_) => ProjectionKind::Capabilities,
        }
    }

    /// Key of a projection payload.
    pub fn of_projection(projection: &Projection) -> Self {
        match projection {
            Projection::Task(snapshot) => Self::Task(snapshot.task_ref.clone()),
            Projection::Candidate(snapshot) => Self::Candidate(snapshot.candidate_ref.clone()),
            Projection::Artifact(snapshot) => Self::Artifact(snapshot.artifact_ref.clone()),
            Projection::Evaluation(snapshot) => Self::Evaluation(snapshot.evaluation_ref.clone()),
            Projection::Iteration(snapshot) => Self::Iteration(snapshot.iteration_ref.clone()),
            Projection::Outcome(snapshot) => Self::Outcome(snapshot.outcome_ref.clone()),
            Projection::Graph(snapshot) => Self::Graph(snapshot.graph_ref.graph_id.clone()),
            Projection::Capabilities(snapshot) => {
                Self::Capabilities(snapshot.capabilities_ref.clone())
            }
        }
    }

    /// Key of a removal target.
    pub fn of_remove_target(target: &RemoveTarget) -> Self {
        match target {
            RemoveTarget::Task { task_id } => Self::Task(task_id.clone()),
            RemoveTarget::Candidate { candidate_ref } => Self::Candidate(candidate_ref.clone()),
            RemoveTarget::Artifact { artifact_id } => Self::Artifact(artifact_id.clone()),
            RemoveTarget::Evaluation { evaluation_id } => Self::Evaluation(evaluation_id.clone()),
            RemoveTarget::Iteration { iteration_id } => Self::Iteration(iteration_id.clone()),
            RemoveTarget::Outcome { outcome_id } => Self::Outcome(outcome_id.clone()),
            RemoveTarget::Graph { graph_id } => Self::Graph(graph_id.clone()),
            RemoveTarget::Capabilities { capabilities_id } => {
                Self::Capabilities(capabilities_id.clone())
            }
        }
    }
}

/// Integrity failure of a snapshot bundle or mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionIntegrityError {
    message: String,
}

impl ProjectionIntegrityError {
    /// Create an integrity error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ProjectionIntegrityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProjectionIntegrityError {}

/// Authoritative projection state.
///
/// All maps are public for read-only rendering; mutation happens inside the
/// client and consumers receive the whole state behind an `Arc`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectionState {
    /// Runtime epoch the projections belong to.
    pub epoch: Option<EpochId>,
    /// Tasks by canonical identity.
    pub tasks: BTreeMap<TaskId, TaskSnapshot>,
    /// Candidates by canonical tuple.
    pub candidates: BTreeMap<CandidateRef, CandidateSnapshot>,
    /// Artifacts by canonical identity.
    pub artifacts: BTreeMap<ArtifactId, ArtifactSnapshot>,
    /// Evaluations by identity.
    pub evaluations: BTreeMap<EvaluationId, EvaluationSnapshot>,
    /// Iterations by identity.
    pub iterations: BTreeMap<IterationId, IterationSnapshot>,
    /// Outcomes by identity.
    pub outcomes: BTreeMap<OutcomeId, OutcomeSnapshot>,
    /// Graphs by identity.
    pub graphs: BTreeMap<GraphId, GraphSnapshot>,
    /// Capabilities by identity.
    pub capabilities: BTreeMap<CapabilitiesId, CreativeCapabilities>,
    /// Projection revisions by key, used for base-revision validation.
    pub revisions: BTreeMap<ProjectionKey, Counter>,
}

impl ProjectionState {
    /// Look up a task.
    pub fn task(&self, id: &TaskId) -> Option<&TaskSnapshot> {
        self.tasks.get(id)
    }

    /// Look up an outcome.
    pub fn outcome(&self, id: &OutcomeId) -> Option<&OutcomeSnapshot> {
        self.outcomes.get(id)
    }

    /// Formal outcome of a task, when the task projects one.
    pub fn outcome_for_task(&self, task_id: &TaskId) -> Option<&OutcomeSnapshot> {
        let task = self.tasks.get(task_id)?;
        let outcome_ref = task.formal_outcome_ref.as_ref()?;
        self.outcomes.get(outcome_ref)
    }

    /// Candidates belonging to a task (presentation filter; never a ranking).
    pub fn candidates_for_task<'a>(
        &'a self,
        task_id: &'a TaskId,
    ) -> impl Iterator<Item = &'a CandidateSnapshot> + 'a {
        self.candidates
            .values()
            .filter(move |candidate| &candidate.task_ref == task_id)
    }

    /// Iterations belonging to a task.
    pub fn iterations_for_task<'a>(
        &'a self,
        task_id: &'a TaskId,
    ) -> impl Iterator<Item = &'a IterationSnapshot> + 'a {
        self.iterations
            .values()
            .filter(move |iteration| &iteration.task_ref == task_id)
    }

    /// Current revision of a projection key.
    pub fn revision(&self, key: &ProjectionKey) -> Option<&Counter> {
        self.revisions.get(key)
    }

    /// Build a complete state from snapshot creations.
    pub(crate) fn from_snapshot(
        epoch: &EpochId,
        mutations: &[ProjectionMutation],
    ) -> Result<Self, ProjectionIntegrityError> {
        let mut state = Self {
            epoch: Some(epoch.clone()),
            ..Self::default()
        };
        for mutation in mutations {
            let ProjectionMutation::Upsert {
                base_revision,
                projection,
            } = mutation
            else {
                return Err(ProjectionIntegrityError::new(
                    "a snapshot bundle must contain creations only",
                ));
            };
            if base_revision.is_some() {
                return Err(ProjectionIntegrityError::new(
                    "snapshot creations must not declare a base revision",
                ));
            }
            let key = ProjectionKey::of_projection(projection);
            if state.revisions.contains_key(&key) {
                return Err(ProjectionIntegrityError::new(format!(
                    "duplicate snapshot entry for {} projection",
                    key.kind()
                )));
            }
            state.insert_projection(projection.clone());
        }
        state.validate_integrity()?;
        Ok(state)
    }

    /// Verify that every interdependent reference resolves inside this state.
    pub(crate) fn validate_integrity(&self) -> Result<(), ProjectionIntegrityError> {
        EffectiveState::current(self).validate()
    }

    /// Validate the effective state a transaction of `upserts` and `removals`
    /// would produce, without applying it.
    ///
    /// The aggregate rules are shared with [`Self::validate_integrity`]; they
    /// read the transaction through a borrowed view, so a key that is both
    /// upserted and removed is decided by the effective-state rule and never by
    /// argument order.
    pub(crate) fn validate_effective_state(
        &self,
        upserts: &[Projection],
        removals: &[RemoveTarget],
    ) -> Result<(), ProjectionIntegrityError> {
        EffectiveState {
            base: self,
            upserts,
            removals,
        }
        .validate()
    }

    /// Validate one mutation against current state without applying it.
    pub(crate) fn validate_mutation(
        &self,
        mutation: &ProjectionMutation,
    ) -> Result<(), ProjectionIntegrityError> {
        match mutation {
            ProjectionMutation::Upsert {
                base_revision,
                projection,
            } => {
                let key = ProjectionKey::of_projection(projection);
                let stored = self.revisions.get(&key);
                match (base_revision, stored) {
                    (None, None) => {}
                    (None, Some(_)) => {
                        return Err(ProjectionIntegrityError::new(format!(
                            "{} upsert declares creation but the projection exists",
                            key.kind()
                        )));
                    }
                    (Some(base), None) => {
                        return Err(ProjectionIntegrityError::new(format!(
                            "{} upsert expects base revision {} but the projection is absent",
                            key.kind(),
                            base
                        )));
                    }
                    (Some(base), Some(stored)) => {
                        if base != stored {
                            return Err(ProjectionIntegrityError::new(format!(
                                "{} upsert base revision {} does not match stored revision {}",
                                key.kind(),
                                base,
                                stored
                            )));
                        }
                    }
                }
                if let Some(base) = base_revision {
                    match projection.revision().numerically_gt(base) {
                        Some(true) => {}
                        Some(false) => {
                            return Err(ProjectionIntegrityError::new(format!(
                                "{} upsert revision {} is not newer than base {}",
                                key.kind(),
                                projection.revision(),
                                base
                            )));
                        }
                        None => {
                            return Err(ProjectionIntegrityError::new(format!(
                                "{} upsert revision exceeds comparable range",
                                key.kind()
                            )));
                        }
                    }
                }
                Ok(())
            }
            ProjectionMutation::Remove {
                base_revision,
                target,
            } => {
                let key = ProjectionKey::of_remove_target(target);
                match self.revisions.get(&key) {
                    None => Err(ProjectionIntegrityError::new(format!(
                        "{} removal expects an existing projection",
                        key.kind()
                    ))),
                    Some(stored) if stored == base_revision => Ok(()),
                    Some(stored) => Err(ProjectionIntegrityError::new(format!(
                        "{} removal base revision {} does not match stored revision {}",
                        key.kind(),
                        base_revision,
                        stored
                    ))),
                }
            }
        }
    }

    /// Apply already-validated mutations.
    pub(crate) fn apply(&mut self, mutations: &[ProjectionMutation]) {
        for mutation in mutations {
            match mutation {
                ProjectionMutation::Upsert { projection, .. } => {
                    self.insert_projection(projection.clone());
                }
                ProjectionMutation::Remove { target, .. } => {
                    let key = ProjectionKey::of_remove_target(target);
                    self.remove_key(&key);
                }
            }
        }
    }

    /// Current state as snapshot creations (used by the fixture provider).
    pub(crate) fn snapshot_mutations(&self) -> Vec<ProjectionMutation> {
        let mut mutations = Vec::new();
        let mut push = |projection: Projection| {
            mutations.push(ProjectionMutation::Upsert {
                base_revision: None,
                projection,
            });
        };
        for task in self.tasks.values() {
            push(Projection::Task(task.clone()));
        }
        for candidate in self.candidates.values() {
            push(Projection::Candidate(candidate.clone()));
        }
        for artifact in self.artifacts.values() {
            push(Projection::Artifact(artifact.clone()));
        }
        for evaluation in self.evaluations.values() {
            push(Projection::Evaluation(evaluation.clone()));
        }
        for iteration in self.iterations.values() {
            push(Projection::Iteration(iteration.clone()));
        }
        for outcome in self.outcomes.values() {
            push(Projection::Outcome(outcome.clone()));
        }
        for graph in self.graphs.values() {
            push(Projection::Graph(graph.clone()));
        }
        for capabilities in self.capabilities.values() {
            push(Projection::Capabilities(capabilities.clone()));
        }
        mutations
    }

    fn insert_projection(&mut self, projection: Projection) {
        let key = ProjectionKey::of_projection(&projection);
        let revision = projection.revision().clone();
        match projection {
            Projection::Task(snapshot) => {
                self.tasks.insert(snapshot.task_ref.clone(), snapshot);
            }
            Projection::Candidate(snapshot) => {
                self.candidates
                    .insert(snapshot.candidate_ref.clone(), snapshot);
            }
            Projection::Artifact(snapshot) => {
                self.artifacts
                    .insert(snapshot.artifact_ref.clone(), snapshot);
            }
            Projection::Evaluation(snapshot) => {
                self.evaluations
                    .insert(snapshot.evaluation_ref.clone(), snapshot);
            }
            Projection::Iteration(snapshot) => {
                self.iterations
                    .insert(snapshot.iteration_ref.clone(), snapshot);
            }
            Projection::Outcome(snapshot) => {
                self.outcomes.insert(snapshot.outcome_ref.clone(), snapshot);
            }
            Projection::Graph(snapshot) => {
                self.graphs
                    .insert(snapshot.graph_ref.graph_id.clone(), snapshot);
            }
            Projection::Capabilities(snapshot) => {
                self.capabilities
                    .insert(snapshot.capabilities_ref.clone(), snapshot);
            }
        }
        self.revisions.insert(key, revision);
    }

    fn remove_key(&mut self, key: &ProjectionKey) {
        match key {
            ProjectionKey::Task(task_id) => {
                self.tasks.remove(task_id);
            }
            ProjectionKey::Candidate(candidate_ref) => {
                self.candidates.remove(candidate_ref);
            }
            ProjectionKey::Artifact(artifact_id) => {
                self.artifacts.remove(artifact_id);
            }
            ProjectionKey::Evaluation(evaluation_id) => {
                self.evaluations.remove(evaluation_id);
            }
            ProjectionKey::Iteration(iteration_id) => {
                self.iterations.remove(iteration_id);
            }
            ProjectionKey::Outcome(outcome_id) => {
                self.outcomes.remove(outcome_id);
            }
            ProjectionKey::Graph(graph_id) => {
                self.graphs.remove(graph_id);
            }
            ProjectionKey::Capabilities(capabilities_id) => {
                self.capabilities.remove(capabilities_id);
            }
        }
        self.revisions.remove(key);
    }
}

/// Borrowed view of the effective state a transaction would produce.
///
/// The view overlays a transaction's `upserts` and `removals` onto the current
/// store without cloning it. Each key is resolved on its own: an upsert shadows
/// a removal of the same key and any existing entry, while a removal hides an
/// existing entry only when no upsert recreates it. Consulting both slices per
/// key makes the result independent of transaction argument order.
struct EffectiveState<'a> {
    base: &'a ProjectionState,
    upserts: &'a [Projection],
    removals: &'a [RemoveTarget],
}

impl<'a> EffectiveState<'a> {
    /// Effective state of the current store with no pending transaction.
    fn current(base: &'a ProjectionState) -> Self {
        Self {
            base,
            upserts: &[],
            removals: &[],
        }
    }

    /// Upsert of a key inside the transaction, when present.
    fn upserted(&self, key: &ProjectionKey) -> Option<&Projection> {
        self.upserts
            .iter()
            .find(|projection| ProjectionKey::of_projection(projection) == *key)
    }

    /// Whether the transaction removes a key.
    fn is_removed(&self, key: &ProjectionKey) -> bool {
        self.removals
            .iter()
            .any(|target| ProjectionKey::of_remove_target(target) == *key)
    }

    /// Effective task, when present.
    fn task(&self, id: &TaskId) -> Option<&TaskSnapshot> {
        let key = ProjectionKey::Task(id.clone());
        if let Some(Projection::Task(snapshot)) = self.upserted(&key) {
            return Some(snapshot);
        }
        if self.is_removed(&key) {
            return None;
        }
        self.base.tasks.get(id)
    }

    /// Effective candidate, when present.
    fn candidate(&self, candidate_ref: &CandidateRef) -> Option<&CandidateSnapshot> {
        let key = ProjectionKey::Candidate(candidate_ref.clone());
        if let Some(Projection::Candidate(snapshot)) = self.upserted(&key) {
            return Some(snapshot);
        }
        if self.is_removed(&key) {
            return None;
        }
        self.base.candidates.get(candidate_ref)
    }

    /// Effective artifact, when present.
    fn artifact(&self, id: &ArtifactId) -> Option<&ArtifactSnapshot> {
        let key = ProjectionKey::Artifact(id.clone());
        if let Some(Projection::Artifact(snapshot)) = self.upserted(&key) {
            return Some(snapshot);
        }
        if self.is_removed(&key) {
            return None;
        }
        self.base.artifacts.get(id)
    }

    /// Effective evaluation, when present.
    fn evaluation(&self, id: &EvaluationId) -> Option<&EvaluationSnapshot> {
        let key = ProjectionKey::Evaluation(id.clone());
        if let Some(Projection::Evaluation(snapshot)) = self.upserted(&key) {
            return Some(snapshot);
        }
        if self.is_removed(&key) {
            return None;
        }
        self.base.evaluations.get(id)
    }

    /// Effective iteration, when present.
    fn iteration(&self, id: &IterationId) -> Option<&IterationSnapshot> {
        let key = ProjectionKey::Iteration(id.clone());
        if let Some(Projection::Iteration(snapshot)) = self.upserted(&key) {
            return Some(snapshot);
        }
        if self.is_removed(&key) {
            return None;
        }
        self.base.iterations.get(id)
    }

    /// Effective outcome, when present.
    fn outcome(&self, id: &OutcomeId) -> Option<&OutcomeSnapshot> {
        let key = ProjectionKey::Outcome(id.clone());
        if let Some(Projection::Outcome(snapshot)) = self.upserted(&key) {
            return Some(snapshot);
        }
        if self.is_removed(&key) {
            return None;
        }
        self.base.outcomes.get(id)
    }

    /// Effective capabilities, when present.
    fn capabilities(&self, id: &CapabilitiesId) -> Option<&CreativeCapabilities> {
        let key = ProjectionKey::Capabilities(id.clone());
        if let Some(Projection::Capabilities(snapshot)) = self.upserted(&key) {
            return Some(snapshot);
        }
        if self.is_removed(&key) {
            return None;
        }
        self.base.capabilities.get(id)
    }

    /// Effective tasks: the transaction's upserts, then retained base entries.
    fn tasks(&self) -> impl Iterator<Item = &TaskSnapshot> + '_ {
        let upserted = self
            .upserts
            .iter()
            .filter_map(|projection| match projection {
                Projection::Task(snapshot) => Some(snapshot),
                _ => None,
            });
        let retained = self.base.tasks.values().filter(|snapshot| {
            let key = ProjectionKey::Task(snapshot.task_ref.clone());
            self.upserted(&key).is_none() && !self.is_removed(&key)
        });
        upserted.chain(retained)
    }

    /// Effective candidates.
    fn candidates(&self) -> impl Iterator<Item = &CandidateSnapshot> + '_ {
        let upserted = self
            .upserts
            .iter()
            .filter_map(|projection| match projection {
                Projection::Candidate(snapshot) => Some(snapshot),
                _ => None,
            });
        let retained = self.base.candidates.values().filter(|snapshot| {
            let key = ProjectionKey::Candidate(snapshot.candidate_ref.clone());
            self.upserted(&key).is_none() && !self.is_removed(&key)
        });
        upserted.chain(retained)
    }

    /// Effective evaluations.
    fn evaluations(&self) -> impl Iterator<Item = &EvaluationSnapshot> + '_ {
        let upserted = self
            .upserts
            .iter()
            .filter_map(|projection| match projection {
                Projection::Evaluation(snapshot) => Some(snapshot),
                _ => None,
            });
        let retained = self.base.evaluations.values().filter(|snapshot| {
            let key = ProjectionKey::Evaluation(snapshot.evaluation_ref.clone());
            self.upserted(&key).is_none() && !self.is_removed(&key)
        });
        upserted.chain(retained)
    }

    /// Effective iterations.
    fn iterations(&self) -> impl Iterator<Item = &IterationSnapshot> + '_ {
        let upserted = self
            .upserts
            .iter()
            .filter_map(|projection| match projection {
                Projection::Iteration(snapshot) => Some(snapshot),
                _ => None,
            });
        let retained = self.base.iterations.values().filter(|snapshot| {
            let key = ProjectionKey::Iteration(snapshot.iteration_ref.clone());
            self.upserted(&key).is_none() && !self.is_removed(&key)
        });
        upserted.chain(retained)
    }

    /// Effective outcomes.
    fn outcomes(&self) -> impl Iterator<Item = &OutcomeSnapshot> + '_ {
        let upserted = self
            .upserts
            .iter()
            .filter_map(|projection| match projection {
                Projection::Outcome(snapshot) => Some(snapshot),
                _ => None,
            });
        let retained = self.base.outcomes.values().filter(|snapshot| {
            let key = ProjectionKey::Outcome(snapshot.outcome_ref.clone());
            self.upserted(&key).is_none() && !self.is_removed(&key)
        });
        upserted.chain(retained)
    }

    /// Effective graphs.
    fn graphs(&self) -> impl Iterator<Item = &GraphSnapshot> + '_ {
        let upserted = self
            .upserts
            .iter()
            .filter_map(|projection| match projection {
                Projection::Graph(snapshot) => Some(snapshot),
                _ => None,
            });
        let retained = self.base.graphs.values().filter(|snapshot| {
            let key = ProjectionKey::Graph(snapshot.graph_ref.graph_id.clone());
            self.upserted(&key).is_none() && !self.is_removed(&key)
        });
        upserted.chain(retained)
    }

    /// Verify that every interdependent reference resolves inside this view.
    fn validate(&self) -> Result<(), ProjectionIntegrityError> {
        for task in self.tasks() {
            if let Some(iteration_ref) = &task.iteration_ref
                && self.iteration(iteration_ref).is_none()
            {
                return Err(ProjectionIntegrityError::new(format!(
                    "task {} references absent iteration {}",
                    task.task_ref, iteration_ref
                )));
            }
            if let Some(candidate_ref) = &task.best_candidate_ref {
                if candidate_ref.task_id != task.task_ref {
                    return Err(ProjectionIntegrityError::new(format!(
                        "task {} projects a best candidate owned by {}",
                        task.task_ref, candidate_ref.task_id
                    )));
                }
                if self.candidate(candidate_ref).is_none() {
                    return Err(ProjectionIntegrityError::new(format!(
                        "task {} references absent candidate {}",
                        task.task_ref, candidate_ref.candidate_id
                    )));
                }
            }
            if let Some(outcome_ref) = &task.formal_outcome_ref
                && self.outcome(outcome_ref).is_none()
            {
                return Err(ProjectionIntegrityError::new(format!(
                    "task {} references absent outcome {}",
                    task.task_ref, outcome_ref
                )));
            }
            if let Some(artifact_ref) = &task.accepted_brief_ref
                && self.artifact(artifact_ref).is_none()
            {
                return Err(ProjectionIntegrityError::new(format!(
                    "task {} references absent brief artifact {}",
                    task.task_ref, artifact_ref
                )));
            }
            if self.capabilities(&task.capabilities_ref).is_none() {
                return Err(ProjectionIntegrityError::new(format!(
                    "task {} references absent capabilities {}",
                    task.task_ref, task.capabilities_ref
                )));
            }
        }
        for candidate in self.candidates() {
            if candidate.candidate_ref.task_id != candidate.task_ref {
                return Err(ProjectionIntegrityError::new(format!(
                    "candidate {} carries a mismatched task tuple",
                    candidate.candidate_ref.candidate_id
                )));
            }
            if self.task(&candidate.task_ref).is_none() {
                return Err(ProjectionIntegrityError::new(format!(
                    "candidate {} references absent task {}",
                    candidate.candidate_ref.candidate_id, candidate.task_ref
                )));
            }
            if self.iteration(&candidate.iteration_ref).is_none() {
                return Err(ProjectionIntegrityError::new(format!(
                    "candidate {} references absent iteration {}",
                    candidate.candidate_ref.candidate_id, candidate.iteration_ref
                )));
            }
            for artifact_ref in &candidate.artifact_refs {
                if self.artifact(artifact_ref).is_none() {
                    return Err(ProjectionIntegrityError::new(format!(
                        "candidate {} references absent artifact {}",
                        candidate.candidate_ref.candidate_id, artifact_ref
                    )));
                }
            }
            for evaluation_ref in &candidate.evaluation_refs {
                if self.evaluation(evaluation_ref).is_none() {
                    return Err(ProjectionIntegrityError::new(format!(
                        "candidate {} references absent evaluation {}",
                        candidate.candidate_ref.candidate_id, evaluation_ref
                    )));
                }
            }
        }
        for evaluation in self.evaluations() {
            if self.candidate(&evaluation.candidate_ref).is_none() {
                return Err(ProjectionIntegrityError::new(format!(
                    "evaluation {} references absent candidate {}",
                    evaluation.evaluation_ref, evaluation.candidate_ref.candidate_id
                )));
            }
            for artifact_ref in &evaluation.artifact_refs {
                if self.artifact(artifact_ref).is_none() {
                    return Err(ProjectionIntegrityError::new(format!(
                        "evaluation {} references absent artifact {}",
                        evaluation.evaluation_ref, artifact_ref
                    )));
                }
            }
        }
        for iteration in self.iterations() {
            if self.task(&iteration.task_ref).is_none() {
                return Err(ProjectionIntegrityError::new(format!(
                    "iteration {} references absent task {}",
                    iteration.iteration_ref, iteration.task_ref
                )));
            }
            for candidate_ref in &iteration.candidate_refs {
                match self.candidate(candidate_ref) {
                    None => {
                        return Err(ProjectionIntegrityError::new(format!(
                            "iteration {} references absent candidate {}",
                            iteration.iteration_ref, candidate_ref.candidate_id
                        )));
                    }
                    Some(candidate) => {
                        if candidate.iteration_ref != iteration.iteration_ref {
                            return Err(ProjectionIntegrityError::new(format!(
                                "iteration {} lists candidate {} owned by another iteration",
                                iteration.iteration_ref, candidate_ref.candidate_id
                            )));
                        }
                    }
                }
            }
        }
        for outcome in self.outcomes() {
            if self.task(&outcome.task_ref).is_none() {
                return Err(ProjectionIntegrityError::new(format!(
                    "outcome {} references absent task {}",
                    outcome.outcome_ref, outcome.task_ref
                )));
            }
        }
        for graph in self.graphs() {
            let mut node_ids: Vec<_> = graph.nodes.iter().map(|node| &node.node_ref).collect();
            let node_count = node_ids.len();
            node_ids.sort();
            node_ids.dedup();
            if node_ids.len() != node_count {
                return Err(ProjectionIntegrityError::new(format!(
                    "graph {} contains duplicate inspection nodes",
                    graph.graph_ref.graph_id
                )));
            }
            // Reuse the sorted, unique borrowed keys as a graph-local ordered
            // index. Keep edge order and source-before-target error precedence.
            for edge in &graph.edges {
                if node_ids.binary_search(&&edge.from).is_err() {
                    return Err(ProjectionIntegrityError::new(format!(
                        "graph {} edge references absent source node",
                        graph.graph_ref.graph_id
                    )));
                }
                if node_ids.binary_search(&&edge.to).is_err() {
                    return Err(ProjectionIntegrityError::new(format!(
                        "graph {} edge references absent target node",
                        graph.graph_ref.graph_id
                    )));
                }
            }
        }
        Ok(())
    }
}
