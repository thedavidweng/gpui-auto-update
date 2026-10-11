# Sparkle `sign_update` interoperability fixture

Disposable, test-only material. Never use this key for releases.

- `pub.b64`: base64 Ed25519 public key in Sparkle's `SUPublicEDKey` format.
- `artifact.bin`: 100,000 random bytes.
- `sig.b64`: `sparkle:edSignature` for `artifact.bin`, produced by Sparkle
  2.10.0 `sign_update --ed-key-file` with the matching private key.
