//! Telegram-owned content-addressed blob storage.
//!
//! The fleet `blob-references` convention (ratatoskr-workspace store) gives every service its own
//! content-addressed store on the deployment target's durable device and forbids any shared blob
//! storage service: references travel as [`BlobRef`] values, never as paths, hosts, or URLs. This
//! crate is that store for `ratatoskr-telegram` - attachments downloaded from the Bot API land
//! here, hashed while streaming, before a capture references them.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, MediaType,
};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncRead;
use uuid::Uuid;

/// The owner service name every reference published by this store carries.
pub const OWNER_SERVICE: &str = "ratatoskr-telegram";

/// Chunk size for streaming copies; bounds memory regardless of attachment size.
const CHUNK_BYTES: usize = 64 * 1024;

/// Everything that can go wrong while storing or verifying blobs.
#[derive(Debug, thiserror::Error)]
pub enum BlobStoreError {
    /// The filesystem failed underneath the store.
    #[error("blob storage io failed")]
    Io(#[from] Arc<io::Error>),
    /// A reference does not describe what the store holds for it.
    #[error("blob reference mismatch: {0}")]
    Mismatch(&'static str),
    /// The source produced more bytes than the caller's budget allows. Nothing is published.
    #[error("the byte budget was exceeded")]
    BudgetExceeded {
        /// The configured ceiling that was crossed.
        limit: u64,
    },
}

/// One stored artifact's ownership facts, kept beside the bytes so verification can check owner,
/// media type, and length without trusting the caller's word for them.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreMeta {
    owner_service: String,
    media_type: String,
    length_bytes: u64,
}

/// The content-addressed store rooted at one directory tree.
#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
    owner: BlobOwner,
}

impl BlobStore {
    /// Open (creating if absent) the store layout under `root`.
    ///
    /// # Errors
    ///
    /// When the directory tree cannot be created or the fixed owner name stops parsing.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, BlobStoreError> {
        let root = root.into();
        std::fs::create_dir_all(Self::staging_dir(&root))
            .map_err(|error| BlobStoreError::Io(Arc::new(error)))?;
        Ok(Self {
            root,
            owner: BlobOwner::parse(OWNER_SERVICE)
                .map_err(|_| BlobStoreError::Mismatch("owner service name is invalid"))?,
        })
    }

    fn staging_dir(root: &Path) -> PathBuf {
        root.join("staging")
    }

    /// The content path a digest finalizes to: `sha256/<first two hex>/<remaining hex>`.
    fn content_path(&self, hex: &str) -> Option<PathBuf> {
        let (prefix, rest) = hex.split_at_checked(2)?;
        Some(self.root.join("sha256").join(prefix).join(rest))
    }

    fn meta_path(final_path: &Path) -> PathBuf {
        let mut with_meta = final_path.to_path_buf().into_os_string();
        with_meta.push(".meta");
        PathBuf::from(with_meta)
    }

    /// Stream `source` into the store, hashing and counting while copying, and publish the
    /// resulting [`BlobRef`]. Storing bytes that already exist converges on the same reference
    /// without duplicating them. When `budget` is set and the source produces more bytes than it
    /// allows, the store aborts with [`BlobStoreError::BudgetExceeded`] and publishes nothing.
    ///
    /// # Errors
    ///
    /// Filesystem failures, a digest that stops matching the contract pattern, and a source
    /// overrunning the budget.
    pub async fn store(
        &self,
        media_type: &MediaType,
        source: &mut (impl AsyncRead + Unpin),
        budget: Option<u64>,
    ) -> Result<BlobRef, BlobStoreError> {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let staging = Self::staging_dir(&self.root).join(format!(
            "{}-{uuid}.part",
            self.owner.as_str(),
            uuid = Uuid::now_v7()
        ));
        let mut hasher = Sha256::new();
        let mut total: u64 = 0;
        let mut chunk = vec![0u8; CHUNK_BYTES];
        let mut file = match tokio::fs::File::create(&staging).await {
            Ok(file) => file,
            Err(error) => return Err(BlobStoreError::Io(Arc::new(error))),
        };
        // Copy, hash, count, and enforce the budget in one pass; any failure after creation drops
        // the staging copy so no partial bytes can ever publish under a content name.
        let copied = async {
            loop {
                let read = source
                    .read(&mut chunk)
                    .await
                    .map_err(|error| BlobStoreError::Io(Arc::new(error)))?;
                if read == 0 {
                    break;
                }
                total += u64::try_from(read).unwrap_or(u64::MAX);
                if budget.is_some_and(|limit| total > limit) {
                    return Err(BlobStoreError::BudgetExceeded {
                        limit: budget.unwrap_or_default(),
                    });
                }
                hasher.update(&chunk[..read]);
                file.write_all(&chunk[..read])
                    .await
                    .map_err(|error| BlobStoreError::Io(Arc::new(error)))?;
            }
            file.flush()
                .await
                .map_err(|error| BlobStoreError::Io(Arc::new(error)))
        };
        if let Err(error) = copied.await {
            let _ = tokio::fs::remove_file(&staging).await;
            return Err(error);
        }
        drop(file);

        let hex = format!("{:x}", hasher.finalize());
        let Some(final_path) = self.content_path(&hex) else {
            return Err(BlobStoreError::Mismatch("digest hex has no split point"));
        };
        if !final_path.exists() {
            if let Some(parent) = final_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| BlobStoreError::Io(Arc::new(error)))?;
            }
            // Hard-link keeps staging and content cheaply on one device; rename is the fallback.
            // Either way both names stay inside this store's own tree.
            if tokio::fs::hard_link(&staging, &final_path).await.is_ok() {
                let _ = tokio::fs::remove_file(&staging).await;
            } else {
                tokio::fs::rename(&staging, &final_path)
                    .await
                    .map_err(|error| BlobStoreError::Io(Arc::new(error)))?;
            }
            let meta = StoreMeta {
                owner_service: self.owner.as_str().to_owned(),
                media_type: media_type.as_str().to_owned(),
                length_bytes: total,
            };
            let meta_bytes = serde_json::to_vec(&meta)
                .map_err(|error| BlobStoreError::Io(Arc::new(io::Error::other(error))))?;
            tokio::fs::write(Self::meta_path(&final_path), meta_bytes)
                .await
                .map_err(|error| BlobStoreError::Io(Arc::new(error)))?;
        } else {
            let _ = tokio::fs::remove_file(&staging).await;
        }

        Ok(BlobRef {
            owner_service: self.owner.clone(),
            digest: ContentDigest {
                algorithm: DigestAlgorithm::Sha256,
                hex: DigestHex::parse(&hex)
                    .map_err(|_| BlobStoreError::Mismatch("digest hex stopped matching"))?,
            },
            media_type: media_type.clone(),
            length_bytes: total,
        })
    }

    /// Verify that the store holds exactly what `blob` claims: ownership facts from the sidecar
    /// record, then the SHA-256 of the stored bytes against the reference digest.
    ///
    /// # Errors
    ///
    /// Missing files, unreadable sidecar records, or any field disagreeing with the reference.
    pub fn verify(&self, blob: &BlobRef) -> Result<(), BlobStoreError> {
        let hex = blob.digest.hex.as_str();
        if blob.digest.algorithm != DigestAlgorithm::Sha256 {
            return Err(BlobStoreError::Mismatch("unsupported digest algorithm"));
        }
        if blob.owner_service != self.owner {
            return Err(BlobStoreError::Mismatch("foreign owner service"));
        }
        let Some(final_path) = self.content_path(hex) else {
            return Err(BlobStoreError::Mismatch("digest hex has no split point"));
        };
        let meta_bytes = std::fs::read(Self::meta_path(&final_path))
            .map_err(|error| BlobStoreError::Io(Arc::new(error)))?;
        let meta: StoreMeta = serde_json::from_slice(&meta_bytes)
            .map_err(|_| BlobStoreError::Mismatch("unreadable store metadata"))?;
        if meta.owner_service != blob.owner_service.as_str() {
            return Err(BlobStoreError::Mismatch("stored owner disagrees"));
        }
        if meta.media_type != blob.media_type.as_str() {
            return Err(BlobStoreError::Mismatch("stored media type disagrees"));
        }
        if meta.length_bytes != blob.length_bytes {
            return Err(BlobStoreError::Mismatch("stored length disagrees"));
        }
        let bytes =
            std::fs::read(&final_path).map_err(|error| BlobStoreError::Io(Arc::new(error)))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != blob.length_bytes {
            return Err(BlobStoreError::Mismatch("content length disagrees"));
        }
        if format!("{:x}", sha2::Sha256::digest(&bytes)) != hex {
            return Err(BlobStoreError::Mismatch("content digest disagrees"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod store_tests {
    #![allow(
        clippy::expect_used,
        clippy::panic,
        clippy::unwrap_used,
        reason = "assertions in a test module"
    )]

    use super::*;
    use sha2::{Digest as _, Sha256};

    const SAMPLE: &[u8] = b"ratatoskr telegram blob-store sample payload\n";

    fn sample_hex() -> String {
        format!("{:x}", Sha256::digest(SAMPLE))
    }

    async fn sample_source() -> std::io::Cursor<&'static [u8]> {
        std::io::Cursor::new(SAMPLE)
    }

    fn pdf_media() -> MediaType {
        MediaType::parse("application/pdf").expect("pdf parses")
    }

    #[tokio::test]
    async fn storing_bytes_publishes_the_fleet_blob_ref_and_content_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).expect("store opens");

        let mut source = sample_source().await;
        let blob = store
            .store(&pdf_media(), &mut source, None)
            .await
            .expect("sample stores");

        assert_eq!(blob.owner_service.as_str(), OWNER_SERVICE);
        assert_eq!(blob.digest.hex.as_str(), sample_hex());
        assert_eq!(blob.digest.algorithm, DigestAlgorithm::Sha256);
        assert_eq!(blob.media_type.as_str(), "application/pdf");
        assert_eq!(blob.length_bytes, SAMPLE.len() as u64);

        let hex = sample_hex();
        let (prefix, rest) = hex.split_at_checked(2).expect("split");
        let content = dir.path().join("sha256").join(prefix).join(rest);
        assert!(
            content.exists(),
            "content lands at the content-addressed path"
        );
        assert!(
            BlobStore::meta_path(&content).exists(),
            "ownership facts sit beside the bytes"
        );
    }

    #[tokio::test]
    async fn identical_bytes_converge_on_one_reference_without_duplication() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).expect("store opens");

        let mut first = sample_source().await;
        let mut second = sample_source().await;
        let one = store
            .store(&pdf_media(), &mut first, None)
            .await
            .expect("first");
        let two = store
            .store(&pdf_media(), &mut second, None)
            .await
            .expect("second");

        assert_eq!(one, two, "identical bytes are one blob");

        let hashed = walk_files(&dir.path().join("sha256"));
        assert_eq!(
            hashed
                .iter()
                .filter(|p| !p.to_string_lossy().ends_with(".meta"))
                .count(),
            1,
            "exactly one content file exists"
        );
    }

    #[tokio::test]
    async fn verify_accepts_its_own_store_and_rejects_tampering() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).expect("store opens");
        let mut source = sample_source().await;
        let blob = store
            .store(&pdf_media(), &mut source, None)
            .await
            .expect("stored");
        store.verify(&blob).expect("fresh store verifies");

        // Wrong media type on an otherwise honest reference.
        let html = MediaType::parse("text/html").expect("html parses");
        let mut wrong_media = blob.clone();
        wrong_media.media_type = html;
        assert!(store.verify(&wrong_media).is_err(), "media type must match");

        // Foreign owner claiming these bytes.
        let vault = BlobOwner::parse("ratatoskr-vault").expect("parses");
        let mut foreign = blob.clone();
        foreign.owner_service = vault;
        assert!(store.verify(&foreign).is_err(), "owner must match");

        // Wrong length.
        let mut short = blob.clone();
        short.length_bytes -= 1;
        assert!(store.verify(&short).is_err(), "length must match");

        // Flip one stored byte in place.
        let hex = sample_hex();
        let (prefix, rest) = hex.split_at_checked(2).expect("split");
        let content = dir.path().join("sha256").join(prefix).join(rest);
        let mut tampered = std::fs::read(&content).expect("read back");
        let last = tampered.len() - 1;
        tampered[last] ^= 0xff;
        std::fs::write(&content, &tampered).expect("rewrite");
        assert!(
            store.verify(&blob).is_err(),
            "tampered bytes must fail verify"
        );
    }

    #[tokio::test]
    async fn a_stream_overrunning_the_budget_aborts_without_publishing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).expect("store opens");

        let mut source = sample_source().await;
        let outcome = store
            .store(&pdf_media(), &mut source, Some(8))
            .await
            .expect_err("an overrunning source must abort");

        assert!(
            matches!(outcome, BlobStoreError::BudgetExceeded { limit: 8 }),
            "{outcome:?}"
        );
        let hashed = walk_files(&dir.path().join("sha256"));
        assert!(hashed.is_empty(), "nothing may publish: {hashed:?}");
        let staged = walk_files(&dir.path().join("staging"));
        assert!(
            staged.is_empty(),
            "no partial staging file survives: {staged:?}"
        );
    }

    #[tokio::test]
    async fn bytes_exactly_within_the_budget_store_normally() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).expect("store opens");

        let mut source = sample_source().await;
        let blob = store
            .store(&pdf_media(), &mut source, Some(SAMPLE.len() as u64))
            .await
            .expect("an exact-budget source stores");
        store.verify(&blob).expect("the stored blob verifies");
    }

    fn walk_files(root: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    found.push(path);
                }
            }
        }
        found
    }
}
