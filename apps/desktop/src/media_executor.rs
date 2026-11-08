//! Binary-boundary media executor (RT6/IMG6, stage 3).
//!
//! `cos-client` owns the descriptor contract, the handle join and the status
//! mapping; `crates/media` owns admission. This module is the only place that
//! turns a relative same-authority media path into a TCP request, so the
//! security properties of the boundary are enforced here in one place:
//!
//! - the origin is the already-validated plain-HTTP **loopback IP literal**
//!   (`runtime::validated_origin`); an unsupported origin is never adapted;
//! - only `/api/cos/media/...` paths are requested, and they are joined onto the
//!   validated origin here. An absolute URL, a scheme-relative path, a
//!   backslash, a query/fragment, a raw dot segment or any percent-encoding is
//!   refused before a socket is opened;
//! - the bearer credential is attached only by this executor and only for the
//!   media route family; a caller-supplied `Authorization`/`Host`/proxy header is
//!   refused rather than merged, so a header cannot override the credential;
//! - redirects and environment proxies are disabled in the agent itself: a 3xx
//!   is returned as a response (and mapped to an unexpected status), never
//!   followed to another origin;
//! - every response body is bounded by the caller's `max_bytes` (the
//!   representation budget from `cos-client`), so an oversized or misdeclared
//!   response is refused without an unbounded read.
//!
//! The token is held for the lifetime of the executor, is never formatted,
//! logged, placed in an error or stored anywhere, and no error type here can
//! carry it. No GPUI object and no decode work exists in this module.

use std::io::Read;
use std::time::Duration;

use cos_client::media::{Cancellation, MediaExecutor, MediaResponse, MediaTransportError};

/// Whole-request budget for one media exchange.
///
/// A descriptor is one small document; a bounded representation is at most
/// 256 MiB over loopback. The budget bounds a stalled peer, not throughput.
const MEDIA_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// The only same-authority route family this executor may request.
const MEDIA_ROUTE_PREFIX: &str = "/api/cos/media/";

/// Blocking same-authority media executor over the validated loopback origin.
///
/// Not `Debug`/`Display`: it owns one copy of the session credential, and no
/// rendering of this value may ever exist.
pub(crate) struct MediaHttp {
    origin: String,
    token: String,
    agent: ureq::Agent,
}

impl MediaHttp {
    /// Build an executor for one configured endpoint.
    ///
    /// The endpoint is validated as a plain-HTTP loopback IP-literal origin
    /// (`http://127.x.x.x[:port]` or `http://[::1][:port]`). An unsupported
    /// endpoint yields `None`; no adapter, proxy or hostname fallback exists.
    pub(crate) fn new(endpoint: &str, token: &str) -> Option<Self> {
        let origin = crate::runtime::validated_origin(endpoint)?;
        if token.is_empty() {
            return None;
        }
        let agent = ureq::AgentBuilder::new()
            .timeout(MEDIA_REQUEST_TIMEOUT)
            // A redirect could replay the credential to another origin, and an
            // environment proxy is an unvalidated second authority: both stay off.
            .redirects(0)
            .try_proxy_from_env(false)
            .build();
        Some(Self {
            origin,
            token: token.to_owned(),
            agent,
        })
    }

    /// Whether a path is one this executor may request.
    ///
    /// This is defense in depth behind `cos-client`'s own handle filter: the
    /// executor repeats the same-authority constraints because it is the last
    /// place before a socket and it must not depend on a caller having used the
    /// validated helper.
    fn same_authority_media_path(path: &str) -> bool {
        if path.len() > 4096 || !path.starts_with(MEDIA_ROUTE_PREFIX) {
            return false;
        }
        if path.contains("://")
            || path.contains('\\')
            || path.contains('?')
            || path.contains('#')
            || path.contains('%')
            || path.bytes().any(|byte| !(0x21..=0x7e).contains(&byte))
        {
            return false;
        }
        !path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
    }

    /// Whether a caller-supplied header would change the request authority.
    fn reserved_header(name: &str) -> bool {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "authorization"
                | "host"
                | "proxy-authorization"
                | "proxy-connection"
                | "transfer-encoding"
                | "content-length"
        )
    }
}

impl MediaExecutor for MediaHttp {
    fn get(
        &mut self,
        path: &str,
        headers: &[(&str, &str)],
        max_bytes: u64,
        cancellation: &Cancellation,
    ) -> Result<MediaResponse, MediaTransportError> {
        if cancellation.is_cancelled() {
            return Err(MediaTransportError::Transport);
        }
        if !Self::same_authority_media_path(path)
            || headers.iter().any(|(name, _)| Self::reserved_header(name))
        {
            // No socket is opened for a path or header this boundary refuses.
            return Err(MediaTransportError::Transport);
        }
        let url = format!("{}{}", self.origin, path);
        let mut call = self.agent.request("GET", &url);
        // The session credential is attached here and only here; every other
        // header is the transport-independent client's own value (never a
        // credential or an origin override).
        call = call.set("Authorization", &format!("Bearer {}", self.token));
        for (name, value) in headers {
            call = call.set(name, value);
        }
        let outcome = call.call();
        if cancellation.is_cancelled() {
            return Err(MediaTransportError::Transport);
        }
        let (status, response) = match outcome {
            Ok(response) => (response.status(), response),
            // A 4xx/5xx body is a response to classify, not a transport failure.
            Err(ureq::Error::Status(status, response)) => (status, response),
            // `ErrorKind` is closed and value-free: it can never carry the URL,
            // the origin, the path, the handle or the token.
            Err(ureq::Error::Transport(_)) => return Err(MediaTransportError::Transport),
        };
        let content_length = response
            .header("Content-Length")
            .and_then(|value| value.trim().parse::<u64>().ok());
        // A body declared beyond the caller's bound is refused before reading.
        if content_length.is_some_and(|length| length > max_bytes) {
            return Err(MediaTransportError::Oversized);
        }
        let limit = max_bytes.saturating_add(1);
        let mut body = Vec::new();
        response
            .into_reader()
            .take(limit)
            .read_to_end(&mut body)
            .map_err(|_| MediaTransportError::Transport)?;
        if body.len() as u64 > max_bytes {
            return Err(MediaTransportError::Oversized);
        }
        Ok(MediaResponse {
            status,
            content_length,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{MEDIA_ROUTE_PREFIX, MediaHttp};
    use cos_client::media::{
        Cancellation, MEDIA_CONTEXT_HEADER, MEDIA_DESCRIPTOR_PATH, MEDIA_HANDLE_HEADER,
        MediaExecutor, MediaFetcher, MediaTransportError,
    };
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    /// One scripted loopback response.
    struct Stub {
        status: &'static str,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
        /// Whether to declare `Content-Length` (false delimits the body by EOF).
        declared_length: bool,
    }

    impl Stub {
        fn json_ok(body: &str) -> Self {
            Self {
                status: "HTTP/1.1 200 OK",
                headers: vec![("Content-Type", "application/json".to_owned())],
                body: body.as_bytes().to_vec(),
                declared_length: true,
            }
        }

        fn status(status: &'static str) -> Self {
            Self {
                status,
                headers: Vec::new(),
                body: br#"{"code":"MEDIA_UNAUTHORIZED"}"#.to_vec(),
                declared_length: true,
            }
        }

        fn bytes(body: Vec<u8>) -> Self {
            Self {
                status: "HTTP/1.1 200 OK",
                headers: vec![("Content-Type", "image/png".to_owned())],
                body,
                declared_length: true,
            }
        }

        fn streamed(body: Vec<u8>) -> Self {
            Self {
                declared_length: false,
                ..Self::bytes(body)
            }
        }

        fn redirect(location: String) -> Self {
            Self {
                status: "HTTP/1.1 302 Found",
                headers: vec![("Location", location)],
                body: Vec::new(),
                declared_length: true,
            }
        }

        /// Render the complete response bytes.
        fn render(&self) -> Vec<u8> {
            let mut response = format!("{}\r\n", self.status);
            for (name, value) in &self.headers {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            if self.declared_length {
                response.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
            }
            response.push_str("Connection: close\r\n\r\n");
            let mut bytes = response.into_bytes();
            bytes.extend_from_slice(&self.body);
            bytes
        }
    }

    fn bind() -> (TcpListener, std::net::SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener binds");
        let address = listener.local_addr().expect("listener address");
        (listener, address)
    }

    fn read_request(stream: &mut TcpStream) -> String {
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
        String::from_utf8_lossy(&request).to_ascii_lowercase()
    }

    /// Serve one request and capture it (lowercased for asserting headers).
    fn serve_once(stub: Stub) -> (String, std::thread::JoinHandle<String>) {
        let (listener, address) = bind();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request accepted");
            let request = read_request(&mut stream);
            let _ = stream.write_all(&stub.render());
            let _ = stream.flush();
            request
        });
        (format!("http://{address}"), handle)
    }

    /// Serve two requests in order (a descriptor read then a byte read).
    fn serve_twice(
        first: Stub,
        second: Stub,
    ) -> (String, std::thread::JoinHandle<(String, String)>) {
        let (listener, address) = bind();
        let handle = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for stub in [first, second] {
                let (mut stream, _) = listener.accept().expect("request accepted");
                let request = read_request(&mut stream);
                let _ = stream.write_all(&stub.render());
                let _ = stream.flush();
                requests.push(request);
            }
            (requests[0].clone(), requests[1].clone())
        });
        (format!("http://{address}"), handle)
    }

    /// A descriptor document for a verified PNG payload.
    fn descriptor_body(
        server: &str,
        audience: &str,
        digest: &str,
        expected_hex: &str,
        handle: &str,
        length: usize,
    ) -> String {
        format!(
            r#"{{"descriptor_id":"descriptor-1","artifact_ref":"artifact-1",
"artifact_digest":"{digest}","representation":"ORIGINAL","representation_version":1,
"media_type":"image/png","byte_length":{length},"width":1,"height":1,
"availability":"AVAILABLE",
"verification":{{"state":"VERIFIED","algorithm":"sha256","expected_digest":"{expected_hex}"}},
"access_handle":"{handle}","server_identity":"{server}","audience":"{audience}",
"expires_at_unix_ms":4102444800000}}"#
        )
    }

    const SERVER: &str = "server-1";
    const AUDIENCE: &str = "desktop:0123456789abcdef0123456789abcdef";
    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// A bounded PNG payload; admission reads only the container signature.
    fn png() -> Vec<u8> {
        let mut bytes = vec![
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H',
            b'D', b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02,
        ];
        bytes.extend_from_slice(&[0_u8; 12]);
        bytes
    }

    /// SHA-256 hex of one payload through the same admission path the client uses.
    fn expected_hex_and_digest(bytes: &[u8]) -> (String, media::Sha256Digest) {
        let admitted = media::admit_remote(
            bytes,
            &media::RemoteExpectation {
                byte_length: Some(bytes.len() as u64),
                media_type: Some("image/png".to_owned()),
                sha256: None,
                verified: false,
            },
            1024 * 1024,
            &media::Cancellation::default(),
        )
        .expect("unverified admission");
        let mut text = String::with_capacity(64);
        for byte in admitted.digest().0 {
            text.push_str(&format!("{byte:02x}"));
        }
        (text, admitted.digest())
    }

    fn repository() -> app_core::MediaRepository {
        app_core::MediaRepository::new(media::LocalLimits {
            max_input_bytes: 1024 * 1024,
            max_live_bytes: 4 * 1024 * 1024,
            max_live_entries: 4,
            max_pixels: 16_777_216,
        })
        .expect("repository limits")
    }

    #[test]
    fn the_route_prefix_is_the_descriptor_prefix_plus_one_segment() {
        assert_eq!(
            MEDIA_ROUTE_PREFIX,
            format!("{MEDIA_DESCRIPTOR_PATH}/"),
            "the executor route family must stay the client's descriptor path"
        );
    }

    #[test]
    fn a_media_request_is_joined_onto_the_validated_loopback_origin_with_the_bearer() {
        let body = png();
        let (hex, _) = expected_hex_and_digest(&body);
        let (origin, server) = serve_once(Stub::json_ok(&descriptor_body(
            SERVER,
            AUDIENCE,
            DIGEST,
            &hex,
            "/api/cos/media/mh1.opaque/blob",
            body.len(),
        )));
        let mut executor = MediaHttp::new(&origin, "local-test-token").expect("loopback origin");
        let response = executor
            .get(
                &format!("/api/cos/media/{DIGEST}"),
                &[
                    ("Accept", "application/json"),
                    (MEDIA_CONTEXT_HEADER, "read-context-value"),
                ],
                64 * 1024,
                &Cancellation::default(),
            )
            .expect("descriptor response");
        assert_eq!(response.status, 200);
        assert_eq!(
            response.content_length,
            Some(response.body.len() as u64),
            "the executor reports the declared length it saw"
        );
        let seen = server.join().expect("server finished");
        assert!(
            seen.starts_with(&format!("get /api/cos/media/{DIGEST}")),
            "{seen}"
        );
        assert!(
            seen.contains("authorization: bearer local-test-token"),
            "the bearer is attached on the same-authority media route: {seen}"
        );
        assert!(seen.contains("x-cos-context: read-context-value"), "{seen}");
    }

    #[test]
    fn a_byte_request_presents_the_handle_header_and_no_context() {
        let (origin, server) = serve_once(Stub::bytes(png()));
        let mut executor = MediaHttp::new(&origin, "token").expect("loopback origin");
        let response = executor
            .get(
                "/api/cos/media/mh1.opaque/blob",
                &[
                    ("Accept", "image/png"),
                    (MEDIA_HANDLE_HEADER, "/api/cos/media/mh1.opaque/blob"),
                ],
                1024 * 1024,
                &Cancellation::default(),
            )
            .expect("byte response");
        assert_eq!(response.status, 200);
        let seen = server.join().expect("server finished");
        assert!(
            seen.contains("x-media-handle: /api/cos/media/mh1.opaque/blob"),
            "{seen}"
        );
        assert!(!seen.contains("x-cos-context"), "{seen}");
    }

    #[test]
    fn an_absolute_or_non_media_path_is_refused_before_a_socket_is_opened() {
        let (listener, address) = bind();
        let mut executor =
            MediaHttp::new(&format!("http://{address}"), "token").expect("loopback origin");
        for path in [
            "http://127.0.0.1:1/api/cos/media/x",
            "//127.0.0.1:1/api/cos/media/x",
            "/api/cos/desktop/server",
            "/api/cos/media/../escape",
            "/api/cos/media/%2e%2e/escape",
            "/api/cos/media/x/blob?token=1",
            "/api/cos/media/x\\blob",
            "/api/cos/media/x/blob#fragment",
            "",
        ] {
            assert_eq!(
                executor.get(path, &[], 64, &Cancellation::default()),
                Err(MediaTransportError::Transport),
                "{path}"
            );
        }
        listener
            .set_nonblocking(true)
            .expect("listener non-blocking");
        assert!(
            listener.accept().is_err(),
            "no refused request may reach the socket"
        );
    }

    #[test]
    fn a_caller_supplied_authority_header_is_refused_instead_of_merged() {
        let (listener, address) = bind();
        let mut executor =
            MediaHttp::new(&format!("http://{address}"), "token").expect("loopback origin");
        for name in [
            "Authorization",
            "authorization",
            "Host",
            "Proxy-Authorization",
        ] {
            assert_eq!(
                executor.get(
                    "/api/cos/media/x",
                    &[(name, "Bearer other")],
                    64,
                    &Cancellation::default()
                ),
                Err(MediaTransportError::Transport),
                "{name} must be refused"
            );
        }
        drop(listener);
    }

    #[test]
    fn an_unsupported_endpoint_is_never_adapted_into_an_executor() {
        for endpoint in [
            "https://127.0.0.1:8127",
            "http://localhost:8127",
            "http://example.invalid:8127",
            "http://192.0.2.1:8127",
            "http://127.0.0.1:8127/path",
            "http://user@127.0.0.1:8127",
            "http://127.0.0.1:0",
            "",
        ] {
            assert!(
                MediaHttp::new(endpoint, "token").is_none(),
                "{endpoint} must not produce an executor"
            );
        }
        assert!(
            MediaHttp::new("http://127.0.0.1:8127", "").is_none(),
            "an empty credential never produces an executor"
        );
    }

    #[test]
    fn a_redirect_is_returned_not_followed() {
        let (stolen_listener, stolen_address) = bind();
        let (origin, server) = serve_once(Stub::redirect(format!(
            "http://{stolen_address}/api/cos/media/steal"
        )));
        let mut executor = MediaHttp::new(&origin, "token").expect("loopback origin");
        let response = executor
            .get("/api/cos/media/x", &[], 64, &Cancellation::default())
            .expect("a 3xx is a response");
        assert_eq!(response.status, 302);
        let seen = server.join().expect("server finished");
        assert!(seen.starts_with("get /api/cos/media/x"), "{seen}");
        stolen_listener
            .set_nonblocking(true)
            .expect("listener non-blocking");
        assert!(
            stolen_listener.accept().is_err(),
            "a redirect target is never contacted"
        );
    }

    #[test]
    fn a_streamed_body_over_the_bound_is_refused_without_an_unbounded_read() {
        let (origin, server) = serve_once(Stub::streamed(vec![b'x'; 64 * 1024]));
        let mut executor = MediaHttp::new(&origin, "token").expect("loopback origin");
        assert_eq!(
            executor.get("/api/cos/media/x", &[], 16, &Cancellation::default()),
            Err(MediaTransportError::Oversized)
        );
        let _ = server.join();
    }

    #[test]
    fn a_declared_body_over_the_bound_is_refused_before_reading_it() {
        let (origin, server) = serve_once(Stub::bytes(vec![b'x'; 1024]));
        let mut executor = MediaHttp::new(&origin, "token").expect("loopback origin");
        assert_eq!(
            executor.get("/api/cos/media/x", &[], 16, &Cancellation::default()),
            Err(MediaTransportError::Oversized)
        );
        let _ = server.join();
    }

    #[test]
    fn a_status_is_returned_to_the_client_mapper() {
        let (origin, server) = serve_once(Stub::status("HTTP/1.1 410 Gone"));
        let mut executor = MediaHttp::new(&origin, "token").expect("loopback origin");
        let response = executor
            .get("/api/cos/media/x", &[], 64, &Cancellation::default())
            .expect("a status is a response");
        assert_eq!(response.status, 410);
        let _ = server.join();
    }

    #[test]
    fn cancellation_before_the_request_opens_no_socket() {
        let (listener, address) = bind();
        let mut executor =
            MediaHttp::new(&format!("http://{address}"), "token").expect("loopback origin");
        let cancellation = Cancellation::default();
        cancellation.cancel();
        assert_eq!(
            executor.get("/api/cos/media/x", &[], 64, &cancellation),
            Err(MediaTransportError::Transport)
        );
        listener
            .set_nonblocking(true)
            .expect("listener non-blocking");
        assert!(listener.accept().is_err());
    }

    #[test]
    fn errors_carry_no_token_path_or_handle_value() {
        let (listener, address) = bind();
        let mut executor = MediaHttp::new(&format!("http://{address}"), "sentinel-token-value")
            .expect("loopback origin");
        // Drop the listener so the connection is refused.
        drop(listener);
        std::thread::sleep(Duration::from_millis(20));
        let outcome = executor.get(
            "/api/cos/media/sentinel-handle/blob",
            &[],
            64,
            &Cancellation::default(),
        );
        assert_eq!(outcome, Err(MediaTransportError::Transport));
        let rendered = format!("{outcome:?}");
        assert!(!rendered.contains("sentinel-token-value"), "{rendered}");
        assert!(!rendered.contains("sentinel-handle"), "{rendered}");
        assert!(!rendered.contains(&format!("{address}")), "{rendered}");
    }

    /// End-to-end at the binary boundary: the real executor, the real client
    /// descriptor/byte path and app-core's repository/admission, against a stub
    /// server. This is the presentation path the shell invokes.
    #[test]
    fn the_repository_admits_bytes_fetched_through_the_real_executor() {
        let body = png();
        let (hex, digest) = expected_hex_and_digest(&body);
        let descriptor = descriptor_body(
            SERVER,
            AUDIENCE,
            DIGEST,
            &hex,
            "/api/cos/media/mh1.opaque/blob",
            body.len(),
        );
        let (origin, server) = serve_twice(Stub::json_ok(&descriptor), Stub::bytes(body.clone()));
        let executor = MediaHttp::new(&origin, "local-test-token").expect("loopback origin");
        let mut source = MediaFetcher::new(executor, SERVER, AUDIENCE);
        let mut repository = repository();
        let artifact = desktop_protocol::ArtifactSnapshot {
            artifact_ref: desktop_protocol::ArtifactId::new("artifact-1").expect("artifact id"),
            artifact_digest: desktop_protocol::ArtifactDigest::new(DIGEST).expect("digest"),
            media_type: Some("image/png".to_owned()),
            availability: desktop_protocol::Availability::Available,
            descriptor_refs: Vec::new(),
            verification: None,
            projection_revision: desktop_protocol::Counter::from_u128(1),
        };
        let session = app_core::MediaSessionContext::new(
            desktop_protocol::ServerId::new(SERVER).expect("server"),
            AUDIENCE,
            desktop_protocol::EpochId::new("epoch-1").expect("epoch"),
            "read-context-value",
        );
        let selection = app_core::MediaSelection::for_artifact(
            &artifact,
            session,
            desktop_protocol::RepresentationKind::Original,
        );
        let presentation = repository.load(&mut source, &selection, &Cancellation::default());
        let admitted = presentation.admitted().expect("bytes admitted");
        assert!(admitted.identity.verified(), "the descriptor was verified");
        assert_eq!(admitted.bytes.as_ref(), body.as_slice());
        assert_eq!(admitted.computed_digest, digest);
        let (descriptor_request, byte_request) = server.join().expect("server finished");
        assert!(
            descriptor_request.contains("x-cos-context: read-context-value"),
            "{descriptor_request}"
        );
        assert!(
            !descriptor_request.contains("x-media-handle"),
            "{descriptor_request}"
        );
        assert!(
            byte_request.contains("x-media-handle: /api/cos/media/mh1.opaque/blob"),
            "{byte_request}"
        );
        assert!(!byte_request.contains("x-cos-context"), "{byte_request}");
        assert!(
            descriptor_request.contains("authorization: bearer local-test-token")
                && byte_request.contains("authorization: bearer local-test-token"),
            "the bearer is attached to both same-authority media routes"
        );
    }

    /// Live evidence (ignored by default; the harness sets the environment).
    ///
    /// It never prints or records the token or an access handle. It writes one
    /// value-free summary to `COS_DESKTOP_MEDIA_LIVE_OUT` when provided.
    #[test]
    #[ignore = "live Runtime evidence; run through the media integration harness"]
    fn live_media_evidence() {
        let Some(endpoint) = std::env::var("COS_DESKTOP_MEDIA_LIVE_ENDPOINT").ok() else {
            return;
        };
        let token = std::env::var("COS_DESKTOP_MEDIA_LIVE_TOKEN").expect("token");
        let other_token = std::env::var("COS_DESKTOP_MEDIA_LIVE_OTHER_TOKEN").expect("other token");
        let context = std::env::var("COS_DESKTOP_MEDIA_LIVE_CONTEXT").expect("context");
        let digest = std::env::var("COS_DESKTOP_MEDIA_LIVE_DIGEST").expect("digest");
        let server = std::env::var("COS_DESKTOP_MEDIA_LIVE_SERVER").expect("server identity");
        let epoch = std::env::var("COS_DESKTOP_MEDIA_LIVE_EPOCH").expect("epoch");
        let artifact_ref = std::env::var("COS_DESKTOP_MEDIA_LIVE_ARTIFACT").expect("artifact ref");
        let audience = std::env::var("COS_DESKTOP_MEDIA_LIVE_AUDIENCE").expect("audience");
        let expired_handle = std::env::var("COS_DESKTOP_MEDIA_LIVE_EXPIRED_HANDLE").ok();
        let foreign_handle = std::env::var("COS_DESKTOP_MEDIA_LIVE_FOREIGN_HANDLE").ok();
        let out = std::env::var("COS_DESKTOP_MEDIA_LIVE_OUT").ok();
        let cancellation = Cancellation::default();
        let mut lines: Vec<String> = Vec::new();

        let executor = MediaHttp::new(&endpoint, &token).expect("validated loopback origin");
        let mut fetcher = MediaFetcher::new(executor, server.clone(), audience.clone());
        let descriptor = fetcher
            .fetch_descriptor(&digest, &context, &cancellation)
            .expect("descriptor fetched");
        lines.push(format!(
            "\"descriptor_id\":{}",
            json_string(descriptor.descriptor_id.as_str())
        ));
        lines.push(format!(
            "\"representation\":{}",
            json_string(&format!("{:?}", descriptor.representation))
        ));
        lines.push(format!(
            "\"media_type\":{}",
            json_string(descriptor.media_type.as_deref().unwrap_or(""))
        ));
        lines.push(format!(
            "\"byte_length\":{}",
            descriptor
                .byte_length
                .map_or_else(|| "null".to_owned(), |value| value.to_string())
        ));
        lines.push(format!(
            "\"width\":{}",
            descriptor
                .width
                .map_or_else(|| "null".to_owned(), |value| value.to_string())
        ));
        lines.push(format!(
            "\"height\":{}",
            descriptor
                .height
                .map_or_else(|| "null".to_owned(), |value| value.to_string())
        ));
        lines.push(format!(
            "\"availability\":{}",
            json_string(&format!("{:?}", descriptor.availability))
        ));
        let verification_hex = descriptor
            .verification
            .as_ref()
            .and_then(|summary| summary.expected_digest.clone())
            .unwrap_or_default();
        lines.push(format!(
            "\"verification_state\":{}",
            json_string(
                &descriptor
                    .verification
                    .as_ref()
                    .map_or_else(String::new, |summary| format!("{:?}", summary.state))
            )
        ));
        lines.push(format!(
            "\"verification_algorithm\":{}",
            json_string(
                &descriptor
                    .verification
                    .as_ref()
                    .and_then(|summary| summary.algorithm.clone())
                    .unwrap_or_default()
            )
        ));
        lines.push(format!(
            "\"expected_digest\":{}",
            json_string(&verification_hex)
        ));
        lines.push(format!(
            "\"access_handle_present\":{}",
            descriptor.access_handle.is_some()
        ));
        lines.push(format!(
            "\"access_handle_length\":{}",
            descriptor
                .access_handle
                .as_ref()
                .map_or(0, |handle| handle.len())
        ));
        lines.push(format!(
            "\"expires_at_unix_ms\":{}",
            descriptor
                .expires_at_unix_ms
                .map_or_else(|| "null".to_owned(), |value| value.to_string())
        ));

        let fetched = fetcher
            .fetch_bytes(&descriptor, &cancellation)
            .expect("bytes fetched");
        let declared_sha = cos_client::media::expected_sha256(&descriptor)
            .expect("verification summary")
            .map(media::Sha256Digest);
        let expectation = media::RemoteExpectation {
            byte_length: descriptor.byte_length,
            media_type: descriptor.media_type.clone(),
            sha256: declared_sha,
            verified: declared_sha.is_some(),
        };
        let budget = usize::try_from(cos_client::media::representation_budget(
            &descriptor.representation,
        ))
        .expect("budget fits");
        let admitted = media::admit_remote(
            &fetched.bytes,
            &expectation,
            budget,
            &media::Cancellation::default(),
        )
        .expect("bytes admitted");
        let computed_hex = {
            let mut text = String::with_capacity(64);
            for byte in admitted.digest().0 {
                text.push_str(&format!("{byte:02x}"));
            }
            text
        };
        lines.push(format!("\"fetched_bytes\":{}", fetched.bytes.len()));
        lines.push(format!(
            "\"computed_digest\":{}",
            json_string(&computed_hex)
        ));
        lines.push(format!("\"admitted_verified\":{}", admitted.verified()));
        lines.push(format!(
            "\"digest_matches_declared\":{}",
            !verification_hex.is_empty() && verification_hex == computed_hex
        ));
        match media::decode_raster(
            admitted.bytes(),
            media::DecodeBudget::ratified_default()
                .limits()
                .expect("decode limits"),
            &media::Cancellation::default(),
        ) {
            Ok(raster) => lines.push(format!(
                "\"decoded\":{{\"width\":{},\"height\":{},\"rgba_bytes\":{}}}",
                raster.size.width(),
                raster.size.height(),
                raster.rgba.len()
            )),
            Err(error) => lines.push(format!(
                "\"decode_error\":{}",
                json_string(&format!("{error:?}"))
            )),
        }
        // The exact presentation request the shell builds; its refusal (if any)
        // is recorded rather than hidden, because the pinned admitted class is
        // still the 2048x2048 media proof.
        let identity = app_core::MediaIdentity {
            server_identity: desktop_protocol::ServerId::new(server.clone()).expect("server"),
            audience: audience.clone(),
            epoch: desktop_protocol::EpochId::new(epoch.clone()).expect("epoch"),
            artifact_ref: desktop_protocol::ArtifactId::new(artifact_ref.clone())
                .expect("artifact ref"),
            artifact_digest: desktop_protocol::ArtifactDigest::new(digest.clone()).expect("digest"),
            revision: desktop_protocol::Counter::from_u128(1),
            representation: descriptor.representation.clone(),
            representation_version: descriptor.representation_version,
            expected_sha256: declared_sha,
            byte_length: descriptor.byte_length,
            width: descriptor.width,
            height: descriptor.height,
        };
        let admitted_media = app_core::AdmittedMedia {
            identity,
            bytes: std::sync::Arc::from(fetched.bytes.clone()),
            computed_digest: admitted.digest(),
            max_bytes: budget,
        };
        match desktop_gpui::media_presentation::media_upload_request(&admitted_media, 1) {
            Ok(_) => lines.push("\"presentation\":\"request\"".to_owned()),
            Err(refusal) => lines.push(format!(
                "\"presentation\":{}",
                json_string(&format!("{refusal:?}"))
            )),
        }

        // Negative matrix through the same real executor and client mapping.
        let mut negatives: Vec<String> = Vec::new();
        let bogus = fetcher.fetch_descriptor(&digest, "not-a-live-context", &cancellation);
        negatives.push(format!(
            "\"descriptor_bogus_context\":{}",
            json_string(&format!("{bogus:?}"))
        ));
        let mut foreign = MediaFetcher::new(
            MediaHttp::new(&endpoint, &other_token).expect("loopback origin"),
            server.clone(),
            audience.clone(),
        );
        let denied = foreign.fetch_descriptor(&digest, &context, &cancellation);
        negatives.push(format!(
            "\"descriptor_foreign_principal\":{}",
            json_string(&format!("{denied:?}"))
        ));
        for (label, handle) in [
            ("expired_handle", expired_handle),
            ("foreign_handle", foreign_handle),
        ] {
            let Some(handle) = handle else {
                negatives.push(format!("\"{label}\":\"NOT_PROVIDED\""));
                continue;
            };
            let mut crafted = descriptor.clone();
            crafted.access_handle = Some(handle);
            crafted.byte_length = None;
            crafted.verification = None;
            let outcome = fetcher.fetch_bytes(&crafted, &cancellation);
            negatives.push(format!(
                "\"{label}\":{}",
                json_string(&format!("{outcome:?}"))
            ));
        }
        lines.push(format!("\"negatives\":{{{}}}", negatives.join(",")));

        let document = format!("{{{}}}", lines.join(","));
        if let Some(path) = out {
            std::fs::write(path, &document).expect("live evidence written");
        }
        println!("{document}");
    }

    /// Minimal JSON string escaping for the value-free evidence writer.
    fn json_string(value: &str) -> String {
        let mut text = String::with_capacity(value.len() + 2);
        text.push('"');
        for character in value.chars() {
            match character {
                '"' => text.push_str("\\\""),
                '\\' => text.push_str("\\\\"),
                '\n' => text.push_str("\\n"),
                '\r' => text.push_str("\\r"),
                '\t' => text.push_str("\\t"),
                character if character.is_control() => {
                    text.push_str(&format!("\\u{:04x}", character as u32));
                }
                character => text.push(character),
            }
        }
        text.push('"');
        text
    }
}
