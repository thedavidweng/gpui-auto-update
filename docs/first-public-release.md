# First public release

A green source or packaged-consumer build does not establish public
distribution. Keep the release PR open until the account gates below are
resolved and the release has independent review.

## Account-holder inputs

Repository secrets and GitHub releases are empty, and all seven crates.io API
endpoints return 404. The local default Cargo credential files and
`CARGO_REGISTRY_TOKEN` environment variable are absent. This is not an
exhaustive assertion about every external secret store.

An authorized account holder must transfer the matched Developer ID
certificate/password pair and the App Store Connect team API key to this
repository's Actions secret store. Supply values through `gh secret set NAME
--repo thedavidweng/gpui-auto-update` standard input or the repository secret
UI. Never put values in issue bodies, logs, shell arguments or tracked files.
Existing signing materials must not be copied from another production
repository without authorization.

| Secret | Required material |
| --- | --- |
| `MACOS_CERTIFICATE_P12` | Base64 of the "Developer ID Application" .p12 |
| `MACOS_CERTIFICATE_PASSWORD` | Password matching that P12 |
| `MACOS_SIGNING_IDENTITY` | Exact Developer ID Application identity |
| `NOTARY_KEY_P8_BASE64` | Base64 of the team API private key |
| `NOTARY_KEY_ID` | That key's ID |
| `NOTARY_ISSUER_ID` | That team's issuer UUID |
| `CARGO_REGISTRY_TOKEN` | crates.io token authorized for first publication and updates of all seven crate names |

The registry token is separate from Apple signing and `GITHUB_TOKEN`. The
account holder must create it; no token has been established by this audit. It
must permit creating the previously unpublished names, not just updating an
existing crate:

```text
gpui-auto-update
gpui-auto-update-core
gpui-auto-update-macos
gpui-auto-update-windows
gpui-auto-update-linux
gpui-auto-update-ui
gpui-auto-update-cli
```

crates.io trusted publishing cannot replace that token for the first publish:
the registry requires an API token while the crate does not yet exist. Once
`0.1.0` is published, add a trusted publisher for each crate (owner
`thedavidweng`, repository `gpui-auto-update`, workflow `release-plz.yml`, no
environment) and switch the job to `permissions: id-token: write` so later
releases need no stored token.

Review the release candidate's version before the first irreversible registry
publication; do not silently substitute a different version.

## Notarization evidence

`macos-e2e.yml` signs with the Developer ID identity and notarizes with the
team API key. A `workflow_dispatch` run with `require_notarization` defaults to
requiring a complete credential set; a missing or partial set fails the run
instead of silently skipping. A skipped signing job is never acceptance.

Run after the credentials exist:

```sh
gh workflow run macos-e2e.yml --repo thedavidweng/gpui-auto-update \
  --ref main -f require_notarization=true
gh run list --repo thedavidweng/gpui-auto-update \
  --workflow macos-e2e.yml --event workflow_dispatch --limit 1
```

Use the returned run ID to watch and download:

```sh
gh run watch RUN_ID --repo thedavidweng/gpui-auto-update --exit-status
gh run download RUN_ID --repo thedavidweng/gpui-auto-update \
  --name developer-id-notarized-reference-app --dir /tmp/gpui-notarized
ditto -x -k /tmp/gpui-notarized/ReferenceApp.zip /tmp/gpui-notarized/unpacked
codesign --verify --deep --strict "/tmp/gpui-notarized/unpacked/Reference App.app"
codesign --display --verbose=4 "/tmp/gpui-notarized/unpacked/Reference App.app"
xcrun stapler validate "/tmp/gpui-notarized/unpacked/Reference App.app"
spctl --assess --type execute --verbose "/tmp/gpui-notarized/unpacked/Reference App.app"
```

Confirm the expected Developer ID authority/team, secure timestamp,
`notarization.json` status `Accepted`, and successful stapler and Gatekeeper
checks. The archive is rebuilt after stapling, because the submitted archive
predates the ticket.

The artifact is a signed reference application for validation, with a
placeholder appcast URL and disposable feed key. It is not a working public
update channel. Do not publish that placeholder channel as a production feed.

## Registry and public acceptance

Once credentials, the release candidate's normal checks, independent review
and notarization evidence are complete, merge the release PR normally. The
existing release-plz workflow publishes only released changes and first
verifies packaged consumers on macOS, Windows and Linux.

Verify all seven exact versions on crates.io and their docs.rs builds; inspect
the GitHub release and tags. A clean consumer outside this workspace must
resolve the public registry dependencies without local path patches, git
dependencies, or a local temporary registry. Re-run the existing consumer
example using the published version on each supported platform. Preserve links
to the publishing run, registry versions, docs.rs and consumer results in the
release PR before claiming public distribution.
