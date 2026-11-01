//! Runtime-mode observation wiring: the plain-HTTP protocol executor and the
//! startup branch that decides whether a real observation client exists.
//!
//! The UI never touches `cos-client`: this module builds the client, hands it to
//! `window::ObservationHost`, and attaches the resulting `ObservationHandle` to
//! `AppCore`. Any missing configuration or construction failure is an explicit
//! unavailable state; there is no fixture fallback in runtime mode.
//!
//! # Transport boundary
//!
//! `COS_DESKTOP_RUNTIME_ENDPOINT` names a plain-HTTP origin (for example
//! `http://127.0.0.1:8123`). `COS_DESKTOP_RUNTIME_TOKEN`, when set, is the
//! bearer credential for the observation `POST`. The token is read once, moved
//! into the executor, and never logged, rendered, put in a diagnostic or echoed
//! in an error message. Redirects and environment proxies are disabled so a
//! credential can never be replayed to another origin. TLS is not implemented:
//! `ureq` is built with `default-features = false` and non-HTTP or credential
//! bearing endpoints are refused at startup.

use std::io::Read;
use std::time::Duration;

use cos_client::{
    ClientConfig, ClientSource, CosClient, DesktopClient, HttpMethod, HttpRequest, HttpResponse,
    ProtocolHttp, ProtocolTransport, ProtocolTransportConfig,
};
use desktop_protocol::{Limits, ObservationScopeRequest, ProtocolVersions, ScopeFilter, Version};

use crate::app::Mode;

/// Environment variable naming the Runtime origin for runtime mode.
pub(crate) const ENDPOINT_ENV: &str = "COS_DESKTOP_RUNTIME_ENDPOINT";
/// Environment variable carrying the optional bearer token. Never logged.
pub(crate) const TOKEN_ENV: &str = "COS_DESKTOP_RUNTIME_TOKEN";

/// Whole-request budget for one protocol exchange.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Bounded accepted response body (staged bundle ceiling plus one control frame).
const MAX_RESPONSE_BYTES: u64 = Limits::DEFAULT
    .max_bundle_bytes
    .saturating_add(Limits::DEFAULT.max_frame_bytes);
/// Protocol pair this build negotiates with the Runtime adapter.
const PROTOCOL: Version = Version::new(2, 0);
/// Wire schema pair this build negotiates with the Runtime adapter.
const SCHEMA: Version = Version::new(2, 0);

/// Why an explicitly configured runtime observation cannot start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeUnavailable {
    /// No endpoint is configured.
    NotConfigured,
    /// The configured endpoint is not a plain-HTTP origin this build may use.
    UnsupportedEndpoint,
    /// No bearer token is configured for the authenticated observation request.
    MissingToken,
}

impl RuntimeUnavailable {
    /// Safe operator-facing label; never echoes an endpoint or token value.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NotConfigured => "runtime endpoint is not configured",
            Self::UnsupportedEndpoint => {
                "runtime endpoint is not a plain-HTTP origin this build supports"
            }
            Self::MissingToken => "runtime bearer token is not configured",
        }
    }
}

/// Resolved runtime connection settings.
///
/// Deliberately not `Debug`/`Display`: the token must never be formatted into a
/// log line, diagnostic report or panic message.
pub(crate) struct RuntimeSettings {
    endpoint: String,
    token: Option<String>,
}

/// One started observation: the client the host drives and the scope to open.
pub(crate) struct RuntimeObservation {
    pub(crate) client: Box<dyn DesktopClient>,
    pub(crate) scope: ObservationScopeRequest,
}

/// What the composition root should launch.
pub(crate) enum LaunchPlan {
    /// Load the explicit fixture provider.
    Fixture { path: std::path::PathBuf },
    /// Start a real Runtime observation with these settings.
    Runtime(RuntimeSettings),
    /// No observation exists; show the explicit disconnected surface.
    RuntimeUnavailable {
        endpoint_configured: bool,
        reason: RuntimeUnavailable,
    },
}

/// Decide the launch plan for one startup mode without side effects.
///
/// A runtime mode can only produce a runtime (ready or unavailable) plan; it can
/// never produce a fixture plan, which is what keeps fixture data from silently
/// standing in for a real connection.
pub(crate) fn plan(mode: &Mode, endpoint: Option<&str>, token: Option<&str>) -> LaunchPlan {
    match mode {
        Mode::Fixture { path } => LaunchPlan::Fixture { path: path.clone() },
        Mode::Runtime {
            endpoint_configured,
        } => match settings(endpoint, token) {
            Ok(settings) => LaunchPlan::Runtime(settings),
            Err(reason) => LaunchPlan::RuntimeUnavailable {
                endpoint_configured: *endpoint_configured || is_configured(endpoint),
                reason,
            },
        },
    }
}

/// Resolve the connection settings for terminal runtime configuration.
///
/// The endpoint must be a plain-HTTP origin without embedded credentials; the
/// token is required by the Runtime adapter's authenticated observation route.
pub(crate) fn settings(
    endpoint: Option<&str>,
    token: Option<&str>,
) -> Result<RuntimeSettings, RuntimeUnavailable> {
    let endpoint = endpoint.map(str::trim).filter(|value| !value.is_empty());
    let Some(endpoint) = endpoint else {
        return Err(RuntimeUnavailable::NotConfigured);
    };
    let Some(origin) = plain_http_origin(endpoint) else {
        return Err(RuntimeUnavailable::UnsupportedEndpoint);
    };
    let token = token.map(str::trim).filter(|value| !value.is_empty());
    let Some(token) = token else {
        return Err(RuntimeUnavailable::MissingToken);
    };
    Ok(RuntimeSettings {
        endpoint: origin,
        token: Some(token.to_owned()),
    })
}

fn is_configured(endpoint: Option<&str>) -> bool {
    endpoint
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
}

/// Validate a plain-HTTP origin: `http://host[:port][/path]` with no userinfo.
///
/// Returns the normalized origin (no trailing slash).
fn plain_http_origin(value: &str) -> Option<String> {
    let rest = value.strip_prefix("http://")?;
    if rest.is_empty() {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return None;
    }
    // Embedded credentials are not supported: they would have to be rendered or
    // stored, and this build has no credential model beyond the bearer token.
    if authority.contains('@') || rest.contains('@') {
        return None;
    }
    if value
        .chars()
        .any(|character| character.is_control() || character.is_whitespace() || character == '\\')
    {
        return None;
    }
    Some(value.trim_end_matches('/').to_owned())
}

/// Build the observation client over the configured Runtime origin.
pub(crate) fn observation(
    settings: RuntimeSettings,
) -> Result<RuntimeObservation, RuntimeUnavailable> {
    let executor = PlainHttp::new(&settings.endpoint, settings.token.as_deref());
    let transport = ProtocolTransport::new(
        executor,
        ProtocolTransportConfig::default(),
        ProtocolVersions {
            protocol: PROTOCOL,
            schema: SCHEMA,
        },
    );
    let client = CosClient::new(
        transport,
        ClientConfig::default(),
        Limits::DEFAULT,
        // The client replaces this with `ClientSource::Runtime` once the
        // handshake succeeds; before that nothing may claim a real server.
        ClientSource::Unknown,
    );
    Ok(RuntimeObservation {
        client: Box::new(client),
        scope: ObservationScopeRequest {
            task_ids: Vec::new(),
            filters: vec![ScopeFilter {
                name: "view".to_owned(),
                value: "all".to_owned(),
            }],
        },
    })
}

/// Blocking plain-HTTP executor handed to the protocol transport.
///
/// Owns the origin, the bearer token, the request timeout and the response body
/// bound. `ureq` is built without TLS or compression features: the only accepted
/// endpoints are loopback development origins.
struct PlainHttp {
    origin: String,
    token: Option<String>,
    agent: ureq::Agent,
    max_body_bytes: u64,
}

impl PlainHttp {
    fn new(origin: &str, token: Option<&str>) -> Self {
        Self::with_max_body_bytes(origin, token, MAX_RESPONSE_BYTES)
    }

    fn with_max_body_bytes(origin: &str, token: Option<&str>, max_body_bytes: u64) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(REQUEST_TIMEOUT)
            // Never follow a redirect or an environment proxy: either could
            // replay the bearer credential to another origin.
            .redirects(0)
            .try_proxy_from_env(false)
            .build();
        Self {
            origin: origin.trim_end_matches('/').to_owned(),
            token: token.map(str::to_owned),
            agent,
            max_body_bytes,
        }
    }
}

impl ProtocolHttp for PlainHttp {
    fn execute(&mut self, request: HttpRequest) -> Result<HttpResponse, String> {
        let url = format!("{}{}", self.origin, request.path);
        let method = match request.method {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
        };
        let mut call = self.agent.request(method, &url);
        for (name, value) in &request.headers {
            call = call.set(name, value);
        }
        if let Some(token) = &self.token {
            call = call.set("authorization", &format!("Bearer {token}"));
        }
        let outcome = match request.method {
            HttpMethod::Get => call.call(),
            HttpMethod::Post => call.send_bytes(&request.body),
        };
        match outcome {
            Ok(response) => self.read_response(response),
            // A 4xx/5xx body carries the protocol error document; it is a
            // response, not a transport failure.
            Err(ureq::Error::Status(status, response)) => Ok(HttpResponse {
                status,
                body: self.read_body(response)?,
            }),
            // `ErrorKind` is a closed enum: it cannot contain the URL, the
            // endpoint or the token.
            Err(ureq::Error::Transport(error)) => {
                Err(format!("plain HTTP transport failed ({:?})", error.kind()))
            }
        }
    }
}

impl PlainHttp {
    fn read_response(&self, response: ureq::Response) -> Result<HttpResponse, String> {
        let status = response.status();
        let body = self.read_body(response)?;
        Ok(HttpResponse { status, body })
    }

    fn read_body(&self, response: ureq::Response) -> Result<Vec<u8>, String> {
        let limit = self.max_body_bytes.saturating_add(1);
        let mut body = Vec::new();
        response
            .into_reader()
            .take(limit)
            .read_to_end(&mut body)
            .map_err(|_| "response body could not be read".to_owned())?;
        if body.len() as u64 > self.max_body_bytes {
            return Err("response body exceeds the bounded size".to_owned());
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LaunchPlan, Mode, PlainHttp, RuntimeUnavailable, observation, plain_http_origin, plan,
        settings,
    };
    use cos_client::{HttpMethod, HttpRequest, ProtocolHttp};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    fn runtime_mode(endpoint_configured: bool) -> Mode {
        Mode::Runtime {
            endpoint_configured,
        }
    }

    fn unavailable(reason: LaunchPlan) -> RuntimeUnavailable {
        match reason {
            LaunchPlan::RuntimeUnavailable { reason, .. } => reason,
            LaunchPlan::Runtime(_) => panic!("expected an unavailable runtime plan"),
            LaunchPlan::Fixture { .. } => panic!("runtime mode must never plan a fixture"),
        }
    }

    #[test]
    fn runtime_mode_without_an_endpoint_or_token_is_explicitly_unavailable() {
        assert_eq!(
            unavailable(plan(&runtime_mode(false), None, None)),
            RuntimeUnavailable::NotConfigured
        );
        assert_eq!(
            unavailable(plan(&runtime_mode(true), Some("  "), None)),
            RuntimeUnavailable::NotConfigured
        );
        // A configured endpoint without a token never plans a fixture client.
        assert_eq!(
            unavailable(plan(
                &runtime_mode(true),
                Some("http://127.0.0.1:8123"),
                None
            )),
            RuntimeUnavailable::MissingToken
        );
        assert_eq!(
            unavailable(plan(
                &runtime_mode(true),
                Some("http://127.0.0.1:8123"),
                Some("   ")
            )),
            RuntimeUnavailable::MissingToken
        );
        // The unavailable label is static text and never echoes the endpoint.
        assert!(
            !RuntimeUnavailable::MissingToken
                .label()
                .contains("127.0.0.1")
        );
    }

    #[test]
    fn runtime_mode_with_endpoint_and_token_plans_a_runtime_observation() {
        let plan = plan(
            &runtime_mode(true),
            Some("http://127.0.0.1:8123/"),
            Some("token"),
        );
        let LaunchPlan::Runtime(settings) = plan else {
            panic!("configured runtime mode must plan a runtime observation");
        };
        // The executor is built from the settings; the scope is the documented
        // all-task view and the client starts unbound from any real server.
        let observation = observation(settings).expect("observation builds");
        assert!(observation.scope.task_ids.is_empty());
        assert_eq!(observation.scope.filters.len(), 1);
        assert_eq!(observation.scope.filters[0].name, "view");
        assert_eq!(observation.scope.filters[0].value, "all");
    }

    #[test]
    fn fixture_mode_stays_an_explicit_fixture_plan() {
        let plan = plan(
            &Mode::Fixture {
                path: std::path::PathBuf::from("fixture.json"),
            },
            Some("http://127.0.0.1:8123"),
            Some("token"),
        );
        assert!(matches!(plan, LaunchPlan::Fixture { .. }));
    }

    #[test]
    fn non_plain_http_endpoints_are_refused_without_echoing_them() {
        for endpoint in [
            "https://127.0.0.1:8123",
            "ftp://127.0.0.1:8123",
            "127.0.0.1:8123",
            "http://",
            "http://user:secret@127.0.0.1:8123",
            "http://127.0.0.1:8123\r\nX-Injected: 1",
        ] {
            match settings(Some(endpoint), Some("token")) {
                Err(reason) => assert_eq!(
                    reason,
                    RuntimeUnavailable::UnsupportedEndpoint,
                    "endpoint {endpoint:?} must be refused"
                ),
                Ok(_) => panic!("endpoint {endpoint:?} must be refused"),
            }
        }
        assert_eq!(
            plain_http_origin("http://127.0.0.1:8123/").as_deref(),
            Some("http://127.0.0.1:8123")
        );
    }

    /// One-shot HTTP/1.1 server: reads the request (headers and Content-Length
    /// body) and answers with the given status/body.
    fn serve_once(
        status_line: &'static str,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener binds");
        let address = listener.local_addr().expect("listener address");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request accepted");
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            let mut headers_end = None;
            while headers_end.is_none() {
                let read = stream.read(&mut buffer).unwrap_or(0);
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                headers_end = request.windows(4).position(|window| window == b"\r\n\r\n");
            }
            if let Some(position) = headers_end {
                let headers = String::from_utf8_lossy(&request[..position]).to_ascii_lowercase();
                let content_length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let body_start = position + 4;
                while request.len() < body_start + content_length {
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
            }
            let response = format!(
                "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            String::from_utf8_lossy(&request).to_string()
        });
        (format!("http://{address}"), handle)
    }

    fn get_request(path: &str) -> HttpRequest {
        HttpRequest {
            method: HttpMethod::Get,
            path: path.to_owned(),
            headers: vec![("accept".to_owned(), "application/json".to_owned())],
            body: Vec::new(),
        }
    }

    #[test]
    fn the_executor_returns_status_and_body_and_sends_the_bearer_token() {
        let (origin, server) = serve_once(
            "HTTP/1.1 401 Unauthorized",
            r#"{"code":"UNAUTHORIZED_CONTEXT"}"#,
        );
        let mut executor = PlainHttp::new(&origin, Some("local-test-token"));
        let response = executor
            .execute(get_request("/api/cos/desktop/server"))
            .expect("a 401 is a response, not a transport failure");
        assert_eq!(response.status, 401);
        assert_eq!(
            String::from_utf8_lossy(&response.body),
            r#"{"code":"UNAUTHORIZED_CONTEXT"}"#
        );
        let request = server.join().expect("server finished");
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer local-test-token"),
            "{request}"
        );
    }

    #[test]
    fn the_executor_bounds_the_response_body() {
        let (origin, server) = serve_once("HTTP/1.1 200 OK", "0123456789abcdef");
        let mut executor = PlainHttp::with_max_body_bytes(&origin, None, 4);
        let error = executor
            .execute(get_request("/api/cos/desktop/server"))
            .expect_err("an oversized body is refused");
        assert!(error.contains("bounded size"), "{error}");
        let _ = server.join();
    }

    #[test]
    fn transport_failures_never_echo_the_token() {
        // Reserve and release a port so the connection is refused.
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener binds");
        let address = listener.local_addr().expect("listener address");
        drop(listener);
        let mut executor = PlainHttp::new(
            &format!("http://{address}"),
            Some("token-that-must-never-leak"),
        );
        // Give the OS a moment before the refused connect on a released port.
        std::thread::sleep(Duration::from_millis(10));
        let error = executor
            .execute(get_request("/api/cos/desktop/server"))
            .expect_err("a refused connection is a transport failure");
        assert!(!error.contains("token-that-must-never-leak"), "{error}");
        assert!(!error.contains(&format!("{address}")), "{error}");
    }
}
