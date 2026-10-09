//! `open_pack` requires a canonical pack, and always will — `verify`, `sign`,
//! `plan` and the provider paths are right to demand one. What it must not do
//! is describe every other archive as a missing file. These tests pin the
//! shape-aware error text.

use std::io::Write;
use std::path::{Path, PathBuf};

use greentic_pack::reader::{SigningPolicy, open_pack};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;

/// The message that started this: it made a healthy designer export look
/// corrupt, and must never come back verbatim.
const OLD_MESSAGE: &str = "manifest.cbor missing from archive";

fn write_zip(dir: &Path, name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).expect("create archive");
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (entry, bytes) in entries {
        zip.start_file(*entry, options).expect("start entry");
        zip.write_all(bytes).expect("write entry");
    }
    zip.finish().expect("finish archive");
    path
}

fn open_err(path: &Path) -> String {
    match open_pack(path, SigningPolicy::DevOk) {
        Ok(_) => panic!("canonical open must fail for a non-canonical archive"),
        Err(result) => result.message,
    }
}

#[test]
fn dw_application_pack_is_named_not_called_corrupt() {
    let temp = TempDir::new().expect("temp dir");
    let path = write_zip(
        temp.path(),
        "dw.gtpack",
        &[
            ("manifest.json", br#"{"manifest_id":"x","manifest":{}}"#),
            (
                "metadata.json",
                br#"{"pack_id":"p","kind":"DwApplication"}"#,
            ),
        ],
    );

    let message = open_err(&path);

    assert!(
        message.contains("DW application pack"),
        "error must name the shape it found, got: {message}"
    );
    assert!(
        message.contains("manifest.json"),
        "error must name the entry that decided the shape, got: {message}"
    );
    assert!(
        message.contains("greentic-pack doctor"),
        "error must point at the command that can inspect it, got: {message}"
    );
    assert!(
        !message.contains(OLD_MESSAGE),
        "the old missing-file message must not resurface, got: {message}"
    );
    assert!(
        !message.to_lowercase().contains("corrupt"),
        "a valid DW pack must never be described as corrupt, got: {message}"
    );
}

#[test]
fn unrecognised_archive_still_fails_loudly() {
    let temp = TempDir::new().expect("temp dir");
    let path = write_zip(temp.path(), "mystery.gtpack", &[("README.txt", b"hello")]);

    let message = open_err(&path);

    assert!(
        message.contains("no other known pack shape matched"),
        "error must say the archive matched nothing, got: {message}"
    );
    assert!(
        message.contains("greentic-pack doctor"),
        "error must point at doctor for details, got: {message}"
    );
}

/// The nested-`manifest.json` trap: an archive whose only JSON manifest lives
/// under `assets/i18n/` is not a DW pack and must not be described as one.
#[test]
fn nested_manifest_json_is_not_a_dw_pack() {
    let temp = TempDir::new().expect("temp dir");
    let path = write_zip(
        temp.path(),
        "i18n.gtpack",
        &[("assets/i18n/_manifest.json", br#"["en"]"#)],
    );

    let message = open_err(&path);

    assert!(
        !message.contains("DW application pack"),
        "a nested _manifest.json must not be mistaken for a DW pack, got: {message}"
    );
    assert!(
        message.contains("no other known pack shape matched"),
        "expected the unrecognised message, got: {message}"
    );
}

/// Not-a-zip is a different failure with a different fix, and keeps its own
/// message. Shape awareness must not swallow it.
#[test]
fn non_zip_keeps_its_own_message() {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("garbage.gtpack");
    std::fs::write(&path, b"not a zip at all").expect("write garbage");

    let message = open_err(&path);

    assert!(
        message.contains("is not a valid gtpack archive"),
        "expected the existing invalid-archive message, got: {message}"
    );
}
