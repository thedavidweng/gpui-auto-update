//! Argument parsing for `feed` commands.
//!
//! Like `keys`, `feed` takes private keys and therefore does not let clap
//! parse its arguments: clap's errors quote offending arguments, which could
//! echo a key pasted onto the command line. Unknown arguments are reported
//! by position only.

use std::path::PathBuf;

use crate::keys::{KeySource, is_secret_flag};

pub enum Failure {
    Help,
    Usage(String),
    Failed(String),
}

impl From<String> for Failure {
    fn from(msg: String) -> Self {
        Self::Failed(msg)
    }
}

impl From<anyhow::Error> for Failure {
    fn from(err: anyhow::Error) -> Self {
        Self::Failed(format!("{err:#}"))
    }
}

pub fn usage<T>(msg: impl Into<String>) -> Result<T, Failure> {
    Err(Failure::Usage(msg.into()))
}

/// The options one command accepts, besides the key sources.
pub struct Spec {
    /// Options that take a value.
    pub values: &'static [&'static str],
    /// Options without a value.
    pub switches: &'static [&'static str],
}

pub struct ParsedArgs {
    values: Vec<(&'static str, String)>,
    switches: Vec<&'static str>,
    pub sources: Vec<KeySource>,
}

impl ParsedArgs {
    pub fn parse(mut args: impl Iterator<Item = String>, spec: &Spec) -> Result<Self, Failure> {
        let mut parsed = Self {
            values: Vec::new(),
            switches: Vec::new(),
            sources: Vec::new(),
        };
        let mut position = 1;
        while let Some(arg) = args.next() {
            position += 1;
            let mut value = |name: &str| match args.next() {
                Some(v) => {
                    position += 1;
                    Ok(v)
                }
                None => usage(format!("{name} requires a value")),
            };
            match arg.as_str() {
                "--help" | "-h" => return Err(Failure::Help),
                "--key-stdin" => parsed.sources.push(KeySource::Stdin),
                "--key-env" => parsed.sources.push(KeySource::Env(value("--key-env")?)),
                "--key-file" => parsed
                    .sources
                    .push(KeySource::File(value("--key-file")?.into())),
                flag if is_secret_flag(flag) => {
                    return usage(
                        "private keys are never accepted as command-line arguments, which leak \
                         into shell history, process lists, and CI logs; use --key-stdin, \
                         --key-env <VAR>, or --key-file <path>",
                    );
                }
                flag => {
                    if let Some(name) = spec.values.iter().find(|n| **n == flag) {
                        let v = value(name)?;
                        parsed.values.push((name, v));
                    } else if let Some(name) = spec.switches.iter().find(|n| **n == flag) {
                        parsed.switches.push(name);
                    } else {
                        // The argument itself is not echoed: it may be a pasted key.
                        return usage(format!(
                            "unexpected argument at position {position} (not shown, since it may \
                             be a secret)"
                        ));
                    }
                }
            }
        }
        Ok(parsed)
    }

    /// The value of an option that may be given at most once.
    pub fn value(&self, name: &str) -> Result<Option<&str>, Failure> {
        let mut found = self.values.iter().filter(|(n, _)| *n == name);
        let first = found.next().map(|(_, v)| v.as_str());
        if found.next().is_some() {
            return usage(format!("{name} may be given only once"));
        }
        Ok(first)
    }

    pub fn required(&self, name: &str) -> Result<&str, Failure> {
        match self.value(name)? {
            Some(v) => Ok(v),
            None => usage(format!("{name} is required")),
        }
    }

    pub fn path(&self, name: &str) -> Result<Option<PathBuf>, Failure> {
        Ok(self.value(name)?.map(PathBuf::from))
    }

    pub fn switch(&self, name: &str) -> bool {
        self.switches.contains(&name)
    }

    /// Every given value option in `names`, in command-line order, for
    /// passing through to another tool.
    pub fn passthrough(&self, names: &[&str]) -> Vec<(&'static str, &str)> {
        self.values
            .iter()
            .filter(|(n, _)| names.contains(n))
            .map(|(n, v)| (*n, v.as_str()))
            .collect()
    }

    /// The single private key source.
    pub fn single_source(&self) -> Result<Option<&KeySource>, Failure> {
        match self.sources.as_slice() {
            [] => Ok(None),
            [source] => Ok(Some(source)),
            _ => usage("give exactly one key source"),
        }
    }
}
