//! Runtime COS REST read adapter (`/api/cos`) — transport-independent.
//!
//! The Runtime answers reads as REST JSON, while the Desktop binds authoritative
//! projections to an epoch and a monotonic projection revision. This module owns
//! that mapping and its guards:
//!
//! * the epoch must be present and is compared against the previously read one;
//! * a projection revision inside one epoch may only move forward — a regression
//!   is refused instead of being accepted as fresh truth;
//! * every axis is required from the Runtime; nothing is defaulted locally.
//!
//! The HTTP call itself is injected through [`HttpExecutor`], so this crate owns
//! no network dependency and stays testable without a server. A concrete
//! executor belongs to the binary/startup boundary.

use serde::Deserialize;

/// One HTTP response body, already bounded by the executor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response body text.
    pub body: String,
}

/// Executor-level failure with a safe message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpFailure {
    /// The endpoint could not be reached.
    #[error("runtime unreachable: {0}")]
    Unreachable(String),
}

/// Blocking GET executor supplied by the binary boundary.
///
/// The caller runs it on a background worker; this crate never calls it from the
/// UI thread.
pub trait HttpExecutor: Send {
    /// Perform one bounded GET against the runtime origin.
    fn get(&self, path: &str) -> Result<HttpResponse, HttpFailure>;
}

/// Runtime task status axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RuntimeTaskStatus {
    /// Assigned but not started.
    Assigned,
    /// Running.
    Active,
    /// Completed.
    Completed,
    /// Cancelled.
    Cancelled,
}

/// One task read as the Runtime projects it.
///
/// Unknown members are ignored: the REST payload is a superset of the axes the
/// Desktop binds to. Every field here is required from the Runtime.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RuntimeTaskView {
    /// Wire schema version; must be 1.
    pub schema_version: u8,
    /// Runtime epoch identity of the answering process.
    pub epoch: String,
    /// Projection revision within that epoch.
    pub projection_revision: u64,
    /// Canonical task identity.
    pub task_id: String,
    /// Human title.
    pub title: String,
    /// Task status axis.
    pub task_status: RuntimeTaskStatus,
    /// Inner phase label, when the Runtime projects one.
    #[serde(default)]
    pub phase: Option<String>,
    /// Current iteration index, when known.
    #[serde(default)]
    pub iteration: Option<u32>,
    /// Capabilities projection reference, when the Runtime projects one.
    #[serde(default)]
    pub capabilities_ref: Option<String>,
    /// Stop requested but not yet confirmed complete.
    #[serde(default)]
    pub stop_pending: bool,
}

/// Read adapter failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeReadError {
    /// The executor could not reach the runtime.
    #[error("runtime unreachable: {0}")]
    Unreachable(String),
    /// The runtime answered with a non-success status.
    #[error("runtime answered {0}")]
    Status(u16),
    /// The body was not the projected task shape.
    #[error("runtime payload is not a task view")]
    Malformed,
    /// Unsupported projection schema version.
    #[error("unsupported projection schema_version: {0}")]
    SchemaVersion(u8),
    /// A required axis was empty.
    #[error("runtime projected an empty {0}")]
    Empty(&'static str),
    /// The payload described a different task than the one requested.
    #[error("runtime payload described a different task")]
    TaskMismatch,
    /// The projection revision moved backwards within one epoch.
    #[error("projection revision regressed from {previous} to {next}")]
    RevisionRegression {
        /// Revision of the last accepted read.
        previous: u64,
        /// Revision the runtime just reported.
        next: u64,
    },
}

/// Reader that guards epoch continuity and revision monotonicity.
pub struct RuntimeReader<E: HttpExecutor> {
    executor: E,
    epoch: Option<String>,
    revision: u64,
}

impl<E: HttpExecutor> RuntimeReader<E> {
    /// Create a reader over the injected executor.
    pub fn new(executor: E) -> Self {
        Self {
            executor,
            epoch: None,
            revision: 0,
        }
    }

    /// Last accepted epoch, when a read succeeded.
    pub fn epoch(&self) -> Option<&str> {
        self.epoch.as_deref()
    }

    /// Last accepted projection revision.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Read one task and enforce the epoch/revision guards.
    pub fn read_task(&mut self, task_id: &str) -> Result<RuntimeTaskView, RuntimeReadError> {
        let path = format!("/api/cos/tasks/{}", encode_segment(task_id));
        let response = self
            .executor
            .get(&path)
            .map_err(|failure| RuntimeReadError::Unreachable(failure.to_string()))?;
        if response.status != 200 {
            return Err(RuntimeReadError::Status(response.status));
        }
        let view: RuntimeTaskView =
            serde_json::from_str(&response.body).map_err(|_| RuntimeReadError::Malformed)?;
        self.accept(view, task_id)
    }

    fn accept(
        &mut self,
        view: RuntimeTaskView,
        requested: &str,
    ) -> Result<RuntimeTaskView, RuntimeReadError> {
        if view.schema_version != 1 {
            return Err(RuntimeReadError::SchemaVersion(view.schema_version));
        }
        require("epoch", &view.epoch)?;
        require("task_id", &view.task_id)?;
        require("title", &view.title)?;
        if view.projection_revision == 0 {
            return Err(RuntimeReadError::Empty("projection_revision"));
        }
        if view.task_id != requested {
            return Err(RuntimeReadError::TaskMismatch);
        }
        let same_epoch = self.epoch.as_deref() == Some(view.epoch.as_str());
        if same_epoch && view.projection_revision < self.revision {
            return Err(RuntimeReadError::RevisionRegression {
                previous: self.revision,
                next: view.projection_revision,
            });
        }
        self.epoch = Some(view.epoch.clone());
        self.revision = view.projection_revision;
        Ok(view)
    }
}

fn require(what: &'static str, value: &str) -> Result<(), RuntimeReadError> {
    if value.is_empty() {
        return Err(RuntimeReadError::Empty(what));
    }
    Ok(())
}

/// Percent-encode one path segment so a task id cannot add path structure.
fn encode_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        HttpExecutor, HttpFailure, HttpResponse, RuntimeReadError, RuntimeReader,
        RuntimeTaskStatus, encode_segment,
    };
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};

    const TASK: &str =
        include_str!("../../../crates/cos-client/fixtures/runtime-task-view-v1.json");

    struct Stub {
        responses: RefCell<Vec<Result<HttpResponse, HttpFailure>>>,
        paths: Arc<Mutex<Vec<String>>>,
    }

    impl Stub {
        fn new(responses: Vec<Result<HttpResponse, HttpFailure>>) -> Self {
            Self {
                responses: RefCell::new(responses),
                paths: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl HttpExecutor for Stub {
        fn get(&self, path: &str) -> Result<HttpResponse, HttpFailure> {
            self.paths.lock().expect("paths").push(path.to_string());
            self.responses.borrow_mut().remove(0)
        }
    }

    fn ok(body: &str) -> Result<HttpResponse, HttpFailure> {
        Ok(HttpResponse {
            status: 200,
            body: body.to_string(),
        })
    }

    #[test]
    fn reads_the_projected_task_and_keeps_the_axes() {
        let mut reader = RuntimeReader::new(Stub::new(vec![ok(TASK)]));
        let view = reader.read_task("task-184").expect("read");
        assert_eq!(view.schema_version, 1);
        assert_eq!(view.epoch, "epoch-7f3c");
        assert_eq!(view.projection_revision, 12);
        assert_eq!(view.task_status, RuntimeTaskStatus::Active);
        assert_eq!(view.title, "Neon alley at dusk");
        assert_eq!(
            view.capabilities_ref.as_deref(),
            Some("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(reader.epoch(), Some("epoch-7f3c"));
        assert_eq!(reader.revision(), 12);
    }

    #[test]
    fn refuses_a_revision_regression_inside_one_epoch() {
        let older = TASK.replace("\"projection_revision\": 12", "\"projection_revision\": 11");
        let mut reader = RuntimeReader::new(Stub::new(vec![ok(TASK), ok(&older)]));
        reader.read_task("task-184").expect("first read");
        let error = reader.read_task("task-184").expect_err("regression");
        assert_eq!(
            error,
            RuntimeReadError::RevisionRegression {
                previous: 12,
                next: 11
            }
        );
        assert_eq!(reader.revision(), 12);
    }

    #[test]
    fn accepts_a_new_epoch_and_resets_the_baseline() {
        let newer_epoch = TASK
            .replace("epoch-7f3c", "epoch-9001")
            .replace(": 12", ": 3");
        let mut reader = RuntimeReader::new(Stub::new(vec![ok(TASK), ok(&newer_epoch)]));
        reader.read_task("task-184").expect("first");
        let view = reader.read_task("task-184").expect("new epoch");
        assert_eq!(view.epoch, "epoch-9001");
        assert_eq!(reader.revision(), 3);
    }

    #[test]
    fn refuses_non_success_and_malformed_bodies() {
        let mut reader = RuntimeReader::new(Stub::new(vec![Ok(HttpResponse {
            status: 404,
            body: "{}".into(),
        })]));
        assert_eq!(
            reader.read_task("task-184"),
            Err(RuntimeReadError::Status(404))
        );

        let mut reader = RuntimeReader::new(Stub::new(vec![ok("{\"schema_version\":1}")]));
        assert_eq!(
            reader.read_task("task-184"),
            Err(RuntimeReadError::Malformed)
        );

        let mut reader = RuntimeReader::new(Stub::new(vec![ok(
            &TASK.replace("\"schema_version\": 1", "\"schema_version\": 2")
        )]));
        assert_eq!(
            reader.read_task("task-184"),
            Err(RuntimeReadError::SchemaVersion(2))
        );
    }

    #[test]
    fn refuses_a_payload_for_another_task_or_an_empty_title() {
        let mut reader = RuntimeReader::new(Stub::new(vec![ok(TASK)]));
        assert_eq!(
            reader.read_task("task-999"),
            Err(RuntimeReadError::TaskMismatch)
        );

        let empty_title = TASK.replace("Neon alley at dusk", "");
        let mut reader = RuntimeReader::new(Stub::new(vec![ok(&empty_title)]));
        assert_eq!(
            reader.read_task("task-184"),
            Err(RuntimeReadError::Empty("title"))
        );
    }

    #[test]
    fn executor_failure_is_reported_without_a_projection() {
        let mut reader = RuntimeReader::new(Stub::new(vec![Err(HttpFailure::Unreachable(
            "connection refused".into(),
        ))]));
        let error = reader.read_task("task-184").expect_err("unreachable");
        assert!(matches!(error, RuntimeReadError::Unreachable(_)));
        assert_eq!(reader.epoch(), None);
    }

    #[test]
    fn path_segments_cannot_add_structure() {
        assert_eq!(encode_segment("task-1"), "task-1");
        assert_eq!(encode_segment("../x"), "..%2Fx");
        assert_eq!(encode_segment("a b"), "a%20b");
        let stub = Stub::new(vec![ok(TASK)]);
        let paths = Arc::clone(&stub.paths);
        let mut reader = RuntimeReader::new(stub);
        let _ = reader.read_task("../x");
        assert_eq!(paths.lock().expect("paths")[0], "/api/cos/tasks/..%2Fx");
    }
}
