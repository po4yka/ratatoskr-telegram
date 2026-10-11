//! Generating the assertion signing key and deriving the public half Edge needs.
//!
//! This service signs identity assertions with a 32-byte Ed25519 seed that it reads as 64
//! hexadecimal characters (`RATATOSKR__PLATFORM__ASSERTION_SIGNING_KEY_FILE`). Edge verifies them
//! with the matching public key, configured as `RATATOSKR__IDENTITY__ASSERTION_KEY` in base64
//! (standard alphabet, padded). Both halves are produced here, from one decoder shared by the
//! webhook, the dispatcher and the `assertion-key` command, so they cannot disagree about the
//! format. Nothing in this module prints, logs or returns the seed inside an error.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use base64::Engine as _;
use ed25519_dalek::SigningKey;

/// Why a seed could not be read, decoded or written. Never carries the seed.
#[derive(Debug, thiserror::Error)]
pub enum AssertionKeyError {
    /// The text is not 64 hexadecimal characters.
    #[error("the assertion signing key must be 64 hex characters")]
    Malformed,
    /// The seed file could not be read.
    #[error("the seed file could not be read")]
    Unreadable(#[source] std::io::Error),
    /// The seed file exists already; a seed is never replaced.
    #[error("the seed file already exists and is never replaced")]
    Exists,
    /// The seed file could not be created or written.
    #[error("the seed file could not be written")]
    Unwritable(#[source] std::io::Error),
    /// The operating system could not supply random bytes.
    #[error("the operating system could not supply random bytes")]
    Entropy,
}

/// Decode a 64-hex-character Ed25519 seed (either case) into its 32 bytes.
///
/// # Errors
///
/// [`AssertionKeyError::Malformed`] when the text is not exactly 64 hexadecimal characters.
pub fn decode_seed(hex_key: &str) -> Result<[u8; 32], AssertionKeyError> {
    fn digit(character: u8) -> Option<u8> {
        match character {
            b'0'..=b'9' => Some(character - b'0'),
            b'a'..=b'f' => Some(character - b'a' + 10),
            b'A'..=b'F' => Some(character - b'A' + 10),
            _ => None,
        }
    }
    let bytes = hex_key.as_bytes();
    if bytes.len() != 64 {
        return Err(AssertionKeyError::Malformed);
    }
    let mut seed = [0_u8; 32];
    for (slot, pair) in seed.iter_mut().zip(bytes.chunks_exact(2)) {
        let [high, low] = pair else {
            return Err(AssertionKeyError::Malformed);
        };
        let (Some(high), Some(low)) = (digit(*high), digit(*low)) else {
            return Err(AssertionKeyError::Malformed);
        };
        *slot = (high << 4) | low;
    }
    Ok(seed)
}

fn public_key_of(seed: &[u8; 32]) -> String {
    base64::engine::general_purpose::STANDARD
        .encode(SigningKey::from_bytes(seed).verifying_key().to_bytes())
}

/// The base64 (standard alphabet, padded) public key of the seed given as 64 hex characters.
///
/// # Errors
///
/// [`AssertionKeyError::Malformed`] when the text is not exactly 64 hexadecimal characters.
pub fn public_key_base64(hex_key: &str) -> Result<String, AssertionKeyError> {
    Ok(public_key_of(&decode_seed(hex_key)?))
}

/// The base64 public key of the seed file at `path`, read the way the services read it (the
/// surrounding whitespace is trimmed).
///
/// # Errors
///
/// [`AssertionKeyError::Unreadable`] when the file cannot be read and
/// [`AssertionKeyError::Malformed`] when it does not hold 64 hexadecimal characters.
pub fn public_key_of_file(path: &Path) -> Result<String, AssertionKeyError> {
    let text = std::fs::read_to_string(path).map_err(AssertionKeyError::Unreadable)?;
    public_key_base64(text.trim())
}

/// Create a new seed file at `path` (exclusively, mode `0600`) holding 64 lowercase hexadecimal
/// characters and a newline, and return the base64 public key. An existing file is refused and
/// left untouched.
///
/// # Errors
///
/// [`AssertionKeyError::Exists`] when `path` exists, [`AssertionKeyError::Entropy`] when no
/// randomness is available, and [`AssertionKeyError::Unwritable`] for any other filesystem failure.
pub fn generate(path: &Path) -> Result<String, AssertionKeyError> {
    let mut seed = [0_u8; 32];
    getrandom::fill(&mut seed).map_err(|_| AssertionKeyError::Entropy)?;
    let mut text = String::with_capacity(65);
    for byte in seed {
        // Writing to a `String` cannot fail.
        let _ = write!(text, "{byte:02x}");
    }
    text.push('\n');

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            AssertionKeyError::Exists
        } else {
            AssertionKeyError::Unwritable(error)
        }
    })?;
    if let Err(error) = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
    {
        // This call created the file exclusively, so removing it cannot discard anyone's seed.
        let _ = std::fs::remove_file(path);
        return Err(AssertionKeyError::Unwritable(error));
    }
    Ok(public_key_of(&seed))
}
