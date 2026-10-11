//! Reading the version resource (`VS_VERSIONINFO`) of a Windows PE image.
//!
//! The reader is portable Rust and touches only the headers, the resource
//! tree, and the version resource, with every offset bounds-checked and
//! every read size capped, so it is safe to run on any verified artifact
//! regardless of its size. It does not execute or map the image.

use std::io::{self, Read, Seek, SeekFrom};

use gpui_auto_update_core::feed::Arch;

const RT_VERSION: u32 = 16;
const RESOURCE_DIRECTORY_INDEX: usize = 2;
const MAX_RESOURCE_TREE_BYTES: u32 = 32 * 1024 * 1024;
const MAX_VERSION_RESOURCE_BYTES: u32 = 1024 * 1024;
const MAX_SECTIONS: u16 = 96;
const MAX_DIRECTORY_ENTRIES: usize = 4096;
const FIXED_FILE_INFO_SIGNATURE: u32 = 0xfeef_04bd;

/// The CPU architecture a PE image was built for (`IMAGE_FILE_HEADER.Machine`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Machine {
    /// `IMAGE_FILE_MACHINE_I386`; Inno Setup installers are always this,
    /// whatever architecture they install.
    X86,
    /// `IMAGE_FILE_MACHINE_AMD64`.
    X86_64,
    /// `IMAGE_FILE_MACHINE_ARM64`.
    Aarch64,
    /// Any other machine value.
    Other(u16),
}

impl Machine {
    fn from_raw(raw: u16) -> Self {
        match raw {
            0x014c => Self::X86,
            0x8664 => Self::X86_64,
            0xaa64 => Self::Aarch64,
            other => Self::Other(other),
        }
    }

    /// The feed architecture an executable of this machine type runs as
    /// natively, or `None` for machines no feed describes.
    pub fn arch(self) -> Option<Arch> {
        match self {
            Self::X86_64 => Some(Arch::X86_64),
            Self::Aarch64 => Some(Arch::Aarch64),
            _ => None,
        }
    }
}

/// Why a version resource could not be read.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PeError {
    /// The file does not start with DOS and PE headers.
    #[error("the file is not a Windows PE image")]
    NotPe,
    /// The image ended before a structure it declares.
    #[error("the PE image is truncated")]
    Truncated,
    /// A header or resource structure is inconsistent.
    #[error("the PE image is malformed: {0}")]
    Malformed(&'static str),
    /// The image has no `RT_VERSION` resource.
    #[error("the PE image has no version resource")]
    NoVersionResource,
    /// Reading the file failed.
    #[error("could not read the PE image: {0}")]
    Io(#[source] io::Error),
}

/// The parts of a PE image's version resource the updater relies on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionInfo {
    machine: Machine,
    strings: Vec<VersionString>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct VersionString {
    table: String,
    key: String,
    value: String,
}

impl VersionInfo {
    /// The image's machine type.
    pub fn machine(&self) -> Machine {
        self.machine
    }

    /// Every value of the `StringFileInfo` entry named `key` (for example
    /// `ProductVersion`), one per string table that has it, in file order.
    /// Key matching is exact.
    pub fn strings<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.strings
            .iter()
            .filter(move |s| s.key == key)
            .map(|s| s.value.as_str())
    }

    /// The string tables (language and code page keys such as
    /// `040904b0`) in file order, without duplicates.
    pub fn string_tables(&self) -> Vec<&str> {
        let mut tables: Vec<&str> = Vec::new();
        for s in &self.strings {
            if !tables.contains(&s.table.as_str()) {
                tables.push(&s.table);
            }
        }
        tables
    }
}

/// Reads the machine type and version strings of the PE image in `file`.
pub fn read_version_info(mut file: impl Read + Seek) -> Result<VersionInfo, PeError> {
    let headers = Headers::read(&mut file)?;
    let (tree_rva, tree_size) = headers
        .resource_directory
        .ok_or(PeError::NoVersionResource)?;
    if tree_size == 0 {
        return Err(PeError::NoVersionResource);
    }
    if tree_size > MAX_RESOURCE_TREE_BYTES {
        return Err(PeError::Malformed("resource table is too large"));
    }
    let tree = headers.read_rva(&mut file, tree_rva, tree_size)?;
    let mut leaves = Vec::new();
    find_version_leaves(&tree, &mut leaves)?;
    if leaves.is_empty() {
        return Err(PeError::NoVersionResource);
    }
    let mut strings = Vec::new();
    for leaf in leaves {
        let entry = slice(&tree, leaf, 16)?;
        let rva = u32_at(entry, 0);
        let size = u32_at(entry, 4);
        if size > MAX_VERSION_RESOURCE_BYTES {
            return Err(PeError::Malformed("version resource is too large"));
        }
        let data = headers.read_rva(&mut file, rva, size)?;
        parse_version_info(&data, &mut strings)?;
    }
    Ok(VersionInfo {
        machine: headers.machine,
        strings,
    })
}

struct Section {
    virtual_address: u32,
    virtual_size: u32,
    raw_offset: u32,
    raw_size: u32,
}

struct Headers {
    machine: Machine,
    resource_directory: Option<(u32, u32)>,
    sections: Vec<Section>,
}

impl Headers {
    fn read(file: &mut (impl Read + Seek)) -> Result<Self, PeError> {
        let dos = read_at(file, 0, 64).map_err(|_| PeError::NotPe)?;
        if &dos[0..2] != b"MZ" {
            return Err(PeError::NotPe);
        }
        let pe_offset = u64::from(u32_at(&dos, 0x3c));
        let signature = read_at(file, pe_offset, 4).map_err(|_| PeError::NotPe)?;
        if signature != b"PE\0\0" {
            return Err(PeError::NotPe);
        }
        let coff = read_at(file, pe_offset + 4, 20)?;
        let machine = Machine::from_raw(u16_at(&coff, 0));
        let section_count = u16_at(&coff, 2);
        let optional_size = u16_at(&coff, 16);
        if section_count > MAX_SECTIONS {
            return Err(PeError::Malformed("too many sections"));
        }
        let optional_offset = pe_offset + 24;
        let optional = read_at(file, optional_offset, usize::from(optional_size))?;
        if optional.len() < 2 {
            return Err(PeError::Malformed("optional header is missing"));
        }
        let (count_at, directories_at) = match u16_at(&optional, 0) {
            0x10b => (92, 96),
            0x20b => (108, 112),
            _ => return Err(PeError::Malformed("unknown optional header magic")),
        };
        let resource_directory = if optional.len() >= count_at + 4 {
            let count = u32_at(&optional, count_at) as usize;
            let at = directories_at + RESOURCE_DIRECTORY_INDEX * 8;
            (count > RESOURCE_DIRECTORY_INDEX && optional.len() >= at + 8)
                .then(|| (u32_at(&optional, at), u32_at(&optional, at + 4)))
        } else {
            None
        };
        let table = read_at(
            file,
            optional_offset + u64::from(optional_size),
            usize::from(section_count) * 40,
        )?;
        let sections = table
            .chunks_exact(40)
            .map(|header| Section {
                virtual_size: u32_at(header, 8),
                virtual_address: u32_at(header, 12),
                raw_size: u32_at(header, 16),
                raw_offset: u32_at(header, 20),
            })
            .collect();
        Ok(Self {
            machine,
            resource_directory,
            sections,
        })
    }

    /// Reads `len` bytes of the image mapped at `rva`, which must lie in the
    /// raw data of a single section.
    fn read_rva(
        &self,
        file: &mut (impl Read + Seek),
        rva: u32,
        len: u32,
    ) -> Result<Vec<u8>, PeError> {
        let section = self
            .sections
            .iter()
            .find(|s| {
                let span = s.virtual_size.max(s.raw_size);
                rva >= s.virtual_address
                    && u64::from(rva) < u64::from(s.virtual_address) + u64::from(span)
            })
            .ok_or(PeError::Malformed("address is outside every section"))?;
        let within = rva - section.virtual_address;
        if u64::from(within) + u64::from(len) > u64::from(section.raw_size) {
            return Err(PeError::Malformed("data extends past its section"));
        }
        read_at(
            file,
            u64::from(section.raw_offset) + u64::from(within),
            len as usize,
        )
    }
}

/// Collects the offsets (within the resource tree) of every data entry
/// under the `RT_VERSION` type, across all names and languages.
fn find_version_leaves(tree: &[u8], leaves: &mut Vec<usize>) -> Result<(), PeError> {
    let types = directory_entries(tree, 0)?;
    for (id, target) in types {
        if id != RT_VERSION {
            continue;
        }
        let names = subdirectory(target)?;
        for (_, target) in directory_entries(tree, names)? {
            let languages = subdirectory(target)?;
            for (_, target) in directory_entries(tree, languages)? {
                if target & 0x8000_0000 != 0 {
                    return Err(PeError::Malformed("resource tree is too deep"));
                }
                leaves.push(target as usize);
            }
        }
    }
    Ok(())
}

fn subdirectory(target: u32) -> Result<usize, PeError> {
    if target & 0x8000_0000 == 0 {
        return Err(PeError::Malformed("resource tree is too shallow"));
    }
    Ok((target & 0x7fff_ffff) as usize)
}

/// The `(name or id, target)` pairs of the resource directory at `offset`.
/// Named entries have their high bit set in the first value and never match
/// a numeric type id.
fn directory_entries(tree: &[u8], offset: usize) -> Result<Vec<(u32, u32)>, PeError> {
    let header = slice(tree, offset, 16)?;
    let count = usize::from(u16_at(header, 12)) + usize::from(u16_at(header, 14));
    if count > MAX_DIRECTORY_ENTRIES {
        return Err(PeError::Malformed(
            "resource directory has too many entries",
        ));
    }
    let entries = slice(tree, offset + 16, count * 8)?;
    Ok(entries
        .chunks_exact(8)
        .map(|entry| (u32_at(entry, 0), u32_at(entry, 4)))
        .collect())
}

/// One block of a version resource: its key, value bytes, and the span of
/// its children.
struct Block<'a> {
    key: String,
    kind: u16,
    value: &'a [u8],
    children: &'a [u8],
}

fn parse_block(data: &[u8]) -> Result<(Block<'_>, usize), PeError> {
    let header = slice(data, 0, 6)?;
    let length = usize::from(u16_at(header, 0));
    let value_length = usize::from(u16_at(header, 2));
    let kind = u16_at(header, 4);
    if length < 6 || length > data.len() {
        return Err(PeError::Malformed("version block length is out of range"));
    }
    let block = &data[..length];
    let (key, after_key) = utf16_until_nul(block, 6)?;
    let value_start = align4(after_key);
    let value_bytes = if kind == 1 {
        value_length * 2
    } else {
        value_length
    };
    let value_end = value_start.saturating_add(value_bytes).min(length);
    let value = block.get(value_start..value_end).unwrap_or(&[]);
    let children_start = align4(value_end).min(length);
    Ok((
        Block {
            key,
            kind,
            value,
            children: &block[children_start..],
        },
        length,
    ))
}

/// Calls `visit` for each child block in `children`.
fn for_each_child<'a>(
    mut children: &'a [u8],
    mut visit: impl FnMut(Block<'a>) -> Result<(), PeError>,
) -> Result<(), PeError> {
    while children.len() >= 6 {
        let (block, length) = parse_block(children)?;
        visit(block)?;
        let next = align4(length);
        if next >= children.len() {
            break;
        }
        children = &children[next..];
    }
    Ok(())
}

fn parse_version_info(data: &[u8], strings: &mut Vec<VersionString>) -> Result<(), PeError> {
    let (root, _) = parse_block(data)?;
    if root.key != "VS_VERSION_INFO" {
        return Err(PeError::Malformed("version resource has an unexpected key"));
    }
    if root.value.len() >= 4 && u32_at(root.value, 0) != FIXED_FILE_INFO_SIGNATURE {
        return Err(PeError::Malformed("fixed file info has a bad signature"));
    }
    for_each_child(root.children, |info| {
        if info.key != "StringFileInfo" {
            return Ok(());
        }
        for_each_child(info.children, |table| {
            let table_key = table.key;
            for_each_child(table.children, |string| {
                if string.kind != 1 {
                    return Ok(());
                }
                strings.push(VersionString {
                    table: table_key.clone(),
                    key: string.key,
                    value: utf16_value(string.value),
                });
                Ok(())
            })
        })
    })
}

/// Decodes a NUL-terminated UTF-16LE string starting at `start`; returns it
/// and the offset just past the terminator.
fn utf16_until_nul(data: &[u8], start: usize) -> Result<(String, usize), PeError> {
    let mut units = Vec::new();
    let mut at = start;
    loop {
        let pair = data
            .get(at..at + 2)
            .ok_or(PeError::Malformed("unterminated version key"))?;
        at += 2;
        let unit = u16::from_le_bytes([pair[0], pair[1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    let text =
        String::from_utf16(&units).map_err(|_| PeError::Malformed("version key is not UTF-16"))?;
    Ok((text, at))
}

/// Decodes a string value, which ends at its first NUL or at the end of the
/// value. Invalid UTF-16 is replaced rather than rejected; such a value can
/// never equal a valid version anyway.
fn utf16_value(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

fn read_at(file: &mut (impl Read + Seek), offset: u64, len: usize) -> Result<Vec<u8>, PeError> {
    file.seek(SeekFrom::Start(offset)).map_err(PeError::Io)?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            PeError::Truncated
        } else {
            PeError::Io(error)
        }
    })?;
    Ok(buf)
}

fn slice(data: &[u8], offset: usize, len: usize) -> Result<&[u8], PeError> {
    offset
        .checked_add(len)
        .and_then(|end| data.get(offset..end))
        .ok_or(PeError::Malformed("structure extends past its table"))
}

fn align4(value: usize) -> usize {
    value.saturating_add(3) & !3
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}
