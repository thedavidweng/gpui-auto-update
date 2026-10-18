# First public release

A green source or packaged-consumer build does not establish public
distribution. Keep release PR #32 open until the account gates below are
resolved and the release has independent review.

## Account-holder inputs

The Apple credentials already used by the Sukiru signing lane are the intended
source. Its successful run `39062736683` used Developer ID signing and API-key
notarization. The identity is `Developer ID Application: MOE AI LLC (NCFNX3LJ83)`;
the certificate expires on 2027-02-01. No new certificate or Apple-ID password
is indicated. Recheck validity at the time of release.

An authorized account holder must transfer the matched certificate/password
pair and team API key to this repository's Actions secret store. Use the
certificate secret named in `macos-e2e.yml`'s import step, plus:

| Secret | Required material |
| --- | --- |
| `MACOS_CERTIFICATE_PASSWORD` | Password matching that P12 |
| `MACOS_SIGNING_IDENTITY` | Exact Developer ID Application identity |
| `NOTARY_KEY_P8_BASE64` | Base64 of the existing team API private key |
| `NOTARY_KEY_ID` | That key's ID |
| `NOTARY_ISSUER_ID` | That team's issuer UUID |
| `CARGO_REGISTRY_TOKEN` | crates.io token authorized for first publication and updates of all seven crate names |

Supply secret values through `gh secret set NAME --repo
thedavidweng/gpui-auto-update` standard input or the repository secret UI.
Never put values in issue bodies, logs, shell arguments or tracked files.
Existing signing materials must not be copied from another production
repository without authorization.

The registry token is separate from Apple signing and `GITHUB_TOKEN`. The
account holder must create it; no token has been established by this audit.
It must permit creating the previously unpublished names, not just updating
an existing crate:

```text
gpui-auto-update
gpui-auto-update-core
gpui-auto-update-macos
gpui-auto-update-windows
gpui-auto-update-linux
gpui-auto-update-ui
gpui-auto-update-cli
```

The release candidate currently proposes version `0.0.0`. Review that version
before the first irreversible registry publication; do not silently substitute
a different version.

## Run after the workflow change is merged

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
checks. The archive is rebuilt after stapling. Missing or partial credentials
fail a required run; a skipped signing job is never acceptance.

The artifact is a signed reference application for validation, with a
placeholder appcast URL and disposable feed key. It is not a working public
update channel. Do not publish that placeholder channel as a production feed.

## Registry and public acceptance

Once credentials, the release candidate's normal checks, independent review
and notarization evidence are complete, merge #32 normally. The existing
release-plz workflow publishes only released changes and first verifies
packaged consumers on macOS, Windows and Linux.

Verify all seven exact versions on crates.io and their docs.rs builds; inspect
the GitHub release and tags. A clean consumer outside this workspace must
resolve the public registry dependencies without local path patches, git
dependencies, or a local temporary registry. Re-run the existing consumer
example using the published version on each supported platform. Preserve
links to the publishing run, registry versions, docs.rs and consumer results
in #32 before claiming public distribution.

Audit on 2026-10-18: repository secrets and GitHub releases list empty; all
seven crates.io API endpoints return 404. The local default Cargo credential
files and `CARGO_REGISTRY_TOKEN` environment variable are absent. This is not
an exhaustive assertion about every external secret store.
