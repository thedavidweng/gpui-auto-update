//! Ed25519 trust keys and artifact verification.
//!
//! Key and signature encodings are those of Sparkle 2, so one key pair can
//! sign macOS appcasts and the native Windows and Linux feeds:
//!
//! - a public key ([`TrustedKey`]) is the standard base64 encoding of the
//!   32-byte Ed25519 public key, the same string as Sparkle's `SUPublicEDKey`;
//! - a signature ([`EdSignature`]) is the standard base64 encoding of the
//!   64-byte pure Ed25519 (RFC 8032) signature over the raw artifact bytes,
//!   the same string as Sparkle's `sparkle:edSignature`.
//!
//! Verification is always fail-closed: there is no mode that accepts a
//! missing or invalid signature.

use std::fmt;
use std::io::Read;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};

const READ_CHUNK: usize = 64 * 1024;

/// Seed of the project's insecure test key pair.
///
/// The seed is published here on purpose: anyone can sign with this key, so
/// it is only for local development and tests. Because its public key is
/// recognizable ([`TrustedKey::is_insecure_test_key`]), release tooling can
/// refuse to treat it as a production trust key.
pub const INSECURE_TEST_KEY_SEED: [u8; 32] = *b"gpui-auto-update-INSECURE-test!!";

/// The public key an application trusts to sign its updates.
#[derive(Clone, PartialEq, Eq)]
pub struct TrustedKey(VerifyingKey);

/// A detached Ed25519 signature over an artifact (`sparkle:edSignature`).
#[derive(Clone, PartialEq, Eq)]
pub struct EdSignature(Signature);

/// Why a public key could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// The key is not valid standard base64.
    #[error("public key is not valid base64")]
    Base64,
    /// The decoded key is not 32 bytes long.
    #[error("public key must decode to 32 bytes, got {0}")]
    Length(usize),
    /// The bytes are not a valid Ed25519 point.
    #[error("public key is not a valid Ed25519 point")]
    InvalidPoint,
    /// The key has small order and cannot be trusted.
    #[error("public key is a weak (small-order) Ed25519 point")]
    Weak,
}

/// Why a signature string could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignatureError {
    /// The signature is not valid standard base64.
    #[error("signature is not valid base64")]
    Base64,
    /// The decoded signature is not 64 bytes long.
    #[error("signature must decode to 64 bytes, got {0}")]
    Length(usize),
}

/// Why an artifact failed verification.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// The artifact's size differs from the length declared in the feed.
    /// `actual` is a lower bound when reading stopped early because the
    /// declared length was exceeded.
    #[error("artifact is {actual} bytes but the feed declares {expected}")]
    LengthMismatch {
        /// Length declared by the feed.
        expected: u64,
        /// Bytes read before the mismatch was detected.
        actual: u64,
    },
    /// The signature does not match the artifact and trusted key.
    #[error("artifact signature does not verify against the trusted key")]
    BadSignature,
    /// Reading the artifact failed.
    #[error("failed to read artifact: {0}")]
    Io(#[from] std::io::Error),
}

impl TrustedKey {
    /// Loads a key from its Sparkle `SUPublicEDKey` form (base64 of 32 bytes).
    /// Surrounding whitespace is ignored.
    pub fn from_base64(text: &str) -> Result<Self, KeyError> {
        let raw = B64.decode(text.trim()).map_err(|_| KeyError::Base64)?;
        let bytes: [u8; 32] = raw
            .as_slice()
            .try_into()
            .map_err(|_| KeyError::Length(raw.len()))?;
        Self::from_bytes(&bytes)
    }

    /// Loads a key from its raw 32 bytes.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, KeyError> {
        let key = VerifyingKey::from_bytes(bytes).map_err(|_| KeyError::InvalidPoint)?;
        // Streaming verification is cofactorless and does not itself reject
        // small-order keys, which would let anyone forge signatures.
        if key.is_weak() {
            return Err(KeyError::Weak);
        }
        Ok(Self(key))
    }

    /// The key in Sparkle `SUPublicEDKey` form.
    pub fn to_base64(&self) -> String {
        B64.encode(self.0.to_bytes())
    }

    /// The public half of the insecure test key ([`INSECURE_TEST_KEY_SEED`]).
    pub fn insecure_test_key() -> Self {
        Self(SigningKey::from_bytes(&INSECURE_TEST_KEY_SEED).verifying_key())
    }

    /// Whether this is the public half of the insecure test key, which must
    /// never be trusted by a production release.
    pub fn is_insecure_test_key(&self) -> bool {
        *self == Self::insecure_test_key()
    }

    /// Verifies that `artifact` is exactly `expected_len` bytes and that
    /// `signature` is a valid signature of those bytes by this key.
    ///
    /// The reader is consumed in bounded chunks and reading stops as soon as
    /// more than `expected_len` bytes have been seen, so an oversized or
    /// endless stream is never read in full.
    pub fn verify_artifact(
        &self,
        signature: &EdSignature,
        expected_len: u64,
        mut artifact: impl Read,
    ) -> Result<(), VerifyError> {
        let mut verifier = self
            .0
            .verify_stream(&signature.0)
            .map_err(|_| VerifyError::BadSignature)?;
        let mut buf = vec![0u8; READ_CHUNK];
        let mut total: u64 = 0;
        loop {
            let n = match artifact.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            };
            total = total.saturating_add(n as u64);
            if total > expected_len {
                return Err(VerifyError::LengthMismatch {
                    expected: expected_len,
                    actual: total,
                });
            }
            verifier.update(&buf[..n]);
        }
        if total != expected_len {
            return Err(VerifyError::LengthMismatch {
                expected: expected_len,
                actual: total,
            });
        }
        verifier
            .finalize_and_verify()
            .map_err(|_| VerifyError::BadSignature)
    }
}

impl fmt::Debug for TrustedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("TrustedKey")
            .field(&self.to_base64())
            .finish()
    }
}

impl EdSignature {
    /// Parses a Sparkle `sparkle:edSignature` value (base64 of 64 bytes).
    /// Surrounding whitespace is ignored.
    pub fn from_base64(text: &str) -> Result<Self, SignatureError> {
        let raw = B64
            .decode(text.trim())
            .map_err(|_| SignatureError::Base64)?;
        let bytes: [u8; 64] = raw
            .as_slice()
            .try_into()
            .map_err(|_| SignatureError::Length(raw.len()))?;
        Ok(Self::from_bytes(&bytes))
    }

    /// Wraps a raw 64-byte signature.
    pub fn from_bytes(bytes: &[u8; 64]) -> Self {
        Self(Signature::from_bytes(bytes))
    }

    /// The signature in `sparkle:edSignature` form.
    pub fn to_base64(&self) -> String {
        B64.encode(self.0.to_bytes())
    }
}

impl fmt::Debug for EdSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("EdSignature")
            .field(&self.to_base64())
            .finish()
    }
}
