//! The typed Platform public-API boundary.
//!
//! Everything this service says to Platform crosses here: capture submission with its mandatory
//! idempotency key, operation reads, the per-operation SSE progress stream, and the identity
//! assertion this service signs so a sender can act on Platform as themselves. Only this crate
//! knows Platform's URL shapes and wire field names; callers see typed values and one closed
//! error taxonomy, mirroring how `ratatoskr-telegram-bot-api` frames the Telegram side.
//!
//! The signing key for assertions is configuration secret: it enters through [`assertion`]
//! construction and never renders in errors or logs.

pub mod assertion;
pub mod session;

pub use session::{Clock, SessionSource};

use std::str::FromStr as _;
use std::time::Duration;

use serde::Deserialize;
use url::Url;
use uuid::Uuid;

/// Why one call to Platform failed. Closed, safe to log, and free of credential material: no
/// variant carries the presented bearer, the signing key, or a response body verbatim.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// The request never completed: connection refused, reset, TLS, DNS.
    #[error("the platform request failed at the transport level")]
    Network(#[source] reqwest::Error),
    /// The whole-call timeout elapsed before an answer.
    #[error("the platform request exceeded its timeout")]
    Timeout,
    /// Platform refused the credential, uniformly: missing, unknown, revoked, expired, or
    /// wrong-audience sessions are indistinguishable by platform contract.
    #[error("platform refused the credential")]
    Unauthenticated,
    /// The operation does not exist or belongs to another principal — one refusal, per contract.
    #[error("platform reports no such operation for this principal")]
    NotFound,
    /// The idempotency key is already bound to a different body, or still in flight.
    #[error("platform refused the request on idempotency grounds")]
    Conflict,
    /// The caller spent its allowance; retry after waiting.
    #[error("platform rate-limited the caller")]
    RateLimited,
    /// Any other 4xx answer: a client-class mistake this build did not anticipate.
    #[error("platform answered with a client error status")]
    ClientError {
        /// The HTTP status code, for the class-only log line.
        status: u16,
    },
    /// A 5xx answer: transient on Platform's side.
    #[error("platform answered with a server error status")]
    ServerError {
        /// The HTTP status code, for the class-only log line.
        status: u16,
    },
    /// The body was not the JSON the typed view expects.
    #[error("a platform response did not match its expected shape")]
    Json(#[source] serde_json::Error),
    /// An SSE frame was not parseable into a progress entry.
    #[error("a platform progress frame was malformed")]
    MalformedFrame,
}

/// What `POST /v1/captures` returns on acceptance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationAccepted {
    /// The operation to follow. Replays of the same key return the original id.
    pub operation_id: Uuid,
}

/// One progress entry from an operation's event stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressFrame {
    /// The persisted entry's identifier — the deduplication key and resume cursor.
    pub event_id: Uuid,
    /// The closed lifecycle status reached.
    pub status: OperationStatus,
    /// Producer display text for the phase, when given. Untrusted upstream.
    pub stage: Option<String>,
    /// Whole-percent estimate, when given.
    pub progress_percent: Option<u8>,
    /// A user-safe message, when the producer gave one. Untrusted upstream.
    pub message: Option<String>,
    /// When the entry was observed, whole seconds since the Unix epoch.
    pub observed_at_secs: i64,
}

/// A live Server-Sent-Events stream of one operation's progress.
#[derive(Debug)]
pub struct EventStream {
    response: Option<reqwest::Response>,
    parser: SseParser,
    terminal_seen: bool,
}

impl EventStream {
    /// The next frame, or `None` once the terminal frame has been delivered and nothing
    /// remains.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] if the transport or a frame fails mid-stream.
    pub async fn next_frame(&mut self) -> Result<Option<ProgressFrame>, PlatformError> {
        loop {
            if self.terminal_seen {
                return Ok(None);
            }
            if let Some(frame) = self.parser.next_progress_frame()? {
                self.terminal_seen |= frame.status.is_terminal();
                return Ok(Some(frame));
            }
            let Some(response) = self.response.as_mut() else {
                return Ok(None);
            };
            match response.chunk().await {
                Ok(Some(bytes)) => self.parser.feed(&bytes),
                Ok(None) => {
                    self.response = None;
                    return Ok(None);
                }
                Err(error) => {
                    return Err(if error.is_timeout() {
                        PlatformError::Timeout
                    } else {
                        PlatformError::Network(error)
                    });
                }
            }
        }
    }
}

/// An incremental `text/event-stream` parser over arbitrarily chunked bytes. Comments are
/// skipped, multi-line data joins with newlines, and only `progress` events surface.
#[derive(Default, Debug)]
struct SseParser {
    buffer: String,
}

impl SseParser {
    fn feed(&mut self, bytes: &[u8]) {
        self.buffer.push_str(&String::from_utf8_lossy(bytes));
    }

    /// The next complete `progress` frame, or `None` when more bytes are needed.
    fn next_progress_frame(&mut self) -> Result<Option<ProgressFrame>, PlatformError> {
        loop {
            let Some((event_end, consumed)) = complete_event(&self.buffer) else {
                return Ok(None);
            };
            let raw: String = self.buffer.drain(..consumed).collect();
            let Some(event_text) = raw.get(..event_end) else {
                return Err(PlatformError::MalformedFrame);
            };
            // A comment or another event name parses to None; keep scanning.
            if let Some(frame) = parse_sse_event(event_text)? {
                return Ok(Some(frame));
            }
        }
    }
}

/// Find one complete event in `buffer`: the text before its blank-line terminator and the total
/// number of bytes the terminator consumes. Handles both `\n` and `\r\n` framing in one pass,
/// so whichever terminator comes first wins.
fn complete_event(buffer: &str) -> Option<(usize, usize)> {
    let bytes = buffer.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        match (bytes.get(index + 1), bytes.get(index + 2)) {
            (Some(b'\n'), _) => return Some((index, index + 2)),
            (Some(b'\r'), Some(b'\n')) => return Some((index, index + 3)),
            _ => {}
        }
    }
    None
}

/// Parse one event's text into a frame: `None` for comments and non-`progress` names.
fn parse_sse_event(event: &str) -> Result<Option<ProgressFrame>, PlatformError> {
    let mut id: Option<String> = None;
    let mut name: Option<String> = None;
    let mut data_lines: Vec<&str> = Vec::new();
    for line in event.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "id" => id = Some(value.to_owned()),
            "event" => name = Some(value.to_owned()),
            "data" => data_lines.push(value),
            _ => {}
        }
    }
    if name.as_deref() != Some("progress") || data_lines.is_empty() {
        return Ok(None);
    }
    let wire: FrameWire =
        serde_json::from_str(&data_lines.join("\n")).map_err(|_| PlatformError::MalformedFrame)?;
    let event_id = id.ok_or(PlatformError::MalformedFrame)?;
    let observed = jiff::Timestamp::from_str(&wire.observed_at)
        .map_err(|_| PlatformError::MalformedFrame)?
        .as_second();
    Ok(Some(ProgressFrame {
        event_id: event_id
            .parse()
            .map_err(|_| PlatformError::MalformedFrame)?,
        status: OperationStatus::parse(&wire.status)?,
        stage: wire.stage,
        progress_percent: wire.progress_percent,
        message: wire.message,
        observed_at_secs: observed,
    }))
}

#[derive(Deserialize)]
struct FrameWire {
    status: String,
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    progress_percent: Option<u8>,
    #[serde(default)]
    message: Option<String>,
    observed_at: String,
}

/// The closed lifecycle vocabulary Platform serves. An unknown value refuses to parse rather
/// than being guessed into the nearest known state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationStatus {
    /// Received and durably recorded; no work started.
    Accepted,
    /// Scheduled, waiting for capacity.
    Queued,
    /// Executing.
    Running,
    /// Terminal: every requested effect was produced.
    Succeeded,
    /// Terminal: some effects produced; warnings explain the rest.
    PartiallySucceeded,
    /// Terminal: no usable effect; errors explain why.
    Failed,
    /// Terminal: stopped before completion.
    Cancelled,
}

impl OperationStatus {
    /// Parse the wire label, refusing anything outside the vocabulary.
    ///
    /// # Errors
    ///
    /// [`PlatformError::MalformedFrame`] for an unknown label.
    pub fn parse(label: &str) -> Result<Self, PlatformError> {
        match label {
            "accepted" => Ok(Self::Accepted),
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "partially_succeeded" => Ok(Self::PartiallySucceeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(PlatformError::MalformedFrame),
        }
    }

    /// Whether this state ends the operation's lifecycle.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::PartiallySucceeded | Self::Failed | Self::Cancelled
        )
    }
}

/// The typed client. Cheap to clone; every call carries its own session credential.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: Url,
}

impl Client {
    /// Build a client against `base_url` with a whole-call timeout.
    ///
    /// # Errors
    ///
    /// [`PlatformError::Network`] if the underlying HTTP client cannot build, which in practice
    /// means TLS backend initialization failed.
    pub fn new(base_url: &Url, timeout: Duration) -> Result<Self, PlatformError> {
        let http = reqwest::ClientBuilder::new()
            .timeout(timeout)
            .use_rustls_tls()
            .build()
            .map_err(PlatformError::Network)?;
        Ok(Self {
            http,
            base_url: base_url.clone(),
        })
    }

    /// Exchange one signed assertion for a Platform session credential.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] per the taxonomy above.
    pub async fn exchange_assertion(
        &self,
        assertion: &str,
    ) -> Result<SessionMinted, PlatformError> {
        let response = self
            .send(
                self.http
                    .post(self.url("/v1/sessions/telegram"))
                    .json(&ExchangeAssertionWire { assertion }),
            )
            .await?;
        let bytes = response.bytes().await.map_err(PlatformError::Network)?;
        let wire: SessionMintedWire =
            serde_json::from_slice(&bytes).map_err(PlatformError::Json)?;
        Ok(SessionMinted {
            credential: secrecy::SecretString::new(wire.credential.into()),
            expires_at: wire.expires_at,
        })
    }

    /// Follow one operation's progress over Server-Sent Events. The returned stream yields
    /// frames in arrival order and ends once a terminal-status frame has been delivered;
    /// reconnecting with the last seen event id resumes after that entry.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] per the taxonomy above.
    pub async fn stream_events(
        &self,
        session: &str,
        operation_id: Uuid,
        last_event_id: Option<&str>,
    ) -> Result<EventStream, PlatformError> {
        let mut builder = self
            .http
            .get(self.url(&format!("/v1/operations/{operation_id}/events")))
            .bearer_auth(session);
        if let Some(last) = last_event_id {
            builder = builder.header("last-event-id", last);
        }
        let response = self.send(builder).await?;
        Ok(EventStream {
            response: Some(response),
            parser: SseParser::default(),
            terminal_seen: false,
        })
    }

    /// Read one operation snapshot. Replays of the same key return the original operation.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] per the taxonomy above.
    pub async fn read_operation(
        &self,
        session: &str,
        operation_id: Uuid,
    ) -> Result<OperationSnapshotView, PlatformError> {
        let response = self
            .send(
                self.http
                    .get(self.url(&format!("/v1/operations/{operation_id}")))
                    .bearer_auth(session),
            )
            .await?;
        let body = response.bytes().await.map_err(PlatformError::Network)?;
        let parsed: SnapshotWire = serde_json::from_slice(&body).map_err(PlatformError::Json)?;
        Ok(OperationSnapshotView {
            status: OperationStatus::parse(&parsed.status)?,
            stage: parsed.stage,
            progress_percent: parsed.progress_percent,
            message: parsed.message,
            errors: parsed.errors,
            warnings: parsed.warnings,
        })
    }

    fn url(&self, path: &str) -> Url {
        let mut joined = self.base_url.clone();
        joined.set_path(path);
        joined
    }

    async fn send(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, PlatformError> {
        let response = builder.send().await.map_err(|error| {
            if error.is_timeout() {
                PlatformError::Timeout
            } else {
                PlatformError::Network(error)
            }
        })?;
        classify(response)
    }

    /// Submit one capture. The idempotency key is mandatory upstream; replaying the same key with
    /// the same body returns the original operation.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] per the taxonomy above.
    pub async fn submit_capture(
        &self,
        session: &str,
        submission: &CaptureSubmission,
    ) -> Result<OperationAccepted, PlatformError> {
        // URL captures keep their exact historical body; blob captures reference stored bytes
        // instead; provenance is additive so tolerant servers ignore what they do not know.
        let mut body = match &submission.source {
            CaptureSource::Url(url) => serde_json::json!({ "url": url }),
            CaptureSource::Blob {
                owner_service,
                digest_hex,
                media_type,
                length_bytes,
            } => serde_json::json!({
                "blob": {
                    "owner_service": owner_service,
                    "digest": { "algorithm": "sha256", "hex": digest_hex },
                    "media_type": media_type,
                    "length_bytes": length_bytes,
                }
            }),
        };
        if let (Some(map), Some(origin)) = (body.as_object_mut(), submission.origin.as_ref()) {
            map.insert("origin".to_owned(), origin.clone());
        }

        let response = self
            .send(
                self.http
                    .post(self.url("/v1/captures"))
                    .bearer_auth(session)
                    .header("idempotency-key", &submission.idempotency_key)
                    .json(&body),
            )
            .await?;
        let body = response.bytes().await.map_err(PlatformError::Network)?;
        let parsed: CaptureAcceptedWire =
            serde_json::from_slice(&body).map_err(PlatformError::Json)?;
        Ok(OperationAccepted {
            operation_id: parsed.operation_id,
        })
    }
}

/// What one capture presents: an address, or this deployment's own stored bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureSource {
    /// A submitted address.
    Url(String),
    /// Stored bytes in this deployment's own blob store. Field-for-field the fleet `BlobRef` wire
    /// shape with the digest algorithm fixed at `sha256`.
    Blob {
        /// The owner service whose store holds the bytes.
        owner_service: String,
        /// Lowercase hex of the SHA-256 over the exact stored bytes.
        digest_hex: String,
        /// The parameterless media type of the stored artifact.
        media_type: String,
        /// The stored byte length.
        length_bytes: u64,
    },
}

/// One capture submission: the mandatory idempotency key, what to capture, and optional
/// provenance facts carried additively (servers tolerant of unknown members ignore them today).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureSubmission {
    /// The deterministic per-sender key Platform deduplicates on.
    pub idempotency_key: String,
    /// What to capture.
    pub source: CaptureSource,
    /// Bounded provenance facts (e.g. forward origin), pre-serialized by the caller.
    pub origin: Option<serde_json::Value>,
}

fn classify(response: reqwest::Response) -> Result<reqwest::Response, PlatformError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    Err(match status.as_u16() {
        401 | 403 => PlatformError::Unauthenticated,
        404 => PlatformError::NotFound,
        409 => PlatformError::Conflict,
        429 => PlatformError::RateLimited,
        code if status.is_client_error() => PlatformError::ClientError { status: code },
        code => PlatformError::ServerError { status: code },
    })
}

/// One error or warning line from an operation snapshot: a stable machine code plus untrusted
/// human text.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SafeLine {
    /// The stable machine-actionable code — the only part a consumer may branch on.
    pub code: String,
    /// Human-readable explanation. Untrusted producer text.
    pub message: String,
}

/// What `GET /v1/operations/{id}` returns, narrowed to what rendering needs. Additive wire
/// fields are ignored by design; an unknown status refuses rather than guessing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationSnapshotView {
    /// The closed lifecycle status reached.
    pub status: OperationStatus,
    /// Producer display text for the phase, when given. Untrusted upstream.
    pub stage: Option<String>,
    /// Whole-percent estimate, when given.
    pub progress_percent: Option<u8>,
    /// A user-safe message, when given. Untrusted upstream.
    pub message: Option<String>,
    /// Terminal failures, newest semantics owned by the producer.
    pub errors: Vec<SafeLine>,
    /// Non-terminal problems.
    pub warnings: Vec<SafeLine>,
}

/// Wire shapes, private: they exist only to be serialized and deserialized.
#[derive(serde::Deserialize)]
struct CaptureAcceptedWire {
    operation_id: Uuid,
}

#[derive(serde::Serialize)]
struct ExchangeAssertionWire<'a> {
    assertion: &'a str,
}

#[derive(Deserialize)]
struct SessionMintedWire {
    credential: String,
    expires_at: String,
}

/// What `POST /v1/sessions/telegram` returns on success.
#[derive(Debug, Clone)]
pub struct SessionMinted {
    /// The bearer credential, shown exactly once upstream.
    pub credential: secrecy::SecretString,
    /// When it stops working, as the upstream RFC 3339 string.
    pub expires_at: String,
}

#[derive(Deserialize)]
struct SnapshotWire {
    status: String,
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    progress_percent: Option<u8>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    errors: Vec<SafeLine>,
    #[serde(default)]
    warnings: Vec<SafeLine>,
}
