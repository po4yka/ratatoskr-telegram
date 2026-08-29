//! Failure classification: the one place a [`BotApiError`] becomes a delivery decision.
//!
//! The mapping is design D5's table, pinned row by row by the tests below. It is pure — no I/O,
//! no clock — so the whole table is unit-testable. The table proper runs on [`FailureShape`], a
//! sealed plain-data mirror of the taxonomy: `reqwest::Error` cannot be constructed in-process,
//! so driving a real `BotApiError::Network` would need a live HTTP harness; the mirror keeps the
//! table fully testable without new dependencies or changes to bot-api's public surface, while
//! every constructible taxonomy variant is additionally pinned through the public [`classify`].
//!
//! WHY unknown API descriptions are TRANSIENT rather than permanent: Telegram introduces
//! transient error texts far faster than permanent ones, and bounded retries dead-letter at the
//! attempt bound anyway — guessing "permanent" from unseen text risks silently dropping
//! deliverable messages, while guessing "transient" only costs a few attempts.

use std::time::Duration;

use bot_api::BotApiError;

/// What the sender must do after one Bot API call ended in [`BotApiError`] (or, for
/// [`Classified::Sent`], in success — constructed by the sender's `Ok` branch, which never
/// reaches [`classify`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classified {
    /// The Bot API acknowledged the write.
    Sent,
    /// The edit answered `message is not modified`: success without a change.
    NotModified,
    /// Telegram asked for a pause; the delay is authoritative.
    RateLimited {
        /// Whole seconds to wait before repeating the call.
        retry_after_secs: i64,
    },
    /// A failure worth bounded retries before dead-lettering.
    Transient,
    /// No provider evidence proves whether the write was applied.
    OutcomeUnknown,
    /// A failure no retry can fix; dead-letters immediately under its safe class.
    Permanent {
        /// The closed, content-free label recorded on the job row.
        class: PermanentClass,
    },
}

/// The closed vocabulary of unfixable failures. These strings become metric labels and database
/// values, so a delivery can never mint a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermanentClass {
    /// The user blocked the bot; nothing will reach this chat again until unblocked.
    BotBlocked,
    /// The chat id does not resolve to any chat the bot can see.
    ChatNotFound,
    /// The bot lost its place in the chat: kicked, deactivated recipient, missing membership,
    /// or insufficient rights.
    MembershipLost,
    /// The target message exists but its content cannot be replaced this way.
    MessageNotEditable,
    /// The message an edit or forward names is gone.
    EditTargetGone,
    /// Our payload was rejected as unparseable (markup entities, malformed request).
    InvalidPayload,
    /// The chat migrated to a supergroup; v1 dead-letters and counts it, following the migration
    /// is deferred to its own task.
    ChatMigrated,
}

impl PermanentClass {
    /// The lowercase snake label stored on rows and used as the metric label value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BotBlocked => "bot_blocked",
            Self::ChatNotFound => "chat_not_found",
            Self::MembershipLost => "membership_lost",
            Self::MessageNotEditable => "message_not_editable",
            Self::EditTargetGone => "edit_target_gone",
            Self::InvalidPayload => "invalid_payload",
            Self::ChatMigrated => "chat_migrated",
        }
    }
}

/// The plain-data shape of one taxonomy failure, stripped of sources that cannot be synthesized
/// in-process (`reqwest::Error`) and reduced to what the decision actually reads.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FailureShape {
    /// Transport failed underneath the call.
    Network,
    /// A local file transfer failed underneath the call.
    Io,
    /// Telegram asked for a pause of `retry_after_secs` seconds.
    RateLimited {
        /// Whole seconds to wait.
        retry_after_secs: i64,
    },
    /// Telegram answered with an error body carrying its own description.
    Api {
        /// Telegram's error text, matched case-insensitively against known needles.
        description: String,
    },
    /// The chat migrated to a supergroup.
    ChatMigrated,
    /// A response could not be parsed as the Bot API answered it.
    Json,
    /// A taxonomy variant this build predates; treated with the conservative default.
    Unknown,
}

/// Classify one Bot API failure into the sender's control-flow decision.
///
/// Total over the taxonomy: every variant maps, including ones added upstream after this file
/// was written ([`FailureShape::Unknown`]).
#[must_use]
pub fn classify(error: &BotApiError) -> Classified {
    classify_shape(&shape_of(error))
}

/// The single total conversion from the live taxonomy onto the testable mirror.
fn shape_of(error: &BotApiError) -> FailureShape {
    match error {
        BotApiError::Network(_) => FailureShape::Network,
        BotApiError::RateLimited { retry_after } => FailureShape::RateLimited {
            retry_after_secs: secs_of(*retry_after),
        },
        BotApiError::Api { description } => FailureShape::Api {
            description: description.clone(),
        },
        BotApiError::ChatMigrated { .. } => FailureShape::ChatMigrated,
        BotApiError::Json => FailureShape::Json,
        BotApiError::Io(_) => FailureShape::Io,
        // The taxonomy is `#[non_exhaustive]`; a variant added upstream lands here and gets the
        // same bounded-retry treatment as an unknown description instead of a compile break in
        // a dependency bump.
        _ => FailureShape::Unknown,
    }
}

/// Design D5's table: shape in, decision out.
fn classify_shape(shape: &FailureShape) -> Classified {
    match shape {
        // Transport and local-file failures carry no proof that the write was not applied.
        FailureShape::Network | FailureShape::Io | FailureShape::Unknown => {
            Classified::OutcomeUnknown
        }
        FailureShape::RateLimited { retry_after_secs } => Classified::RateLimited {
            retry_after_secs: *retry_after_secs,
        },
        // Their reply to us did not parse or our request did not parse to them: re-sending
        // identical bytes storms without new information, so this is treated as our payload
        // problem and dead-letters.
        FailureShape::Json => Classified::OutcomeUnknown,
        // v1 dead-letters migrations and counts them; following the supergroup id is deferred to
        // its own task rather than improvised here.
        FailureShape::ChatMigrated => Classified::Permanent {
            class: PermanentClass::ChatMigrated,
        },
        FailureShape::Api { description } => classify_description(description),
    }
}

/// The description-matching half of the table, case-insensitive over Telegram's own text.
fn classify_description(description: &str) -> Classified {
    let lowered = description.to_lowercase();
    let needle_hit = |needles: &[&str]| needles.iter().any(|needle| lowered.contains(needle));

    if lowered.contains("message is not modified") {
        return Classified::NotModified;
    }
    if lowered.contains("bot was blocked by the user") {
        return Classified::Permanent {
            class: PermanentClass::BotBlocked,
        };
    }
    if lowered.contains("chat not found") {
        return Classified::Permanent {
            class: PermanentClass::ChatNotFound,
        };
    }
    if needle_hit(&[
        "user is deactivated",
        "kicked",
        "bot is not a member",
        "not enough rights",
    ]) {
        return Classified::Permanent {
            class: PermanentClass::MembershipLost,
        };
    }
    if needle_hit(&[
        "message can't be edited",
        "message can not be edited",
        "there is no text in the message to edit",
    ]) {
        return Classified::Permanent {
            class: PermanentClass::MessageNotEditable,
        };
    }
    if needle_hit(&["message to edit not found", "message to forward not found"]) {
        return Classified::Permanent {
            class: PermanentClass::EditTargetGone,
        };
    }
    if lowered.contains("can't parse entities") {
        return Classified::Permanent {
            class: PermanentClass::InvalidPayload,
        };
    }

    // Unknown text: bounded retries, then the attempt bound dead-letters it. See the module
    // rationale for why unknown is never guessed as permanent.
    Classified::Transient
}

/// Whole seconds of a retry delay, saturating at `i64::MAX`: a delay that large is unusable
/// anyway, and the sender reschedules from persisted state rather than holding a timer.
fn secs_of(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use bot_api::{BotApiError, ChatId};

    use super::{Classified, FailureShape, PermanentClass, classify, classify_shape};

    fn api(description: &str) -> BotApiError {
        BotApiError::Api {
            description: description.to_owned(),
        }
    }

    #[test]
    fn network_error_has_unknown_application_outcome() {
        assert_eq!(
            classify_shape(&FailureShape::Network),
            Classified::OutcomeUnknown,
            "transport failures do not prove whether a write was applied"
        );
    }

    #[test]
    fn io_failure_has_unknown_application_outcome_too() {
        let error = BotApiError::Io(Arc::new(std::io::Error::other("synthetic io failure")));
        assert_eq!(classify(&error), Classified::OutcomeUnknown);
    }

    #[test]
    fn rate_limit_carries_retry_after() {
        let error = BotApiError::RateLimited {
            retry_after: Duration::from_secs(30),
        };
        assert_eq!(
            classify(&error),
            Classified::RateLimited {
                retry_after_secs: 30
            },
            "the authoritative delay must survive into the sender's decision"
        );
    }

    #[test]
    fn not_modified_description_is_a_success_no_op() {
        assert_eq!(
            classify(&api("Bad Request: message is not modified")),
            Classified::NotModified
        );
    }

    #[test]
    fn description_matching_is_case_insensitive() {
        assert_eq!(
            classify(&api("Bad Request: MESSAGE IS NOT MODIFIED")),
            Classified::NotModified
        );
    }

    #[test]
    fn blocked_user_is_permanent_and_never_retried() {
        assert_eq!(
            classify(&api("Forbidden: bot was blocked by the user")),
            Classified::Permanent {
                class: PermanentClass::BotBlocked
            }
        );
    }

    #[test]
    fn chat_not_found_is_permanent() {
        assert_eq!(
            classify(&api("Bad Request: chat not found")),
            Classified::Permanent {
                class: PermanentClass::ChatNotFound
            }
        );
    }

    #[test]
    fn membership_loss_descriptions_are_permanent() {
        for description in [
            "Forbidden: user is deactivated",
            "Forbidden: bot was kicked from the group chat",
            "Forbidden: bot is not a member of the channel chat",
            "Bad Request: not enough rights to send text messages to the chat",
        ] {
            assert_eq!(
                classify(&api(description)),
                Classified::Permanent {
                    class: PermanentClass::MembershipLost
                },
                "`{description}` must classify as membership loss"
            );
        }
    }

    #[test]
    fn message_not_editable_descriptions_are_permanent() {
        for description in [
            "Bad Request: message can't be edited",
            "Bad Request: message can not be edited",
            "Bad Request: there is no text in the message to edit",
        ] {
            assert_eq!(
                classify(&api(description)),
                Classified::Permanent {
                    class: PermanentClass::MessageNotEditable
                },
                "`{description}` must classify as a non-editable message"
            );
        }
    }

    #[test]
    fn gone_edit_target_is_permanent() {
        for description in [
            "Bad Request: message to edit not found",
            "Bad Request: message to forward not found",
        ] {
            assert_eq!(
                classify(&api(description)),
                Classified::Permanent {
                    class: PermanentClass::EditTargetGone
                },
                "`{description}` must classify as a gone edit target"
            );
        }
    }

    #[test]
    fn invalid_markup_is_permanent_invalid_payload() {
        assert_eq!(
            classify(&api(
                "Bad Request: can't parse entities: Unclosed tag at byte 12"
            )),
            Classified::Permanent {
                class: PermanentClass::InvalidPayload
            },
            "retrying identical bytes that failed to parse only storms the API"
        );
    }

    #[test]
    fn unparseable_reply_has_unknown_application_outcome() {
        assert_eq!(
            classify(&BotApiError::Json),
            Classified::OutcomeUnknown,
            "an unparseable response cannot prove whether the request was applied"
        );
    }

    #[test]
    fn migrated_to_supergroup_is_permanent_in_v1() {
        let error = BotApiError::ChatMigrated {
            to: ChatId(-1_002_003_004_005),
        };
        assert_eq!(
            classify(&error),
            Classified::Permanent {
                class: PermanentClass::ChatMigrated
            },
            "v1 dead-letters migrations; following the new chat id is a separate task"
        );
    }

    #[test]
    fn unknown_api_error_is_transient_within_bounds() {
        assert_eq!(
            classify(&api("Bad Request: something telegram invented last week")),
            Classified::Transient,
            "unknown descriptions get bounded retries instead of a permanent guess"
        );
    }

    #[test]
    fn unknown_taxonomy_variant_defaults_to_unknown_outcome() {
        assert_eq!(
            classify_shape(&FailureShape::Unknown),
            Classified::OutcomeUnknown,
            "variants added upstream after this table cannot prove non-application"
        );
    }
}
