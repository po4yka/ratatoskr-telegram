//! Shared opaque token generation for callback and deep-link interaction authority.

use base64::Engine as _;
use sqlx::types::Uuid;

use crate::{Database, PersistenceError};

/// Minimized provenance for a forwarded capture.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureOrigin {
    /// Forwarded from a user account.
    User {
        /// Telegram user id.
        user_id: i64,
        /// Original whole-second Unix timestamp.
        sent_at_secs: i64,
    },
    /// Forwarded with the sender hidden.
    HiddenUser {
        /// Telegram's display name for the hidden sender.
        sender_name: String,
        /// Original whole-second Unix timestamp.
        sent_at_secs: i64,
    },
    /// Forwarded from a chat.
    Chat {
        /// Originating chat id.
        chat_id: i64,
        /// Original whole-second Unix timestamp.
        sent_at_secs: i64,
    },
    /// Forwarded from a channel.
    Channel {
        /// Originating channel id.
        chat_id: i64,
        /// Original channel message id.
        message_id: i64,
        /// Original whole-second Unix timestamp.
        sent_at_secs: i64,
    },
}

/// Stored attachment facts carried in a server-side operation intent.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlobCapture {
    /// Service whose store owns the bytes.
    pub owner_service: String,
    /// Digest algorithm (`sha256`).
    pub algorithm: String,
    /// Lowercase digest of the exact stored bytes.
    pub digest_hex: String,
    /// Parameterless media type.
    pub media_type: String,
    /// Stored byte length.
    pub length_bytes: u64,
}

/// Closed forward/blob presentation metadata kept behind an opaque token.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentMetadata {
    /// Minimized forwarding provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward: Option<CaptureOrigin>,
    /// Stored attachment facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<BlobCapture>,
}

/// Telegram surface on which an opaque token may be presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSurface {
    /// Telegram callback query data.
    Callback,
    /// The payload of an exact `/start <token>` message.
    DeepLink,
}

/// Complete Telegram presentation scope. A message binding exists only for callback buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenScope {
    /// Bot identity that issued the authority.
    pub bot_id: i64,
    /// Telegram actor bound to the authority.
    pub telegram_user_id: i64,
    /// Telegram chat bound to the authority.
    pub chat_id: i64,
    /// Provider-acknowledged message carrying the button, when applicable.
    pub message_id: Option<i64>,
}

/// One attempt to consume opaque interaction authority.
#[derive(Debug, Clone, Copy)]
pub struct TokenPresentation<'a> {
    /// Opaque client-presented value.
    pub token: &'a str,
    /// Surface on which it was received.
    pub surface: TokenSurface,
    /// Complete current actor scope.
    pub scope: TokenScope,
    /// Wall-clock timestamp, in whole seconds since the Unix epoch.
    pub now: i64,
}

/// Why no server-side action was released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRefusal {
    /// The value is malformed or names no stored authority.
    Invalid,
    /// The authority reached its strict expiry boundary.
    Expired,
    /// A stored authority belongs to another presentation scope.
    ScopeMismatch,
    /// A prior presentation already consumed the one-time authority.
    Consumed,
    /// The referenced dialogue no longer has the expected version or step.
    StaleState,
}

/// Typed action released after a successful one-time consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleasedAction {
    /// Present the already-submitted Platform operation.
    OperationStatus,
}

/// Closed presentation facts stored behind an operation-status deep link.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationIntentPayload {
    /// Submitted address, absent for attachment captures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Bounded forward or blob provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<IntentMetadata>,
}

/// New operation-status authority. The registry mints the client-visible value.
#[derive(Debug, Clone)]
pub struct NewOperationIntent {
    /// Complete owner scope.
    pub scope: TokenScope,
    /// Stable Platform operation reference.
    pub operation_id: Uuid,
    /// Bounded server-side presentation facts.
    pub payload: OperationIntentPayload,
    /// Strict expiry, in whole seconds since the Unix epoch.
    pub expires_at: i64,
}

/// Typed server-side state released by a successful token presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasedToken {
    /// Closed action named by the stored row.
    pub action: ReleasedAction,
    /// Platform operation reference for an operation-status intent.
    pub operation_id: Uuid,
    /// Bounded presentation facts kept out of the client-visible token.
    pub payload: OperationIntentPayload,
}

/// One still-live operation intent as the dispatcher sees it by operation reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationIntentRecord {
    /// Opaque token used to compose the Telegram deep link.
    pub token: String,
    /// Bot identity that issued the link.
    pub bot_id: i64,
    /// Originating Telegram chat.
    pub chat_id: i64,
    /// Platform operation reference.
    pub operation_id: Uuid,
    /// Bounded presentation facts.
    pub payload: OperationIntentPayload,
}

type LockedTokenRow = (
    String,
    String,
    i64,
    i64,
    i64,
    Option<i64>,
    Option<Uuid>,
    Option<serde_json::Value>,
    i64,
    Option<i64>,
);

const fn surface_name(surface: TokenSurface) -> &'static str {
    match surface {
        TokenSurface::Callback => "callback",
        TokenSurface::DeepLink => "deep_link",
    }
}

fn token_has_valid_grammar(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn decode_operation_token(row: &LockedTokenRow) -> Result<ReleasedToken, PersistenceError> {
    let operation_id = row
        .6
        .ok_or_else(|| query_decode_error("operation_id", "missing"))?;
    let raw = row
        .7
        .clone()
        .ok_or_else(|| query_decode_error("payload", "missing"))?;
    let payload = serde_json::from_value(raw)
        .map_err(|error| query_decode_error("payload", error.to_string()))?;
    Ok(ReleasedToken {
        action: ReleasedAction::OperationStatus,
        operation_id,
        payload,
    })
}

fn query_decode_error(index: &str, message: impl Into<String>) -> PersistenceError {
    PersistenceError::Query(sqlx::Error::ColumnDecode {
        index: index.to_owned(),
        source: message.into().into(),
    })
}

impl Database {
    /// Mint and store one owner-bound operation-status deep-link authority.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if payload encoding or storage fails.
    pub async fn issue_operation_intent(
        &self,
        intent: &NewOperationIntent,
        now: i64,
    ) -> Result<String, PersistenceError> {
        let token = mint_token();
        let payload = serde_json::to_value(&intent.payload)
            .map_err(|error| query_decode_error("payload", error.to_string()))?;
        sqlx::query(
            "insert into telegram.interaction_tokens
             (token, surface, action, bot_id, telegram_user_id, chat_id, operation_id, payload,
              created_at, expires_at)
             values ($1, 'deep_link', 'operation_status', $2, $3, $4, $5, $6,
                     to_timestamp($7), to_timestamp($8))",
        )
        .bind(&token)
        .bind(intent.scope.bot_id)
        .bind(intent.scope.telegram_user_id)
        .bind(intent.scope.chat_id)
        .bind(intent.operation_id)
        .bind(payload)
        .bind(now)
        .bind(intent.expires_at)
        .execute(self.pool())
        .await
        .map_err(PersistenceError::Query)?;
        Ok(token)
    }

    /// Find one live, unconsumed operation intent for delayed dispatcher rendering.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] when storage cannot read the intent.
    pub async fn find_live_operation_intent_by_operation(
        &self,
        operation_id: Uuid,
        now: i64,
    ) -> Result<Option<OperationIntentRecord>, PersistenceError> {
        let row: Option<(String, i64, i64, Uuid, serde_json::Value)> = sqlx::query_as(
            "select token, bot_id, chat_id, operation_id, payload
             from telegram.interaction_tokens
             where surface = 'deep_link' and action = 'operation_status'
               and operation_id = $1 and consumed_at is null
               and expires_at > to_timestamp($2)
             order by created_at desc, token desc
             limit 1",
        )
        .bind(operation_id)
        .bind(now)
        .fetch_optional(self.pool())
        .await
        .map_err(PersistenceError::Query)?;
        row.map(|(token, bot_id, chat_id, operation_id, raw)| {
            let payload = serde_json::from_value(raw)
                .map_err(|error| query_decode_error("payload", error.to_string()))?;
            Ok(OperationIntentRecord {
                token,
                bot_id,
                chat_id,
                operation_id,
                payload,
            })
        })
        .transpose()
    }

    /// Find the Telegram owner used to authenticate an operation follower.
    ///
    /// Expiry and consumption do not remove this ownership fact: following an already-submitted
    /// operation outlives the presentation link.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] when storage cannot read the owner.
    pub async fn find_operation_intent_owner(
        &self,
        operation_id: Uuid,
    ) -> Result<Option<i64>, PersistenceError> {
        sqlx::query_scalar(
            "select telegram_user_id from telegram.interaction_tokens
             where surface = 'deep_link' and action = 'operation_status' and operation_id = $1
             order by created_at desc, token desc limit 1",
        )
        .bind(operation_id)
        .fetch_optional(self.pool())
        .await
        .map_err(PersistenceError::Query)
    }

    /// Resolve and consume one owner-bound operation-status deep link.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] when storage cannot evaluate the authority.
    pub async fn resolve_operation_intent(
        &self,
        presentation: TokenPresentation<'_>,
    ) -> Result<Result<ReleasedToken, TokenRefusal>, PersistenceError> {
        if presentation.surface != TokenSurface::DeepLink {
            return Ok(Err(TokenRefusal::ScopeMismatch));
        }
        self.consume_interaction_token(presentation).await
    }

    /// Validate and consume one interaction token.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Query`] if storage cannot evaluate the authority.
    pub async fn consume_interaction_token(
        &self,
        presentation: TokenPresentation<'_>,
    ) -> Result<Result<ReleasedToken, TokenRefusal>, PersistenceError> {
        if !token_has_valid_grammar(presentation.token) {
            return Ok(Err(TokenRefusal::Invalid));
        }
        let mut transaction = self.pool().begin().await.map_err(PersistenceError::Query)?;
        let row: Option<LockedTokenRow> = sqlx::query_as(
            "select surface, action, bot_id, telegram_user_id, chat_id, expected_message_id,
                    operation_id, payload, extract(epoch from expires_at)::bigint,
                    extract(epoch from consumed_at)::bigint
             from telegram.interaction_tokens
             where token = $1
             for update",
        )
        .bind(presentation.token)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        let Some(row) = row else {
            return Ok(Err(TokenRefusal::Invalid));
        };
        if row.9.is_some() {
            return Ok(Err(TokenRefusal::Consumed));
        }
        if row.8 <= presentation.now {
            return Ok(Err(TokenRefusal::Expired));
        }
        let scope = presentation.scope;
        if row.0 != surface_name(presentation.surface)
            || row.2 != scope.bot_id
            || row.3 != scope.telegram_user_id
            || row.4 != scope.chat_id
            || row.5 != scope.message_id
        {
            return Ok(Err(TokenRefusal::ScopeMismatch));
        }
        if row.1 != "operation_status" {
            return Ok(Err(TokenRefusal::StaleState));
        }
        let released = decode_operation_token(&row)?;
        sqlx::query(
            "update telegram.interaction_tokens
             set consumed_at = to_timestamp($2), consumed_by_user = $3
             where token = $1",
        )
        .bind(presentation.token)
        .bind(presentation.now)
        .bind(scope.telegram_user_id)
        .execute(&mut *transaction)
        .await
        .map_err(PersistenceError::Query)?;
        transaction
            .commit()
            .await
            .map_err(PersistenceError::Query)?;
        Ok(Ok(released))
    }
}

pub(crate) fn mint_token() -> String {
    let mut bytes = [0_u8; 48];
    for chunk in bytes.chunks_exact_mut(16) {
        chunk.copy_from_slice(Uuid::new_v4().as_bytes());
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::mint_token;

    #[test]
    fn minted_token_uses_the_full_url_safe_callback_budget() {
        let token = mint_token();

        assert_eq!(token.len(), 64);
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
            "the token must use the unpadded URL-safe Base64 alphabet",
        );
    }
}
