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
            // An absent capabilities_ref means the Runtime did not project the
            // fact; only a declared reference can dangle.
            if let Some(capabilities_ref) = &task.capabilities_ref
                && self.capabilities(capabilities_ref).is_none()
            {
                return Err(ProjectionIntegrityError::new(format!(
                    "task {} references absent capabilities {}",
                    task.task_ref, capabilities_ref
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

#[cfg(test)]
mod recorded_observation_v1 {
    //! MP-RT1: the recorded Runtime grant must pass the same snapshot-entry
    //! validation a live `SNAPSHOT_END` takes. The fixture is a byte-identical
    //! copy of the Runtime recording (`fixtures/desktop-observation-v1/`).

    use super::ProjectionState;
    use desktop_protocol::{
        HelloAccepted, ProjectionMutation, SnapshotChunk, SnapshotCounts, SnapshotEnd,
        projection_bundle_digest,
    };
    use serde::Deserialize;

    /// One recorded exchange envelope: `{name, request, response, body}`.
    #[derive(Deserialize)]
    struct RecordedEnvelope {
        name: String,
        response: RecordedResponse,
        body: GrantDocument,
    }

    #[derive(Deserialize)]
    struct RecordedResponse {
        status: u16,
    }

    #[derive(Deserialize)]
    struct GrantDocument {
        schema_version: u32,
        server: HelloAccepted,
        snapshot: SnapshotDocument,
    }

    #[derive(Deserialize)]
    struct SnapshotDocument {
        chunks: Vec<SnapshotChunk>,
        end: SnapshotEnd,
    }

    const RECORDED_GRANT: &str =
        include_str!("../fixtures/desktop-observation-v1/observation-grant.json");

    #[test]
    fn recorded_grant_snapshot_validates_without_dangling_references() {
        let envelope: RecordedEnvelope =
            serde_json::from_str(RECORDED_GRANT).expect("the recorded grant parses");
        assert_eq!(envelope.name, "observation-grant");
        assert_eq!(envelope.response.status, 200);
        let document = envelope.body;
        assert_eq!(document.schema_version, 1);
        let mutations: Vec<ProjectionMutation> = document
            .snapshot
            .chunks
            .iter()
            .flat_map(|chunk| chunk.mutations.clone())
            .collect();
        // `from_snapshot` runs the same entry and integrity validation a live
        // snapshot end takes, including every dangling-reference check.
        let state = ProjectionState::from_snapshot(&document.server.epoch, &mutations)
            .expect("the recorded snapshot validates");
        assert_eq!(
            state.tasks.len(),
            document.snapshot.end.expected.tasks as usize
        );
        assert_eq!(
            SnapshotCounts::of(&mutations),
            document.snapshot.end.expected
        );
        assert_eq!(
            projection_bundle_digest(&mutations).expect("canonical digest"),
            document.snapshot.end.integrity_digest
        );
        assert!(
            state.tasks.values().all(|task| task.iteration_ref.is_none()
                && task.best_candidate_ref.is_none()
                && task.formal_outcome_ref.is_none()
                && task.accepted_brief_ref.is_none()
                && task.capabilities_ref.is_none()),
            "v1 projects TASK only, so no recorded task declares a reference"
        );
    }
}

#[cfg(test)]
mod integrity_tests {
    use super::{ProjectionKey, ProjectionState};
    use desktop_protocol::{Counter, EpochId, Projection, ProjectionMutation};

    const GRANT: &str = include_str!("../fixtures/desktop-observation-v1/observation-grant.json");

    /// The recorded snapshot's mutations, which the existing test already proves valid.
    fn recorded() -> (EpochId, Vec<ProjectionMutation>) {
        let document: serde_json::Value =
            serde_json::from_str(GRANT).expect("the recorded grant parses");
        let epoch = EpochId::new(
            document["body"]["server"]["epoch"]
                .as_str()
                .expect("an epoch"),
        )
        .expect("a valid epoch");
        let mutations: Vec<ProjectionMutation> = document["body"]["snapshot"]["chunks"]
            .as_array()
            .expect("chunks")
            .iter()
            .flat_map(|chunk| {
                chunk["mutations"]
                    .as_array()
                    .expect("mutations")
                    .iter()
                    .map(|mutation| serde_json::from_value(mutation.clone()).expect("a mutation"))
                    .collect::<Vec<_>>()
            })
            .collect();
        (epoch, mutations)
    }

    fn populated() -> (ProjectionState, Vec<ProjectionMutation>) {
        let (epoch, mutations) = recorded();
        let state = ProjectionState::from_snapshot(&epoch, &mutations).expect("valid snapshot");
        (state, mutations)
    }

    /// The first upsert and its creation form, for the checks below.
    fn first_upsert(mutations: &[ProjectionMutation]) -> (Option<Counter>, ProjectionMutation) {
        for mutation in mutations {
            if let ProjectionMutation::Upsert {
                base_revision,
                projection,
            } = mutation
            {
                return (
                    base_revision.clone(),
                    ProjectionMutation::Upsert {
                        base_revision: base_revision.clone(),
                        projection: projection.clone(),
                    },
                );
            }
        }
        panic!("the recorded snapshot contains an upsert");
    }

    /// Rebuild the recorded projection with a different revision.
    ///
    /// The fixture's projections are task snapshots; anything else is a fixture change this test does
    /// not know how to rebuild, so it says so instead of silently skipping.
    fn with_revision(
        projection: &desktop_protocol::Projection,
        revision: Counter,
    ) -> desktop_protocol::Projection {
        match projection {
            desktop_protocol::Projection::Task(snapshot) => {
                let mut next = snapshot.clone();
                next.projection_revision = revision;
                desktop_protocol::Projection::Task(next)
            }
            other => panic!("the fixture projection is a task snapshot, got {other:?}"),
        }
    }

    #[test]
    fn the_recorded_upsert_is_a_creation_and_is_accepted_by_an_empty_state() {
        // The recorded snapshot is a full state, so its upserts create. This is the positive control
        // for the three refusals below: the same mutation validates when it is being created.
        let (epoch, mutations) = recorded();
        let empty = ProjectionState::from_snapshot(&epoch, &[]).expect("an empty snapshot");
        let (base, upsert) = first_upsert(&mutations);
        assert!(base.is_none(), "the recorded upsert declares creation");
        assert!(
            empty.validate_mutation(&upsert).is_ok(),
            "a creation is accepted by an empty state"
        );
    }

    #[test]
    fn a_creation_upsert_is_refused_when_the_projection_already_exists() {
        // Replaying a creation over an existing projection would overwrite authoritative state, so it
        // is refused.
        let (state, mutations) = populated();
        let (_base, upsert) = first_upsert(&mutations);
        let error = state
            .validate_mutation(&upsert)
            .expect_err("a creation over an existing projection must be refused");
        assert!(
            format!("{error}").contains("declares creation"),
            "the refusal names the cause: {error}"
        );
    }

    #[test]
    fn an_update_upsert_is_refused_when_the_projection_is_absent() {
        // The mirror case: a mutation that claims to update something the state does not hold. The
        // recorded creation is turned into an update for this check.
        let (epoch, mutations) = recorded();
        let empty = ProjectionState::from_snapshot(&epoch, &[]).expect("an empty snapshot");
        let (_base, mut upsert) = first_upsert(&mutations);
        if let ProjectionMutation::Upsert { base_revision, .. } = &mut upsert {
            *base_revision = Some(Counter::new("1").expect("a counter"));
        }
        let error = empty
            .validate_mutation(&upsert)
            .expect_err("an update against an absent projection must be refused");
        assert!(
            format!("{error}").contains("is absent"),
            "the refusal names the cause: {error}"
        );
    }

    #[test]
    fn a_base_revision_that_does_not_match_the_stored_one_is_refused() {
        // Optimistic concurrency: the mutation must name the revision it was computed against. A
        // different base means the peer built on a state this client never had, so the update is
        // refused even though the projection exists.
        let (state, mutations) = populated();
        let (_base, mut upsert) = first_upsert(&mutations);
        if let ProjectionMutation::Upsert { base_revision, .. } = &mut upsert {
            *base_revision = Some(Counter::new("999999999").expect("a counter"));
        }
        let error = state
            .validate_mutation(&upsert)
            .expect_err("a mismatched base must be refused");
        assert!(
            format!("{error}").contains("does not match stored revision"),
            "the refusal names the cause: {error}"
        );
    }

    #[test]
    fn a_matching_base_with_a_newer_revision_is_a_valid_update() {
        // The rule has a **positive** case as well: an update that names the stored revision and
        // moves it forward must be accepted, or the refusals above would be indistinguishable from a
        // rule that refuses every update.
        let (state, mutations) = populated();
        let (_base, upsert) = first_upsert(&mutations);
        let ProjectionMutation::Upsert { projection, .. } = &upsert else {
            panic!("the recorded upsert is an upsert");
        };
        let key = ProjectionKey::of_projection(projection);
        let stored = state
            .revisions
            .get(&key)
            .cloned()
            .expect("the state holds the projection");
        let newer = Counter::new(
            stored
                .as_str()
                .parse::<u128>()
                .expect("a counter")
                .saturating_add(1)
                .to_string(),
        )
        .expect("a counter");
        let valid = ProjectionMutation::Upsert {
            base_revision: Some(stored),
            projection: with_revision(projection, newer),
        };
        assert!(
            state.validate_mutation(&valid).is_ok(),
            "a matching base with a newer revision is accepted"
        );
    }

    #[test]
    fn an_update_whose_revision_does_not_move_forward_is_refused() {
        // Base match is not enough: the new revision must be strictly newer. Equal and decreasing are
        // both refused, so a replay cannot masquerade as progress.
        let (state, mutations) = populated();
        let (_base, upsert) = first_upsert(&mutations);
        let ProjectionMutation::Upsert { projection, .. } = &upsert else {
            panic!("the recorded upsert is an upsert");
        };
        let key = ProjectionKey::of_projection(projection);
        let stored = state
            .revisions
            .get(&key)
            .cloned()
            .expect("the state holds the projection");

        for (label, revision) in [
            ("equal", stored.clone()),
            (
                "lower",
                Counter::new(
                    stored
                        .as_str()
                        .parse::<u128>()
                        .expect("a counter")
                        .saturating_sub(1)
                        .to_string(),
                )
                .expect("a counter"),
            ),
        ] {
            let mutation = ProjectionMutation::Upsert {
                base_revision: Some(stored.clone()),
                projection: with_revision(projection, revision),
            };
            let error = state
                .validate_mutation(&mutation)
                .expect_err("a revision that does not move forward is refused");
            assert!(
                format!("{error}").contains("is not newer than base"),
                "the {label} revision must be refused for not moving forward, got {error}"
            );
        }
    }

    #[test]
    fn a_removal_needs_an_existing_projection_and_the_stored_base() {
        // `Remove` carries the same optimistic-concurrency rule as an update: the target must exist
        // and the base must be the stored revision. Neither branch had a test.
        let (state, mutations) = populated();
        let (_base, upsert) = first_upsert(&mutations);
        let ProjectionMutation::Upsert { projection, .. } = &upsert else {
            panic!("the recorded upsert is an upsert");
        };
        let key = ProjectionKey::of_projection(projection);
        let stored = state
            .revisions
            .get(&key)
            .cloned()
            .expect("the state holds the projection");
        let target = match &key {
            ProjectionKey::Task(task_id) => desktop_protocol::RemoveTarget::Task {
                task_id: task_id.clone(),
            },
            other => panic!("the fixture projection is a task, got {other:?}"),
        };

        // Accepted: existing projection, matching base.
        let accepted = ProjectionMutation::Remove {
            base_revision: stored.clone(),
            target: target.clone(),
        };
        assert!(
            state.validate_mutation(&accepted).is_ok(),
            "a removal of an existing projection with the stored base is accepted"
        );

        // Refused: matching base but the projection is not there.
        let (epoch, all) = recorded();
        let empty = ProjectionState::from_snapshot(&epoch, &[]).expect("an empty snapshot");
        let absent = ProjectionMutation::Remove {
            base_revision: stored.clone(),
            target: target.clone(),
        };
        let error = empty
            .validate_mutation(&absent)
            .expect_err("a removal of an absent projection is refused");
        assert!(
            format!("{error}").contains("expects an existing projection"),
            "the refusal names the cause: {error}"
        );

        // Refused: existing projection, wrong base.
        let wrong = ProjectionMutation::Remove {
            base_revision: Counter::new("999999999").expect("a counter"),
            target,
        };
        let error = state
            .validate_mutation(&wrong)
            .expect_err("a removal with the wrong base is refused");
        assert!(
            format!("{error}").contains("does not match stored revision"),
            "the refusal names the cause: {error}"
        );
        let _ = all;
    }

    #[test]
    fn applying_a_validated_update_moves_the_projection_and_its_revision() {
        // Validation and application must agree: after a mutation the validator accepted, the derived
        // view must show the new revision, and the next update must then be measured against it. A
        // validator that accepted an update the store then ignored would leave the two disagreeing.
        let (mut state, mutations) = populated();
        let (_base, upsert) = first_upsert(&mutations);
        let ProjectionMutation::Upsert { projection, .. } = &upsert else {
            panic!("the recorded upsert is an upsert");
        };
        let Projection::Task(snapshot) = projection else {
            panic!("the fixture projection is a task snapshot");
        };
        let task_id = snapshot.task_ref.clone();
        let key = ProjectionKey::of_projection(projection);
        let stored = state
            .revisions
            .get(&key)
            .cloned()
            .expect("the state holds the projection");
        let newer = Counter::new(
            stored
                .as_str()
                .parse::<u128>()
                .expect("a counter")
                .saturating_add(1)
                .to_string(),
        )
        .expect("a counter");
        let mutation = ProjectionMutation::Upsert {
            base_revision: Some(stored),
            projection: with_revision(projection, newer.clone()),
        };
        state
            .validate_mutation(&mutation)
            .expect("a matching base with a newer revision is accepted");
        state.apply(std::slice::from_ref(&mutation));
        assert_eq!(
            state
                .task(&task_id)
                .map(|task| task.projection_revision.clone()),
            Some(newer),
            "the applied update is visible in the derived view"
        );
        // And the same mutation is now stale, because the revision moved.
        assert!(
            state.validate_mutation(&mutation).is_err(),
            "replaying the accepted update is refused against the new state"
        );
    }

    #[test]
    fn applying_a_validated_removal_drops_the_projection_and_its_revision() {
        // The removal half: the projection disappears and its revision bookkeeping goes with it, so a
        // later creation is accepted rather than treated as an update against a stale revision.
        let (mut state, mutations) = populated();
        let (_base, upsert) = first_upsert(&mutations);
        let ProjectionMutation::Upsert { projection, .. } = &upsert else {
            panic!("the recorded upsert is an upsert");
        };
        let Projection::Task(snapshot) = projection else {
            panic!("the fixture projection is a task snapshot");
        };
        let task_id = snapshot.task_ref.clone();
        let key = ProjectionKey::of_projection(projection);
        let stored = state
            .revisions
            .get(&key)
            .cloned()
            .expect("the state holds the projection");
        let removal = ProjectionMutation::Remove {
            base_revision: stored,
            target: desktop_protocol::RemoveTarget::Task {
                task_id: task_id.clone(),
            },
        };
        state
            .validate_mutation(&removal)
            .expect("the removal matches the stored revision");
        state.apply(std::slice::from_ref(&removal));
        assert!(
            state.task(&task_id).is_none(),
            "the removed projection is gone from the derived view"
        );
        assert!(
            !state.revisions.contains_key(&key),
            "its revision bookkeeping is gone too"
        );
        // A creation is now accepted, which is only true if the revision was dropped with it.
        let recreation = ProjectionMutation::Upsert {
            base_revision: None,
            projection: projection.clone(),
        };
        assert!(
            state.validate_mutation(&recreation).is_ok(),
            "after a removal the projection may be created again"
        );
    }

    /// How an upsert and a removal of the same key resolve in the overlay.
    ///
    /// The rule is per key and **independent of argument order**: an upsert shadows a removal of the
    /// same key, while a removal hides an entry no upsert recreates. Asserting the overlay's answer
    /// directly is what makes this meaningful - a validator that merely returns `Ok` would not show
    /// which way the key resolved.
    #[test]
    fn an_upsert_shadows_a_removal_of_the_same_key() {
        let (state, mutations) = populated();
        let (_base, upsert) = first_upsert(&mutations);
        let ProjectionMutation::Upsert { projection, .. } = &upsert else {
            panic!("the recorded upsert is an upsert");
        };
        let Projection::Task(snapshot) = projection else {
            panic!("the fixture projection is a task snapshot");
        };
        let task_id = snapshot.task_ref.clone();
        let key = ProjectionKey::of_projection(projection);
        let stored = state
            .revisions
            .get(&key)
            .cloned()
            .expect("the state holds the projection");
        let target = desktop_protocol::RemoveTarget::Task {
            task_id: task_id.clone(),
        };
        // The upsert carries a **different** revision, so "the overlay answered with the base entry"
        // is distinguishable from "it answered with the upsert".
        let bumped_revision = Counter::new(
            stored
                .as_str()
                .parse::<u128>()
                .expect("a counter")
                .saturating_add(1)
                .to_string(),
        )
        .expect("a counter");
        let bumped = with_revision(projection, bumped_revision.clone());
        let both = super::EffectiveState {
            base: &state,
            upserts: std::slice::from_ref(&bumped),
            removals: std::slice::from_ref(&target),
        };
        assert_eq!(
            both.task(&task_id)
                .map(|task| task.projection_revision.clone()),
            Some(bumped_revision.clone()),
            "the overlay must answer with the upsert, not the base entry"
        );
        // The aggregate iterator the integrity rules use must agree with the single-key answer, and
        // must yield the task **once**.
        let listed: Vec<_> = both
            .tasks()
            .filter(|task| task.task_ref == task_id)
            .map(|task| task.projection_revision.clone())
            .collect();
        assert_eq!(
            listed,
            vec![bumped_revision],
            "the aggregate iterator yields the upserted task exactly once"
        );
        // And the validator agrees with the overlay.
        state
            .validate_effective_state(std::slice::from_ref(&bumped), std::slice::from_ref(&target))
            .expect("an upsert shadows the removal of the same key");
    }

    #[test]
    fn a_removal_alone_hides_an_existing_entry() {
        // Without an upsert the removal must take effect, which is the other half of the same rule.
        let (state, mutations) = populated();
        let (_base, upsert) = first_upsert(&mutations);
        let ProjectionMutation::Upsert { projection, .. } = &upsert else {
            panic!("the recorded upsert is an upsert");
        };
        let Projection::Task(snapshot) = projection else {
            panic!("the fixture projection is a task snapshot");
        };
        let task_id = snapshot.task_ref.clone();
        let target = desktop_protocol::RemoveTarget::Task {
            task_id: task_id.clone(),
        };
        let removal_only = super::EffectiveState {
            base: &state,
            upserts: &[],
            removals: std::slice::from_ref(&target),
        };
        assert!(
            removal_only.task(&task_id).is_none(),
            "a removal hides an entry that no upsert recreates"
        );
        // The aggregate iterator must hide it too: an integrity rule that iterated `tasks()` would
        // otherwise still see the entry the single-key answer already hides.
        assert!(
            removal_only.tasks().all(|task| task.task_ref != task_id),
            "the aggregate iterator must not yield the removed task"
        );
        // The same key is still present in the base state, so the answer above came from the overlay
        // rather than from the entry never existing.
        assert!(
            state.task(&task_id).is_some(),
            "the base state still holds it"
        );
        // An unrelated key is untouched by the removal.
        let other = state
            .tasks
            .keys()
            .find(|id| **id != task_id)
            .cloned()
            .expect("the fixture observes more than one task");
        assert!(
            removal_only.task(&other).is_some(),
            "the removal must not hide a different key"
        );
        // The recorded fixture carries no outcome for this task, so the removal is also internally
        // valid - which is the case the aggregate check is meant to allow.
        assert!(
            state.outcome_for_task(&task_id).is_none(),
            "the fixture carries no outcome for the first task"
        );
        state
            .validate_effective_state(&[], std::slice::from_ref(&target))
            .expect("a removal without a dangling reference is valid");
    }

    /// Every dangling-reference rule in the aggregate check, exercised one at a time.
    ///
    /// The recorded-fixture test proves the rules pass on a consistent snapshot; these cases prove
    /// each rule **refuses** when the edge it guards is broken. Without them, a rule that silently
    /// stopped checking would look identical to a rule that never needed to.
    #[test]
    fn a_broken_reference_edge_is_refused_by_the_aggregate_check() {
        let (state, mutations) = populated();
        let (_base, upsert) = first_upsert(&mutations);
        let ProjectionMutation::Upsert { projection, .. } = &upsert else {
            panic!("the recorded upsert is an upsert");
        };
        let Projection::Task(task) = projection else {
            panic!("the fixture projection is a task snapshot");
        };

        // A task whose declared iteration is not in the state: the edge the first rule guards.
        let absent_iteration =
            desktop_protocol::IterationId::new("fixture:absent-iteration").expect("an id");
        let mut broken = task.clone();
        broken.iteration_ref = Some(absent_iteration);
        let error = state
            .validate_effective_state(std::slice::from_ref(&Projection::Task(broken)), &[])
            .expect_err("a task referencing an absent iteration must be refused");
        assert!(
            format!("{error}").contains("references absent iteration"),
            "the refusal names the cause: {error}"
        );

        // A best candidate owned by a different task: the tuple rule, before the existence rule.
        let other_task = state
            .tasks
            .keys()
            .find(|id| **id != task.task_ref)
            .cloned()
            .expect("the fixture observes more than one task");
        let mut mismatched = task.clone();
        mismatched.best_candidate_ref = Some(desktop_protocol::CandidateRef {
            task_id: other_task,
            run_id: desktop_protocol::RunId::new("fixture:run").expect("an id"),
            iteration: 0,
            artifact_ref: desktop_protocol::ArtifactId::new("fixture:candidate-artifact")
                .expect("an id"),
            candidate_id: desktop_protocol::CandidateId::new("fixture:candidate").expect("an id"),
        });
        let error = state
            .validate_effective_state(std::slice::from_ref(&Projection::Task(mismatched)), &[])
            .expect_err("a best candidate owned by another task must be refused");
        assert!(
            format!("{error}").contains("owned by"),
            "the refusal names the cause: {error}"
        );

        // A declared reference to a projection that is simply **not in the state**: the edge dangles
        // on its own, with no removal involved. (A removal in the same transaction would make the
        // edge dangle too, through the overlay, but that is a different case and is not built here.)
        for (label, field) in [
            ("formal outcome", "outcome"),
            ("accepted brief", "artifact"),
            ("capabilities", "capabilities"),
        ] {
            let mut referencing = task.clone();
            match field {
                "outcome" => {
                    referencing.formal_outcome_ref = Some(
                        desktop_protocol::OutcomeId::new("fixture:absent-outcome").expect("an id"),
                    );
                }
                "artifact" => {
                    referencing.accepted_brief_ref = Some(
                        desktop_protocol::ArtifactId::new("fixture:absent-artifact")
                            .expect("an id"),
                    );
                }
                _ => {
                    referencing.capabilities_ref = Some(
                        desktop_protocol::CapabilitiesId::new("fixture:absent-capabilities")
                            .expect("an id"),
                    );
                }
            }
            let error = state
                .validate_effective_state(std::slice::from_ref(&Projection::Task(referencing)), &[])
                .expect_err("a declared reference to an absent projection must be refused");
            assert!(
                format!("{error}").contains("references absent"),
                "the {label} reference must be refused, got {error}"
            );
        }
    }

    /// A snapshot that actually contains the projections below the task.
    ///
    /// The recorded fixture is made of **task projections only**, so the candidate, artifact,
    /// evaluation and iteration edges have no coverage from it at all. This document is the smallest
    /// consistent snapshot that exercises them; every case below breaks one edge in it.
    const MIXED_SNAPSHOT: &str = r#"[
      {"op": "UPSERT", "projection": {"projection": "TASK",
        "task_ref": "fixture:task", "title": "t", "task_status": "ACTIVE",
        "inner_phase": "RUNNING", "projection_revision": "1",
        "iteration_ref": "fixture:iteration"}},
      {"op": "UPSERT", "projection": {"projection": "ITERATION",
        "iteration_ref": "fixture:iteration", "task_ref": "fixture:task", "index": 0,
        "candidate_refs": [], "projection_revision": "1"}},
      {"op": "UPSERT", "projection": {"projection": "ARTIFACT",
        "artifact_ref": "fixture:artifact", "artifact_digest": "sha256:00",
        "availability": "AVAILABLE", "projection_revision": "1"}},
      {"op": "UPSERT", "projection": {"projection": "CANDIDATE",
        "candidate_ref": {"task_id": "fixture:task", "run_id": "fixture:run",
          "iteration": 0, "artifact_ref": "fixture:artifact",
          "candidate_id": "fixture:candidate"},
        "task_ref": "fixture:task", "iteration_ref": "fixture:iteration",
        "artifact_refs": ["fixture:artifact"], "display_label": "A",
        "projection_revision": "1"}},
      {"op": "UPSERT", "projection": {"projection": "EVALUATION",
        "evaluation_ref": "fixture:evaluation",
        "candidate_ref": {"task_id": "fixture:task", "run_id": "fixture:run",
          "iteration": 0, "artifact_ref": "fixture:artifact",
          "candidate_id": "fixture:candidate"},
        "artifact_refs": ["fixture:artifact"], "decision": "PASS",
        "authority": "fixture", "projection_revision": "1"}}
    ]"#;

    fn mixed() -> (ProjectionState, EpochId) {
        let epoch = EpochId::new("fixture:epoch").expect("an epoch");
        let mutations: Vec<ProjectionMutation> =
            serde_json::from_str(MIXED_SNAPSHOT).expect("the mixed snapshot parses");
        let state = ProjectionState::from_snapshot(&epoch, &mutations).expect("it is consistent");
        (state, epoch)
    }

    #[test]
    fn the_mixed_snapshot_is_consistent_to_begin_with() {
        // The positive control: every case below must fail **because of the edge it breaks**, not
        // because the starting document is already invalid.
        let (state, _epoch) = mixed();
        assert_eq!(state.tasks.len(), 1);
        assert_eq!(state.candidates.len(), 1);
        assert_eq!(state.artifacts.len(), 1);
        assert_eq!(state.evaluations.len(), 1);
        assert_eq!(state.iterations.len(), 1);
        state
            .validate_effective_state(&[], &[])
            .expect("the mixed snapshot is consistent");
    }

    #[test]
    fn a_broken_edge_below_the_task_is_refused() {
        let (state, _epoch) = mixed();
        let candidate = state
            .candidates
            .values()
            .next()
            .expect("the mixed snapshot carries a candidate")
            .clone();
        let evaluation = state
            .evaluations
            .values()
            .next()
            .expect("the mixed snapshot carries an evaluation")
            .clone();
        let iteration = state
            .iterations
            .values()
            .next()
            .expect("the mixed snapshot carries an iteration")
            .clone();

        // A candidate whose tuple disagrees with the task it claims to belong to.
        let mut mismatched = candidate.clone();
        mismatched.candidate_ref.task_id =
            desktop_protocol::TaskId::new("fixture:another-task").expect("an id");
        let error = state
            .validate_effective_state(
                std::slice::from_ref(&Projection::Candidate(mismatched)),
                &[],
            )
            .expect_err("a mismatched candidate tuple must be refused");
        assert!(
            format!("{error}").contains("mismatched task tuple"),
            "the refusal names the cause: {error}"
        );

        // A candidate referencing an artifact that is not there.
        let mut dangling_artifact = candidate.clone();
        dangling_artifact
            .artifact_refs
            .push(desktop_protocol::ArtifactId::new("fixture:absent-artifact").expect("an id"));
        let error = state
            .validate_effective_state(
                std::slice::from_ref(&Projection::Candidate(dangling_artifact)),
                &[],
            )
            .expect_err("a candidate referencing an absent artifact must be refused");
        assert!(
            format!("{error}").contains("references absent artifact"),
            "the refusal names the cause: {error}"
        );

        // A candidate referencing an evaluation that is not there.
        let mut dangling_evaluation = candidate.clone();
        dangling_evaluation
            .evaluation_refs
            .push(desktop_protocol::EvaluationId::new("fixture:absent-evaluation").expect("an id"));
        let error = state
            .validate_effective_state(
                std::slice::from_ref(&Projection::Candidate(dangling_evaluation)),
                &[],
            )
            .expect_err("a candidate referencing an absent evaluation must be refused");
        assert!(
            format!("{error}").contains("references absent evaluation"),
            "the refusal names the cause: {error}"
        );

        // An evaluation referencing a candidate that is not there.
        let mut absent_candidate = evaluation.clone();
        absent_candidate.candidate_ref.candidate_id =
            desktop_protocol::CandidateId::new("fixture:absent-candidate").expect("an id");
        let error = state
            .validate_effective_state(
                std::slice::from_ref(&Projection::Evaluation(absent_candidate)),
                &[],
            )
            .expect_err("an evaluation referencing an absent candidate must be refused");
        assert!(
            format!("{error}").contains("references absent candidate"),
            "the refusal names the cause: {error}"
        );

        // An iteration listing a candidate it does not own: the listed candidate **exists**, so the
        // existence rule passes and only the ownership rule can refuse this.
        let mut second_iteration = iteration.clone();
        second_iteration.iteration_ref =
            desktop_protocol::IterationId::new("fixture:iteration-2").expect("an id");
        second_iteration.index = 1;
        second_iteration.candidate_refs = vec![candidate.candidate_ref.clone()];
        let error = state
            .validate_effective_state(
                std::slice::from_ref(&Projection::Iteration(second_iteration)),
                &[],
            )
            .expect_err("a candidate owned by another iteration must be refused");
        assert!(
            format!("{error}").contains("owned by another iteration"),
            "the refusal names the cause: {error}"
        );
    }

    #[test]
    fn the_recorded_snapshot_is_the_positive_control() {
        // Building the state from these mutations runs the same integrity checks a live snapshot end
        // runs, so a state that builds is a state that validates. This is what makes the refusals
        // above meaningful rather than vacuous.
        let (_state, mutations) = populated();
        assert_eq!(
            mutations.len(),
            39,
            "the recorded snapshot carries thirty-nine mutations"
        );
    }
}
