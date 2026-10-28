# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/thedavidweng/gpui-auto-update/releases/tag/gpui-auto-update-core-v0.1.0) - 2026-10-28

### Fixed

- *(deps)* require patched rustls 0.23.46 ([#34](https://github.com/thedavidweng/gpui-auto-update/pull/34))

### Other

- Complete the README and documentation (T26, #27)
- Select release channels on Windows and Linux through the facade (T23, #24)
- Merge remote-tracking branch 'origin/spec-1-integration' into ticket/T10-macos-background
- Adopt Sparkle's scheduled discoveries and coordinate relaunch (T10, #11)
- Accept bodies of exactly the size limit in HttpClient
- Fix CI on Windows and on macOS runners without SIP
- Merge T25 ([#26](https://github.com/thedavidweng/gpui-auto-update/pull/26)): Project release engineering
- Merge remote-tracking branch 'origin/spec-1-integration' into ticket/T12-key-management
- Merge remote-tracking branch 'origin/spec-1-integration' into ticket/T25-release-engineering
- Add CLI key management (T12, #13)
- Merge remote-tracking branch 'origin/spec-1-integration' into ticket/T4-artifact-download
- Add verified artifact download, staging, and feed check source (T4, #5)
- Merge origin/spec-1-integration into ticket/T5-preferences-policy
- Add automatic-update preference persistence and check policy (T5, #6)
- Merge origin/spec-1-integration into ticket/T3-signed-feed
- Add signed native feed and verified update check (T3, #4)
- Add core update state, errors, capability, and check coordinator (T2, #3)
- Add release-plz release automation and fresh-consumer packaging check (T25, #26)
- Bootstrap workspace, CI, and repository scaffolding (T1, #2)
