//! Deep-link intents and structured outbound payloads: the schema shapes and repository
//! behavior this change adds. Each test runs against its own disposable database.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "assertions in a test binary"
)]

use sqlx::Row;
use telegram_persistence::test_support::TestDatabase;

/// A fixed synthetic instant: whole seconds since the Unix epoch, never read from a wall clock.
const T0: i64 = 1_800_000_000;

/// A connected pool over the disposable database, for raw assertions.
async fn database() -> TestDatabase {
    TestDatabase::create()
        .await
        .expect("a disposable database must be creatable")
}

/// `(data_type, is_nullable)` of one column of one `telegram` table.
async fn column_shape(
    db: &telegram_persistence::Database,
    table: &str,
    column: &str,
) -> (String, String) {
    let row = sqlx::query(
        "select data_type, is_nullable
         from information_schema.columns
         where table_schema = 'telegram'
           and table_name = $1
           and column_name = $2",
    )
    .bind(table)
    .bind(column)
    .fetch_one(db.pool())
    .await
    .expect("the catalog read");
    (row.get("data_type"), row.get("is_nullable"))
}

/// Asserts one column carries exactly the expected type and nullability.
async fn expect_column(
    db: &telegram_persistence::Database,
    table: &str,
    column: &str,
    data_type: &str,
    nullable: &str,
) {
    let actual = column_shape(db, table, column).await;
    assert_eq!(
        actual,
        (data_type.to_owned(), nullable.to_owned()),
        "unexpected shape of {table}.{column}"
    );
}

/// The primary-key column names of one `telegram` table, in key order.
async fn primary_key(db: &telegram_persistence::Database, table: &str) -> Vec<String> {
    let rows = sqlx::query(
        "select a.attname as column_name
         from pg_index i
         join pg_class t on t.oid = i.indrelid
         join pg_attribute a on a.attrelid = t.oid and a.attnum = any(i.indkey)
         where t.relnamespace = 'telegram'::regnamespace
           and t.relname = $1
           and i.indisprimary
         order by a.attnum",
    )
    .bind(table)
    .fetch_all(db.pool())
    .await
    .expect("catalog read");
    rows.iter().map(|row| row.get("column_name")).collect()
}

/// The intents table exists on a fresh database with its app-minted UUID key (no default),
/// ownership columns, closed kind vocabulary, and timestamptz expiry; outbound jobs carry their
/// whole message payload as jsonb instead of bare text.
#[test]
fn interaction_intents_exist_with_expected_shape() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let test = database().await;

        // Existence first: everything below is meaningless until the relation is there.
        let present: i64 = sqlx::query_scalar(
            "select count(*) from information_schema.tables
             where table_schema = 'telegram' and table_name = 'interaction_intents'",
        )
        .fetch_one(test.pool())
        .await
        .expect("catalog read");
        assert_eq!(
            present, 1,
            "relation telegram.interaction_intents must exist"
        );

        // App-minted keys: the identifier IS the opaque token clients see, so the application
        // mints it and no database default may stand in.
        assert_eq!(
            primary_key(&test.database, "interaction_intents").await,
            ["id"]
        );
        let default: Option<String> = sqlx::query(
            "select column_default
             from information_schema.columns
             where table_schema = 'telegram' and table_name = 'interaction_intents'
               and column_name = 'id'",
        )
        .fetch_one(test.pool())
        .await
        .expect("catalog read")
        .get("column_default");
        assert!(default.is_none(), "the id is minted by the application");

        for (column, data_type) in [
            ("bot_id", "bigint"),
            ("telegram_user_id", "bigint"),
            ("chat_id", "bigint"),
            ("kind", "text"),
            ("operation_id", "uuid"),
            ("created_at", "timestamp with time zone"),
            ("expires_at", "timestamp with time zone"),
        ] {
            expect_column(
                &test.database,
                "interaction_intents",
                column,
                data_type,
                "NO",
            )
            .await;
        }
        // The address is nullable since item 6: attachment captures present a blob instead.
        expect_column(
            &test.database,
            "interaction_intents",
            "source_url",
            "text",
            "YES",
        )
        .await;
        // Provenance facts ride in an optional object-typed jsonb column.
        expect_column(
            &test.database,
            "interaction_intents",
            "metadata",
            "jsonb",
            "YES",
        )
        .await;

        // The kind vocabulary is closed at this flow's single intent kind.
        let checks: i64 = sqlx::query_scalar(
            "select count(*) from information_schema.check_constraints
             where constraint_schema = 'telegram'
               and constraint_name like '%interaction_intents%kind%'",
        )
        .fetch_one(test.pool())
        .await
        .expect("catalog read");
        assert!(checks >= 1, "kind carries a CHECK vocabulary");

        // Outbound payloads are whole: jsonb replaces the bare text column.
        expect_column(&test.database, "outbound_jobs", "payload", "jsonb", "NO").await;
        let body_gone: i64 = sqlx::query_scalar(
            "select count(*) from information_schema.columns
             where table_schema = 'telegram' and table_name = 'outbound_jobs'
               and column_name = 'body'",
        )
        .fetch_one(test.pool())
        .await
        .expect("catalog read");
        assert_eq!(body_gone, 0, "the bare-text body column is gone");
    });
}

/// A stored intent resolves for its owner while live, and stops resolving at expiry or for any
/// other user - indistinguishable absences either way.
#[tokio::test]
async fn inserted_intent_is_found_by_owner_until_expiry() {
    let test = database().await;
    let intent_id = uuid::Uuid::now_v7();
    let inserted = telegram_persistence::intents::NewIntent {
        id: intent_id,
        bot_id: 42,
        telegram_user_id: 900_700_601,
        chat_id: 900_700_601,
        operation_id: uuid::Uuid::now_v7(),
        source_url: Some("https://example.test/article".to_owned()),
        metadata: None,
        expires_at_secs: T0 + 3_600,
    };
    test.database
        .insert_intent(&inserted, T0)
        .await
        .expect("the insert succeeds");

    let owner_view = test
        .database
        .find_live_intent(intent_id, 900_700_601, T0 + 60)
        .await
        .expect("the lookup succeeds")
        .expect("a live intent resolves for its owner");
    assert_eq!(owner_view.id, intent_id);
    assert_eq!(
        owner_view.source_url.as_deref(),
        Some("https://example.test/article")
    );

    let expired = test
        .database
        .find_live_intent(intent_id, 900_700_601, T0 + 3_600)
        .await
        .expect("the lookup succeeds");
    assert!(expired.is_none(), "at expiry the intent is gone");

    let foreign = test
        .database
        .find_live_intent(intent_id, 111_222_333, T0 + 60)
        .await
        .expect("the lookup succeeds");
    assert!(foreign.is_none(), "another user resolves nothing");
}

/// An enqueued job restores its whole payload - text, parse mode, keyboard - bit-identically
/// through claim, which is what a restart redelivery does.
#[tokio::test]
async fn outbound_payload_round_trips_with_markup() {
    use telegram_persistence::outbound_jobs::{MessagePayload, NewOutboundJob, OutboundJobKind};

    let test = database().await;
    let markup = serde_json::json!({
        "inline_keyboard": [[{"text": "Open", "url": "https://t.me/ratatoskr_test_bot"}]]
    });
    let payload = MessagePayload {
        text: "<b>Completed</b>".to_owned(),
        parse_mode: Some("HTML".to_owned()),
        reply_markup: Some(markup),
    };
    let content_hash = payload.canonical().expect("canonical serialization");
    test.database
        .enqueue_outbound_job(
            &NewOutboundJob {
                bot_id: 42,
                chat_id: 900_700_601,
                kind: OutboundJobKind::SendMessage,
                payload: payload.clone(),
                content_hash,
                operation_id: None,
                revision: None,
                correlation_id: None,
                next_attempt_at: Some(T0),
            },
            T0,
        )
        .await
        .expect("enqueue");

    let claimed = test
        .database
        .claim_due_outbound_job(T0, 30)
        .await
        .expect("claim")
        .expect("one job due");
    assert_eq!(claimed.payload, payload, "bit-identical restore");
}

/// The intents table carries optional bounded metadata next to an optional source address:
/// attachment captures present a blob instead of a URL, and the table itself refuses rows that
/// present neither.
#[tokio::test]
async fn intents_carry_bounded_metadata_and_optional_source_address() {
    use telegram_persistence::intents::{BlobCapture, IntentMetadata, NewIntent};

    let test = database().await;

    // Shape: nullable address, object-typed optional metadata.
    let (url_type, url_nullable) =
        column_shape(&test.database, "interaction_intents", "source_url").await;
    assert_eq!(url_type, "text");
    assert_eq!(url_nullable, "YES");
    let (meta_type, meta_nullable) =
        column_shape(&test.database, "interaction_intents", "metadata").await;
    assert_eq!(meta_type, "jsonb");
    assert_eq!(meta_nullable, "YES");

    // Attachment capture: no address, blob facts in metadata.
    let attachment = NewIntent {
        id: uuid::Uuid::now_v7(),
        bot_id: 42,
        telegram_user_id: 900_700_601,
        chat_id: 900_700_601,
        operation_id: uuid::Uuid::now_v7(),
        source_url: None,
        metadata: Some(IntentMetadata {
            forward: None,
            blob: Some(BlobCapture {
                owner_service: "ratatoskr-telegram".to_owned(),
                algorithm: "sha256".to_owned(),
                digest_hex: "ab".repeat(32),
                media_type: "application/pdf".to_owned(),
                length_bytes: 1024,
            }),
        }),
        expires_at_secs: T0 + 3_600,
    };
    test.database
        .insert_intent(&attachment, T0)
        .await
        .expect("an attachment intent inserts");

    // URL capture unchanged: address present, provenance optional.
    let url_capture = NewIntent {
        id: uuid::Uuid::now_v7(),
        bot_id: 42,
        telegram_user_id: 900_700_601,
        chat_id: 900_700_601,
        operation_id: uuid::Uuid::now_v7(),
        source_url: Some("https://example.test/a".to_owned()),
        metadata: None,
        expires_at_secs: T0 + 3_600,
    };
    test.database
        .insert_intent(&url_capture, T0)
        .await
        .expect("a URL intent still inserts");

    // Neither presentation: the table refuses it outright.
    let neither = NewIntent {
        id: uuid::Uuid::now_v7(),
        bot_id: 42,
        telegram_user_id: 900_700_601,
        chat_id: 900_700_601,
        operation_id: uuid::Uuid::now_v7(),
        source_url: None,
        metadata: None,
        expires_at_secs: T0 + 3_600,
    };
    test.database
        .insert_intent(&neither, T0)
        .await
        .expect_err("an intent that presents nothing is refused");

    // Round trips: both shapes come back with their metadata intact.
    let stored_attachment = test
        .database
        .find_live_intent(attachment.id, 900_700_601, T0)
        .await
        .expect("lookup succeeds")
        .expect("attachment intent resolves");
    assert_eq!(stored_attachment.source_url, None);
    assert_eq!(
        stored_attachment
            .metadata
            .as_ref()
            .and_then(|m| m.blob.as_ref()),
        attachment.metadata.as_ref().and_then(|m| m.blob.as_ref())
    );
    let stored_url = test
        .database
        .find_live_intent(url_capture.id, 900_700_601, T0)
        .await
        .expect("lookup succeeds")
        .expect("url intent resolves");
    assert_eq!(
        stored_url.source_url.as_deref(),
        Some("https://example.test/a")
    );
    assert!(stored_url.metadata.is_none());
}

/// Forward-origin facts survive the persistence boundary unchanged and reject unknown members -
/// the minimized provenance record has a closed shape.
#[tokio::test]
async fn forward_origin_metadata_round_trips_through_the_persistence_boundary() {
    use telegram_persistence::intents::{CaptureOrigin, IntentMetadata, NewIntent};

    let test = database().await;
    let origin = IntentMetadata {
        forward: Some(CaptureOrigin::Channel {
            chat_id: -100_200_300,
            message_id: 77,
            sent_at_secs: 1_700_000_000,
        }),
        blob: None,
    };
    let intent = NewIntent {
        id: uuid::Uuid::now_v7(),
        bot_id: 42,
        telegram_user_id: 900_700_601,
        chat_id: 900_700_601,
        operation_id: uuid::Uuid::now_v7(),
        source_url: Some("https://example.test/story".to_owned()),
        metadata: Some(origin.clone()),
        expires_at_secs: T0 + 3_600,
    };
    test.database
        .insert_intent(&intent, T0)
        .await
        .expect("a forwarded-link intent inserts");

    let stored = test
        .database
        .find_live_intent(intent.id, 900_700_601, T0)
        .await
        .expect("lookup succeeds")
        .expect("intent resolves");
    assert_eq!(stored.metadata, Some(origin));

    // Unknown members are refused: the shape is closed, not a free-form bag.
    let open: Result<IntentMetadata, _> = serde_json::from_value(serde_json::json!({
        "forward": {
            "kind": "channel",
            "chat_id": -100_200_300,
            "message_id": 77,
            "sent_at_secs": 1_700_000_000,
            "author_signature": "unexpected"
        }
    }));
    assert!(open.is_err(), "unknown members are refused");
}
