//! Safe, bounded diagnostics retained for independent readers.

use std::collections::VecDeque;

use crate::observation::{PublicationConnection, PublicationError};

/// Maximum number of diagnostic events retained for readers.
///
/// The owner keeps one fixed ring; when it is full the oldest event is evicted
/// before the next is appended, so the retained count never exceeds this value.
pub const MAX_DIAGNOSTIC_EVENTS: usize = 4096;

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

/// Counts per safe category across the ring's retained events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticCounts {
    /// Events without a separate category.
    pub activity: usize,
    /// Connection transitions.
    pub connection: usize,
    /// Errors reduced to a safe category.
    pub error: usize,
    /// Requested resynchronizations.
    pub resync: usize,
}

impl DiagnosticCounts {
    fn tally(&mut self, kind: &DiagnosticKind, add: bool) {
        let bucket = match kind {
            DiagnosticKind::Activity => &mut self.activity,
            DiagnosticKind::Connection(_) => &mut self.connection,
            DiagnosticKind::Error(_) => &mut self.error,
            DiagnosticKind::Resync => &mut self.resync,
        };
        if add {
            *bucket += 1;
        } else {
            *bucket = bucket.saturating_sub(1);
        }
    }
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
    /// Number of events the ring retains, independent of the reader cursor.
    ///
    /// The support report restates this value instead of recomputing a count from
    /// the cursor window, so ring retention and the report cannot diverge.
    pub retained_events: usize,
    /// Oldest sequence the ring retains, independent of the reader cursor.
    pub oldest_retained_sequence: Option<u64>,
    /// Category counts across the ring's retained events, independent of the cursor.
    pub retained_counts: DiagnosticCounts,
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
    counts: DiagnosticCounts,
    next_sequence: u64,
    latest_sequence: u64,
    exhausted: bool,
}

impl DiagnosticRing {
    pub(crate) fn new() -> Self {
        Self {
            events: VecDeque::with_capacity(MAX_DIAGNOSTIC_EVENTS),
            counts: DiagnosticCounts::default(),
            next_sequence: 1,
            latest_sequence: 0,
            exhausted: false,
        }
    }

    pub(crate) fn append(&mut self, kind: DiagnosticKind) {
        if self.exhausted {
            return;
        }

        if self.events.len() == MAX_DIAGNOSTIC_EVENTS
            && let Some(evicted) = self.events.pop_front()
        {
            self.counts.tally(&evicted.kind, false);
        }

        let sequence = self.next_sequence;
        self.counts.tally(&kind, true);
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
            retained_events: self.events.len(),
            oldest_retained_sequence: self.events.front().map(|event| event.sequence),
            retained_counts: self.counts,
            latest_sequence: self.latest_sequence,
            exhausted: self.exhausted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DiagnosticKind, DiagnosticRing, EventGap, MAX_DIAGNOSTIC_EVENTS};

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
    #[test]
    fn the_ring_retains_exactly_the_declared_cap_and_evicts_the_oldest() {
        const APPENDS: usize = MAX_DIAGNOSTIC_EVENTS + 904;
        let mut ring = DiagnosticRing::new();
        for _ in 0..APPENDS {
            ring.append(DiagnosticKind::Activity);
        }
        let batch = ring.read_after(0);
        assert_eq!(
            MAX_DIAGNOSTIC_EVENTS, 4096,
            "the ratified retention count is fixed in code"
        );
        assert_eq!(batch.retained_events, MAX_DIAGNOSTIC_EVENTS);
        assert_eq!(batch.events.len(), MAX_DIAGNOSTIC_EVENTS);
        assert_eq!(batch.latest_sequence, APPENDS as u64);
        let oldest = batch.events.first().expect("events are retained").sequence;
        assert_eq!(oldest, (APPENDS - MAX_DIAGNOSTIC_EVENTS + 1) as u64);
        assert_eq!(
            batch.event_gap,
            Some(EventGap {
                from: 1,
                to: oldest - 1
            }),
            "the evicted prefix is one exact lost interval"
        );
        assert_eq!(batch.retained_counts.activity, MAX_DIAGNOSTIC_EVENTS);
        assert_eq!(batch.retained_counts.connection, 0);
        assert_eq!(
            batch.oldest_retained_sequence,
            Some(oldest),
            "the ring-wide oldest is independent of the reader cursor"
        );
        assert!(
            batch
                .events
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence),
            "sequences are strictly increasing"
        );
        assert!(
            batch
                .events
                .iter()
                .all(|event| event.sequence < batch.latest_sequence + 1)
        );
    }

    #[test]
    fn retention_never_exceeds_the_cap_at_any_append() {
        let mut ring = DiagnosticRing::new();
        for index in 1..=(MAX_DIAGNOSTIC_EVENTS * 2) {
            ring.append(DiagnosticKind::Activity);
            let snapshot = ring.read_after(0);
            assert!(snapshot.retained_events <= MAX_DIAGNOSTIC_EVENTS);
            assert!(snapshot.events.len() <= MAX_DIAGNOSTIC_EVENTS);
            if index == MAX_DIAGNOSTIC_EVENTS {
                assert_eq!(snapshot.retained_events, MAX_DIAGNOSTIC_EVENTS);
                assert!(snapshot.event_gap.is_none(), "an exact fill loses nothing");
            }
            if index > MAX_DIAGNOSTIC_EVENTS {
                assert_eq!(snapshot.retained_events, MAX_DIAGNOSTIC_EVENTS);
            }
        }
    }

    #[test]
    fn a_continuous_stream_never_reports_a_gap() {
        let mut ring = DiagnosticRing::new();
        let mut cursor = 0u64;
        for _ in 0..(MAX_DIAGNOSTIC_EVENTS * 2) {
            ring.append(DiagnosticKind::Activity);
            let batch = ring.read_after(cursor);
            assert!(
                batch.event_gap.is_none(),
                "a reader that keeps up loses nothing, even across evictions"
            );
            cursor = batch.latest_sequence;
        }
        assert_eq!(cursor, (MAX_DIAGNOSTIC_EVENTS * 2) as u64);
    }

    #[test]
    fn normal_eviction_does_not_fake_a_gap_for_a_current_cursor() {
        let mut ring = DiagnosticRing::new();
        for _ in 0..MAX_DIAGNOSTIC_EVENTS {
            ring.append(DiagnosticKind::Activity);
        }
        let at_cap = ring.read_after(MAX_DIAGNOSTIC_EVENTS as u64);
        assert!(at_cap.events.is_empty());
        assert!(at_cap.event_gap.is_none());
        for _ in 0..10 {
            ring.append(DiagnosticKind::Activity);
        }
        let after_eviction = ring.read_after(MAX_DIAGNOSTIC_EVENTS as u64);
        assert!(
            after_eviction.event_gap.is_none(),
            "eviction alone is not a loss for a cursor that is still inside the ring"
        );
        assert_eq!(after_eviction.events.len(), 10);
        assert_eq!(after_eviction.retained_events, MAX_DIAGNOSTIC_EVENTS);
        // The evicted prefix is only a gap for a cursor that actually fell behind it.
        let behind = ring.read_after(0);
        assert_eq!(behind.event_gap, Some(EventGap { from: 1, to: 10 }));
    }

    #[test]
    fn a_real_discontinuity_is_reported_once_as_one_interval() {
        let mut ring = DiagnosticRing::new();
        for _ in 0..(MAX_DIAGNOSTIC_EVENTS + 500) {
            ring.append(DiagnosticKind::Activity);
        }
        let lost = ring.read_after(100);
        assert_eq!(
            lost.event_gap,
            Some(EventGap { from: 101, to: 500 }),
            "one interval, not one report per lost event"
        );
        let advanced = ring.read_after(lost.latest_sequence);
        assert!(
            advanced.event_gap.is_none(),
            "advancing the cursor past the loss does not repeat the gap"
        );
        assert!(advanced.events.is_empty());
    }
}
