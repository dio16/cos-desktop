//! Window-local interaction state only; the client owns the retained command registry.
use app_core::{CommandReadError, InvalidCommandReason, PublicationError, TaggedRead};
use desktop_protocol::{CommandId, TaskId};
use ui_model::StopDisplay;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReceiptData {
    Found,
    NotRetained,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadFailure {
    Denied,
    Superseded,
    Capacity,
    Unavailable,
}

impl ReadFailure {
    pub fn from_error(error: &CommandReadError) -> Self {
        match error {
            CommandReadError::AuthorizationDenied => Self::Denied,
            CommandReadError::Superseded => Self::Superseded,
            CommandReadError::CapacityExceeded
            | CommandReadError::Client(PublicationError::InvalidCommand(
                InvalidCommandReason::CapacityExceeded | InvalidCommandReason::ReferenceLimit,
            )) => Self::Capacity,
            CommandReadError::AuthorizationUnavailable | CommandReadError::Client(_) => {
                Self::Unavailable
            }
        }
    }
}

pub(super) enum ReadCompletion {
    Data(TaggedRead<ReceiptData>),
    Failed(ReadFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReceiptFeedback {
    Idle,
    Checking,
    Found,
    NotRetained,
    PreviousFailure(ReadFailure),
}

impl ReceiptFeedback {
    pub fn requires_manual_intervention(self) -> bool {
        matches!(self, Self::NotRetained | Self::PreviousFailure(_))
    }

    pub fn note(self) -> Option<(&'static str, &'static str)> {
        let local = "Local attempt history only; current receipt and authorization are not verified. The command may have executed; do not resend.";
        Some(match self {
            Self::Idle => return None,
            Self::Checking => (
                "Receipt check in progress",
                "Explicit read only; no Stop is sent or retried.",
            ),
            Self::Found => (
                "Receipt check found a current record",
                "The current command state comes only from the authorized publication.",
            ),
            Self::NotRetained => (
                "Receipt not retained in the checked view",
                "Absence is not proof of non-execution; manual intervention may be required.",
            ),
            Self::PreviousFailure(ReadFailure::Capacity) => {
                ("Previous receipt-check attempt: capacity blocked", local)
            }
            Self::PreviousFailure(ReadFailure::Denied) => {
                ("Previous receipt-check attempt: read denied", local)
            }
            Self::PreviousFailure(ReadFailure::Unavailable) => {
                ("Previous receipt-check attempt: unavailable", local)
            }
            Self::PreviousFailure(ReadFailure::Superseded) => {
                ("Previous receipt-check result is no longer current", local)
            }
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReadAttemptId(u64);

enum ReadAttempt {
    Idle,
    Checking(ReadAttemptId),
    Finished(ReadCompletion),
}

#[derive(Clone, Debug)]
pub(super) enum StopProgress {
    Dispatching,
    Retaining,
    Retained,
    AdmissionError(StopDisplay),
    RetentionError(StopDisplay),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct IntentId(u64);

pub(super) struct ActiveStop {
    pub task: TaskId,
    pub session_generation: u64,
    pub command_id: Option<CommandId>,
    retained: bool,
    pub progress: StopProgress,
    pub retryable_admission: bool,
    intent: IntentId,
    read_attempt: ReadAttempt,
}

impl ActiveStop {
    /// Only the exact-tag value proves a current record; a failure is recorded as
    /// local attempt history, never as the current receipt or authorization.
    pub fn receipt_feedback(&self) -> ReceiptFeedback {
        match &self.read_attempt {
            ReadAttempt::Idle => ReceiptFeedback::Idle,
            ReadAttempt::Checking(_) => ReceiptFeedback::Checking,
            ReadAttempt::Finished(ReadCompletion::Failed(reason)) => {
                ReceiptFeedback::PreviousFailure(*reason)
            }
            ReadAttempt::Finished(ReadCompletion::Data(tagged)) => {
                let mut feedback = ReceiptFeedback::PreviousFailure(ReadFailure::Superseded);
                let _ = tagged.consume(|data| {
                    feedback = match data {
                        ReceiptData::Found => ReceiptFeedback::Found,
                        ReceiptData::NotRetained => ReceiptFeedback::NotRetained,
                    };
                });
                feedback
            }
        }
    }
}

/// One active UI operation, not a second registry. Replacing its target drops
/// only a window reference; it never settles or resends the client's old intent.
#[derive(Default)]
pub(super) struct StopTracker {
    pub active: Option<ActiveStop>,
    sequence: u64,
}

impl StopTracker {
    pub fn can_begin(&self) -> bool {
        self.sequence < u64::MAX
            && self
                .active
                .as_ref()
                .is_none_or(|active| active.retryable_admission && active.command_id.is_none())
    }

    /// Retire an intent that belongs to a different task or observation generation.
    pub fn synchronize(&mut self, task: &TaskId, generation: u64) -> Option<CommandId> {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.task != *task || active.session_generation != generation)
        {
            return self.release();
        }
        None
    }

    /// Return only a reference actually acquired by this window, exactly once.
    pub fn release(&mut self) -> Option<CommandId> {
        let active = self.active.take()?;
        if active.retained {
            active.command_id
        } else {
            None
        }
    }

    pub fn begin(&mut self, task: TaskId, generation: u64) -> Option<IntentId> {
        if !self.can_begin() {
            return None;
        }
        self.sequence = self.sequence.checked_add(1)?;
        let intent = IntentId(self.sequence);
        self.active = Some(ActiveStop {
            task,
            session_generation: generation,
            command_id: None,
            retained: false,
            progress: StopProgress::Dispatching,
            retryable_admission: false,
            intent,
            read_attempt: ReadAttempt::Idle,
        });
        Some(intent)
    }

    pub fn admitted(&mut self, intent: IntentId, command: CommandId) -> bool {
        let Some(active) = self
            .active
            .as_mut()
            .filter(|active| active.intent == intent)
        else {
            return false;
        };
        if active.command_id.is_some() {
            return false;
        }
        active.command_id = Some(command);
        active.progress = StopProgress::Retaining;
        active.retryable_admission = false;
        true
    }

    pub fn retained(
        &mut self,
        intent: IntentId,
        command: &CommandId,
        result: &Result<(), CommandReadError>,
    ) -> bool {
        let Some(active) = self.active.as_mut().filter(|active| {
            active.intent == intent
                && active.command_id.as_ref() == Some(command)
                && matches!(active.progress, StopProgress::Retaining)
        }) else {
            return false;
        };
        active.retained = result.is_ok();
        active.progress = match result {
            Ok(()) => StopProgress::Retained,
            Err(error) => StopProgress::RetentionError(StopDisplay::retention_error(error)),
        };
        true
    }

    pub fn refused(&mut self, intent: IntentId, error: &CommandReadError) {
        if let Some(active) = self
            .active
            .as_mut()
            .filter(|active| active.intent == intent && active.command_id.is_none())
        {
            active.retryable_admission = matches!(
                error,
                CommandReadError::CapacityExceeded
                    | CommandReadError::Client(PublicationError::InvalidCommand(
                        InvalidCommandReason::CapacityExceeded
                    ))
            );
            active.progress = StopProgress::AdmissionError(StopDisplay::admission_error(error));
        }
    }

    pub fn can_read(&self) -> bool {
        self.sequence < u64::MAX
            && self.active.as_ref().is_some_and(|active| {
                active.command_id.is_some()
                    && !matches!(&active.read_attempt, ReadAttempt::Checking(_))
            })
    }

    pub fn begin_read(&mut self) -> Option<(IntentId, ReadAttemptId, CommandId)> {
        if !self.can_read() {
            return None;
        }
        let sequence = self.sequence.checked_add(1)?;
        let active = self.active.as_mut()?;
        let command = active.command_id.clone()?;
        self.sequence = sequence;
        let attempt = ReadAttemptId(sequence);
        active.read_attempt = ReadAttempt::Checking(attempt);
        Some((active.intent, attempt, command))
    }

    pub fn read_finished(
        &mut self,
        intent: IntentId,
        attempt: ReadAttemptId,
        completion: ReadCompletion,
    ) -> bool {
        let Some(active) = self.active.as_mut().filter(|active| {
            active.intent == intent
                && matches!(&active.read_attempt, ReadAttempt::Checking(current) if *current == attempt)
        }) else {
            return false;
        };
        active.read_attempt = ReadAttempt::Finished(completion);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_switch_allows_a_new_intent_but_rejects_late_a_b_a_completions() {
        let a = TaskId::fixture("task/a");
        let b = TaskId::fixture("task/b");
        let mut tracker = StopTracker::default();
        let first = tracker.begin(a.clone(), 1).unwrap();
        assert!(
            tracker.begin(a.clone(), 1).is_none(),
            "rapid activation is single-flight"
        );
        assert!(tracker.admitted(first, CommandId::fixture("command/a-old")));
        assert!(tracker.retained(first, &CommandId::fixture("command/a-old"), &Ok(())));
        assert_eq!(
            tracker.synchronize(&b, 1),
            Some(CommandId::fixture("command/a-old"))
        );
        assert_eq!(
            tracker.synchronize(&b, 1),
            None,
            "the old credit is released once"
        );
        let second = tracker.begin(b.clone(), 1).unwrap();
        assert!(!tracker.admitted(first, CommandId::fixture("command/stale-a")));
        tracker.synchronize(&a, 1);
        let third = tracker.begin(a, 1).unwrap();
        assert!(!tracker.admitted(first, CommandId::fixture("command/stale-a-again")));
        assert!(!tracker.admitted(second, CommandId::fixture("command/stale-b")));
        assert!(tracker.admitted(third, CommandId::fixture("command/a-new")));
        assert_eq!(
            tracker.active.as_ref().unwrap().command_id,
            Some(CommandId::fixture("command/a-new"))
        );
        assert!(!tracker.can_begin());
    }

    #[test]
    fn generation_switch_and_window_close_leave_late_successful_credits_to_the_completion() {
        let task = TaskId::fixture("task/a");
        let command = CommandId::fixture("command/retaining");
        let mut tracker = StopTracker::default();
        let old = tracker.begin(task.clone(), 1).unwrap();
        assert!(tracker.admitted(old, command.clone()));
        assert_eq!(tracker.synchronize(&task, 2), None, "no credit granted yet");
        let current = tracker.begin(task, 2).unwrap();
        assert!(
            !tracker.retained(old, &command, &Ok(())),
            "caller must release the orphaned successful credit"
        );
        assert!(tracker.admitted(current, CommandId::fixture("command/current")));
        assert!(tracker.retained(current, &CommandId::fixture("command/current"), &Ok(())));
        assert_eq!(
            tracker.release(),
            Some(CommandId::fixture("command/current"))
        );
        assert_eq!(tracker.release(), None);
        assert!(!tracker.retained(current, &CommandId::fixture("command/current"), &Ok(())));
        assert!(!tracker.admitted(current, CommandId::fixture("command/closed")));
    }

    #[test]
    fn local_read_attempt_history_survives_publication_churn_and_never_becomes_receipt_truth() {
        let task = TaskId::fixture("task/a");
        let command = CommandId::fixture("command/stop");
        let mut tracker = StopTracker::default();
        let intent = tracker.begin(task.clone(), 1).unwrap();
        assert!(tracker.admitted(intent, command));
        let (checking, attempt, _) = tracker.begin_read().unwrap();
        assert_eq!(checking, intent);
        assert!(!tracker.can_read(), "one explicit read is in flight");
        assert!(
            tracker.begin_read().is_none(),
            "rapid checks are not dispatched"
        );
        assert!(tracker.read_finished(
            intent,
            attempt,
            ReadCompletion::Failed(ReadFailure::Capacity)
        ));
        assert_eq!(
            tracker.active.as_ref().unwrap().receipt_feedback(),
            ReceiptFeedback::PreviousFailure(ReadFailure::Capacity)
        );
        assert!(
            !tracker.read_finished(intent, attempt, ReadCompletion::Failed(ReadFailure::Denied)),
            "a stale completion cannot overwrite the newer local record"
        );
        let (_, second, _) = tracker.begin_read().unwrap();
        assert!(tracker.read_finished(intent, second, ReadCompletion::Failed(ReadFailure::Denied)));
        assert_eq!(
            tracker.active.as_ref().unwrap().receipt_feedback(),
            ReceiptFeedback::PreviousFailure(ReadFailure::Denied)
        );
        assert!(
            !tracker
                .active
                .as_ref()
                .unwrap()
                .receipt_feedback()
                .note()
                .is_none()
        );
        assert!(tracker.synchronize(&task, 2).is_none());
        assert!(
            tracker.active.is_none(),
            "task/generation change retires the attempt"
        );
    }
}
