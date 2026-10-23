//! Fixture evidence only: no projection payloads, credentials or Runtime truth.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde_json::Value;

static NEXT_EXECUTION: AtomicU64 = AtomicU64::new(1);
const MAX_TRACE_BYTES: u64 = 8 * 1024 * 1024;

pub(super) struct FixtureTrace {
    file: Option<File>,
    pending_path: Option<OsString>,
    execution_id: String,
    started: Instant,
    sequence: u64,
    bytes: u64,
    healthy: bool,
}

impl FixtureTrace {
    pub(super) fn new() -> Self {
        let mut trace = Self::without_file();
        // Construction may run at startup; open/write only from the provider's
        // first poll, which the native owner performs on its background task.
        trace.pending_path = std::env::var_os("COS_FIXTURE_TRACE");
        trace
    }

    fn without_file() -> Self {
        Self {
            file: None,
            pending_path: None,
            execution_id: String::new(),
            started: Instant::now(),
            sequence: 0,
            bytes: 0,
            healthy: true,
        }
    }

    fn activate(&mut self, file: File) {
        match NEXT_EXECUTION.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        }) {
            Ok(serial) => {
                self.execution_id = format!("fixture:{}:{}", std::process::id(), serial);
                self.file = Some(file);
            }
            Err(_) => self.fail(),
        }
    }

    #[cfg(test)]
    pub(super) fn with_file(file: File) -> Self {
        let mut trace = Self::without_file();
        trace.activate(file);
        trace
    }

    pub(super) fn enabled(&self) -> bool {
        self.file.is_some() || self.pending_path.is_some()
    }

    pub(super) fn record(
        &mut self,
        session_serial: u64,
        step_id: &str,
        event: &str,
        frames: &[Vec<u8>],
        cancel_reason: Option<&str>,
        consumer_state: Option<(&str, &str)>,
    ) {
        if !self.ensure_active(event != "closed") {
            return;
        }
        let Some(sequence) = self.next_sequence() else {
            return;
        };
        let mut transactions = Vec::new();
        if event == "delivered" {
            for frame in frames {
                if let Ok(value) = serde_json::from_slice::<Value>(frame)
                    && let Some(id) = value
                        .get("message")
                        .and_then(|message| message.get("transaction_id"))
                {
                    transactions.push(id.clone());
                }
            }
        }
        let value = serde_json::json!({
            "execution_id": self.execution_id, "pid": std::process::id(),
            "session_serial": session_serial, "step_id": step_id,
            "sequence": sequence, "monotone_ns": self.started.elapsed().as_nanos().to_string(),
            "event": event, "frame_count": frames.len(), "transactions": transactions,
            "cancel_reason": cancel_reason,
            "consumer_access": consumer_state.map(|state| state.0),
            "connection": consumer_state.map(|state| state.1)
        });
        self.write_line(value, sequence);
    }

    /// Record one client command dispatch the provider actually received.
    ///
    /// Fixture evidence only: the exact identity and payload kind observed on the
    /// wire, in the same ordered envelope as every other provider record, so a
    /// native run's Send count is never inferred from the screen.
    pub(super) fn record_command(
        &mut self,
        session_serial: u64,
        command_id: &str,
        payload_kind: &str,
    ) {
        if !self.ensure_active(true) {
            return;
        }
        let Some(sequence) = self.next_sequence() else {
            return;
        };
        let value = serde_json::json!({
            "execution_id": self.execution_id, "pid": std::process::id(),
            "session_serial": session_serial, "step_id": "",
            "sequence": sequence, "monotone_ns": self.started.elapsed().as_nanos().to_string(),
            "event": "command", "command_id": command_id, "payload_kind": payload_kind,
            "frame_count": 0, "transactions": [],
            "cancel_reason": Value::Null,
            "consumer_access": Value::Null,
            "connection": Value::Null
        });
        self.write_line(value, sequence);
    }

    /// Open the pending trace file on first use.
    ///
    /// The terminal close record never creates a missing trace, so a run without
    /// a configured path stays silent instead of producing evidence after the fact.
    fn ensure_active(&mut self, allow_open: bool) -> bool {
        if allow_open && let Some(path) = self.pending_path.take() {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(file) => self.activate(file),
                Err(_) => self.fail(),
            }
        }
        self.healthy && self.file.is_some()
    }

    fn next_sequence(&mut self) -> Option<u64> {
        match self.sequence.checked_add(1) {
            Some(sequence) => Some(sequence),
            None => {
                self.fail();
                None
            }
        }
    }

    fn write_line(&mut self, value: Value, sequence: u64) {
        let Ok(mut bytes) = serde_json::to_vec(&value) else {
            self.fail();
            return;
        };
        bytes.push(b'\n');
        if self.bytes.saturating_add(bytes.len() as u64) > MAX_TRACE_BYTES {
            self.fail();
            return;
        }
        let result = self
            .file
            .as_mut()
            .map(|file| file.write_all(&bytes).and_then(|()| file.flush()));
        if !matches!(result, Some(Ok(()))) {
            self.fail();
            return;
        }
        self.sequence = sequence;
        self.bytes += bytes.len() as u64;
    }

    pub(super) fn fail(&mut self) {
        if self.healthy {
            eprintln!("FIXTURE_TRACE_NOT_PROVEN: evidence is incomplete");
        }
        self.healthy = false;
    }
}
