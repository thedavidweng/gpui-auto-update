//! Reading and writing private key text through secret-safe channels.
//!
//! Private keys come only from standard input, a named environment variable,
//! or a file, never from argv, where they would be visible to other users of
//! the machine and saved in shell history and CI logs. Errors name the
//! channel but never include its contents.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use super::private_key::PrivateKey;

/// Upper bound on accepted key text. A legacy key is 128 base64 characters;
/// anything far larger is the wrong input, not a key.
const MAX_KEY_TEXT: u64 = 16 * 1024;

/// Where a private key is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// Standard input.
    Stdin,
    /// A named environment variable.
    Env(String),
    /// A file, such as one exported by Sparkle's `generate_keys -x`.
    File(PathBuf),
}

impl KeySource {
    /// Human-readable channel name for messages (never the key itself).
    pub fn describe(&self) -> String {
        match self {
            Self::Stdin => "standard input".to_owned(),
            Self::Env(var) => format!("environment variable `{var}`"),
            Self::File(path) => format!("file `{}`", path.display()),
        }
    }

    /// Reads and parses the key.
    pub fn read(&self) -> Result<PrivateKey, String> {
        let text = self.read_text()?;
        PrivateKey::from_sparkle_base64(&text)
            .map_err(|e| format!("private key from {}: {e}", self.describe()))
    }

    fn read_text(&self) -> Result<Zeroizing<String>, String> {
        let from = || self.describe();
        match self {
            Self::Stdin => read_limited(io::stdin().lock())
                .map_err(|e| format!("failed to read private key from {}: {e}", from())),
            Self::Env(var) => match std::env::var(var) {
                Ok(value) if !value.trim().is_empty() => Ok(Zeroizing::new(value)),
                Ok(_) | Err(std::env::VarError::NotPresent) => {
                    Err(format!("{} is not set or empty", from()))
                }
                Err(std::env::VarError::NotUnicode(_)) => {
                    Err(format!("{} does not contain valid UTF-8", from()))
                }
            },
            Self::File(path) => File::open(path)
                .and_then(read_limited)
                .map_err(|e| format!("failed to read private key from {}: {e}", from())),
        }
    }
}

fn read_limited(reader: impl Read) -> io::Result<Zeroizing<String>> {
    let mut buf = Zeroizing::new(Vec::new());
    reader.take(MAX_KEY_TEXT + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_KEY_TEXT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "input is too large to be a private key",
        ));
    }
    let text = std::str::from_utf8(&buf)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "input is not valid UTF-8"))?;
    if text.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "input is empty",
        ));
    }
    Ok(Zeroizing::new(text.to_owned()))
}

/// Writes `key` in Sparkle export format to a new file readable only by the
/// current user. Refuses to overwrite an existing file so a key is never
/// silently replaced.
pub fn write_new_key_file(path: &Path, key: &PrivateKey) -> io::Result<()> {
    let mut file = create_private(path)?;
    file.write_all(key.to_sparkle_base64().as_bytes())?;
    file.sync_all()
}

fn create_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

/// A private key written to a fresh owner-only temporary directory, for
/// handing to tools that only accept key files (`generate_keys -f`). The
/// file is overwritten and removed on drop.
pub struct TempKeyFile {
    dir: PathBuf,
    path: PathBuf,
    len: usize,
}

impl TempKeyFile {
    /// Writes `key` to a new temporary file.
    pub fn new(key: &PrivateKey) -> io::Result<Self> {
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(io::Error::other)?;
        let name: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
        let dir = std::env::temp_dir().join(format!("gpui-auto-update-key-{name}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            fs::DirBuilder::new().mode(0o700).create(&dir)?;
        }
        #[cfg(not(unix))]
        fs::create_dir(&dir)?;
        let path = dir.join("private-key");
        let this = Self {
            len: key.to_sparkle_base64().len(),
            dir,
            path,
        };
        write_new_key_file(&this.path, key)?;
        Ok(this)
    }

    /// Path of the key file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempKeyFile {
    fn drop(&mut self) {
        if let Ok(mut file) = OpenOptions::new().write(true).open(&self.path) {
            let _ = file.write_all(&vec![0u8; self.len]);
            let _ = file.sync_all();
        }
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(&self.dir);
    }
}
