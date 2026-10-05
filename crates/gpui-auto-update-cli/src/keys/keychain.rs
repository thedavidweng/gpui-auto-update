//! The macOS login Keychain, through Sparkle's own `generate_keys` tool.
//!
//! Sparkle keeps local signing keys in the login Keychain (a generic
//! password per *account*, `ed25519` by default), and `sign_update` and
//! `generate_appcast` read them from there. Driving `generate_keys` rather
//! than the Keychain API keeps the stored item exactly as Sparkle expects.

use std::path::PathBuf;
use std::process::Command;

use gpui_auto_update_core::trust::TrustedKey;

use super::private_key::PrivateKey;
use super::secret_io::TempKeyFile;

/// Sparkle's default Keychain account name.
pub const DEFAULT_ACCOUNT: &str = "ed25519";

/// A Keychain account reached through a `generate_keys` executable.
#[derive(Debug, Clone)]
pub struct Keychain {
    tool: PathBuf,
    account: String,
}

impl Keychain {
    /// Uses `generate_keys` from `sparkle_bin` (a Sparkle distribution's
    /// `bin` directory), else from `$SPARKLE_BIN`, else from `PATH`.
    pub fn new(sparkle_bin: Option<PathBuf>, account: Option<String>) -> Result<Self, String> {
        if !cfg!(target_os = "macos") {
            return Err(
                "the Keychain workflow uses Sparkle's generate_keys and is only available on macOS; \
                 use --key-stdin, --key-env, or --key-file instead"
                    .to_owned(),
            );
        }
        let dir = sparkle_bin.or_else(|| std::env::var_os("SPARKLE_BIN").map(PathBuf::from));
        let tool = match dir {
            Some(dir) => dir.join("generate_keys"),
            None => PathBuf::from("generate_keys"),
        };
        Ok(Self {
            tool,
            account: account.unwrap_or_else(|| DEFAULT_ACCOUNT.to_owned()),
        })
    }

    /// The account name.
    pub fn account(&self) -> &str {
        &self.account
    }

    /// The public key of the account's stored private key.
    pub fn public_key(&self) -> Result<TrustedKey, String> {
        let stdout = self.run(&["-p"], Echo::Output)?;
        let line = stdout
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .unwrap_or_default();
        TrustedKey::from_base64(line).map_err(|e| {
            format!(
                "generate_keys -p did not print a usable public key for account `{}`: {e}",
                self.account
            )
        })
    }

    /// Creates a key for the account if it has none (Sparkle's
    /// `generate_keys` behavior) and returns the account's public key.
    pub fn generate(&self) -> Result<TrustedKey, String> {
        self.run(&[], Echo::Output)?;
        self.public_key()
    }

    /// Stores an existing private key in the account.
    pub fn import(&self, key: &PrivateKey) -> Result<TrustedKey, String> {
        let file = TempKeyFile::new(key)
            .map_err(|e| format!("failed to stage private key for generate_keys: {e}"))?;
        let path = file
            .path()
            .to_str()
            .ok_or("temporary directory path is not UTF-8")?;
        self.run(&["-f", path], Echo::Nothing)?;
        drop(file);
        let stored = self.public_key()?;
        if stored != key.public_key() {
            return Err(format!(
                "Keychain account `{}` holds a different key ({}) than the one imported; \
                 it probably already had a key",
                self.account,
                stored.to_base64()
            ));
        }
        Ok(stored)
    }

    fn run(&self, args: &[&str], echo: Echo) -> Result<String, String> {
        let output = Command::new(&self.tool)
            .args(["--account", &self.account])
            .args(args)
            .output()
            .map_err(|e| {
                format!(
                    "failed to run Sparkle's `{}` ({e}); pass --sparkle-bin <dir> or set SPARKLE_BIN \
                     to the `bin` directory of a Sparkle distribution",
                    self.tool.display()
                )
            })?;
        if !output.status.success() {
            let detail = match echo {
                // generate_keys reports errors on stdout.
                Echo::Output => format!(
                    ": {}{}",
                    String::from_utf8_lossy(&output.stdout).trim(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
                Echo::Nothing => String::new(),
            };
            return Err(format!(
                "`generate_keys` failed for Keychain account `{}` ({}){detail}",
                self.account, output.status,
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// Whether a failed `generate_keys` run's output may be shown.
#[derive(Clone, Copy)]
enum Echo {
    Output,
    /// Some `-f` failure messages quote the imported key.
    Nothing,
}
