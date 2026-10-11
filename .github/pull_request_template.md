## Summary

<!-- What does this change do, and why? Link the issue it resolves. -->

Closes #

## Platform testing

<!-- Updates run privileged, irreversible code paths. State what you actually ran. -->

- [ ] macOS (version / arch):
- [ ] Windows (version / arch, installer or portable):
- [ ] Linux (distro / arch / desktop session):
- [ ] Not platform-specific: covered by tests that run on all CI platforms

## Security checklist

- [ ] No path that could silently skip signature or length verification
- [ ] Feed metadata cannot influence filesystem paths without validation
- [ ] No private keys, tokens, or sensitive paths in logs or user-visible messages
- [ ] New `unsafe` code is confined to a platform backend module and documented with `// SAFETY:` comments
- [ ] New dependencies are maintained, permissively licensed, and pass `cargo deny check`

## Checks

- [ ] `cargo fmt --all`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Public API changes are documented and noted in `CHANGELOG.md`
