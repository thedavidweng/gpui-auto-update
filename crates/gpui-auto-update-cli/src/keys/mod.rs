//! `gpui-auto-update keys`: Sparkle-compatible Ed25519 signing keys.

mod keychain;
mod private_key;
mod secret_io;

use std::path::PathBuf;
use std::process::ExitCode;

use gpui_auto_update_core::trust::TrustedKey;

pub use keychain::Keychain;
pub use private_key::PrivateKey;
pub use secret_io::KeySource;
use secret_io::write_new_key_file;

pub const USAGE: &str = "\
Manage Sparkle-compatible Ed25519 update signing keys.

Usage: gpui-auto-update keys <command> [options]

Commands:
  generate    Create a new key pair and print its public key
                (--output <path> | --keychain) [--test]
  import      Store an existing Sparkle private key
                <key source> (--output <path> | --keychain)
  public-key  Print the public key (SUPublicEDKey) of a private key
                <key source>
  check       Fail unless a private key matches the app's public key
                <key source> --public-key <SUPublicEDKey> [--allow-test-key]

Key sources (private keys are never accepted as arguments):
  --key-stdin          Read the base64 key from standard input
  --key-env <VAR>      Read the base64 key from environment variable VAR
  --key-file <path>    Read the base64 key from a file (generate_keys -x format)
  --keychain           Use Sparkle's login Keychain item (macOS)

Options:
  --output <path>      Write the private key to a new owner-only file
  --account <name>     Keychain account (default: ed25519)
  --sparkle-bin <dir>  Directory containing Sparkle's generate_keys
                       (default: $SPARKLE_BIN, then PATH)
  --test               Use the published, insecure test key (development only)
  --allow-test-key     Let `check` accept the insecure test key

Private keys may be 32-byte seeds or legacy 96-byte Sparkle keys, base64.
See docs/key-management.md for CI setup and key rotation.";

/// Runs `keys` with the arguments that follow it.
pub fn run(args: impl Iterator<Item = String>) -> ExitCode {
    let result = Options::parse(args).and_then(|opts| match opts.command {
        Cmd::Help => {
            println!("{USAGE}");
            Ok(())
        }
        Cmd::Generate => generate(&opts),
        Cmd::Import => import(&opts),
        Cmd::PublicKey => public_key(&opts),
        Cmd::Check => check(&opts),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(msg)) => {
            eprintln!("error: {msg}\n\nRun `gpui-auto-update keys --help` for usage.");
            ExitCode::from(2)
        }
        Err(Failure::Failed(msg)) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

enum Failure {
    Usage(String),
    Failed(String),
}

impl From<String> for Failure {
    fn from(msg: String) -> Self {
        Self::Failed(msg)
    }
}

fn usage<T>(msg: impl Into<String>) -> Result<T, Failure> {
    Err(Failure::Usage(msg.into()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cmd {
    Help,
    Generate,
    Import,
    PublicKey,
    Check,
}

struct Options {
    command: Cmd,
    sources: Vec<KeySource>,
    keychain: bool,
    output: Option<PathBuf>,
    account: Option<String>,
    sparkle_bin: Option<PathBuf>,
    public_key: Option<String>,
    test: bool,
    allow_test_key: bool,
}

impl Options {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, Failure> {
        let command = match args.next().as_deref() {
            None | Some("--help" | "-h" | "help") => Cmd::Help,
            Some("generate") => Cmd::Generate,
            Some("import") => Cmd::Import,
            Some("public-key") => Cmd::PublicKey,
            Some("check") => Cmd::Check,
            Some(other) => return usage(format!("unknown keys command `{other}`")),
        };
        let mut opts = Self {
            command,
            sources: Vec::new(),
            keychain: false,
            output: None,
            account: None,
            sparkle_bin: None,
            public_key: None,
            test: false,
            allow_test_key: false,
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
                "--help" | "-h" => opts.command = Cmd::Help,
                "--key-stdin" => opts.sources.push(KeySource::Stdin),
                "--key-env" => opts.sources.push(KeySource::Env(value("--key-env")?)),
                "--key-file" => opts
                    .sources
                    .push(KeySource::File(value("--key-file")?.into())),
                "--keychain" => opts.keychain = true,
                "--output" | "-o" => opts.output = Some(value("--output")?.into()),
                "--account" => opts.account = Some(value("--account")?),
                "--sparkle-bin" => opts.sparkle_bin = Some(value("--sparkle-bin")?.into()),
                "--public-key" => opts.public_key = Some(value("--public-key")?),
                "--test" => opts.test = true,
                "--allow-test-key" => opts.allow_test_key = true,
                flag if is_secret_flag(flag) => {
                    return usage(
                        "private keys are never accepted as command-line arguments, which leak \
                         into shell history, process lists, and CI logs; use --key-stdin, \
                         --key-env <VAR>, --key-file <path>, or --keychain",
                    );
                }
                // The argument itself is not echoed: it may be a pasted key.
                _ => {
                    return usage(format!(
                        "unexpected argument at position {position} (not shown, since it may be a secret)"
                    ));
                }
            }
        }
        Ok(opts)
    }

    fn keychain(&self) -> Result<Keychain, Failure> {
        Ok(Keychain::new(
            self.sparkle_bin.clone(),
            self.account.clone(),
        )?)
    }

    /// The single file-or-channel private key source, rejecting `--keychain`.
    fn plain_source(&self) -> Result<&KeySource, Failure> {
        match self.sources.as_slice() {
            [source] => Ok(source),
            [] => usage(
                "a key source is required: --key-stdin, --key-env <VAR>, or --key-file <path>",
            ),
            _ => usage("give exactly one key source"),
        }
    }

    fn reject_keychain_options(&self) -> Result<(), Failure> {
        if !self.keychain && (self.account.is_some() || self.sparkle_bin.is_some()) {
            return usage("--account and --sparkle-bin only apply with --keychain");
        }
        Ok(())
    }

    fn reject(&self, present: bool, flag: &str) -> Result<(), Failure> {
        if present {
            return usage(format!("{flag} does not apply to this command"));
        }
        Ok(())
    }
}

/// Whether `flag` looks like an attempt to pass a private key as an
/// argument (`--private-key`, `--key-file=...`, and similar).
pub fn is_secret_flag(flag: &str) -> bool {
    let name = flag.split('=').next().unwrap_or(flag);
    matches!(
        name,
        "--key" | "--private-key" | "--secret" | "--ed-key" | "-s" | "-k"
    ) || flag.starts_with("--key-stdin=")
        || flag.starts_with("--key-env=")
        || flag.starts_with("--key-file=")
}

/// Where `generate` and `import` put a private key.
enum Destination {
    File(PathBuf),
    Keychain(Keychain),
}

fn destination(opts: &Options) -> Result<Destination, Failure> {
    opts.reject_keychain_options()?;
    match (&opts.output, opts.keychain) {
        (Some(path), false) => Ok(Destination::File(path.clone())),
        (None, true) => Ok(Destination::Keychain(opts.keychain()?)),
        (Some(_), true) => usage("choose one of --output or --keychain"),
        (None, false) => usage(
            "choose where to store the private key: --output <path> or --keychain \
             (private keys are never printed)",
        ),
    }
}

fn generate(opts: &Options) -> Result<(), Failure> {
    opts.reject(!opts.sources.is_empty(), "a key source")?;
    opts.reject(opts.public_key.is_some(), "--public-key")?;
    opts.reject(opts.allow_test_key, "--allow-test-key")?;
    let dest = destination(opts)?;
    if opts.test {
        let Destination::File(path) = &dest else {
            return usage("--test keys cannot be stored in the Keychain; use --output <path>");
        };
        let key = PrivateKey::insecure_test_key();
        write_key_file(path, &key)?;
        eprintln!(
            "warning: wrote the INSECURE, publicly known test key to {}. Anyone can sign with it. \
             Use it only for local development; release tooling refuses it.",
            path.display()
        );
        println!("{}", key.public_key().to_base64());
        return Ok(());
    }
    match dest {
        Destination::File(path) => {
            let key = PrivateKey::generate()
                .map_err(|e| format!("failed to get randomness from the OS: {e}"))?;
            write_key_file(&path, &key)?;
            eprintln!(
                "Wrote a new private key to {} (owner read/write only). Store it as a CI secret, \
                 keep a backup, then delete this file.",
                path.display()
            );
            print_public(&key.public_key());
        }
        Destination::Keychain(keychain) => {
            let public = keychain.generate()?;
            eprintln!(
                "Keychain account `{}` holds the signing key (an existing key is kept).",
                keychain.account()
            );
            print_public(&public);
        }
    }
    Ok(())
}

fn import(opts: &Options) -> Result<(), Failure> {
    opts.reject(opts.test, "--test")?;
    opts.reject(opts.public_key.is_some(), "--public-key")?;
    opts.reject(opts.allow_test_key, "--allow-test-key")?;
    let source = opts.plain_source()?;
    let dest = destination(opts)?;
    let key = source.read()?;
    if key.is_insecure_test_key() {
        return Err(Failure::Failed(
            "refusing to import the insecure test key; it is only for local development".into(),
        ));
    }
    match dest {
        Destination::File(path) => {
            write_key_file(&path, &key)?;
            eprintln!(
                "Wrote the private key to {} (owner read/write only).",
                path.display()
            );
        }
        Destination::Keychain(keychain) => {
            keychain.import(&key)?;
            eprintln!(
                "Imported the private key into Keychain account `{}`.",
                keychain.account()
            );
        }
    }
    print_public(&key.public_key());
    Ok(())
}

/// The public key of the private key named by the options' key source.
fn source_public_key(opts: &Options) -> Result<TrustedKey, Failure> {
    opts.reject_keychain_options()?;
    if opts.keychain {
        if !opts.sources.is_empty() {
            return usage("give exactly one key source");
        }
        return Ok(opts.keychain()?.public_key()?);
    }
    let key = opts.plain_source()?.read()?;
    note_if_legacy(&key);
    Ok(key.public_key())
}

fn public_key(opts: &Options) -> Result<(), Failure> {
    opts.reject(opts.test, "--test")?;
    opts.reject(opts.output.is_some(), "--output")?;
    opts.reject(opts.public_key.is_some(), "--public-key")?;
    opts.reject(opts.allow_test_key, "--allow-test-key")?;
    let public = source_public_key(opts)?;
    warn_if_test_key(&public);
    println!("{}", public.to_base64());
    Ok(())
}

fn note_if_legacy(key: &PrivateKey) {
    if key.is_legacy() {
        eprintln!(
            "note: this is a legacy 96-byte Sparkle key; it signs normally, but its seed cannot \
             be recovered"
        );
    }
}

fn check(opts: &Options) -> Result<(), Failure> {
    opts.reject(opts.test, "--test")?;
    opts.reject(opts.output.is_some(), "--output")?;
    let Some(configured) = &opts.public_key else {
        return usage("--public-key <SUPublicEDKey> is required");
    };
    let configured = TrustedKey::from_base64(configured)
        .map_err(|e| format!("configured public key is unusable: {e}"))?;
    let actual = source_public_key(opts)?;
    if actual != configured {
        return Err(Failure::Failed(format!(
            "private key does not match the app's public key\n  \
             configured public key:     {}\n  \
             private key's public key:  {}\n\
             Releases signed with this key would be rejected by every installed copy.",
            configured.to_base64(),
            actual.to_base64()
        )));
    }
    if actual.is_insecure_test_key() {
        if !opts.allow_test_key {
            return Err(Failure::Failed(
                "the key pair is the insecure, publicly known test key and must not sign \
                 production releases (pass --allow-test-key for local development)"
                    .into(),
            ));
        }
        warn_if_test_key(&actual);
    }
    println!(
        "ok: private key matches public key {}",
        configured.to_base64()
    );
    Ok(())
}

fn write_key_file(path: &std::path::Path, key: &PrivateKey) -> Result<(), Failure> {
    write_new_key_file(path, key).map_err(|e| {
        Failure::Failed(if e.kind() == std::io::ErrorKind::AlreadyExists {
            format!(
                "{} already exists; refusing to overwrite a key file",
                path.display()
            )
        } else {
            format!("failed to write {}: {e}", path.display())
        })
    })
}

fn print_public(key: &TrustedKey) {
    println!("{}", key.to_base64());
    eprintln!(
        "Configure this public key in the app (SUPublicEDKey on macOS). \
         Changing it later requires the rotation procedure in docs/key-management.md."
    );
}

fn warn_if_test_key(key: &TrustedKey) {
    if key.is_insecure_test_key() {
        eprintln!("warning: this is the INSECURE, publicly known test key; never ship it");
    }
}
