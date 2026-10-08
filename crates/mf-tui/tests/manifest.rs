#![cfg(unix)]
mod common;

use common::{elf, macho, put16, put32, put64, sample_manifest};
use mf_runtime::{MAX_MANIFEST_SECTION_BYTES, WorkflowManifestError};
use mf_tui::manifest::{ManifestReadError, read_manifest};
use std::{error::Error, fs, io::Write, os::unix::fs::PermissionsExt};

fn read(bytes: &[u8]) -> Result<mf_runtime::WorkflowManifest, ManifestReadError> {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(bytes).unwrap();
    read_manifest(file.path())
}

#[test]
fn reads_both_formats_and_architectures_without_executable_permission() {
    let manifest = sample_manifest();
    let payload = manifest.to_bytes().unwrap();
    for mut bytes in [elf(Some(&payload), 1), macho(Some(&payload), 1)] {
        assert_eq!(read(&bytes).unwrap(), manifest.clone());
        if &bytes[..4] == b"\x7fELF" {
            put16(&mut bytes, 18, 183);
        } else {
            put32(&mut bytes, 4, 0x0100000c);
        }
        assert_eq!(read(&bytes).unwrap(), manifest.clone());
    }
    for bytes in [elf(None, 0), macho(None, 0)] {
        assert!(matches!(
            read(&bytes),
            Err(ManifestReadError::MissingManifest)
        ));
    }
    let file = tempfile::NamedTempFile::new().unwrap();
    fs::write(file.path(), elf(Some(&payload), 1)).unwrap();
    fs::set_permissions(file.path(), fs::Permissions::from_mode(0o600)).unwrap();
    file.as_file().set_len(512 * 1024 * 1024).unwrap();
    assert_eq!(read_manifest(file.path()).unwrap(), manifest);
}

#[test]
fn rejects_duplicate_sections_and_wrong_macho_segments() {
    let payload = sample_manifest().to_bytes().unwrap();
    for bytes in [elf(Some(&payload), 2), macho(Some(&payload), 2)] {
        assert!(matches!(
            read(&bytes),
            Err(ManifestReadError::InvalidSection)
        ));
    }
    let mut bytes = macho(Some(&payload), 1);
    bytes[40..46].copy_from_slice(b"__TEXT");
    assert!(matches!(
        read(&bytes),
        Err(ManifestReadError::InvalidSection)
    ));
    bytes = elf(Some(&payload), 1);
    put32(&mut bytes, 196, 8);
    assert!(matches!(
        read(&bytes),
        Err(ManifestReadError::InvalidSection)
    ));
}

#[test]
fn rejects_bad_ranges_and_bounds_metadata_and_payloads() {
    let payload = sample_manifest().to_bytes().unwrap();
    let mut bytes = elf(Some(&payload), 1);
    put64(&mut bytes, 216, u64::MAX);
    assert!(matches!(
        read(&bytes),
        Err(ManifestReadError::InvalidRange { .. })
    ));
    bytes = elf(Some(&payload), 1);
    put64(&mut bytes, 224, MAX_MANIFEST_SECTION_BYTES as u64 + 1);
    let file = tempfile::NamedTempFile::new().unwrap();
    fs::write(file.path(), bytes).unwrap();
    file.as_file()
        .set_len((MAX_MANIFEST_SECTION_BYTES + 4096) as u64)
        .unwrap();
    assert!(matches!(
        read_manifest(file.path()),
        Err(ManifestReadError::TooLarge { .. })
    ));
    bytes = macho(Some(&payload), 1);
    put32(&mut bytes, 20, 2 * 1024 * 1024);
    assert!(matches!(
        read(&bytes),
        Err(ManifestReadError::MetadataLimit)
    ));
    bytes = macho(Some(&payload), 1);
    put64(&mut bytes, 144, u64::MAX);
    assert!(read(&bytes).is_err());
    for bytes in [elf(Some(&payload), 1), macho(Some(&payload), 1)] {
        assert!(read(&bytes[..bytes.len() - 1]).is_err());
    }
}

#[test]
fn preserves_corrupt_manifest_and_io_causes_without_fallback() {
    for bytes in [elf(Some(b"bad"), 1), macho(Some(b"bad"), 1)] {
        let error = read(&bytes).unwrap_err();
        assert!(matches!(error, ManifestReadError::Manifest { .. }));
        assert!(error.source().unwrap().is::<WorkflowManifestError>());
    }
    let root = tempfile::tempdir().unwrap();
    let error = read_manifest(&root.path().join("missing")).unwrap_err();
    assert!(error.source().unwrap().is::<std::io::Error>());
    assert!(matches!(
        read_manifest(root.path()),
        Err(ManifestReadError::NotRegular { .. })
    ));
    for bytes in [
        b"#!/bin/sh\nexit 99\n".as_slice(),
        b"MZnot-a-supported-executable",
    ] {
        assert!(read(bytes).is_err());
    }
    let mut bytes = elf(None, 0);
    put16(&mut bytes, 16, 1);
    assert!(matches!(
        read(&bytes),
        Err(ManifestReadError::UnsupportedContainer)
    ));
}
