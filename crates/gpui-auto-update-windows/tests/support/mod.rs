//! Shared fixtures: synthetic PE images with version resources, signed
//! native feeds served from loopback, and disposable keys.
#![allow(dead_code)]

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signer, SigningKey};
use gpui_auto_update_core::fetch::FetchPolicy;
use gpui_auto_update_core::trust::TrustedKey;

pub const MACHINE_I386: u16 = 0x014c;
pub const MACHINE_AMD64: u16 = 0x8664;
pub const MACHINE_ARM64: u16 = 0xaa64;

/// Builds minimal but well-formed PE images whose only section is `.rsrc`.
#[derive(Clone)]
pub struct PeImage {
    pub machine: u16,
    pub pe32_plus: bool,
    /// `(StringTable key, [(name, value)])`; no tables means no
    /// StringFileInfo block.
    pub tables: Vec<(String, Vec<(String, String)>)>,
    /// Whether a version resource is present at all.
    pub with_version: bool,
    /// Padding appended after the image, to make artifacts of a given size.
    pub trailer: Vec<u8>,
}

impl PeImage {
    /// An i386 image (like an Inno Setup installer) whose `ProductVersion`
    /// is `version`.
    pub fn installer(version: &str) -> Self {
        Self {
            machine: MACHINE_I386,
            pe32_plus: false,
            tables: vec![(
                "040904b0".to_owned(),
                vec![
                    ("CompanyName".to_owned(), "Example".to_owned()),
                    ("FileVersion".to_owned(), "1.5.0.0".to_owned()),
                    ("ProductVersion".to_owned(), version.to_owned()),
                ],
            )],
            with_version: true,
            trailer: Vec::new(),
        }
    }

    /// A PE32+ image for `machine` whose `ProductVersion` is `version`.
    pub fn executable(machine: u16, version: &str) -> Self {
        Self {
            machine,
            pe32_plus: true,
            ..Self::installer(version)
        }
    }

    pub fn without_version(mut self) -> Self {
        self.with_version = false;
        self
    }

    pub fn with_tables(mut self, tables: Vec<(&str, Vec<(&str, &str)>)>) -> Self {
        self.tables = tables
            .into_iter()
            .map(|(name, strings)| {
                (
                    name.to_owned(),
                    strings
                        .into_iter()
                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                        .collect(),
                )
            })
            .collect();
        self
    }

    pub fn with_trailer(mut self, trailer: &[u8]) -> Self {
        self.trailer = trailer.to_vec();
        self
    }

    pub fn build(&self) -> Vec<u8> {
        const SECTION_RVA: u32 = 0x1000;
        const RAW_OFFSET: usize = 0x200;
        let rsrc = self.resource_section(SECTION_RVA);

        let mut out = vec![0u8; RAW_OFFSET];
        out[0..2].copy_from_slice(b"MZ");
        put32(&mut out, 0x3c, 0x40);
        let pe = 0x40;
        out[pe..pe + 4].copy_from_slice(b"PE\0\0");
        let coff = pe + 4;
        put16(&mut out, coff, self.machine);
        put16(&mut out, coff + 2, 1);
        let (magic, dirs_at, opt_size) = if self.pe32_plus {
            (0x20b, 112, 112 + 16 * 8)
        } else {
            (0x10b, 96, 96 + 16 * 8)
        };
        put16(&mut out, coff + 16, opt_size as u16);
        put16(&mut out, coff + 18, 0x0102);
        let opt = coff + 20;
        put16(&mut out, opt, magic);
        put32(&mut out, opt + dirs_at - 4, 16);
        // Data directory 2 is the resource table.
        put32(&mut out, opt + dirs_at + 2 * 8, SECTION_RVA);
        put32(&mut out, opt + dirs_at + 2 * 8 + 4, rsrc.len() as u32);
        let section = opt + opt_size;
        out[section..section + 5].copy_from_slice(b".rsrc");
        put32(&mut out, section + 8, rsrc.len() as u32);
        put32(&mut out, section + 12, SECTION_RVA);
        let raw_size = rsrc.len().next_multiple_of(0x200);
        put32(&mut out, section + 16, raw_size as u32);
        put32(&mut out, section + 20, RAW_OFFSET as u32);

        out.extend_from_slice(&rsrc);
        out.resize(RAW_OFFSET + raw_size, 0);
        out.extend_from_slice(&self.trailer);
        out
    }

    fn resource_section(&self, rva: u32) -> Vec<u8> {
        let mut out = Vec::new();
        // Root directory: one ID entry for the type.
        let type_id = if self.with_version { 16 } else { 3 };
        directory(&mut out, type_id, 0x8000_0000 | 24);
        // Name level.
        directory(&mut out, 1, 0x8000_0000 | 48);
        // Language level, pointing at the data entry.
        directory(&mut out, 0x409, 72);
        let data = self.version_info();
        put32_push(&mut out, rva + 88);
        put32_push(&mut out, data.len() as u32);
        put32_push(&mut out, 0);
        put32_push(&mut out, 0);
        assert_eq!(out.len(), 88);
        out.extend_from_slice(&data);
        out
    }

    fn version_info(&self) -> Vec<u8> {
        let mut fixed = Vec::new();
        for value in [
            0xfeef_04bd_u32,
            0x0001_0000,
            0x0001_0005,
            0,
            0x0001_0005,
            0,
            0x3f,
            0,
            0x0004_0004,
            1,
            0,
            0,
            0,
        ] {
            fixed.extend_from_slice(&value.to_le_bytes());
        }
        let mut children = Vec::new();
        if !self.tables.is_empty() {
            let tables: Vec<Vec<u8>> = self
                .tables
                .iter()
                .map(|(name, strings)| {
                    let strings: Vec<Vec<u8>> = strings
                        .iter()
                        .map(|(key, value)| {
                            let mut text = utf16(value);
                            text.extend_from_slice(&[0, 0]);
                            let chars = (text.len() / 2) as u16;
                            node(key, 1, &text, chars, &[])
                        })
                        .collect();
                    node(name, 1, &[], 0, &strings)
                })
                .collect();
            children.push(node("StringFileInfo", 1, &[], 0, &tables));
        }
        node("VS_VERSION_INFO", 0, &fixed, fixed.len() as u16, &children)
    }
}

fn directory(out: &mut Vec<u8>, id: u32, target: u32) {
    for _ in 0..3 {
        put32_push(out, 0);
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    put32_push(out, id);
    put32_push(out, target);
}

/// One version-resource block: header, key, value, children, each start
/// aligned to 4 bytes.
fn node(key: &str, kind: u16, value: &[u8], value_len: u16, children: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![0u8; 6];
    out.extend_from_slice(&utf16(key));
    out.extend_from_slice(&[0, 0]);
    pad4(&mut out);
    out.extend_from_slice(value);
    for child in children {
        pad4(&mut out);
        out.extend_from_slice(child);
    }
    let len = out.len() as u16;
    out[0..2].copy_from_slice(&len.to_le_bytes());
    out[2..4].copy_from_slice(&value_len.to_le_bytes());
    out[4..6].copy_from_slice(&kind.to_le_bytes());
    out
}

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn pad4(out: &mut Vec<u8>) {
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn put16(out: &mut [u8], at: usize, value: u16) {
    out[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put32_push(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

// ---------------------------------------------------------------------------
// Keys, feeds, and loopback serving

pub fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}

pub fn trusted_key() -> TrustedKey {
    TrustedKey::from_base64(&B64.encode(signing_key().verifying_key().to_bytes())).unwrap()
}

pub fn sign(bytes: &[u8]) -> String {
    B64.encode(signing_key().sign(bytes).to_bytes())
}

pub fn loopback_policy() -> FetchPolicy {
    FetchPolicy {
        allow_insecure_http: true,
        timeout: Duration::from_secs(5),
        ..FetchPolicy::default()
    }
}

/// One Windows feed entry for `arch` whose artifact at `url` is `artifact`,
/// signed over `signed`.
pub fn item(version: &str, arch: &str, url: &str, artifact: &[u8], signed: &[u8]) -> String {
    format!(
        r#"    <item>
      <sparkle:version>{version}</sparkle:version>
      <enclosure url="{url}" length="{length}" type="application/octet-stream"
                 sparkle:os="windows" gpui-auto-update:arch="{arch}"
                 sparkle:edSignature="{signature}"/>
    </item>
"#,
        length = artifact.len(),
        signature = sign(signed),
    )
}

pub fn feed(items: &[String]) -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0"
     xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"
     xmlns:gpui-auto-update="https://github.com/thedavidweng/gpui-auto-update/xml-namespaces/feed">
  <channel>
{}  </channel>
</rss>
"#,
        items.concat()
    )
    .into_bytes()
}

/// Serves `routes` (`path -> body`) on a loopback port until the process
/// exits; unknown paths get 404. Returns the base URL without a trailing
/// slash.
pub fn serve(routes: Vec<(&'static str, Vec<u8>)>) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = server.server_addr().to_ip().unwrap();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let body = routes
                .iter()
                .find(|(path, _)| *path == request.url())
                .map(|(_, body)| body.clone());
            let _ = match body {
                Some(body) => request.respond(
                    tiny_http::Response::from_data(body).with_chunked_threshold(usize::MAX),
                ),
                None => request.respond(tiny_http::Response::empty(404)),
            };
        }
    });
    format!("http://{addr}")
}
