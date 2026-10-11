//! Inspection of built release artifacts: Linux managed-install archives and
//! Windows executables. Nothing is extracted to disk.

use std::io::Read as _;
use std::path::Path;

use gpui_auto_update_core::feed::Arch;

/// File name of the Linux ownership marker (docs/linux-managed-install.md).
pub const MARKER_FILE_NAME: &str = "gpui-auto-update.managed";

/// Exact marker contents for `app`, contract version 1.
pub fn marker_contents(app: &str) -> String {
    format!("gpui-auto-update managed-install 1\napp={app}\n")
}

const MAX_ENTRIES: usize = 100_000;
const MAX_MARKER_BYTES: u64 = 4096;

/// Checks a Linux release archive against the managed-install contract and
/// returns every problem found.
pub fn linux_archive(path: &Path, app: &str, version: &str, arch: Arch) -> Vec<String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) => return vec![format!("cannot open {}: {e}", path.display())],
    };
    let expected_root = format!("{app}-{version}-linux-{arch}");
    let bin = format!("bin/{app}");
    let marker = format!("share/{app}/{MARKER_FILE_NAME}");
    let mut roots: Vec<String> = Vec::new();
    let mut bin_ok = None;
    let mut marker_ok = None;
    let mut problems = Vec::new();

    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let entries = match archive.entries() {
        Ok(entries) => entries,
        Err(e) => return vec![format!("{} is not a gzip tar archive: {e}", path.display())],
    };
    for (index, entry) in entries.enumerate() {
        if index >= MAX_ENTRIES {
            problems.push(format!(
                "{} has more than {MAX_ENTRIES} entries",
                path.display()
            ));
            break;
        }
        let mut entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                problems.push(format!(
                    "{} is not a well-formed gzip tar archive: {e}",
                    path.display()
                ));
                return problems;
            }
        };
        let Ok(name) = entry.path().map(|p| p.to_string_lossy().into_owned()) else {
            problems.push(format!(
                "{} has an entry with an unreadable path",
                path.display()
            ));
            continue;
        };
        let trimmed = name.trim_end_matches('/');
        let (root, rest) = trimmed.split_once('/').unwrap_or((trimmed, ""));
        if !roots.iter().any(|r| r == root) {
            roots.push(root.to_owned());
        }
        if root != expected_root {
            continue;
        }
        if rest == bin {
            let mode = entry.header().mode().unwrap_or(0);
            bin_ok = Some(entry.header().entry_type().is_file() && mode & 0o111 != 0);
        } else if rest == marker {
            let mut contents = Vec::new();
            let read = (&mut entry)
                .take(MAX_MARKER_BYTES + 1)
                .read_to_end(&mut contents);
            marker_ok = Some(
                entry.header().entry_type().is_file()
                    && read.is_ok()
                    && contents == marker_contents(app).as_bytes(),
            );
        }
    }

    if roots.iter().all(|r| r != &expected_root) {
        let found = roots.join(", ");
        let other_arch = [Arch::X86_64, Arch::Aarch64]
            .into_iter()
            .filter(|a| *a != arch)
            .find(|a| roots.iter().any(|r| r.ends_with(&format!("-linux-{a}"))));
        problems.push(match other_arch {
            Some(other) => format!(
                "{} is built for {other} (release root {found}), but it is configured as the {arch} artifact",
                path.display()
            ),
            None => format!(
                "{} must contain exactly one top-level directory named {expected_root}, found {}",
                path.display(),
                if found.is_empty() { "nothing" } else { &found }
            ),
        });
        return problems;
    }
    if roots.len() > 1 {
        problems.push(format!(
            "{} must contain only the {expected_root} directory, found {}",
            path.display(),
            roots.join(", ")
        ));
    }
    match bin_ok {
        None => problems.push(format!(
            "{} has no {expected_root}/{bin}; the executable doubles as the update helper",
            path.display()
        )),
        Some(false) => problems.push(format!(
            "{expected_root}/{bin} in {} must be a regular executable file",
            path.display()
        )),
        Some(true) => {}
    }
    match marker_ok {
        None => problems.push(format!(
            "{} has no ownership marker {expected_root}/{marker}; without it the installation is not self-updated",
            path.display()
        )),
        Some(false) => problems.push(format!(
            "the ownership marker {expected_root}/{marker} in {} must contain exactly {:?}",
            path.display(),
            marker_contents(app)
        )),
        Some(true) => {}
    }
    problems
}

/// The machine a Windows PE image is built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeMachine {
    Known(Arch),
    X86,
    Other(u16),
}

impl std::fmt::Display for PeMachine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Known(arch) => write!(f, "{arch}"),
            Self::X86 => f.write_str("32-bit x86"),
            Self::Other(machine) => write!(f, "machine type {machine:#06x}"),
        }
    }
}

/// Reads the COFF machine type of a PE executable.
pub fn pe_machine(path: &Path) -> Result<PeMachine, String> {
    let mut head = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(4096).read_to_end(&mut head))
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let not_pe = || format!("{} is not a Windows executable (PE image)", path.display());
    if !head.starts_with(b"MZ") || head.len() < 0x40 {
        return Err(not_pe());
    }
    let offset = u32::from_le_bytes(head[0x3c..0x40].try_into().unwrap()) as usize;
    let header = head.get(offset..offset + 6).ok_or_else(not_pe)?;
    if &header[..4] != b"PE\0\0" {
        return Err(not_pe());
    }
    Ok(match u16::from_le_bytes([header[4], header[5]]) {
        0x8664 => PeMachine::Known(Arch::X86_64),
        0xaa64 => PeMachine::Known(Arch::Aarch64),
        0x014c => PeMachine::X86,
        other => PeMachine::Other(other),
    })
}
