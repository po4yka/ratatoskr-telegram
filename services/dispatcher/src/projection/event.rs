//! The typed input the projection consumer renders from: exactly what the published
//! `platform.operation.progressed.v1` contract carries that rendering needs, nothing more.
//!
//! Additive wire fields are IGNORED by design — serde's default drops unknown keys, so a producer
//! adding a field never breaks this parser (`x-ratatoskr-unknown-policy: preserve` on the wire,
//! deliberate narrowing at the consumer). The status enum is CLOSED like the contract's: an
//! unknown or missing status is a parse error, never a guess, because guessing an unknown
//! lifecycle state is how unfinished work gets reported as finished.

use serde::Deserialize;
use thiserror::Error as ThisError;
use uuid::Uuid;

/// Why one envelope could not become an [`OperationEvent`]. Closed and safe to log: no variant
/// carries the payload text.
#[derive(Debug, ThisError)]
pub enum ParseError {
    /// The envelope is not valid JSON, or a required field is missing or mistyped.
    #[error("an operation event envelope did not match the published contract")]
    Json(#[source] serde_json::Error),
    /// The snapshot carried a status outside the contract's closed enum.
    #[error("an operation event carried an unknown lifecycle status")]
    UnknownStatus {
        /// The refused raw status string, for the class-only log line.
        status: String,
    },
    /// The envelope timestamp was not an RFC 3339 UTC instant.
    #[error("an operation event carried an unparsable occurred_at")]
    Timestamp(String),
    /// An identifier field was not a canonical UUID.
    #[error("an operation event carried a malformed identifier")]
    MalformedId(String),
}

/// One progressed fact, flattened from envelope plus snapshot for the consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationEvent {
    /// The envelope's globally unique occurrence id — dedup key and trace anchor.
    pub event_id: Uuid,
    /// When the producer observed the fact, whole seconds since the Unix epoch.
    pub occurred_at_secs: i64,
    /// The contracts `EntityRef` correlation string; tracing only.
    pub correlation_id: String,
    /// The operation this snapshot belongs to.
    pub operation_id: Uuid,
    /// The closed lifecycle state the operation HAS reached.
    pub status: OperationStatus,
    /// Producer display text for the phase inside the status. UNTRUSTED: escaped by the renderer,
    /// never branched on.
    pub stage: Option<String>,
    /// Whole-percent completion estimate, when the producer publishes one.
    pub progress_percent: Option<u8>,
    /// Terminal failures, newest semantics owned by the producer. Messages UNTRUSTED.
    pub errors: Vec<SafeLine>,
    /// Non-terminal problems. Messages UNTRUSTED.
    pub warnings: Vec<SafeLine>,
    /// A user-safe line the transport carried, when any (SSE progress entries carry one;
    /// envelope snapshots do not). UNTRUSTED; escaped by the renderer.
    pub message: Option<String>,
}

/// The closed lifecycle vocabulary of the published contract. An unknown value refuses to parse.
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
    /// Terminal: stopped on request before completion.
    Cancelled,
}

impl OperationStatus {
    /// The lowercase snake label used in logs and future metric labels.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::PartiallySucceeded => "partially_succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
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

/// One error or warning line: a stable machine code plus untrusted human text.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SafeLine {
    /// The stable machine-actionable code — the only part a consumer may branch on.
    pub code: String,
    /// Human-readable explanation. UNTRUSTED producer text; escaped by the renderer.
    pub message: String,
}

/// Parse one delivered envelope (JSON text) into the typed event.
///
/// # Errors
///
/// [`ParseError`] for malformed JSON, missing required fields, an unknown status, or an
/// unparsable timestamp — each refusing rather than guessing.
pub fn from_envelope_json(json: &str) -> Result<OperationEvent, ParseError> {
    let wire: EnvelopeWire = serde_json::from_str(json).map_err(ParseError::Json)?;

    let operation_id = Uuid::parse_str(&wire.payload.operation.operation_id)
        .map_err(|_| ParseError::MalformedId(wire.payload.operation.operation_id.clone()))?;
    let event_id = Uuid::parse_str(&wire.event_id)
        .map_err(|_| ParseError::MalformedId(wire.event_id.clone()))?;
    let occurred_at_secs = rfc3339_z_to_epoch_secs(&wire.occurred_at)?;
    let status = match wire.payload.operation.status.as_str() {
        "accepted" => OperationStatus::Accepted,
        "queued" => OperationStatus::Queued,
        "running" => OperationStatus::Running,
        "succeeded" => OperationStatus::Succeeded,
        "partially_succeeded" => OperationStatus::PartiallySucceeded,
        "failed" => OperationStatus::Failed,
        "cancelled" => OperationStatus::Cancelled,
        other => {
            return Err(ParseError::UnknownStatus {
                status: other.to_owned(),
            });
        }
    };

    Ok(OperationEvent {
        event_id,
        occurred_at_secs,
        correlation_id: wire.correlation_id,
        operation_id,
        status,
        stage: wire.payload.operation.stage,
        progress_percent: wire.payload.operation.progress_percent,
        errors: wire.payload.operation.errors,
        warnings: wire.payload.operation.warnings,
        message: None,
    })
}

/// Convert an RFC 3339 UTC instant (`YYYY-MM-DDTHH:MM:SS(.fff)?Z`, the contract's only spelling)
/// to whole epoch seconds, truncating any fractional part. Fraction digits are not validated
/// beyond their presence: they are below the resolution anything downstream stores.
fn rfc3339_z_to_epoch_secs(text: &str) -> Result<i64, ParseError> {
    let reject = || ParseError::Timestamp(text.to_owned());
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes.last() != Some(&b'Z') {
        return Err(reject());
    }
    let separator = |index: usize, expected: u8| -> Result<(), ParseError> {
        if bytes.get(index) == Some(&expected) {
            Ok(())
        } else {
            Err(reject())
        }
    };
    separator(4, b'-')?;
    separator(7, b'-')?;
    separator(10, b'T')?;
    separator(13, b':')?;
    separator(16, b':')?;
    let digit = |range: std::ops::Range<usize>| -> Result<i64, ParseError> {
        text.get(range)
            .ok_or_else(reject)?
            .parse::<i64>()
            .map_err(|_| reject())
    };
    let year = digit(0..4)?;
    let month = digit(5..7)?;
    let day = digit(8..10)?;
    let hour = digit(11..13)?;
    let minute = digit(14..16)?;
    let second = digit(17..19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(reject());
    }

    let days = days_from_civil(year, month, day);
    Ok(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Days from 1970-01-01 to the given proleptic-Gregorian date (Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let year_of_era = y - era * 400;
    let m = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * m + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The wire shapes, private: they exist only to be deserialized and converted. Identifiers cross
/// as strings and are validated here, so no serde feature is needed on `uuid`.
#[derive(Deserialize)]
struct EnvelopeWire {
    event_id: String,
    occurred_at: String,
    correlation_id: String,
    payload: PayloadWire,
}

#[derive(Deserialize)]
struct PayloadWire {
    operation: SnapshotWire,
}

#[derive(Deserialize)]
struct SnapshotWire {
    operation_id: String,
    status: String,
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    progress_percent: Option<u8>,
    #[serde(default)]
    errors: Vec<SafeLine>,
    #[serde(default)]
    warnings: Vec<SafeLine>,
}

#[cfg(test)]
#[allow(clippy::panic, reason = "assertions inside the in-crate test module")]
mod tests {
    use super::{OperationStatus, ParseError, from_envelope_json};

    /// A minimal valid envelope copied field-for-field from the published contract's shapes
    /// (`platform.operation.progressed.v1`, snapshot required fields included).
    const ENVELOPE: &str = r#"{
        "event_id": "018f0000-0000-7000-8000-00000000abcd",
        "occurred_at": "2026-08-17T10:00:00Z",
        "correlation_id": "operation:018f0000-0000-7000-8000-000000000001",
        "payload": {
            "operation": {
                "operation_id": "018f0000-0000-7000-8000-000000000002",
                "kind": "content.document.extract",
                "status": "running",
                "accepted_at": "2026-08-17T09:59:00Z",
                "status_changed_at": "2026-08-17T10:00:00Z",
                "retryable": false,
                "correlation_id": "operation:018f0000-0000-7000-8000-000000000001",
                "stage": "downloading",
                "progress_percent": 42,
                "errors": [
                    {
                        "code": "content.source.temporary",
                        "message": "source refused the connection",
                        "retryable": true
                    }
                ],
                "warnings": [
                    {
                        "code": "content.metadata.partial",
                        "message": "some metadata fields were missing"
                    }
                ]
            }
        }
    }"#;

    #[test]
    fn envelope_parses_snapshot_fields() {
        let event = from_envelope_json(ENVELOPE).expect("the contract-shaped envelope parses");
        assert_eq!(
            event.occurred_at_secs, 1_786_960_800,
            "RFC3339 Z becomes epoch seconds"
        );
        assert_eq!(
            event.operation_id.to_string(),
            "018f0000-0000-7000-8000-000000000002"
        );
        assert_eq!(event.status, OperationStatus::Running);
        assert_eq!(event.stage.as_deref(), Some("downloading"));
        assert_eq!(event.progress_percent, Some(42));
        assert_eq!(event.errors.len(), 1);
        assert_eq!(event.errors[0].code, "content.source.temporary");
        assert_eq!(event.errors[0].message, "source refused the connection");
        assert_eq!(event.warnings.len(), 1);
        assert_eq!(
            event.correlation_id,
            "operation:018f0000-0000-7000-8000-000000000001"
        );
    }

    #[test]
    fn unknown_status_is_refused_not_guessed() {
        let json = ENVELOPE.replace("\"status\": \"running\"", "\"status\": \"teleported\"");
        let ParseError::UnknownStatus { status } = from_envelope_json(&json).unwrap_err() else {
            panic!("an unknown status must refuse as UnknownStatus");
        };
        assert_eq!(status, "teleported", "the refusal names the refused value");
    }

    #[test]
    fn additive_fields_are_ignored() {
        // A producer added fields at every level; none of them may break parsing.
        let json = r#"{
            "event_id": "018f0000-0000-7000-8000-00000000abcd",
            "occurred_at": "2026-08-17T10:00:00Z",
            "correlation_id": "operation:018f0000-0000-7000-8000-000000000001",
            "extra_envelope_field": {"deeply": ["nested"]},
            "payload": {
                "extra_payload_field": 7,
                "operation": {
                    "operation_id": "018f0000-0000-7000-8000-000000000002",
                    "kind": "content.document.extract",
                    "status": "queued",
                    "accepted_at": "2026-08-17T09:59:00Z",
                    "status_changed_at": "2026-08-17T10:00:00Z",
                    "retryable": false,
                    "correlation_id": "operation:018f0000-0000-7000-8000-000000000001",
                    "results": [],
                    "extra_snapshot_field": {"future": true}
                }
            }
        }"#;
        let event = from_envelope_json(json).expect("additive fields never break parsing");
        assert_eq!(event.status, OperationStatus::Queued);
        assert_eq!(event.stage, None);
        assert_eq!(event.progress_percent, None);
        assert!(event.errors.is_empty());
    }
}
