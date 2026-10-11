//! Minimal property-list reader for Info.plist files and codesign
//! entitlements. XML plists are parsed directly; binary plists are converted
//! with `plutil` on macOS.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::Path;

use anyhow::{Context, Result, bail};

const MAX_PLIST_BYTES: u64 = 16 * 1024 * 1024;

pub type Dictionary = BTreeMap<String, Value>;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    String(String),
    Boolean(bool),
    Integer(i64),
    Real(f64),
    Array(Vec<Value>),
    Dictionary(Dictionary),
    /// `<data>` and `<date>` contents, kept as text.
    Other(String),
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_owned())
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Boolean(b)
    }
}

impl Value {
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Integer(i) => Some(*i as f64),
            Value::Real(r) => Some(*r),
            _ => None,
        }
    }
}

pub fn read_file(path: &Path) -> Result<Dictionary> {
    let file =
        std::fs::File::open(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_PLIST_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PLIST_BYTES {
        bail!("{} is too large for a property list", path.display());
    }
    if bytes.starts_with(b"bplist") {
        bytes = binary_to_xml(path)?;
    }
    parse_dictionary(&bytes).with_context(|| format!("cannot parse {}", path.display()))
}

#[cfg(target_os = "macos")]
fn binary_to_xml(path: &Path) -> Result<Vec<u8>> {
    let out = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "xml1", "-o", "-"])
        .arg(path)
        .output()
        .context("cannot run plutil")?;
    if !out.status.success() {
        bail!(
            "plutil cannot convert {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

#[cfg(not(target_os = "macos"))]
fn binary_to_xml(path: &Path) -> Result<Vec<u8>> {
    bail!(
        "{} is a binary property list; convert it with `plutil -convert xml1` on macOS",
        path.display()
    )
}

pub fn parse_dictionary(xml: &[u8]) -> Result<Dictionary> {
    let text = std::str::from_utf8(xml).context("property list is not UTF-8")?;
    let options = roxmltree::ParsingOptions {
        // Plists carry a DOCTYPE; roxmltree never fetches external entities.
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    let doc = roxmltree::Document::parse_with_options(text, options)?;
    let root = doc.root_element();
    if root.tag_name().name() != "plist" {
        bail!("root element is <{}>, not <plist>", root.tag_name().name());
    }
    let value = root
        .children()
        .find(roxmltree::Node::is_element)
        .context("empty property list")?;
    match parse_value(value)? {
        Value::Dictionary(d) => Ok(d),
        _ => bail!("property list is not a dictionary"),
    }
}

fn parse_value(node: roxmltree::Node<'_, '_>) -> Result<Value> {
    let text = || node.text().unwrap_or("").to_owned();
    Ok(match node.tag_name().name() {
        "string" => Value::String(text()),
        "true" => Value::Boolean(true),
        "false" => Value::Boolean(false),
        "integer" => Value::Integer(
            text()
                .trim()
                .parse()
                .with_context(|| format!("invalid <integer> {:?}", text()))?,
        ),
        "real" => Value::Real(
            text()
                .trim()
                .parse()
                .with_context(|| format!("invalid <real> {:?}", text()))?,
        ),
        "data" | "date" => Value::Other(text()),
        "array" => Value::Array(
            node.children()
                .filter(roxmltree::Node::is_element)
                .map(parse_value)
                .collect::<Result<_>>()?,
        ),
        "dict" => {
            let mut dict = Dictionary::new();
            let mut children = node.children().filter(roxmltree::Node::is_element);
            while let Some(key) = children.next() {
                if key.tag_name().name() != "key" {
                    bail!(
                        "expected <key> in <dict>, found <{}>",
                        key.tag_name().name()
                    );
                }
                let value = children.next().with_context(|| {
                    format!("<key>{}</key> has no value", key.text().unwrap_or(""))
                })?;
                dict.insert(key.text().unwrap_or("").to_owned(), parse_value(value)?);
            }
            Value::Dictionary(dict)
        }
        other => bail!("unsupported property-list element <{other}>"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_xml_info_plist() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>com.example.A&amp;B</string>
  <key>SUEnableAutomaticChecks</key><true/>
  <key>SUScheduledCheckInterval</key><integer>86400</integer>
  <key>Ratio</key><real>0.5</real>
  <key>Empty</key><string/>
  <key>Names</key><array><string>a</string><string>b</string></array>
  <key>Nested</key><dict><key>k</key><false/></dict>
</dict>
</plist>"#;
        let d = parse_dictionary(xml).unwrap();
        assert_eq!(d["CFBundleIdentifier"].as_string(), Some("com.example.A&B"));
        assert_eq!(d["SUEnableAutomaticChecks"], Value::Boolean(true));
        assert_eq!(d["SUScheduledCheckInterval"].as_number(), Some(86400.0));
        assert_eq!(d["Ratio"].as_number(), Some(0.5));
        assert_eq!(d["Empty"].as_string(), Some(""));
        assert_eq!(d["Names"].as_array().map(<[Value]>::len), Some(2));
        let Value::Dictionary(nested) = &d["Nested"] else {
            panic!("nested dict")
        };
        assert_eq!(nested["k"], Value::Boolean(false));
    }

    #[test]
    fn rejects_malformed_property_lists() {
        assert!(parse_dictionary(b"<plist><array/></plist>").is_err());
        assert!(parse_dictionary(b"<plist><dict><key>a</key></dict></plist>").is_err());
        assert!(parse_dictionary(b"<html/>").is_err());
        assert!(
            parse_dictionary(b"<plist><dict><key>n</key><integer>x</integer></dict></plist>")
                .is_err()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_binary_property_lists_through_plutil() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Info.plist");
        std::fs::write(
            &path,
            "<plist version=\"1.0\"><dict><key>CFBundleVersion</key><string>7</string></dict></plist>",
        )
        .unwrap();
        let status = std::process::Command::new("/usr/bin/plutil")
            .args(["-convert", "binary1"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(std::fs::read(&path).unwrap().starts_with(b"bplist"));
        let d = read_file(&path).unwrap();
        assert_eq!(d["CFBundleVersion"].as_string(), Some("7"));
    }
}
