//! GPUI-free application coordination. All client work runs on a background executor.

pub mod navigation;
pub mod preferences;

use cos_client::{DesktopClient, SnapshotHandle};

#[derive(Debug)]
pub enum CommandError {
    NotConnected,
    Client(cos_client::ClientError),
}

/// Application ownership is separate from each window's local presentation state.
/// A missing client is a real unconfigured/unsupported Runtime, never a fixture fallback.
pub struct AppCore {
    pub navigation: navigation::Navigation,
    client: Option<Box<dyn DesktopClient>>,
    unavailable_reason: &'static str,
}
impl AppCore {
    /// Returns an explicit local observation for a disconnected UI. Each window preserves its own
    /// local selection separately; an `Ok` value from this constructor can only represent the absence
    /// of a connected Runtime, never fixture mode.
    pub fn disconnected(endpoint_configured: bool) -> Self {
        Self {
            navigation: navigation::Navigation::default(),
            client: None,
            unavailable_reason: if endpoint_configured {
                "Disconnected — production transport not implemented"
            } else {
                "Disconnected — Runtime not configured"
            },
        }
    }

    /// Binary lifecycle runs `connect`/`pump` on a background executor, then transfers ownership here.
    pub fn with_client(client: Box<dyn DesktopClient>) -> Self {
        Self {
            navigation: navigation::Navigation::default(),
            client: Some(client),
            unavailable_reason: "",
        }
    }
    pub fn snapshot(&self) -> Option<SnapshotHandle> {
        self.client.as_ref().map(|client| client.snapshot())
    }

    pub fn connection_label(&self) -> String {
        self.client.as_ref().map_or_else(
            || self.unavailable_reason.to_owned(),
            |client| client.connection().label().to_owned(),
        )
    }

    pub fn source_label(&self) -> String {
        self.client.as_ref().map_or_else(
            || "Runtime not connected".to_owned(),
            |client| match client.source() {
                cos_client::ClientSource::Fixture { scenario_id } => {
                    format!("Fixture {scenario_id}")
                }
                cos_client::ClientSource::Runtime { server_identity } => {
                    format!("Runtime {server_identity:?}")
                }
                cos_client::ClientSource::Unknown => "Unknown source".to_owned(),
            },
        )
    }

    /// Routes an explicit stop request through cos-client and returns its receipt token only.
    /// Receipt acceptance is not canonical stop completion.
    pub fn request_stop(
        &mut self,
        task: &desktop_protocol::TaskId,
    ) -> Result<desktop_protocol::CommandId, CommandError> {
        let client = self.client.as_mut().ok_or(CommandError::NotConnected)?;
        client.stop_task(task).map_err(CommandError::Client)
    }

    pub fn command_status(
        &self,
        id: &desktop_protocol::CommandId,
    ) -> Option<&cos_client::CommandStatus> {
        self.client
            .as_ref()
            .and_then(|client| client.command_status(id))
    }

    pub fn unavailable_reason(&self) -> &'static str {
        self.unavailable_reason
    }

    /// Local disconnect: cancels observation only. The returned client events are consumed by the
    /// background session owner, not by window input/render callbacks.
    pub fn disconnect(&mut self) {
        if let Some(client) = self.client.as_mut() {
            client.disconnect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AppCore;

    #[test]
    fn missing_client_is_explicit_never_a_fixture_fallback() {
        let unconfigured = AppCore::disconnected(false);
        assert_eq!(
            unconfigured.connection_label(),
            "Disconnected — Runtime not configured"
        );
        assert_eq!(unconfigured.source_label(), "Runtime not connected");
        assert!(unconfigured.snapshot().is_none());

        let configured = AppCore::disconnected(true);
        assert_eq!(
            configured.connection_label(),
            "Disconnected — production transport not implemented"
        );
        assert!(configured.snapshot().is_none());
    }

    #[test]
    fn stop_without_client_is_an_explicit_error_not_completion() {
        let mut core = AppCore::disconnected(false);
        let task = desktop_protocol::TaskId::fixture("task/Task-184");
        assert!(matches!(
            core.request_stop(&task),
            Err(super::CommandError::NotConnected)
        ));
        assert!(core.snapshot().is_none());
    }

    #[test]
    fn local_disconnect_never_sends_stop_implicitly() {
        let mut core = AppCore::disconnected(false);
        core.disconnect();
        assert!(core.snapshot().is_none());
        assert_eq!(
            core.connection_label(),
            "Disconnected — Runtime not configured"
        );
    }
}
