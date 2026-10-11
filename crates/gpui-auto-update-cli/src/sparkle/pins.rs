//! The Sparkle distributions this tool knows how to verify.
//!
//! The table lives in `sparkle-pins.json` at the crate root so that the
//! scheduled upstream-release check can read it without building the CLI.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const PINS_JSON: &str = include_str!("../../sparkle-pins.json");

#[derive(Debug, Deserialize)]
pub struct Pins {
    pub default: String,
    pub releases: Vec<Pin>,
}

/// One official Sparkle release archive, identified by its SHA-256.
#[derive(Debug, Clone, Deserialize)]
pub struct Pin {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// `LSMinimumSystemVersion` of the framework in this release.
    pub minimum_system_version: String,
}

impl Pins {
    pub fn load() -> Result<Self> {
        let pins: Pins =
            serde_json::from_str(PINS_JSON).context("sparkle-pins.json is malformed")?;
        if pins.get(&pins.default).is_none() {
            bail!(
                "sparkle-pins.json: default version {} is not listed",
                pins.default
            );
        }
        Ok(pins)
    }

    pub fn get(&self, version: &str) -> Option<&Pin> {
        self.releases.iter().find(|p| p.version == version)
    }

    pub fn default_pin(&self) -> &Pin {
        self.get(&self.default)
            .expect("checked when the table was loaded")
    }

    pub fn resolve(&self, version: Option<&str>) -> Result<&Pin> {
        match version {
            None => Ok(self.default_pin()),
            Some(v) => self.get(v).with_context(|| {
                format!(
                    "Sparkle {v} is not pinned by this tool (pinned: {}); pass --url and --sha256 to declare a distribution explicitly",
                    self.releases
                        .iter()
                        .map(|p| p.version.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_table_is_well_formed() {
        let pins = Pins::load().unwrap();
        for pin in &pins.releases {
            assert_eq!(pin.sha256.len(), 64, "{}", pin.version);
            assert!(pin.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(
                pin.url
                    .starts_with("https://github.com/sparkle-project/Sparkle/releases/download/")
            );
            assert!(
                pin.url
                    .ends_with(&format!("/Sparkle-{}.tar.xz", pin.version))
            );
        }
    }
}
