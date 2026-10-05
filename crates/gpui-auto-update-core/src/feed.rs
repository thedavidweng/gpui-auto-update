//! The native (Windows and Linux) update feed: parsing, validation, and
//! release selection.
//!
//! A native feed is a Sparkle-compatible RSS 2.0 appcast. The format and every
//! validation rule are documented in `docs/feed-format.md`. In short:
//!
//! - every `<item>` must carry a strict-semver `sparkle:version` and exactly
//!   one `<enclosure>` with `url`, `length`, `sparkle:edSignature`,
//!   `sparkle:os`, and `gpui-auto-update:arch`;
//! - validation is all-or-nothing: one invalid item rejects the whole feed,
//!   so an unsigned or malformed release can never be silently skipped or
//!   selected;
//! - selection picks the highest version applicable to the target, so item
//!   order in the document does not matter.

use std::collections::HashSet;
use std::fmt;

use url::Url;

use crate::trust::{EdSignature, SignatureError};
use crate::version::{ReleaseVersion, VersionError};

/// Sparkle's XML namespace.
pub const SPARKLE_NS: &str = "http://www.andymatuschak.org/xml-namespaces/sparkle";

/// This project's XML namespace, used for the `arch` enclosure attribute that
/// Sparkle does not define. The conventional prefix is `gpui-auto-update`.
pub const NATIVE_NS: &str = "https://github.com/thedavidweng/gpui-auto-update/xml-namespaces/feed";

const MAX_TEXT_LEN: usize = 1024;
const MAX_CHANNEL_LEN: usize = 64;
const MAX_SYSTEM_VERSION_LEN: usize = 32;

/// Bounds applied while parsing a feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedLimits {
    /// Largest accepted feed document, in bytes.
    pub max_feed_bytes: u64,
    /// Most `<item>` elements accepted in one feed.
    pub max_items: usize,
    /// Largest artifact `length` a feed may declare, in bytes.
    pub max_artifact_bytes: u64,
}

impl Default for FeedLimits {
    /// 1 MiB feeds, 1000 items, 512 MiB artifacts.
    fn default() -> Self {
        Self {
            max_feed_bytes: 1024 * 1024,
            max_items: 1000,
            max_artifact_bytes: 512 * 1024 * 1024,
        }
    }
}

/// Operating system an artifact is built for (`sparkle:os`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Os {
    /// `windows`
    Windows,
    /// `linux`
    Linux,
    /// `macos` (native feeds normally do not contain these; Sparkle reads
    /// macOS appcasts itself).
    Macos,
}

impl Os {
    /// The `sparkle:os` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Os::Windows => "windows",
            Os::Linux => "linux",
            Os::Macos => "macos",
        }
    }

    /// Parses a `sparkle:os` value. Matching is exact and case-sensitive.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "windows" => Some(Os::Windows),
            "linux" => Some(Os::Linux),
            "macos" => Some(Os::Macos),
            _ => None,
        }
    }

    /// The operating system this binary was compiled for, if supported.
    pub fn current() -> Option<Self> {
        if cfg!(target_os = "windows") {
            Some(Os::Windows)
        } else if cfg!(target_os = "linux") {
            Some(Os::Linux)
        } else if cfg!(target_os = "macos") {
            Some(Os::Macos)
        } else {
            None
        }
    }
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// CPU architecture an artifact is built for (`gpui-auto-update:arch`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arch {
    /// `x86_64`
    X86_64,
    /// `aarch64`
    Aarch64,
}

impl Arch {
    /// The `gpui-auto-update:arch` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }

    /// Parses an architecture value. Matching is exact and case-sensitive;
    /// aliases such as `amd64` or `arm64` are not accepted.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "x86_64" => Some(Arch::X86_64),
            "aarch64" => Some(Arch::Aarch64),
            _ => None,
        }
    }

    /// The architecture this binary was compiled for, if supported.
    pub fn current() -> Option<Self> {
        if cfg!(target_arch = "x86_64") {
            Some(Arch::X86_64)
        } else if cfg!(target_arch = "aarch64") {
            Some(Arch::Aarch64)
        } else {
            None
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A release channel name (`sparkle:channel`): 1 to 64 ASCII letters, digits,
/// `.`, `_`, or `-`, not starting with `.`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Channel(String);

impl Channel {
    /// Validates a channel name.
    pub fn new(name: &str) -> Option<Self> {
        let valid = !name.is_empty()
            && name.len() <= MAX_CHANNEL_LEN
            && !name.starts_with('.')
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        valid.then(|| Self(name.to_owned()))
    }

    /// The channel name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A dotted numeric operating-system version such as `10.0.19045` or `6.8`,
/// used for `sparkle:minimumSystemVersion`. Missing trailing components
/// compare as zero.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SystemVersion(Vec<u32>);

impl SystemVersion {
    /// Parses one to four dot-separated decimal components.
    pub fn parse(text: &str) -> Option<Self> {
        if text.is_empty() || text.len() > MAX_SYSTEM_VERSION_LEN {
            return None;
        }
        let parts = text
            .split('.')
            .map(|p| {
                if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                    None
                } else {
                    p.parse::<u32>().ok()
                }
            })
            .collect::<Option<Vec<_>>>()?;
        (parts.len() <= 4).then_some(Self(parts))
    }

    fn component(&self, i: usize) -> u32 {
        self.0.get(i).copied().unwrap_or(0)
    }
}

impl PartialOrd for SystemVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SystemVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let n = self.0.len().max(other.0.len());
        (0..n)
            .map(|i| self.component(i).cmp(&other.component(i)))
            .find(|o| o.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

impl fmt::Display for SystemVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, part) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            write!(f, "{part}")?;
        }
        Ok(())
    }
}

/// The downloadable file of a feed item (`<enclosure>`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Artifact {
    /// Absolute `https` or `http` URL. Whether plain `http` may actually be
    /// fetched is decided by the transport policy.
    pub url: Url,
    /// Exact size in bytes; the downloaded file must match it.
    pub length: u64,
    /// Ed25519 signature over the artifact bytes.
    pub signature: EdSignature,
    /// Target operating system.
    pub os: Os,
    /// Target architecture.
    pub arch: Arch,
    /// The enclosure's MIME `type`, informational only.
    pub content_type: Option<String>,
}

/// One validated release entry (`<item>`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FeedItem {
    /// `sparkle:version`: the authoritative, comparable release version.
    pub version: ReleaseVersion,
    /// `sparkle:shortVersionString`: optional display version. Never used for
    /// comparison or paths.
    pub display_version: Option<String>,
    /// `<title>`.
    pub title: Option<String>,
    /// `<pubDate>`, verbatim.
    pub published: Option<String>,
    /// `sparkle:channel`; `None` is the default channel.
    pub channel: Option<Channel>,
    /// `sparkle:minimumSystemVersion`.
    pub minimum_system_version: Option<SystemVersion>,
    /// `sparkle:criticalUpdate`. `Some(None)` marks the release critical for
    /// everyone; `Some(Some(v))` only for installations older than `v`.
    pub critical: Option<Option<ReleaseVersion>>,
    /// `sparkle:releaseNotesLink`.
    pub release_notes_url: Option<Url>,
    /// `sparkle:fullReleaseNotesLink`.
    pub full_release_notes_url: Option<Url>,
    /// `<description>`: inline release notes (often HTML), verbatim.
    pub description: Option<String>,
    /// The artifact to download.
    pub artifact: Artifact,
}

/// A parsed and fully validated feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feed {
    items: Vec<FeedItem>,
}

/// The installation a feed is evaluated for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateTarget {
    /// Operating system to match against `sparkle:os`.
    pub os: Os,
    /// Architecture to match against `gpui-auto-update:arch`.
    pub arch: Arch,
    /// Channels opted into in addition to the default channel.
    pub channels: Vec<Channel>,
    /// The running OS version, if known. When `None`, minimum-system-version
    /// requirements are not enforced.
    pub system_version: Option<SystemVersion>,
}

impl UpdateTarget {
    /// A target on the default channel with an unknown system version.
    pub fn new(os: Os, arch: Arch) -> Self {
        Self {
            os,
            arch,
            channels: Vec::new(),
            system_version: None,
        }
    }

    /// Also accept releases on `channel`.
    pub fn with_channel(mut self, channel: Channel) -> Self {
        self.channels.push(channel);
        self
    }

    /// Enforce minimum-system-version requirements against `version`.
    pub fn with_system_version(mut self, version: SystemVersion) -> Self {
        self.system_version = Some(version);
        self
    }

    fn accepts(&self, item: &FeedItem) -> bool {
        let channel_ok = match &item.channel {
            None => true,
            Some(c) => self.channels.contains(c),
        };
        let system_ok = match (&self.system_version, &item.minimum_system_version) {
            (Some(running), Some(min)) => running >= min,
            _ => true,
        };
        self.matches_platform(item) && channel_ok && system_ok
    }

    fn matches_platform(&self, item: &FeedItem) -> bool {
        item.artifact.os == self.os && item.artifact.arch == self.arch
    }
}

/// A newer release chosen from a feed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SelectedUpdate {
    /// The chosen feed item.
    pub item: FeedItem,
    /// Whether the release is critical for the current version.
    pub is_critical: bool,
}

/// The result of evaluating a feed for one installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// No applicable release is newer than the current version.
    UpToDate,
    /// A newer applicable release exists.
    UpdateAvailable(Box<SelectedUpdate>),
}

/// Why a feed was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FeedError {
    /// The document exceeds [`FeedLimits::max_feed_bytes`].
    #[error("feed is larger than the {limit}-byte limit")]
    TooLarge {
        /// The limit that was exceeded.
        limit: u64,
    },
    /// The document is not well-formed UTF-8 XML, or contains a DTD.
    #[error("feed is not valid XML: {0}")]
    Xml(String),
    /// The root element is not `<rss>` with a `<channel>`.
    #[error("feed is not an RSS appcast")]
    NotRss,
    /// The feed has more items than [`FeedLimits::max_items`].
    #[error("feed has {count} items; at most {limit} are allowed")]
    TooManyItems {
        /// Items found (counting stops just past the limit).
        count: usize,
        /// The limit.
        limit: usize,
    },
    /// An item is invalid; the whole feed is rejected.
    #[error("feed item {index} is invalid: {reason}")]
    InvalidItem {
        /// Zero-based position of the item in the document.
        index: usize,
        /// What is wrong with it.
        reason: ItemError,
    },
    /// Two items for the same platform have equal version precedence.
    #[error("feed lists version {version} for {os}/{arch} more than once")]
    DuplicateVersion {
        /// The repeated version.
        version: String,
        /// Platform of the duplicates.
        os: Os,
        /// Architecture of the duplicates.
        arch: Arch,
    },
    /// The feed has items, but none for the target's OS and architecture,
    /// which usually means the wrong feed is configured.
    #[error("feed has no entries for {os}/{arch}")]
    NoEntriesForTarget {
        /// Target OS.
        os: Os,
        /// Target architecture.
        arch: Arch,
    },
}

/// What is wrong with a feed item.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ItemError {
    /// A required element or attribute is absent.
    #[error("missing {0}")]
    Missing(&'static str),
    /// An element that may appear once appears more than once.
    #[error("{0} appears more than once")]
    Duplicate(&'static str),
    /// `sparkle:version` (or a critical-update threshold) is not a valid
    /// release version.
    #[error(transparent)]
    Version(#[from] VersionError),
    /// `sparkle:edSignature` is malformed.
    #[error("invalid sparkle:edSignature: {0}")]
    Signature(#[from] SignatureError),
    /// A URL is not an absolute `https`/`http` URL.
    #[error("{field} is not an absolute https or http URL")]
    InvalidUrl {
        /// The offending field.
        field: &'static str,
    },
    /// `length` is not a positive decimal integer.
    #[error("enclosure length {0:?} is not a positive integer")]
    InvalidLength(String),
    /// `length` exceeds [`FeedLimits::max_artifact_bytes`].
    #[error("artifact length {length} exceeds the {limit}-byte limit")]
    ArtifactTooLarge {
        /// Declared length.
        length: u64,
        /// The limit.
        limit: u64,
    },
    /// `sparkle:os` is not a known value.
    #[error("unknown sparkle:os {0:?}")]
    UnknownOs(String),
    /// `gpui-auto-update:arch` is not a known value.
    #[error("unknown architecture {0:?}")]
    UnknownArch(String),
    /// `sparkle:channel` is not a valid channel name.
    #[error("invalid channel name {0:?}")]
    InvalidChannel(String),
    /// `sparkle:minimumSystemVersion` is not a dotted numeric version.
    #[error("invalid minimum system version {0:?}")]
    InvalidSystemVersion(String),
    /// A text field is too long or contains control characters.
    #[error("{0} is too long or contains control characters")]
    InvalidText(&'static str),
}

impl Feed {
    /// Parses and validates a feed document.
    pub fn parse(bytes: &[u8], limits: &FeedLimits) -> Result<Self, FeedError> {
        if bytes.len() as u64 > limits.max_feed_bytes {
            return Err(FeedError::TooLarge {
                limit: limits.max_feed_bytes,
            });
        }
        let text = std::str::from_utf8(bytes)
            .map_err(|_| FeedError::Xml("document is not UTF-8".to_owned()))?;
        let options = roxmltree::ParsingOptions {
            allow_dtd: false,
            ..Default::default()
        };
        let doc = roxmltree::Document::parse_with_options(text, options)
            .map_err(|e| FeedError::Xml(e.to_string()))?;

        let root = doc.root_element();
        if !root.has_tag_name("rss") {
            return Err(FeedError::NotRss);
        }
        let channel = root
            .children()
            .find(|n| n.has_tag_name("channel"))
            .ok_or(FeedError::NotRss)?;

        let mut items = Vec::new();
        for (index, node) in channel
            .children()
            .filter(|n| n.has_tag_name("item"))
            .enumerate()
        {
            if index >= limits.max_items {
                return Err(FeedError::TooManyItems {
                    count: index + 1,
                    limit: limits.max_items,
                });
            }
            let item = parse_item(node, limits)
                .map_err(|reason| FeedError::InvalidItem { index, reason })?;
            items.push(item);
        }

        let mut seen = HashSet::new();
        for item in &items {
            // Keyed on the semver without build metadata so that entries
            // equal in precedence (e.g. `1.0.0` and `1.0.0+rebuild`) collide.
            let key = (
                item.artifact.os,
                item.artifact.arch,
                precedence_key(&item.version),
            );
            if !seen.insert(key) {
                return Err(FeedError::DuplicateVersion {
                    version: item.version.to_string(),
                    os: item.artifact.os,
                    arch: item.artifact.arch,
                });
            }
        }

        Ok(Self { items })
    }

    /// All items, in document order.
    pub fn items(&self) -> &[FeedItem] {
        &self.items
    }

    /// Chooses the newest release applicable to `target` and compares it
    /// with `current`.
    pub fn select(
        &self,
        target: &UpdateTarget,
        current: &ReleaseVersion,
    ) -> Result<Selection, FeedError> {
        if !self.items.is_empty() && !self.items.iter().any(|i| target.matches_platform(i)) {
            return Err(FeedError::NoEntriesForTarget {
                os: target.os,
                arch: target.arch,
            });
        }
        let newest = self
            .items
            .iter()
            .filter(|i| target.accepts(i))
            .max_by(|a, b| a.version.cmp_precedence(&b.version));
        Ok(match newest {
            Some(item) if item.version.is_newer_than(current) => {
                let is_critical = match &item.critical {
                    None => false,
                    Some(None) => true,
                    Some(Some(threshold)) => threshold.is_newer_than(current),
                };
                Selection::UpdateAvailable(Box::new(SelectedUpdate {
                    item: item.clone(),
                    is_critical,
                }))
            }
            _ => Selection::UpToDate,
        })
    }
}

fn precedence_key(version: &ReleaseVersion) -> String {
    let text = version.as_str();
    text.split_once('+')
        .map_or(text, |(core, _)| core)
        .to_owned()
}

type Node<'a, 'i> = roxmltree::Node<'a, 'i>;

fn parse_item(item: Node<'_, '_>, limits: &FeedLimits) -> Result<FeedItem, ItemError> {
    let version_text = sparkle_text(item, "version", "sparkle:version")?
        .ok_or(ItemError::Missing("sparkle:version"))?;
    let version = ReleaseVersion::parse(&version_text)?;

    let display_version = sparkle_text(item, "shortVersionString", "sparkle:shortVersionString")?
        .map(|t| checked_text(t, "sparkle:shortVersionString"))
        .transpose()?;
    let title = plain_text(item, "title")?
        .map(|t| checked_text(t, "title"))
        .transpose()?;
    let published = plain_text(item, "pubDate")?
        .map(|t| checked_text(t, "pubDate"))
        .transpose()?;
    let description = unique_child(item, |n| n.has_tag_name("description"), "description")?
        .map(|n| element_text(n).trim().to_owned());

    let channel = sparkle_text(item, "channel", "sparkle:channel")?
        .map(|t| Channel::new(&t).ok_or(ItemError::InvalidChannel(t)))
        .transpose()?;
    let minimum_system_version =
        sparkle_text(item, "minimumSystemVersion", "sparkle:minimumSystemVersion")?
            .map(|t| SystemVersion::parse(&t).ok_or(ItemError::InvalidSystemVersion(t)))
            .transpose()?;

    let critical = unique_child(
        item,
        |n| n.has_tag_name((SPARKLE_NS, "criticalUpdate")),
        "sparkle:criticalUpdate",
    )?
    .map(|n| {
        n.attribute((SPARKLE_NS, "version"))
            .map(ReleaseVersion::parse)
            .transpose()
    })
    .transpose()?;

    let release_notes_url = sparkle_text(item, "releaseNotesLink", "sparkle:releaseNotesLink")?
        .map(|t| web_url(&t, "sparkle:releaseNotesLink"))
        .transpose()?;
    let full_release_notes_url =
        sparkle_text(item, "fullReleaseNotesLink", "sparkle:fullReleaseNotesLink")?
            .map(|t| web_url(&t, "sparkle:fullReleaseNotesLink"))
            .transpose()?;

    let enclosure = unique_child(item, |n| n.has_tag_name("enclosure"), "enclosure")?
        .ok_or(ItemError::Missing("enclosure"))?;
    let artifact = parse_enclosure(enclosure, limits)?;

    Ok(FeedItem {
        version,
        display_version,
        title,
        published,
        channel,
        minimum_system_version,
        critical,
        release_notes_url,
        full_release_notes_url,
        description,
        artifact,
    })
}

fn parse_enclosure(enc: Node<'_, '_>, limits: &FeedLimits) -> Result<Artifact, ItemError> {
    let url = web_url(
        enc.attribute("url").ok_or(ItemError::Missing("url"))?,
        "enclosure url",
    )?;

    let length_text = enc
        .attribute("length")
        .ok_or(ItemError::Missing("length"))?;
    let length = if !length_text.is_empty() && length_text.bytes().all(|b| b.is_ascii_digit()) {
        length_text.parse::<u64>().ok()
    } else {
        None
    }
    .filter(|&n| n > 0)
    .ok_or_else(|| ItemError::InvalidLength(length_text.to_owned()))?;
    if length > limits.max_artifact_bytes {
        return Err(ItemError::ArtifactTooLarge {
            length,
            limit: limits.max_artifact_bytes,
        });
    }

    let signature = EdSignature::from_base64(
        enc.attribute((SPARKLE_NS, "edSignature"))
            .ok_or(ItemError::Missing("sparkle:edSignature"))?,
    )?;

    let os_text = enc
        .attribute((SPARKLE_NS, "os"))
        .ok_or(ItemError::Missing("sparkle:os"))?;
    let os = Os::parse(os_text).ok_or_else(|| ItemError::UnknownOs(os_text.to_owned()))?;
    let arch_text = enc
        .attribute((NATIVE_NS, "arch"))
        .ok_or(ItemError::Missing("gpui-auto-update:arch"))?;
    let arch =
        Arch::parse(arch_text).ok_or_else(|| ItemError::UnknownArch(arch_text.to_owned()))?;

    let content_type = enc
        .attribute("type")
        .map(|t| checked_text(t.to_owned(), "enclosure type"))
        .transpose()?;

    Ok(Artifact {
        url,
        length,
        signature,
        os,
        arch,
        content_type,
    })
}

fn unique_child<'a, 'i>(
    parent: Node<'a, 'i>,
    pred: impl Fn(&Node<'a, 'i>) -> bool,
    name: &'static str,
) -> Result<Option<Node<'a, 'i>>, ItemError> {
    let mut found = parent.children().filter(|n| n.is_element() && pred(n));
    let first = found.next();
    if found.next().is_some() {
        return Err(ItemError::Duplicate(name));
    }
    Ok(first)
}

fn element_text(node: Node<'_, '_>) -> String {
    node.children()
        .filter(|c| c.is_text())
        .filter_map(|c| c.text())
        .collect()
}

fn sparkle_text(
    item: Node<'_, '_>,
    local: &str,
    name: &'static str,
) -> Result<Option<String>, ItemError> {
    Ok(
        unique_child(item, |n| n.has_tag_name((SPARKLE_NS, local)), name)?
            .map(|n| element_text(n).trim().to_owned()),
    )
}

fn plain_text(item: Node<'_, '_>, name: &'static str) -> Result<Option<String>, ItemError> {
    Ok(unique_child(
        item,
        |n| n.tag_name().namespace().is_none() && n.tag_name().name() == name,
        name,
    )?
    .map(|n| element_text(n).trim().to_owned()))
}

fn checked_text(text: String, field: &'static str) -> Result<String, ItemError> {
    if text.len() > MAX_TEXT_LEN || text.chars().any(char::is_control) {
        return Err(ItemError::InvalidText(field));
    }
    Ok(text)
}

fn web_url(text: &str, field: &'static str) -> Result<Url, ItemError> {
    let url = Url::parse(text.trim()).map_err(|_| ItemError::InvalidUrl { field })?;
    if !matches!(url.scheme(), "https" | "http") || url.host().is_none() {
        return Err(ItemError::InvalidUrl { field });
    }
    Ok(url)
}
