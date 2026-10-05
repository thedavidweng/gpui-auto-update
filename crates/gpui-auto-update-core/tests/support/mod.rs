//! Shared helpers for feed tests: disposable keys and feed XML builders.

#![allow(dead_code)]

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signer, SigningKey};
use gpui_auto_update_core::trust::TrustedKey;

/// A disposable test-only signing key derived from a fixed seed.
pub fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

pub fn trusted_key(key: &SigningKey) -> TrustedKey {
    TrustedKey::from_base64(&B64.encode(key.verifying_key().to_bytes())).unwrap()
}

pub fn sign(key: &SigningKey, data: &[u8]) -> String {
    B64.encode(key.sign(data).to_bytes())
}

/// One `<item>` of a native feed. Optional fields are omitted when `None`.
#[derive(Clone)]
pub struct Item {
    pub version: String,
    pub short_version: Option<String>,
    pub title: Option<String>,
    pub pub_date: Option<String>,
    pub channel: Option<String>,
    pub minimum_system_version: Option<String>,
    pub critical: Option<Option<String>>,
    pub release_notes_link: Option<String>,
    pub description: Option<String>,
    pub url: String,
    pub length: Option<String>,
    pub signature: Option<String>,
    pub os: Option<String>,
    pub arch: Option<String>,
    /// Raw XML appended inside `<item>`.
    pub extra: String,
}

impl Item {
    /// A well-formed Linux x86_64 item for `version` whose artifact is
    /// `artifact`, signed by `key`.
    pub fn signed(version: &str, key: &SigningKey, artifact: &[u8]) -> Self {
        Self {
            version: version.to_owned(),
            short_version: None,
            title: Some(format!("Version {version}")),
            pub_date: None,
            channel: None,
            minimum_system_version: None,
            critical: None,
            release_notes_link: None,
            description: None,
            url: format!("https://downloads.example.com/app-{version}-linux-x86_64.tar.gz"),
            length: Some(artifact.len().to_string()),
            signature: Some(sign(key, artifact)),
            os: Some("linux".to_owned()),
            arch: Some("x86_64".to_owned()),
            extra: String::new(),
        }
    }

    pub fn os(mut self, os: &str) -> Self {
        self.os = Some(os.to_owned());
        self
    }

    pub fn arch(mut self, arch: &str) -> Self {
        self.arch = Some(arch.to_owned());
        self
    }

    pub fn channel(mut self, channel: &str) -> Self {
        self.channel = Some(channel.to_owned());
        self
    }

    pub fn url(mut self, url: &str) -> Self {
        self.url = url.to_owned();
        self
    }

    pub fn xml(&self) -> String {
        let mut out = String::from("    <item>\n");
        let el = |out: &mut String, name: &str, value: &Option<String>| {
            if let Some(v) = value {
                out.push_str(&format!("      <{name}>{}</{name}>\n", escape(v)));
            }
        };
        el(&mut out, "title", &self.title);
        el(&mut out, "pubDate", &self.pub_date);
        out.push_str(&format!(
            "      <sparkle:version>{}</sparkle:version>\n",
            escape(&self.version)
        ));
        el(&mut out, "sparkle:shortVersionString", &self.short_version);
        el(&mut out, "sparkle:channel", &self.channel);
        el(
            &mut out,
            "sparkle:minimumSystemVersion",
            &self.minimum_system_version,
        );
        match &self.critical {
            None => {}
            Some(None) => out.push_str("      <sparkle:criticalUpdate/>\n"),
            Some(Some(v)) => out.push_str(&format!(
                "      <sparkle:criticalUpdate sparkle:version=\"{}\"/>\n",
                escape(v)
            )),
        }
        el(
            &mut out,
            "sparkle:releaseNotesLink",
            &self.release_notes_link,
        );
        if let Some(d) = &self.description {
            out.push_str(&format!(
                "      <description><![CDATA[{d}]]></description>\n"
            ));
        }
        out.push_str(&format!("      <enclosure url=\"{}\"", escape(&self.url)));
        let attr = |out: &mut String, name: &str, value: &Option<String>| {
            if let Some(v) = value {
                out.push_str(&format!(" {name}=\"{}\"", escape(v)));
            }
        };
        attr(&mut out, "length", &self.length);
        out.push_str(" type=\"application/gzip\"");
        attr(&mut out, "sparkle:os", &self.os);
        attr(&mut out, "gpui-auto-update:arch", &self.arch);
        attr(&mut out, "sparkle:edSignature", &self.signature);
        out.push_str("/>\n");
        out.push_str(&self.extra);
        out.push_str("    </item>\n");
        out
    }
}

pub fn feed(items: &[Item]) -> String {
    let body: String = items.iter().map(Item::xml).collect();
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0"
     xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"
     xmlns:gpui-auto-update="https://github.com/thedavidweng/gpui-auto-update/xml-namespaces/feed">
  <channel>
    <title>Example App</title>
{body}  </channel>
</rss>
"#
    )
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
