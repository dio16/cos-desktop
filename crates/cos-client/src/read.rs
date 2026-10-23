//! Client-private retention and Runtime-issued read evidence. No UI or ACL policy.
//!
//! This module owns the bounded registry record shape, the authorization
//! binding, the readiness sets and the receipt-read budget. It performs no
//! policy choice: every decision here is either contract shape or evidence
//! supplied by a trusted producer.

use std::collections::{BTreeMap, VecDeque, btree_map};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::Waker;

use desktop_protocol::{
    AuthorizedReceiptLookup, CommandEnvelope, CommandId, ObservationScope,
    OpenAuthorizedObservation, ReadContextId, ReadRequestId, ReadRevision, ReadStamp,
    ReceiptReadId,
};

use crate::{ClientError, CommandState};

pub(crate) const MAX_UNRESOLVED: usize = 64;
pub(crate) const MAX_TOMBSTONES: usize = 64;
pub(crate) const MAX_RECENT: usize = 256;
pub(crate) const MAX_REFERENCES: usize = 64;
pub(crate) const MAX_PENDING_READS: usize = 64;
/// Bounded identity-level read evidence retained per binding.
pub const MAX_READ_EVIDENCE: usize = 512;
/// Bounded distinct invalidation floors remembered across contexts.
pub(crate) const MAX_INVALIDATION_FLOORS: usize = 8;
/// Derived maximum number of distinct retained records.
pub(crate) const MAX_DISTINCT_RETAINED: usize =
    MAX_UNRESOLVED + MAX_TOMBSTONES + MAX_RECENT + MAX_REFERENCES;

/// Close-only authority supplied by the actual publication owner before observation.
#[derive(Clone)]
pub struct ObservationControl {
    /// Wake the background pump when input/output or local work becomes ready.
    pub wake: Waker,
    /// Synchronously close the shared visibility fence and return its sequence.
    ///
    /// Closing an already-closed fence is idempotent and returns the current
    /// sequence, so a consumer can read the shared sequence without reopening.
    pub close_visibility: Arc<dyn Fn() -> u64 + Send + Sync>,
}

/// Safe reasons for withholding all projection and command disclosure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadUnavailable {
    /// No verified channel or no current complete grant.
    AuthorizationUnavailable,
    /// Projection synchronization has not completed.
    Synchronizing,
    /// The binding was invalidated or replaced.
    Superseded,
    /// The observation has been shut down.
    Closed,
    /// A non-reused counter could not be advanced.
    Exhausted,
}

/// Retention class of one record; drives the disjoint capacity rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetentionClass {
    /// Ordinary unsettled slot (at most [`MAX_UNRESOLVED`]).
    Ordinary,
    /// Receipt tombstone slot (at most [`MAX_TOMBSTONES`]).
    Tombstone,
    /// Settled slot with its settlement sequence (at most [`MAX_RECENT`] + references).
    Settled(u64),
}

/// One authoritative retained command record.
///
/// The lifecycle state is separate from the private envelope, origin context and
/// current decision. There is no second store and no unfiltered UI accessor.
#[derive(Clone)]
pub struct CommandStatus {
    pub(crate) envelope: Option<CommandEnvelope>,
    pub(crate) status: CommandState,
    pub(crate) origin: Option<Arc<ReadContextId>>,
    pub(crate) decision: Option<(Arc<ReadStamp>, bool)>,
    pub(crate) needs_read: bool,
    pub(crate) class: RetentionClass,
    pub(crate) references: usize,
}

impl CommandStatus {
    /// Actual retained lifecycle state; a trusted client API, never a read grant.
    pub fn state(&self) -> &CommandState {
        &self.status
    }

    /// Whether this record's lifecycle fields are receipt-bearing or server-derived.
    pub(crate) fn server_derived(&self) -> bool {
        match &self.status {
            CommandState::Prepared | CommandState::Pending { .. } => false,
            CommandState::ResultUnknown { receipt, .. } => receipt.is_some(),
            CommandState::Accepted(_)
            | CommandState::Rejected(_)
            | CommandState::CanonicalReflected(_)
            | CommandState::Failed { .. }
            | CommandState::Unsupported { .. } => true,
        }
    }

    pub(crate) fn prepared(envelope: CommandEnvelope, origin: Option<Arc<ReadContextId>>) -> Self {
        Self {
            envelope: Some(envelope),
            status: CommandState::Prepared,
            origin,
            decision: None,
            needs_read: false,
            class: RetentionClass::Ordinary,
            references: 0,
        }
    }

    pub(crate) fn readable(&self, binding: &ReadBinding) -> bool {
        if let Some((stamp, readable)) = &self.decision
            && stamp.as_ref() == binding.stamp.as_ref()
        {
            return *readable;
        }
        binding.local_intents_readable
            && !self.server_derived()
            && self.origin.as_deref() == Some(binding.context.as_ref())
    }

    /// Current per-identity decision for this exact stamp, when one exists.
    ///
    /// `None` means no current evidence; it never means "not readable".
    pub fn permission(&self, stamp: &ReadStamp) -> Option<bool> {
        self.decision
            .as_ref()
            .filter(|(evidence, _)| evidence.as_ref() == stamp)
            .map(|(_, readable)| *readable)
    }

    /// Whether current positive evidence for this exact stamp permits disclosure.
    pub(crate) fn readable_with(&self, stamp: &ReadStamp) -> bool {
        self.decision
            .as_ref()
            .is_some_and(|(evidence, readable)| *readable && evidence.as_ref() == stamp)
    }
}

impl std::fmt::Debug for CommandStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandStatus")
            .field("state", &self.status)
            .field("references", &self.references)
            .finish_non_exhaustive()
    }
}

/// Trusted retention result; authorization is checked separately.
#[derive(Clone, Debug)]
pub enum CommandLookup<'a> {
    /// The actual retained record, never a synthesized execution outcome.
    Found(&'a CommandStatus),
    /// No retained record; public disclosure still requires positive read evidence.
    NotRetained,
}

/// Borrowed authorization-filtered iterator over the single bounded registry.
pub struct ReadableCommands<'a> {
    pub(crate) records: btree_map::Iter<'a, CommandId, CommandStatus>,
    pub(crate) binding: &'a ReadBinding,
}

impl<'a> Iterator for ReadableCommands<'a> {
    type Item = (&'a CommandId, &'a CommandStatus);

    fn next(&mut self) -> Option<Self::Item> {
        self.records
            .find(|(_, record)| record.readable(self.binding))
    }
}

/// Current positive or negative per-identity read evidence for one binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadEvidence {
    /// `Allowed` with no retained server receipt and true local absence.
    AllowedNoReceipt(Arc<ReadStamp>),
    /// A scoped denial; it discloses no receipt, retention or execution fact.
    Denied(Arc<ReadStamp>),
}

/// Validated client evidence borrowed by the background publication owner.
pub enum PublicationAccess<'a> {
    /// No data may be disclosed.
    Withheld {
        /// Safe reason.
        reason: ReadUnavailable,
    },
    /// Current grant plus a complete validated projection.
    Validated {
        /// Exact binding stamp.
        stamp: &'a ReadStamp,
        /// Granted scope.
        scope: &'a ObservationScope,
        /// Sequence of the shared visibility fence this view belongs to.
        visibility_sequence: u64,
        /// Records readable under this binding.
        readable_commands: ReadableCommands<'a>,
        /// Identity-level read evidence not carried by a record.
        ///
        /// `Allowed`/no receipt is not a lifecycle state, and a denial is not an
        /// execution fact, so both are carried here instead of fabricating a
        /// record. Only evidence for the current stamp may answer publicly, and
        /// the set is bounded and insertion-ordered.
        read_evidence: &'a BTreeMap<CommandId, (u64, ReadEvidence)>,
    },
}

pub(crate) struct ReadBinding {
    pub(crate) stamp: Arc<ReadStamp>,
    pub(crate) context: Arc<ReadContextId>,
    pub(crate) scope: ObservationScope,
    pub(crate) local_intents_readable: bool,
    pub(crate) visibility_sequence: u64,
}

impl ReadBinding {
    pub(crate) fn matches(&self, stamp: &ReadStamp) -> bool {
        stamp == self.stamp.as_ref()
    }
}

pub(crate) const TOMBSTONE_REASON: &str = "CommandReceiptTombstone";

/// Whether this state is the explicit receipt tombstone shape.
pub(crate) fn tombstone_shape(state: &CommandState) -> bool {
    matches!(state, CommandState::ResultUnknown { reason, receipt: Some(_) } if reason == TOMBSTONE_REASON)
}

#[derive(Clone)]
pub(crate) struct PendingReceiptRead {
    pub(crate) request: AuthorizedReceiptLookup,
    pub(crate) deadline_ms: u64,
    pub(crate) retained_reference: bool,
}

#[derive(Default)]
pub(crate) struct ReadState {
    /// Shared close-only publication fence supplied by app-core.
    pub(crate) control: Option<ObservationControl>,
    /// Exact observation request awaiting a grant for the current attempt.
    pub(crate) pending_request: Option<OpenAuthorizedObservation>,
    /// Whether the caller-supplied request identity was already submitted.
    pub(crate) request_sent: bool,
    /// Retention interest set the pending request declares (distinct identities).
    pub(crate) pending_retained: Vec<CommandId>,
    /// Current validated binding, when one is open.
    pub(crate) binding: Option<ReadBinding>,
    /// Last observed shared visibility sequence (odd while closed).
    pub(crate) closed_sequence: u64,
    /// Highest invalidation revision already applied per context.
    ///
    /// Bounded and monotonic: an older or mismatched notification can close the
    /// binding but can never lower or erase an existing floor.
    pub(crate) invalidation_floors: BTreeMap<ReadContextId, ReadRevision>,
    /// Insertion order of the floors, for deterministic bounded eviction.
    pub(crate) floor_order: VecDeque<ReadContextId>,
    /// Admitted in-flight reads, bounded by [`MAX_PENDING_READS`].
    pub(crate) pending: BTreeMap<CommandId, PendingReceiptRead>,
    /// Fair scan cursor for the automatic read drain.
    pub(crate) automatic_cursor: Option<CommandId>,
    /// Identities denied or timed out under the current binding; not re-polled.
    ///
    /// Bounded insertion-ordered: the oldest entries are evicted first, so an
    /// endless stream of distinct identities cannot grow this unboundedly.
    pub(crate) stalled: BTreeMap<CommandId, u64>,
    /// Current per-identity read evidence with no retained record.
    pub(crate) read_evidence: BTreeMap<CommandId, (u64, ReadEvidence)>,
    /// Monotonic insertion sequence for the two bounded maps above.
    pub(crate) evidence_sequence: u64,
}

impl ReadState {
    /// Record identity-level evidence, staying within the bounded budget.
    pub(crate) fn record_evidence(&mut self, id: CommandId, evidence: ReadEvidence) {
        self.evidence_sequence += 1;
        let sequence = self.evidence_sequence;
        self.read_evidence.insert(id, (sequence, evidence));
        self.evict_evidence();
    }

    /// Mark an identity as stalled (denied or timed out) with the same bound.
    pub(crate) fn record_stalled(&mut self, id: CommandId) {
        self.evidence_sequence += 1;
        let sequence = self.evidence_sequence;
        self.stalled.insert(id, sequence);
        self.evict_evidence();
    }

    fn evict_evidence(&mut self) {
        while self.read_evidence.len() > MAX_READ_EVIDENCE {
            let Some(oldest) = self
                .read_evidence
                .iter()
                .min_by_key(|(_, (sequence, _))| *sequence)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.read_evidence.remove(&oldest);
        }
        while self.stalled.len() > MAX_READ_EVIDENCE {
            let Some(oldest) = self
                .stalled
                .iter()
                .min_by_key(|(_, sequence)| *sequence)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.stalled.remove(&oldest);
        }
    }

    /// Advance the invalidation floor for one context, never regressing it.
    pub(crate) fn raise_floor(&mut self, context: ReadContextId, revision: ReadRevision) {
        let current = self.invalidation_floors.get(&context);
        let keep_existing = current
            .and_then(|current| revision.as_counter().numerically_gt(current.as_counter()))
            .map(|newer| !newer)
            .unwrap_or(current.is_some());
        if keep_existing {
            return;
        }
        if !self.invalidation_floors.contains_key(&context) {
            self.floor_order.push_back(context.clone());
        }
        self.invalidation_floors.insert(context, revision);
        while self.floor_order.len() > MAX_INVALIDATION_FLOORS {
            if let Some(oldest) = self.floor_order.pop_front() {
                self.invalidation_floors.remove(&oldest);
            }
        }
    }

    /// Whether a grant at this revision is late for its context.
    pub(crate) fn is_late_grant(&self, context: &ReadContextId, revision: &ReadRevision) -> bool {
        self.invalidation_floors.get(context).is_some_and(|floor| {
            !revision
                .as_counter()
                .numerically_gt(floor.as_counter())
                .unwrap_or(false)
        })
    }
}

impl ReadState {
    /// Close the binding locally and return the shared visibility sequence.
    pub(crate) fn close_binding(&mut self) -> u64 {
        self.binding = None;
        self.pending.clear();
        self.stalled.clear();
        self.read_evidence.clear();
        let sequence = match &self.control {
            Some(control) => (control.close_visibility)(),
            None => {
                self.closed_sequence = (self.closed_sequence + 1) | 1;
                self.closed_sequence
            }
        };
        self.closed_sequence = sequence;
        sequence
    }

    /// Current shared visibility sequence without opening the fence.
    pub(crate) fn current_sequence(&mut self) -> u64 {
        match &self.control {
            Some(control) => {
                let sequence = (control.close_visibility)();
                self.closed_sequence = sequence;
                sequence
            }
            None => self.closed_sequence,
        }
    }
}

pub(crate) fn capacity_error() -> ClientError {
    ClientError::InvalidCommand("CommandCapacityExceeded".to_owned())
}

pub(crate) fn unsettled(state: &CommandState) -> bool {
    matches!(
        state,
        CommandState::Prepared
            | CommandState::Pending { .. }
            | CommandState::Accepted(_)
            | CommandState::ResultUnknown { .. }
    )
}

/// Build a fresh non-reused observation request identity.
pub fn next_read_request_id() -> ReadRequestId {
    ReadRequestId::new(correlation("read-req"))
        .expect("generated read request identity satisfies the opaque id rules")
}

/// Build a fresh non-reused receipt-read identity.
pub fn next_receipt_read_id() -> ReceiptReadId {
    ReceiptReadId::new(correlation("receipt-read"))
        .expect("generated receipt read identity satisfies the opaque id rules")
}

fn correlation(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!("{prefix}-{}-{nanos}-{sequence}", std::process::id())
}
