//! Ed25519 trust keys and artifact verification.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signer, SigningKey};
use gpui_auto_update_core::trust::{EdSignature, KeyError, TrustedKey, VerifyError};

const SPARKLE_PUB: &str = include_str!("fixtures/sparkle-sign-update/pub.b64");
const SPARKLE_SIG: &str = include_str!("fixtures/sparkle-sign-update/sig.b64");
const SPARKLE_ARTIFACT: &[u8] = include_bytes!("fixtures/sparkle-sign-update/artifact.bin");

fn disposable_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn public_b64(key: &SigningKey) -> String {
    B64.encode(key.verifying_key().to_bytes())
}

fn sign_b64(key: &SigningKey, data: &[u8]) -> String {
    B64.encode(key.sign(data).to_bytes())
}

#[test]
fn verifies_an_artifact_signed_by_sparkle_sign_update() {
    let key = TrustedKey::from_base64(SPARKLE_PUB).unwrap();
    let sig = EdSignature::from_base64(SPARKLE_SIG).unwrap();
    key.verify_artifact(&sig, SPARKLE_ARTIFACT.len() as u64, SPARKLE_ARTIFACT)
        .unwrap();
}

#[test]
fn public_key_round_trips_in_sparkle_format() {
    let key = TrustedKey::from_base64(SPARKLE_PUB).unwrap();
    assert_eq!(key.to_base64(), SPARKLE_PUB.trim());
}

#[test]
fn valid_signature_over_tampered_bytes_is_rejected() {
    let key = TrustedKey::from_base64(SPARKLE_PUB).unwrap();
    let sig = EdSignature::from_base64(SPARKLE_SIG).unwrap();
    let mut tampered = SPARKLE_ARTIFACT.to_vec();
    tampered[50_000] ^= 0x01;
    let err = key
        .verify_artifact(&sig, tampered.len() as u64, tampered.as_slice())
        .unwrap_err();
    assert!(matches!(err, VerifyError::BadSignature), "{err:?}");
}

#[test]
fn signature_from_a_different_key_is_rejected() {
    let trusted = TrustedKey::from_base64(&public_b64(&disposable_key(1))).unwrap();
    let data = b"artifact bytes";
    let sig = EdSignature::from_base64(&sign_b64(&disposable_key(2), data)).unwrap();
    let err = trusted
        .verify_artifact(&sig, data.len() as u64, &data[..])
        .unwrap_err();
    assert!(matches!(err, VerifyError::BadSignature), "{err:?}");
}

#[test]
fn wrong_declared_length_is_rejected_even_with_a_valid_signature() {
    let signer = disposable_key(3);
    let key = TrustedKey::from_base64(&public_b64(&signer)).unwrap();
    let data = b"exactly twenty bytes";
    let sig = EdSignature::from_base64(&sign_b64(&signer, data)).unwrap();

    for declared in [data.len() as u64 - 1, data.len() as u64 + 1, 0] {
        let err = key.verify_artifact(&sig, declared, &data[..]).unwrap_err();
        assert!(
            matches!(err, VerifyError::LengthMismatch { expected, .. } if expected == declared),
            "declared {declared}: {err:?}"
        );
    }
}

#[test]
fn verification_stops_reading_once_the_declared_length_is_exceeded() {
    let signer = disposable_key(4);
    let key = TrustedKey::from_base64(&public_b64(&signer)).unwrap();
    let sig = EdSignature::from_base64(&sign_b64(&signer, b"small")).unwrap();
    // An endless stream must not be consumed without bound.
    let endless = std::io::repeat(0xAB);
    let err = key.verify_artifact(&sig, 5, endless).unwrap_err();
    assert!(
        matches!(err, VerifyError::LengthMismatch { expected: 5, .. }),
        "{err:?}"
    );
}

#[test]
fn malformed_public_keys_are_rejected() {
    assert!(matches!(
        TrustedKey::from_base64("not base64!"),
        Err(KeyError::Base64)
    ));
    assert!(matches!(
        TrustedKey::from_base64(&B64.encode([7u8; 31])),
        Err(KeyError::Length(31))
    ));
    // Sparkle private keys (32-byte seeds are indistinguishable, but legacy
    // 96-byte keys are not) must not be accepted as public keys.
    assert!(matches!(
        TrustedKey::from_base64(&B64.encode([7u8; 96])),
        Err(KeyError::Length(96))
    ));
}

#[test]
fn weak_public_keys_are_rejected() {
    // The identity point (y = 1) has small order and would accept forged
    // signatures under cofactorless verification.
    let mut identity = [0u8; 32];
    identity[0] = 1;
    assert!(matches!(
        TrustedKey::from_base64(&B64.encode(identity)),
        Err(KeyError::Weak)
    ));
}

#[test]
fn malformed_signatures_are_rejected() {
    for bad in ["", "%%%", &B64.encode([0u8; 63]), &B64.encode([0u8; 65])] {
        assert!(
            EdSignature::from_base64(bad).is_err(),
            "{bad:?} must not parse as a signature"
        );
    }
}

/// Public key of `INSECURE_TEST_KEY_SEED`, derived independently with
/// `openssl pkey -pubout`.
const INSECURE_TEST_PUB: &str = "X4kvjOoTZUsPKjjF0W/qzutLOb3d9LPmYZ4szZGg8wI=";

#[test]
fn insecure_test_key_is_recognizable_from_its_public_key() {
    let key = TrustedKey::from_base64(INSECURE_TEST_PUB).unwrap();
    assert!(key.is_insecure_test_key());
    assert_eq!(TrustedKey::insecure_test_key(), key);
    assert!(
        !TrustedKey::from_base64(SPARKLE_PUB)
            .unwrap()
            .is_insecure_test_key()
    );
}
