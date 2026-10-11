# Native feed format (Windows and Linux)

On macOS, Sparkle reads an official Sparkle appcast produced by Sparkle's own
tools. On Windows and Linux, `gpui-auto-update` reads a **native feed**: an
RSS 2.0 document that uses the same field conventions as a Sparkle appcast
and the same Ed25519 key and signature encodings. One key pair can therefore
sign releases for all three platforms.

The parser and selection rules live in `gpui-auto-update-core`
(`feed`, `trust`, `fetch`, `check`, `download`, and `version` modules). This
document is the contract they implement.

## Example

```xml
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0"
     xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"
     xmlns:gpui-auto-update="https://github.com/thedavidweng/gpui-auto-update/xml-namespaces/feed">
  <channel>
    <title>Example App</title>
    <item>
      <title>Version 1.5.0</title>
      <pubDate>Mon, 05 Oct 2026 12:00:00 +0000</pubDate>
      <sparkle:version>1.5.0</sparkle:version>
      <sparkle:shortVersionString>1.5</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>10.0.19045</sparkle:minimumSystemVersion>
      <sparkle:releaseNotesLink>https://example.com/notes/1.5.0.html</sparkle:releaseNotesLink>
      <enclosure
          url="https://downloads.example.com/example-1.5.0-windows-x86_64.exe"
          length="48213504"
          type="application/octet-stream"
          sparkle:os="windows"
          gpui-auto-update:arch="x86_64"
          sparkle:edSignature="WVyVJpOx+a5+vNWJVY79TRjFKveNk+VhGJf2iti4CZtJsJewIUGvh/1AKKEAFbH1qUwx+vro1ECuzOsMmumoBA=="/>
    </item>
  </channel>
</rss>
```

## Feeds per platform

Publish one feed per operating system and architecture, for example
`appcast-windows-x86_64.xml`, `appcast-windows-aarch64.xml`,
`appcast-linux-x86_64.xml`, and `appcast-linux-aarch64.xml`. The application
is configured with the one feed URL that matches its build; architecture is
never guessed from file names.

Each entry still declares its `sparkle:os` and `gpui-auto-update:arch`, and
the client only selects entries that match its own OS and architecture. A
feed that has entries but none for the client's platform is rejected as a
configuration error rather than reported as "up to date".

## Document structure

- The root element must be `<rss>` with a `<channel>` child. Each release is
  one `<item>` directly inside `<channel>`. Other channel elements (`title`,
  `link`, `description`, `language`) are ignored.
- The document must be UTF-8 XML. Document type declarations (`<!DOCTYPE>`)
  are rejected, so entity expansion cannot be used against the parser.
- Namespaces are matched by URI, not by prefix. Sparkle's namespace is
  `http://www.andymatuschak.org/xml-namespaces/sparkle`; this project's
  namespace, used only for the architecture attribute that Sparkle lacks, is
  `https://github.com/thedavidweng/gpui-auto-update/xml-namespaces/feed`.

## Item fields

Each field may appear at most once per item. A repeated field rejects the
feed.

| Field | Required | Meaning |
| --- | --- | --- |
| `sparkle:version` | yes | The authoritative release version. Must be a strict [SemVer 2.0.0](https://semver.org) string, at most 64 bytes, with no `v` prefix or surrounding whitespace (for example `1.5.0` or `2.0.0-beta.1`). It is the only value compared to the running version. |
| `sparkle:shortVersionString` | no | A display version. It is never compared and never used in paths. |
| `title` | no | Display title. |
| `pubDate` | no | Publication date, kept verbatim (RFC 822 format, as in RSS). |
| `sparkle:channel` | no | Release channel, for example `beta`. Without it, the item is on the default channel. Names are 1 to 64 ASCII letters, digits, `.`, `_`, or `-`, and do not start with `.`. |
| `sparkle:minimumSystemVersion` | no | Lowest OS version the release supports, as one to four dot-separated numbers (for example `10.0.19045` for Windows, or a kernel version such as `5.15` for Linux). Missing trailing parts count as zero. |
| `sparkle:criticalUpdate` | no | Marks the release as critical. With a `sparkle:version="X"` attribute (strict SemVer), it is critical only for installations older than `X`. |
| `sparkle:releaseNotesLink` | no | Absolute `https` or `http` URL of the release notes. |
| `sparkle:fullReleaseNotesLink` | no | Absolute `https` or `http` URL of the full change log. |
| `description` | no | Inline release notes, usually HTML in a CDATA section, kept verbatim. Applications must sanitize it before rendering. |
| `enclosure` | yes | The artifact. Exactly one per item; see below. |

Short text fields (`title`, `pubDate`, `sparkle:shortVersionString`, and the
enclosure `type`) are limited to 1024 bytes and may not contain control
characters.

Sparkle delta updates (`sparkle:deltas`) are not used by native feeds and are
ignored.

## Enclosure attributes

| Attribute | Required | Meaning |
| --- | --- | --- |
| `url` | yes | Absolute `https` or `http` URL of the artifact. Whether plain `http` may actually be downloaded is decided by the transport policy, which allows only `https` by default. |
| `length` | yes | Exact artifact size in bytes, as a positive decimal integer with no sign or spaces. The downloaded file must match it exactly, and it may not exceed the client's artifact size limit (512 MiB by default). |
| `sparkle:edSignature` | yes | Ed25519 signature of the artifact, encoded exactly like Sparkle's (see below). |
| `sparkle:os` | yes | `windows`, `linux`, or `macos`, matched exactly and case-sensitively. |
| `gpui-auto-update:arch` | yes | `x86_64` or `aarch64`, matched exactly. Aliases such as `amd64` and `arm64` are rejected. |
| `type` | no | MIME type, informational only (for example `application/gzip` for Linux tarballs or `application/octet-stream` for Windows installers). |

## Keys and signatures

The format is the same as Sparkle 2's EdDSA support, and the implementation
is tested against output of Sparkle's `sign_update`:

- **Public key**: the standard base64 encoding of the 32-byte Ed25519 public
  key. It is the same string Sparkle uses for `SUPublicEDKey`, and the same
  string is shipped in Windows and Linux builds. Weak (small-order) keys are
  rejected when loaded.
- **Signature** (`sparkle:edSignature`): the standard base64 encoding of the
  64-byte pure Ed25519 signature (RFC 8032, not Ed25519ph) over the raw
  artifact bytes exactly as downloaded. Nothing else is signed: not the URL,
  the version, nor the length.
- **Private key**: never part of the feed or the application. Sparkle's
  exported private key (base64 of a 32-byte seed) can sign native artifacts
  too.

## Validation is fail-closed

A feed is validated completely before any release is selected, and a single
invalid item rejects the whole feed. In particular:

- an item without `sparkle:edSignature`, or with a value that is not base64
  of exactly 64 bytes, rejects the feed;
- an item whose `sparkle:version` is not strict SemVer rejects the feed;
- a missing, zero, malformed, or over-limit `length` rejects the feed;
- an unknown `sparkle:os` or architecture, or a missing one, rejects the
  feed;
- two items for the same OS and architecture whose versions are equal in
  SemVer precedence (for example `1.0.0` and `1.0.0+rebuild`) reject the
  feed.

There is no best-effort mode that skips bad entries or accepts unsigned
releases. Rejecting the whole feed means a publishing mistake is reported
instead of silently hiding a release or offering an older one.

A successful check only proves that the selected entry is well formed. The
artifact itself is trusted only after its downloaded bytes match `length` and
verify against the application's public key.

## Selecting a release

Given the running version, OS, architecture, opted-in channels, and
(optionally) the running OS version, the client:

1. keeps items whose `sparkle:os` and `gpui-auto-update:arch` match;
2. keeps items on the default channel or on a channel the user opted into;
3. if the OS version is known, drops items whose
   `sparkle:minimumSystemVersion` is higher (when it is unknown, this filter
   is skipped);
4. picks the item with the highest SemVer precedence, regardless of its
   position in the document;
5. reports that item as available if it is newer than the running version,
   and otherwise reports up to date. An equal version, a version that differs
   only in build metadata, or a newer running version are all up to date.

SemVer precedence ignores build metadata and orders pre-releases before their
release (`2.0.0-beta.1` < `2.0.0`).

### Choosing a channel

Applications query the channel with `Updater::channel` and change it with
`Updater::set_channel` on every platform. `None` means the default channel
only, and `Some(beta)` means the default channel plus `beta`, so a beta user
still receives a stable release that is newer than every beta. The next check
uses the new channel.

| Platform | Where the channel lives |
| --- | --- |
| macOS | Sparkle's allowed channels (`allowedChannelsForUpdater:`) |
| Windows | The backend's feed check (`WindowsUpdateConfig::with_channel` sets the initial channel) |
| Linux | The native feed check (`NativeFeed::with_channel` sets the initial channel) |

No platform persists the choice: Sparkle does not store allowed channels, and
the Windows and Linux backends follow the same rule. Set the channel again on
every launch, for example from the application's own settings. A name that is
not a valid `sparkle:channel` name is rejected with a configuration error.

## Versions and file system paths

Only `sparkle:version` may ever name a file or directory, and only after it
has passed strict SemVer validation. That grammar allows only ASCII letters,
digits, `-`, `.`, and `+`, and forbids empty identifiers, so a valid version
is always a single normal path component. It can never be `.`, `..`, an
absolute path, a drive prefix, or contain a separator. Values such as
`../../evil` or `1.0.0/..` are rejected while parsing the feed. URLs, titles,
display versions, and channel names are never used to build paths.

## Transport

Feeds can be served from any static HTTPS origin or CDN. The client applies
these bounds by default:

| Bound | Default |
| --- | --- |
| Feed size | 1 MiB (checked against `Content-Length` and while reading) |
| Items per feed | 1000 |
| Artifact size | 512 MiB |
| Connect timeout | 10 s |
| Per-request timeout | 30 s |
| Redirects | 5, each hop checked |
| Schemes | `https` only |

TLS certificates are verified against the operating system's trust store.
Redirects are followed one hop at a time. A redirect without a usable
`Location`, to a scheme other than `https` (or `http` when explicitly
allowed for local testing), or from `https` to `http` is refused. Responses
are not decompressed, so size limits apply to the bytes on the wire.

Serve mutable feed files with short cache lifetimes, give every versioned
artifact an immutable URL, and upload artifacts before publishing the feed
that refers to them.

## Known limitation: feed metadata is not signed

As in a Sparkle appcast without a signed feed, the signatures cover artifacts,
not the feed. An attacker who can modify the feed (but does not have the
private key) cannot make the client accept an unsigned or altered artifact.
However, they can withhold updates, or relabel an older signed artifact with a
higher version number. Backends must therefore check that the version inside
a verified artifact matches the selected `sparkle:version` before installing;
`download::StagedArtifact::expected_version` provides that version.
Sparkle-style signed feeds are a candidate future addition.
