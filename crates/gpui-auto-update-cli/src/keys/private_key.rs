//! Ed25519 private signing keys in Sparkle's `generate_keys` formats.
//!
//! Sparkle stores and exports a private key as standard base64 of either
//!
//! - 32 bytes: the RFC 8032 seed (all keys Sparkle generates today), or
//! - 96 bytes: the legacy format, a 64-byte expanded secret (clamped
//!   SHA-512(seed) scalar followed by the hash prefix) and then the 32-byte
//!   public key. The seed cannot be recovered from this form.
//!
//! Any other length is rejected, as Sparkle's own tools do.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::hazmat::{ExpandedSecretKey, raw_sign};
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use gpui_auto_update_core::trust::{EdSignature, INSECURE_TEST_KEY_SEED, TrustedKey};
use zeroize::Zeroizing;

/// A private signing key. Its contents are wiped from memory on drop and
/// never appear in `Debug` output or error messages.
pub struct PrivateKey {
    inner: Inner,
    public: VerifyingKey,
}

enum Inner {
    Seed(SigningKey),
    Legacy {
        expanded: ExpandedSecretKey,
        raw: Zeroizing<[u8; 96]>,
    },
}

/// Why private key text could not be loaded. Variants never carry key bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivateKeyError {
    /// The text is not valid standard base64.
    Base64,
    /// The decoded key is neither 32 nor 96 bytes long.
    Length(usize),
    /// A legacy 96-byte key whose embedded public key does not match its
    /// secret half.
    LegacyMismatch,
    /// The derived public key is unusable (small order).
    WeakPublicKey,
}

impl fmt::Display for PrivateKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Base64 => f.write_str("private key is not valid base64"),
            Self::Length(n) => write!(
                f,
                "private key must decode to 32 bytes (seed) or 96 bytes (legacy Sparkle format), got {n}"
            ),
            Self::LegacyMismatch => f.write_str(
                "legacy 96-byte private key is corrupt: its embedded public key does not match its secret",
            ),
            Self::WeakPublicKey => f.write_str("private key yields a weak (small-order) public key"),
        }
    }
}

impl std::error::Error for PrivateKeyError {}

impl PrivateKey {
    /// Generates a new random key from the operating system's RNG.
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::fill(seed.as_mut())?;
        Ok(Self::from_seed(&seed))
    }

    /// The project's insecure, publicly known test key. See
    /// [`INSECURE_TEST_KEY_SEED`].
    pub fn insecure_test_key() -> Self {
        Self::from_seed(&INSECURE_TEST_KEY_SEED)
    }

    fn from_seed(seed: &[u8; 32]) -> Self {
        let key = SigningKey::from_bytes(seed);
        Self {
            public: key.verifying_key(),
            inner: Inner::Seed(key),
        }
    }

    /// Parses Sparkle private key text (base64 of 32 or 96 bytes).
    /// Surrounding whitespace is ignored.
    pub fn from_sparkle_base64(text: &str) -> Result<Self, PrivateKeyError> {
        let raw = Zeroizing::new(
            B64.decode(text.trim())
                .map_err(|_| PrivateKeyError::Base64)?,
        );
        let key = match raw.len() {
            32 => {
                let mut seed = Zeroizing::new([0u8; 32]);
                seed.copy_from_slice(&raw);
                Self::from_seed(&seed)
            }
            96 => {
                let mut legacy = Zeroizing::new([0u8; 96]);
                legacy.copy_from_slice(&raw);
                let mut secret = Zeroizing::new([0u8; 64]);
                secret.copy_from_slice(&legacy[..64]);
                let expanded = ExpandedSecretKey::from_bytes(&secret);
                let public = VerifyingKey::from(&expanded);
                if public.as_bytes()[..] != legacy[64..] {
                    return Err(PrivateKeyError::LegacyMismatch);
                }
                Self {
                    inner: Inner::Legacy {
                        expanded,
                        raw: legacy,
                    },
                    public,
                }
            }
            n => return Err(PrivateKeyError::Length(n)),
        };
        if key.public.is_weak() {
            return Err(PrivateKeyError::WeakPublicKey);
        }
        Ok(key)
    }

    /// The key in the text form Sparkle's `generate_keys -x` exports and
    /// `-f` / `sign_update --ed-key-file` accept. Legacy keys keep their
    /// 96-byte form.
    pub fn to_sparkle_base64(&self) -> Zeroizing<String> {
        Zeroizing::new(match &self.inner {
            Inner::Seed(key) => B64.encode(key.as_bytes()),
            Inner::Legacy { raw, .. } => B64.encode(raw.as_slice()),
        })
    }

    /// The matching public key, as configured in the application
    /// (`SUPublicEDKey`).
    pub fn public_key(&self) -> TrustedKey {
        TrustedKey::from_bytes(self.public.as_bytes())
            .expect("public key validity is checked on construction")
    }

    /// Whether this is the project's insecure, publicly known test key.
    pub fn is_insecure_test_key(&self) -> bool {
        self.public_key().is_insecure_test_key()
    }

    /// Whether the key uses Sparkle's legacy 96-byte format.
    pub fn is_legacy(&self) -> bool {
        matches!(self.inner, Inner::Legacy { .. })
    }

    /// Signs `data` with pure Ed25519, producing a Sparkle
    /// `sparkle:edSignature` value.
    // No `keys` subcommand signs; this is the signing entry point for feeds.
    #[allow(dead_code)]
    pub fn sign(&self, data: &[u8]) -> EdSignature {
        let signature = match &self.inner {
            Inner::Seed(key) => key.sign(data),
            Inner::Legacy { expanded, .. } => {
                raw_sign::<sha2::Sha512>(expanded, data, &self.public)
            }
        };
        EdSignature::from_bytes(&signature.to_bytes())
    }
}

impl fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrivateKey")
            .field("public", &self.public_key())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    //! Expected values are the RFC 8032 section 7.1 TEST 1 vector.

    use super::*;

    const SEED: &str = "nWGxne/9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A=";
    const PUBLIC: &str = "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=";
    const LEGACY: &str = "MHyDhk8oM8tCei7xwAoBPP3/J2jZgMCjpSDwBpBN6U+bTwr+KAt0aneGhOdUQlAgV7dHOgPwj5b1o46Sh+Afj9damAGCsQq31Uv+08lkBzoO4XLz2qYjJa8CGmj3B1Ea";
    /// Signature of the empty message.
    const EMPTY_SIG: &str =
        "5VZDAMNgrHKQhuLMgG6CioSHfx645dl02HPgZSJJAVVfuIIVkKM7rMYeOXAc+bRr0lv18FlbviRlUUFDjnoQCw==";

    #[test]
    fn seed_key_signs_the_rfc_vector() {
        let key = PrivateKey::from_sparkle_base64(SEED).unwrap();
        assert_eq!(key.public_key().to_base64(), PUBLIC);
        assert_eq!(key.sign(b"").to_base64(), EMPTY_SIG);
        assert!(!key.is_legacy());
    }

    #[test]
    fn legacy_key_signs_identically_to_its_seed() {
        let key = PrivateKey::from_sparkle_base64(LEGACY).unwrap();
        assert!(key.is_legacy());
        assert_eq!(key.public_key().to_base64(), PUBLIC);
        assert_eq!(key.sign(b"").to_base64(), EMPTY_SIG);
    }

    #[test]
    fn signatures_verify_with_core_trust() {
        let data = vec![7u8; 100_000];
        for text in [SEED, LEGACY] {
            let key = PrivateKey::from_sparkle_base64(text).unwrap();
            let sig = key.sign(&data);
            key.public_key()
                .verify_artifact(&sig, data.len() as u64, data.as_slice())
                .unwrap();
        }
    }

    #[test]
    fn sparkle_text_round_trips_in_its_original_format() {
        for text in [SEED, LEGACY] {
            let key = PrivateKey::from_sparkle_base64(&format!(" {text}\n")).unwrap();
            assert_eq!(key.to_sparkle_base64().as_str(), text);
        }
    }

    #[test]
    fn other_lengths_are_rejected_like_sparkle() {
        let err = PrivateKey::from_sparkle_base64(&B64.encode([1u8; 64])).unwrap_err();
        assert_eq!(err, PrivateKeyError::Length(64));
        assert_eq!(
            PrivateKey::from_sparkle_base64("not base64!").unwrap_err(),
            PrivateKeyError::Base64
        );
    }

    #[test]
    fn corrupt_legacy_key_is_rejected() {
        let mut raw = B64.decode(LEGACY).unwrap();
        raw[95] ^= 1;
        assert_eq!(
            PrivateKey::from_sparkle_base64(&B64.encode(raw)).unwrap_err(),
            PrivateKeyError::LegacyMismatch
        );
    }

    #[test]
    fn debug_output_redacts_the_secret() {
        let key = PrivateKey::from_sparkle_base64(SEED).unwrap();
        let debug = format!("{key:?}");
        assert!(!debug.contains(SEED));
        assert!(debug.contains(PUBLIC));
    }

    #[test]
    fn generated_keys_are_distinct_seed_keys() {
        let a = PrivateKey::generate().unwrap();
        let b = PrivateKey::generate().unwrap();
        assert!(!a.is_legacy());
        assert_ne!(a.public_key(), b.public_key());
        assert!(!a.is_insecure_test_key());
    }

    #[test]
    fn insecure_test_key_is_recognized() {
        assert!(PrivateKey::insecure_test_key().is_insecure_test_key());
        assert!(
            !PrivateKey::from_sparkle_base64(SEED)
                .unwrap()
                .is_insecure_test_key()
        );
    }
}
