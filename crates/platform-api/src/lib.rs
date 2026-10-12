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
pub mod assertion_key;
pub mod session;
mod stream;

pub use session::{Clock, SessionSource};
pub use stream::EventStream;

use std::time::Duration;

use serde::Deserialize;
use url::Url;
use uuid::Uuid;

const MAX_JSON_RESPONSE_BYTES: usize = 256 * 1024;

/// How long a progress stream may stay silent before it ends: three of Platform's 15 second
/// keep-alive intervals.
pub const PLATFORM_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(45);

pub use ratatoskr_github_contracts::{
    RepositoryActionRequest, RepositoryActionResult, RepositoryPreviewRequest,
    RepositoryPreviewResponse,
};

/// Why one call to Platform failed. Closed, safe to log, and free of credential material: no
/// variant carries the presented bearer, the signing key, or a response body verbatim.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// The request never completed: connection refused, reset, TLS, DNS.
    #[error("the platform request failed at the transport level")]
    Network(#[source] reqwest::Error),
    /// A call exceeded its whole-call timeout, or a progress stream stayed silent for
    /// [`PLATFORM_STREAM_IDLE_TIMEOUT`].
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

/// What a capture route (`POST /v1/captures` or `POST /v1/captures/blobs`) returns on acceptance.
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
    /// Every request/response call: bounded as a whole by the call timeout.
    http: reqwest::Client,
    /// The progress stream only: no total timeout, a connect bound and a read-idle bound.
    stream_http: reqwest::Client,
    base_url: Url,
}

impl Client {
    /// Build a client against `base_url` with a whole-call timeout for request/response calls and
    /// [`PLATFORM_STREAM_IDLE_TIMEOUT`] for the progress stream.
    ///
    /// # Errors
    ///
    /// [`PlatformError::Network`] if the underlying HTTP client cannot build, which in practice
    /// means TLS backend initialization failed.
    pub fn new(base_url: &Url, timeout: Duration) -> Result<Self, PlatformError> {
        Self::with_timeouts(base_url, timeout, PLATFORM_STREAM_IDLE_TIMEOUT)
    }

    /// Build a client with both bounds named: `call` for every request/response call, `idle` for
    /// the progress stream. [`Client::new`] passes [`PLATFORM_STREAM_IDLE_TIMEOUT`]; tests pass a
    /// short value.
    ///
    /// # Errors
    ///
    /// [`PlatformError::Network`] if the underlying HTTP client cannot build.
    pub fn with_timeouts(
        base_url: &Url,
        call: Duration,
        idle: Duration,
    ) -> Result<Self, PlatformError> {
        let http = reqwest::ClientBuilder::new()
            .timeout(call)
            .use_rustls_tls()
            .build()
            .map_err(PlatformError::Network)?;
        // A total timeout covers the response body, so it would cut every stream at the call
        // timeout although Platform holds it open with keep-alives. The read timeout resets on
        // every received chunk and ends a stream that has gone silent.
        let stream_http = reqwest::ClientBuilder::new()
            .connect_timeout(call)
            .read_timeout(idle)
            .use_rustls_tls()
            .build()
            .map_err(PlatformError::Network)?;
        Ok(Self {
            http,
            stream_http,
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
            user_id: wire.user_id,
        })
    }

    /// Read the current closed capability document for one session.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] per the taxonomy above.
    pub async fn capabilities(&self, session: &str) -> Result<CapabilityDocument, PlatformError> {
        let response = self
            .send(
                self.http
                    .get(self.url("/v1/capabilities"))
                    .bearer_auth(session),
            )
            .await?;
        bounded_json(response).await
    }

    /// Search or browse the authenticated user's library.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] per the taxonomy above.
    pub async fn search_library(
        &self,
        session: &str,
        query: &LibrarySearch,
    ) -> Result<LibraryPage, PlatformError> {
        let mut url = self.url("/v1/library/search");
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("q", &query.query);
            if let Some(read_state) = query.read_state {
                pairs.append_pair("read_state", read_state.as_str());
            }
            pairs.append_pair("limit", &query.limit.to_string());
            pairs.append_pair("offset", &query.offset.to_string());
        }
        let response = self.send(self.http.get(url).bearer_auth(session)).await?;
        bounded_json(response).await
    }

    /// Idempotently replace one caller-owned analysis read state.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] per the taxonomy above.
    pub async fn replace_library_read_state(
        &self,
        session: &str,
        analysis_id: Uuid,
        read_state: LibraryReadState,
    ) -> Result<ReadStateResource, PlatformError> {
        let response = self
            .send(
                self.http
                    .put(self.url(&format!("/v1/library/items/{analysis_id}/read-state")))
                    .bearer_auth(session)
                    .json(&ReplaceReadState { read_state }),
            )
            .await?;
        bounded_json(response).await
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
            .stream_http
            .get(self.url(&format!("/v1/operations/{operation_id}/events")))
            .bearer_auth(session);
        if let Some(last) = last_event_id {
            builder = builder.header("last-event-id", last);
        }
        let response = self.send(builder).await?;
        Ok(EventStream::new(response))
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

    /// Resolve one repository preview through Platform's authenticated GitHub gateway.
    ///
    /// This is read-only: it cannot catalog, track, star, or otherwise mutate repository state.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] per the taxonomy above.
    pub async fn preview_repository(
        &self,
        session: &str,
        request: &RepositoryPreviewRequest,
    ) -> Result<RepositoryPreviewResponse, PlatformError> {
        let response = self
            .send(
                self.http
                    .post(self.url("/v1/gh/repositories/preview"))
                    .bearer_auth(session)
                    .json(request),
            )
            .await?;
        let body = response.bytes().await.map_err(PlatformError::Network)?;
        serde_json::from_slice(&body).map_err(PlatformError::Json)
    }

    /// Submit one already-confirmed repository action through Platform's GitHub gateway.
    ///
    /// The request itself carries confirmation evidence and its stable idempotency identity;
    /// callers must reuse the same value when a response is uncertain.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] per the taxonomy above.
    pub async fn apply_repository_action(
        &self,
        session: &str,
        request: &RepositoryActionRequest,
    ) -> Result<RepositoryActionResult, PlatformError> {
        let response = self
            .send(
                self.http
                    .post(self.url("/v1/gh/repositories/actions"))
                    .bearer_auth(session)
                    .json(request),
            )
            .await?;
        let body = response.bytes().await.map_err(PlatformError::Network)?;
        serde_json::from_slice(&body).map_err(PlatformError::Json)
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
        let (path, body) = submission.request_parts();

        let response = self
            .send(
                self.http
                    .post(self.url(path))
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
/// provenance facts carried additively (Platform tolerates and ignores unknown members).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureSubmission {
    /// The key Platform deduplicates on. It must describe [`CaptureSubmission::request_parts`]:
    /// Platform fingerprints the exact body, so one key with two bodies is refused.
    pub idempotency_key: String,
    /// What to capture.
    pub source: CaptureSource,
    /// Bounded provenance facts (e.g. forward origin), pre-serialized by the caller.
    pub origin: Option<serde_json::Value>,
}

impl CaptureSubmission {
    /// The route path and JSON body this submission sends, and nothing else: `submit_capture`
    /// serialises exactly this value, so anything derived from it (the idempotency key above all)
    /// describes the request that goes on the wire.
    ///
    /// A URL capture posts `{"url": ...}` to `/v1/captures`; a blob capture posts the `BlobRef`
    /// to its own route, `/v1/captures/blobs` (XR-021 CONTRACTS S11). The optional `origin`
    /// provenance is an additive member that both routes tolerate and ignore.
    #[must_use]
    pub fn request_parts(&self) -> (&'static str, serde_json::Value) {
        let (path, mut body) = match &self.source {
            CaptureSource::Url(url) => ("/v1/captures", serde_json::json!({ "url": url })),
            CaptureSource::Blob {
                owner_service,
                digest_hex,
                media_type,
                length_bytes,
            } => (
                "/v1/captures/blobs",
                serde_json::json!({
                    "blob": {
                        "owner_service": owner_service,
                        "digest": { "algorithm": "sha256", "hex": digest_hex },
                        "media_type": media_type,
                        "length_bytes": length_bytes,
                    }
                }),
            ),
        };
        if let (Some(map), Some(origin)) = (body.as_object_mut(), self.origin.as_ref()) {
            map.insert("origin".to_owned(), origin.clone());
        }
        (path, body)
    }
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
    user_id: Uuid,
}

/// What `POST /v1/sessions/telegram` returns on success.
#[derive(Debug, Clone)]
pub struct SessionMinted {
    /// The bearer credential, shown exactly once upstream.
    pub credential: secrecy::SecretString,
    /// When it stops working, as the upstream RFC 3339 string.
    pub expires_at: String,
    /// Canonical Platform user authenticated by the assertion.
    pub user_id: Uuid,
}

/// Current public capabilities visible to one authenticated caller.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityDocument {
    /// Public API version.
    pub api_version: String,
    /// Client-version floor object, retained only to enforce the exact response shape.
    pub minimum_client_versions: serde_json::Value,
    /// Closed feature names.
    pub capabilities: Vec<String>,
    /// Last-observed service documents, retained only to enforce the exact response shape.
    pub services: Vec<serde_json::Value>,
}

impl CapabilityDocument {
    /// Whether this response advertises one exact feature name.
    #[must_use]
    pub fn contains(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|name| name == capability)
    }
}

/// Effective read state in the public library contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryReadState {
    /// Not yet read.
    Unread,
    /// Read.
    Read,
}

impl LibraryReadState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::Read => "read",
        }
    }
}

/// One bounded public search request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibrarySearch {
    /// Search text; blank means recency browse.
    pub query: String,
    /// Optional effective-state filter.
    pub read_state: Option<LibraryReadState>,
    /// Requested page size.
    pub limit: u32,
    /// Zero-based result offset.
    pub offset: u64,
}

/// One minimized public library result.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryItem {
    /// Accepted analysis identity.
    pub analysis_id: Uuid,
    /// Canonical document identity.
    pub document_id: Uuid,
    /// Bounded display title.
    pub title: String,
    /// Optional match snippet.
    pub snippet: Option<String>,
    /// Optional finite positive relevance score.
    pub score: Option<f32>,
    /// Effective read state.
    pub read_state: LibraryReadState,
}

/// One bounded library page.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryPage {
    /// Ordered summaries.
    pub items: Vec<LibraryItem>,
    /// Applied page size.
    pub limit: u32,
    /// Applied offset.
    pub offset: u64,
    /// Whether another item exists after this page.
    pub has_more: bool,
}

#[derive(serde::Serialize)]
struct ReplaceReadState {
    read_state: LibraryReadState,
}

/// Authoritative state returned after replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadStateResource {
    /// Stored effective state.
    pub read_state: LibraryReadState,
}

async fn bounded_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, PlatformError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_JSON_RESPONSE_BYTES as u64)
    {
        return Err(PlatformError::MalformedFrame);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(PlatformError::Network)? {
        if bytes.len().saturating_add(chunk.len()) > MAX_JSON_RESPONSE_BYTES {
            return Err(PlatformError::MalformedFrame);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(PlatformError::Json)
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
