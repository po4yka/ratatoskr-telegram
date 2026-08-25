//! Bearer sessions per Telegram sender: exchanged by signed assertion, cached until shortly
//! before expiry, minted once under concurrency.

use std::collections::HashMap;
use std::str::FromStr as _;
use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use secrecy::ExposeSecret as _;
use tokio::sync::Mutex;

use crate::{Client, PlatformError, assertion::AssertionIssuer};

/// Where "now" comes from. Production uses the process clock; tests freeze time so refresh
/// behavior is asserted without sleeping.
pub trait Clock: Send + Sync {
    /// The current instant.
    fn now(&self) -> Timestamp;
}

/// Process-clock implementation of [`Clock`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// How long before expiry a cached session is considered stale and re-exchanged.
const REFRESH_MARGIN_SECS: i64 = 300;

/// One cached bearer credential and when it stops working.
#[derive(Debug, Clone)]
struct CachedSession {
    credential: String,
    expires_at: Timestamp,
}

/// Hands out a Platform bearer credential per Telegram sender.
///
/// The first caller for a sender triggers one assertion exchange; everyone else reuses the
/// result until shortly before expiry. Concurrent first callers share the single in-flight
/// exchange instead of racing the nonce store with competing assertions.
pub struct SessionSource {
    client: Client,
    issuer: AssertionIssuer,
    clock: Box<dyn Clock>,
    cached: Mutex<HashMap<String, Arc<CachedSession>>>,
    in_flight: Mutex<()>,
}

impl std::fmt::Debug for SessionSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionSource")
            .field("senders_cached", &self.cached)
            .finish_non_exhaustive()
    }
}

impl SessionSource {
    /// Build a source over the given client, issuer, and clock.
    #[must_use]
    pub fn new(client: Client, issuer: AssertionIssuer, clock: Box<dyn Clock>) -> Self {
        Self {
            client,
            issuer,
            clock,
            cached: Mutex::default(),
            // A single lock serializes exchanges across ALL senders; contention here is bounded
            // by the owner-only deployment and the exchange is rare (once per sender per hour).
            in_flight: Mutex::new(()),
        }
    }

    /// The underlying Platform client, for the calls a session authenticates.
    #[must_use]
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The bearer credential for `subject`, exchanging a fresh session when needed.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] if issuance or the exchange fails.
    pub async fn credential(&self, subject: &str) -> Result<String, PlatformError> {
        if let Some(hit) = self.fresh_entry(subject).await {
            return Ok(hit.credential.clone());
        }
        let fresh = self.exchange(subject).await?;
        Ok(fresh.credential)
    }

    /// The cached entry for `subject` when it is still inside its refresh margin.
    async fn fresh_entry(&self, subject: &str) -> Option<Arc<CachedSession>> {
        let cached = self.cached.lock().await;
        let session = cached.get(subject)?;
        let stale_from = session.expires_at - SignedDuration::from_secs(REFRESH_MARGIN_SECS);
        (self.clock.now() < stale_from).then(|| Arc::clone(session))
    }

    /// Exchange exactly once per need: concurrent callers queue on one lock and the loser
    /// re-checks the cache instead of racing the nonce store with a second assertion.
    async fn exchange(&self, subject: &str) -> Result<CachedSession, PlatformError> {
        let _guard = self.in_flight.lock().await;
        if let Some(hit) = self.fresh_entry(subject).await {
            return Ok((*hit).clone());
        }
        let now = self.clock.now();
        let token = self
            .issuer
            .issue(subject, now, Duration::from_secs(EXCHANGE_LIFETIME_SECS))?;
        let minted = self.client.exchange_assertion(&token).await?;
        let expires_at =
            Timestamp::from_str(&minted.expires_at).map_err(|_| PlatformError::MalformedFrame)?;
        let fresh = Arc::new(CachedSession {
            credential: minted.credential.expose_secret().to_owned(),
            expires_at,
        });
        self.cached
            .lock()
            .await
            .insert(subject.to_owned(), Arc::clone(&fresh));
        Ok((*fresh).clone())
    }
}

/// The lifetime requested for an exchanged assertion. Upstream caps sessions at one hour.
const EXCHANGE_LIFETIME_SECS: u64 = 3_600;
