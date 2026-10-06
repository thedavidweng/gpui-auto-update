//! A lenient reading of a feed document that keeps every enclosure, valid or
//! not, so that `verify` can report all problems instead of only the first.
//!
//! Acceptance is still decided by core's fail-closed parser for native feeds;
//! this scan exists only to produce complete diagnostics and to find the
//! artifacts to download.

use gpui_auto_update_core::feed::{NATIVE_NS, SPARKLE_NS};

/// Which document a feed is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A Windows or Linux native feed (docs/feed-format.md).
    Native,
    /// A macOS Sparkle appcast.
    Sparkle,
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Native => "native feed",
            Self::Sparkle => "Sparkle appcast",
        })
    }
}

#[derive(Debug)]
pub struct Item {
    pub index: usize,
    pub version: Option<String>,
    pub short_version: Option<String>,
    pub informational: bool,
    pub enclosures: Vec<Enclosure>,
}

impl Item {
    /// How diagnostics refer to the item.
    pub fn label(&self) -> String {
        match &self.version {
            Some(v) => format!("item {} (version {v})", self.index),
            None => format!("item {}", self.index),
        }
    }
}

#[derive(Debug)]
pub struct Enclosure {
    /// `Some(deltaFrom)` for a delta enclosure inside `sparkle:deltas`.
    pub delta_from: Option<Option<String>>,
    pub url: Option<String>,
    pub length: Option<String>,
    pub signature: Option<String>,
    pub os: Option<String>,
    pub arch: Option<String>,
}

impl Enclosure {
    pub fn kind(&self) -> &'static str {
        if self.delta_from.is_some() {
            "delta enclosure"
        } else {
            "enclosure"
        }
    }
}

pub struct Document {
    pub items: Vec<Item>,
}

impl Document {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes).map_err(|_| "the feed is not UTF-8".to_owned())?;
        let options = roxmltree::ParsingOptions {
            allow_dtd: false,
            ..Default::default()
        };
        let doc = roxmltree::Document::parse_with_options(text, options)
            .map_err(|e| format!("the feed is not valid XML: {e}"))?;
        let root = doc.root_element();
        let channel = root
            .children()
            .find(|n| n.has_tag_name("channel"))
            .filter(|_| root.has_tag_name("rss"))
            .ok_or_else(|| "the feed is not an RSS document with a <channel>".to_owned())?;
        let items = channel
            .children()
            .filter(|n| n.has_tag_name("item"))
            .enumerate()
            .map(|(index, item)| read_item(index, item))
            .collect();
        Ok(Self { items })
    }

    /// A native feed declares Windows or Linux artifacts, or uses this
    /// project's architecture attribute; anything else is read as a Sparkle
    /// appcast.
    pub fn detect_format(&self) -> Format {
        let native = self
            .enclosures()
            .any(|e| e.arch.is_some() || matches!(e.os.as_deref(), Some("windows" | "linux")));
        if native {
            Format::Native
        } else {
            Format::Sparkle
        }
    }

    pub fn enclosures(&self) -> impl Iterator<Item = &Enclosure> {
        self.items.iter().flat_map(|i| &i.enclosures)
    }
}

fn child_text(node: roxmltree::Node<'_, '_>, name: (&str, &str)) -> Option<String> {
    node.children()
        .find(|n| n.has_tag_name(name))
        .map(|n| n.text().unwrap_or_default().to_owned())
}

fn read_item(index: usize, item: roxmltree::Node<'_, '_>) -> Item {
    let mut enclosures: Vec<Enclosure> = item
        .children()
        .filter(|n| n.has_tag_name("enclosure"))
        .map(|n| read_enclosure(n, None))
        .collect();
    for deltas in item
        .children()
        .filter(|n| n.has_tag_name((SPARKLE_NS, "deltas")))
    {
        enclosures.extend(
            deltas
                .children()
                .filter(|n| n.has_tag_name("enclosure"))
                .map(|n| {
                    let from = n.attribute((SPARKLE_NS, "deltaFrom")).map(str::to_owned);
                    read_enclosure(n, Some(from))
                }),
        );
    }
    // Older Sparkle appcasts carry the versions as enclosure attributes.
    let full = item.children().find(|n| n.has_tag_name("enclosure"));
    let attr = |name: &str| {
        full.and_then(|n| n.attribute((SPARKLE_NS, name)))
            .map(str::to_owned)
    };
    Item {
        index,
        version: child_text(item, (SPARKLE_NS, "version")).or_else(|| attr("version")),
        short_version: child_text(item, (SPARKLE_NS, "shortVersionString"))
            .or_else(|| attr("shortVersionString")),
        informational: item
            .children()
            .any(|n| n.has_tag_name((SPARKLE_NS, "informationalUpdate"))),
        enclosures,
    }
}

fn read_enclosure(node: roxmltree::Node<'_, '_>, delta_from: Option<Option<String>>) -> Enclosure {
    let attr = |name: (&str, &str)| node.attribute(name).map(str::to_owned);
    Enclosure {
        delta_from,
        url: node.attribute("url").map(str::to_owned),
        length: node.attribute("length").map(str::to_owned),
        signature: attr((SPARKLE_NS, "edSignature")),
        os: attr((SPARKLE_NS, "os")),
        arch: attr((NATIVE_NS, "arch")),
    }
}
