//! Safe, bounded diagnostics retained for independent readers.

use std::collections::VecDeque;

use crate::observation::{PublicationConnection, PublicationError};

/// Maximum number of diagnostic events retained for readers.
pub const MAX_DIAGNOSTIC_EVENTS: usize = 1024;

/// Safe category of an observation event; event payloads are intentionally omitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// An event with no separately exposed diagnostic category.
    Activity,
    /// A connection-state transition reduced to its safe public category.
    Connection(PublicationConnection),
    /// An error reduced to the existing safe publication category.
    Error(PublicationError),
    /// The client requested a full resynchronization.
    Resync,
}

/// One retained event with its process-owner-monotone sequence number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticEvent {
    /// Sequence starts at one and never wraps or reuses a value.
    pub sequence: u64,
    /// Safe category only; no remote payload or identity is retained.
    pub kind: DiagnosticKind,
}

/// Inclusive interval of events no longer retained after the reader cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventGap {
    /// First lost sequence, inclusive.
    pub from: u64,
    /// Last lost sequence, inclusive.
    pub to: u64,
}

/// Nondestructive bounded result for one reader cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticBatch {
    /// Retained events whose sequence is strictly greater than the cursor.
    pub events: Vec<DiagnosticEvent>,
    /// Exact lost interval after the cursor and before the oldest retained event.
    pub event_gap: Option<EventGap>,
    /// Latest sequence ever appended, or zero before the first event.
    pub latest_sequence: u64,
    /// True once the sequence space is exhausted; later events are not appended.
    pub exhausted: bool,
}

/// Owner-side storage shared with all observation handles.
///
/// The ring is allocated once and replaces the oldest record before appending
/// when full. Sequence exhaustion is permanent; the counter is never wrapped.
pub(crate) struct DiagnosticRing {
    events: VecDeque<DiagnosticEvent>,
    next_sequence: u64,
    latest_sequence: u64,
    exhausted: bool,
}

impl DiagnosticRing {
    pub(crate) fn new() -> Self {
        Self {
            events: VecDeque::with_capacity(MAX_DIAGNOSTIC_EVENTS),
            next_sequence: 1,
            latest_sequence: 0,
            exhausted: false,
        }
    }

    pub(crate) fn append(&mut self, kind: DiagnosticKind) {
        if self.exhausted {
            return;
        }

        if self.events.len() == MAX_DIAGNOSTIC_EVENTS {
            self.events.pop_front();
        }

        let sequence = self.next_sequence;
        self.events.push_back(DiagnosticEvent { sequence, kind });
        self.latest_sequence = sequence;
        if let Some(next_sequence) = sequence.checked_add(1) {
            self.next_sequence = next_sequence;
        } else {
            self.exhausted = true;
        }
    }

    pub(crate) fn read_after(&self, after_sequence: u64) -> DiagnosticBatch {
        let first_index = self
            .events
            .partition_point(|event| event.sequence <= after_sequence);
        let events = if first_index == self.events.len() {
            Vec::new()
        } else {
            let mut events = Vec::with_capacity(self.events.len() - first_index);
            events.extend(self.events.iter().skip(first_index).cloned());
            events
        };

        let event_gap = if after_sequence > self.latest_sequence {
            None
        } else {
            self.events.front().and_then(|oldest| {
                let last_lost = oldest.sequence.saturating_sub(1);
                (after_sequence < last_lost).then(|| EventGap {
                    from: after_sequence + 1,
                    to: last_lost,
                })
            })
        };

        DiagnosticBatch {
            events,
            event_gap,
            latest_sequence: self.latest_sequence,
            exhausted: self.exhausted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DiagnosticKind, DiagnosticRing};

    #[test]
    fn sequence_exhaustion_does_not_wrap_or_reuse_a_sequence() {
        let mut ring = DiagnosticRing::new();
        ring.next_sequence = u64::MAX;

        ring.append(DiagnosticKind::Activity);
        let max_sequence = ring.read_after(u64::MAX - 1);
        assert_eq!(max_sequence.events.len(), 1);
        assert_eq!(max_sequence.events[0].sequence, u64::MAX);
        assert_eq!(max_sequence.latest_sequence, u64::MAX);
        assert!(max_sequence.exhausted);

        ring.append(DiagnosticKind::Resync);
        let after_exhaustion = ring.read_after(u64::MAX - 1);
        assert_eq!(after_exhaustion.events.len(), 1);
        assert_eq!(after_exhaustion.events[0].sequence, u64::MAX);
        assert_eq!(after_exhaustion.latest_sequence, u64::MAX);
        assert!(after_exhaustion.exhausted);
        assert!(ring.read_after(u64::MAX).events.is_empty());
    }
}
