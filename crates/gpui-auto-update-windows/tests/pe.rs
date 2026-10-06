//! Reading the version resource embedded in Windows executables, which is
//! how a verified artifact confirms the release its unsigned feed entry
//! claims.

mod support;

use std::io::Cursor;

use gpui_auto_update_core::ErrorKind;
use gpui_auto_update_core::feed::Arch;
use gpui_auto_update_core::version::ReleaseVersion;
use gpui_auto_update_windows::confirm_embedded_version;
use gpui_auto_update_windows::pe::{Machine, PeError, read_version_info};
use support::{MACHINE_AMD64, MACHINE_ARM64, PeImage};

fn read(image: &PeImage) -> Result<gpui_auto_update_windows::pe::VersionInfo, PeError> {
    read_version_info(Cursor::new(image.build()))
}

#[test]
fn reads_product_version_and_machine_of_an_installer() {
    let info = read(&PeImage::installer("1.5.0")).unwrap();
    assert_eq!(info.machine(), Machine::X86);
    assert_eq!(info.machine().arch(), None);
    assert_eq!(
        info.strings("ProductVersion").collect::<Vec<_>>(),
        ["1.5.0"]
    );
    assert_eq!(info.strings("FileVersion").collect::<Vec<_>>(), ["1.5.0.0"]);
    assert_eq!(info.strings("Missing").count(), 0);
}

#[test]
fn reads_pe32_plus_executables_for_both_architectures() {
    let x64 = read(&PeImage::executable(MACHINE_AMD64, "2.0.0-beta.1")).unwrap();
    assert_eq!(x64.machine(), Machine::X86_64);
    assert_eq!(x64.machine().arch(), Some(Arch::X86_64));
    assert_eq!(
        x64.strings("ProductVersion").collect::<Vec<_>>(),
        ["2.0.0-beta.1"]
    );

    let arm = read(&PeImage::executable(MACHINE_ARM64, "2.0.0")).unwrap();
    assert_eq!(arm.machine(), Machine::Aarch64);
    assert_eq!(arm.machine().arch(), Some(Arch::Aarch64));
}

#[test]
fn reads_every_string_table() {
    let image = PeImage::installer("1.5.0").with_tables(vec![
        ("040904b0", vec![("ProductVersion", "1.5.0")]),
        ("040704b0", vec![("ProductVersion", "1.4.0")]),
    ]);
    let info = read(&image).unwrap();
    assert_eq!(
        info.strings("ProductVersion").collect::<Vec<_>>(),
        ["1.5.0", "1.4.0"]
    );
}

#[test]
fn rejects_files_that_are_not_pe_images() {
    assert!(matches!(
        read_version_info(Cursor::new(b"#!/bin/sh\necho hi\n".to_vec())),
        Err(PeError::NotPe)
    ));
    assert!(matches!(
        read_version_info(Cursor::new(Vec::new())),
        Err(PeError::NotPe)
    ));
    let mut bad_signature = PeImage::installer("1.5.0").build();
    bad_signature[0x40] = b'X';
    assert!(matches!(
        read_version_info(Cursor::new(bad_signature)),
        Err(PeError::NotPe)
    ));
}

#[test]
fn rejects_truncated_images() {
    let image = PeImage::installer("1.5.0").build();
    for len in [0x41, 0x100, 0x210, image.len() - 0x180] {
        let truncated = image[..len].to_vec();
        assert!(
            read_version_info(Cursor::new(truncated)).is_err(),
            "accepted an image truncated to {len} bytes"
        );
    }
}

#[test]
fn reports_a_missing_version_resource() {
    assert!(matches!(
        read(&PeImage::installer("1.5.0").without_version()),
        Err(PeError::NoVersionResource)
    ));
}

#[test]
fn rejects_out_of_range_resource_offsets() {
    let mut image = PeImage::installer("1.5.0").build();
    // The language-level entry of the resource tree points far outside it.
    let entry = 0x200 + 48 + 16 + 4;
    image[entry..entry + 4].copy_from_slice(&0x0fff_0000u32.to_le_bytes());
    assert!(read_version_info(Cursor::new(image)).is_err());
}

#[test]
fn confirms_the_embedded_version_against_the_feed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("setup.exe");
    std::fs::write(&path, PeImage::installer("1.5.0").build()).unwrap();
    let expected = ReleaseVersion::parse("1.5.0").unwrap();
    assert_eq!(
        confirm_embedded_version(&path, "ProductVersion", &expected),
        Ok(())
    );

    let newer = ReleaseVersion::parse("1.6.0").unwrap();
    let error = confirm_embedded_version(&path, "ProductVersion", &newer).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ArchiveValidation);
    let diagnostic = error.diagnostic().unwrap();
    assert!(diagnostic.contains("1.5.0") && diagnostic.contains("1.6.0"));
}

#[test]
fn confirmation_requires_the_version_string_to_be_present_and_unanimous() {
    let dir = tempfile::tempdir().unwrap();
    let expected = ReleaseVersion::parse("1.5.0").unwrap();
    let cases = [
        PeImage::installer("1.5.0").with_tables(vec![("040904b0", vec![("FileVersion", "1.5.0")])]),
        PeImage::installer("1.5.0").with_tables(vec![]),
        PeImage::installer("1.5.0").with_tables(vec![
            ("040904b0", vec![("ProductVersion", "1.5.0")]),
            ("040704b0", vec![("ProductVersion", "1.0.0")]),
        ]),
        PeImage::installer("1.5.0").without_version(),
        PeImage::installer("v1.5.0"),
        PeImage::installer("1.5.0.0"),
    ];
    for (index, image) in cases.iter().enumerate() {
        let path = dir.path().join(format!("case-{index}.exe"));
        std::fs::write(&path, image.build()).unwrap();
        let error = confirm_embedded_version(&path, "ProductVersion", &expected)
            .expect_err(&format!("case {index} was accepted"));
        assert_eq!(error.kind(), ErrorKind::ArchiveValidation, "case {index}");
    }
    // Machine type is irrelevant to version confirmation.
    let path = dir.path().join("arm.exe");
    std::fs::write(&path, PeImage::executable(MACHINE_ARM64, "1.5.0").build()).unwrap();
    assert_eq!(
        confirm_embedded_version(&path, "ProductVersion", &expected),
        Ok(())
    );
}
