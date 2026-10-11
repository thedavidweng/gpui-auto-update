//! Just enough Mach-O parsing to read the dynamic-library and run-path load
//! commands of an executable, including universal (fat) binaries. Layouts
//! follow `<mach-o/loader.h>` and `<mach-o/fat.h>`.

use anyhow::{Result, bail};

const MH_MAGIC: u32 = 0xfeed_face;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_MAGIC_64: u32 = 0xcafe_babf;

const LC_REQ_DYLD: u32 = 0x8000_0000;
const LC_LOAD_DYLIB: u32 = 0xc;
const LC_LAZY_LOAD_DYLIB: u32 = 0x20;
const LC_LOAD_WEAK_DYLIB: u32 = 0x18 | LC_REQ_DYLD;
const LC_RPATH: u32 = 0x1c | LC_REQ_DYLD;
const LC_REEXPORT_DYLIB: u32 = 0x1f | LC_REQ_DYLD;
const LC_LOAD_UPWARD_DYLIB: u32 = 0x23 | LC_REQ_DYLD;

const CPU_TYPE_X86_64: u32 = 0x0100_0007;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;

/// Load commands of one architecture slice.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Slice {
    pub arch: String,
    pub dylibs: Vec<String>,
    pub rpaths: Vec<String>,
}

pub fn parse(bytes: &[u8]) -> Result<Vec<Slice>> {
    let Some(be_magic) = read_u32_be(bytes, 0) else {
        bail!("file is too short to be a Mach-O binary");
    };
    match be_magic {
        FAT_MAGIC | FAT_MAGIC_64 => parse_fat(bytes, be_magic == FAT_MAGIC_64),
        _ => Ok(vec![parse_thin(bytes)?]),
    }
}

fn parse_fat(bytes: &[u8], wide: bool) -> Result<Vec<Slice>> {
    let count = read_u32_be(bytes, 4).unwrap_or(0) as usize;
    let entry_size = if wide { 32 } else { 20 };
    if count == 0 || count > 64 {
        bail!("universal binary has an implausible slice count {count}");
    }
    let mut slices = Vec::with_capacity(count);
    for i in 0..count {
        let at = 8 + i * entry_size;
        let (offset, size) = if wide {
            (read_u64_be(bytes, at + 8), read_u64_be(bytes, at + 16))
        } else {
            (
                read_u32_be(bytes, at + 8).map(u64::from),
                read_u32_be(bytes, at + 12).map(u64::from),
            )
        };
        let (Some(offset), Some(size)) = (offset, size) else {
            bail!("universal binary header is truncated");
        };
        let start = usize::try_from(offset)?;
        let end = start
            .checked_add(usize::try_from(size)?)
            .filter(|&end| end <= bytes.len());
        let Some(end) = end else {
            bail!("universal binary slice {i} lies outside the file");
        };
        slices.push(parse_thin(&bytes[start..end])?);
    }
    Ok(slices)
}

fn parse_thin(bytes: &[u8]) -> Result<Slice> {
    let header_size = match read_u32_le(bytes, 0) {
        Some(MH_MAGIC_64) => 32,
        Some(MH_MAGIC) => 28,
        _ => bail!("not a Mach-O binary"),
    };
    let (Some(cpu), Some(ncmds), Some(sizeofcmds)) = (
        read_u32_le(bytes, 4),
        read_u32_le(bytes, 16),
        read_u32_le(bytes, 20),
    ) else {
        bail!("Mach-O header is truncated");
    };
    let end = header_size + sizeofcmds as usize;
    if end > bytes.len() {
        bail!("Mach-O load commands extend past the end of the file");
    }
    let mut slice = Slice {
        arch: match cpu {
            CPU_TYPE_ARM64 => "arm64".to_owned(),
            CPU_TYPE_X86_64 => "x86_64".to_owned(),
            other => format!("cputype {other:#x}"),
        },
        ..Slice::default()
    };
    let mut at = header_size;
    for _ in 0..ncmds {
        let (Some(cmd), Some(size)) = (read_u32_le(bytes, at), read_u32_le(bytes, at + 4)) else {
            bail!("Mach-O load command is truncated");
        };
        let size = size as usize;
        if size < 8 || at + size > end {
            bail!("Mach-O load command has an invalid size");
        }
        let command = &bytes[at..at + size];
        match cmd {
            LC_LOAD_DYLIB | LC_LOAD_WEAK_DYLIB | LC_REEXPORT_DYLIB | LC_LAZY_LOAD_DYLIB
            | LC_LOAD_UPWARD_DYLIB => slice.dylibs.push(command_string(command)?),
            LC_RPATH => slice.rpaths.push(command_string(command)?),
            _ => {}
        }
        at += size;
    }
    Ok(slice)
}

/// The NUL-terminated string whose offset is stored at byte 8 of `dylib`
/// and `rpath` commands.
fn command_string(command: &[u8]) -> Result<String> {
    let Some(offset) = read_u32_le(command, 8) else {
        bail!("Mach-O load command is truncated");
    };
    let Some(raw) = command.get(offset as usize..) else {
        bail!("Mach-O load command string lies outside the command");
    };
    let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    Ok(String::from_utf8_lossy(&raw[..len]).into_owned())
}

fn read_u32_le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn read_u32_be(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn read_u64_be(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        b.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 64-bit little-endian image with the given load commands, laid out
    /// as in `<mach-o/loader.h>`.
    fn thin(cpu: u32, commands: &[(u32, &str)]) -> Vec<u8> {
        let mut cmds = Vec::new();
        for &(cmd, s) in commands {
            let header = if cmd == LC_RPATH { 12 } else { 24 };
            let size = (header + s.len() + 1).next_multiple_of(8);
            let mut c = vec![0u8; size];
            c[0..4].copy_from_slice(&cmd.to_le_bytes());
            c[4..8].copy_from_slice(&(size as u32).to_le_bytes());
            c[8..12].copy_from_slice(&(header as u32).to_le_bytes());
            c[header..header + s.len()].copy_from_slice(s.as_bytes());
            cmds.extend(c);
        }
        let mut out = Vec::new();
        out.extend(MH_MAGIC_64.to_le_bytes());
        out.extend(cpu.to_le_bytes());
        out.extend(0u32.to_le_bytes()); // cpusubtype
        out.extend(2u32.to_le_bytes()); // MH_EXECUTE
        out.extend((commands.len() as u32).to_le_bytes());
        out.extend((cmds.len() as u32).to_le_bytes());
        out.extend(0u32.to_le_bytes()); // flags
        out.extend(0u32.to_le_bytes()); // reserved
        out.extend(cmds);
        out
    }

    #[test]
    fn reads_dylibs_and_rpaths_from_a_thin_binary() {
        let bin = thin(
            CPU_TYPE_ARM64,
            &[
                (LC_LOAD_DYLIB, "@rpath/Sparkle.framework/Versions/B/Sparkle"),
                (0x19, ""), // LC_SEGMENT_64, ignored
                (LC_RPATH, "@executable_path/../Frameworks"),
                (LC_LOAD_WEAK_DYLIB, "/usr/lib/libSystem.B.dylib"),
            ],
        );
        let slices = parse(&bin).unwrap();
        assert_eq!(
            slices,
            [Slice {
                arch: "arm64".into(),
                dylibs: vec![
                    "@rpath/Sparkle.framework/Versions/B/Sparkle".into(),
                    "/usr/lib/libSystem.B.dylib".into()
                ],
                rpaths: vec!["@executable_path/../Frameworks".into()],
            }]
        );
    }

    #[test]
    fn reads_every_slice_of_a_universal_binary() {
        let arm = thin(CPU_TYPE_ARM64, &[(LC_RPATH, "@loader_path/../Frameworks")]);
        let x86 = thin(CPU_TYPE_X86_64, &[]);
        let mut fat = Vec::new();
        fat.extend(FAT_MAGIC.to_be_bytes());
        fat.extend(2u32.to_be_bytes());
        let first = 4096u32;
        let second = first + arm.len() as u32;
        for (cpu, off, len) in [
            (CPU_TYPE_ARM64, first, arm.len()),
            (CPU_TYPE_X86_64, second, x86.len()),
        ] {
            fat.extend(cpu.to_be_bytes());
            fat.extend(0u32.to_be_bytes());
            fat.extend(off.to_be_bytes());
            fat.extend((len as u32).to_be_bytes());
            fat.extend(12u32.to_be_bytes());
        }
        fat.resize(first as usize, 0);
        fat.extend(&arm);
        fat.extend(&x86);

        let slices = parse(&fat).unwrap();
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].arch, "arm64");
        assert_eq!(slices[0].rpaths, ["@loader_path/../Frameworks"]);
        assert_eq!(slices[1].arch, "x86_64");
        assert!(slices[1].rpaths.is_empty());
    }

    #[test]
    fn rejects_scripts_and_truncated_images() {
        assert!(parse(b"#!/bin/sh\nexit 0\n").is_err());
        let mut bin = thin(CPU_TYPE_ARM64, &[(LC_RPATH, "@executable_path")]);
        bin.truncate(40);
        assert!(parse(&bin).is_err());
        assert!(parse(&[]).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_a_system_binary() {
        let bytes = std::fs::read("/bin/ls").unwrap();
        let slices = parse(&bytes).unwrap();
        assert!(!slices.is_empty());
        assert!(
            slices
                .iter()
                .all(|s| s.dylibs.iter().any(|d| d.starts_with("/usr/lib/libSystem")))
        );
    }
}
