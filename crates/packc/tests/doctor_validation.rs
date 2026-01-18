use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::prelude::*;
use greentic_types::{PackManifest, encode_pack_manifest};
use serde_json::Value;
use zip::CompressionMethod;
use zip::ZipWriter;
use zip::write::FileOptions;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn fixture_dir(name: &str) -> PathBuf {
    workspace_root()
        .join("crates")
        .join("packc")
        .join("tests")
        .join("fixtures")
        .join("packs")
        .join(name)
}

#[test]
fn doctor_json_includes_validation() {
    let pack_dir = fixture_dir("valid-minimal");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .args(["doctor", pack_dir.to_str().unwrap(), "--json"])
        .output()
        .expect("run doctor");
    assert!(output.status.success(), "doctor should succeed");

    let payload: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(
        payload.get("validation").is_some(),
        "doctor --json should include validation report"
    );
}

#[test]
fn doctor_fails_on_missing_provider_schema() {
    let pack_dir = fixture_dir("missing-provider-schema");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .args(["doctor", pack_dir.to_str().unwrap(), "--json"])
        .output()
        .expect("run doctor");
    assert!(
        !output.status.success(),
        "doctor should fail when validation errors exist"
    );

    let payload: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let diagnostics = payload
        .get("validation")
        .and_then(|val| val.get("diagnostics"))
        .and_then(|val| val.as_array())
        .expect("validation diagnostics present");
    assert!(
        diagnostics.iter().any(|diag| {
            diag.get("code")
                .and_then(|val| val.as_str())
                .map(|code| code == "PACK_MISSING_FILE")
                .unwrap_or(false)
                && diag
                    .get("path")
                    .and_then(|val| val.as_str())
                    .map(|path| path == "schemas/messaging/demo/config.schema.json")
                    .unwrap_or(false)
        }),
        "expected missing provider schema diagnostic"
    );
}

#[test]
fn doctor_reports_sbom_dangling_path() {
    let temp = tempfile::tempdir().expect("temp dir");
    let pack_path = temp.path().join("sbom-dangling.gtpack");
    write_gtpack_from_fixture(&fixture_dir("sbom-dangling"), &pack_path);

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .args(["doctor", "--pack", pack_path.to_str().unwrap(), "--json"])
        .output()
        .expect("run doctor");
    assert!(
        !output.status.success(),
        "doctor should fail for dangling SBOM paths"
    );

    let payload: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let diagnostics = payload
        .get("validation")
        .and_then(|val| val.get("diagnostics"))
        .and_then(|val| val.as_array())
        .expect("validation diagnostics present");
    assert!(
        diagnostics.iter().any(|diag| {
            diag.get("code")
                .and_then(|val| val.as_str())
                .map(|code| code == "PACK_SBOM_DANGLING_PATH")
                .unwrap_or(false)
                && diag
                    .get("path")
                    .and_then(|val| val.as_str())
                    .map(|path| path == "missing.txt")
                    .unwrap_or(false)
        }),
        "expected dangling SBOM diagnostic"
    );
}

fn write_gtpack_from_fixture(fixture: &Path, dest: &Path) {
    let manifest_bytes = fs::read(fixture.join("manifest.json")).expect("read manifest fixture");
    let manifest: PackManifest =
        serde_json::from_slice(&manifest_bytes).expect("parse manifest fixture");
    let manifest_cbor = encode_pack_manifest(&manifest).expect("encode manifest");
    let sbom_bytes = fs::read(fixture.join("sbom.json")).expect("read sbom fixture");

    let dest_file = File::create(dest).expect("create pack");
    let mut writer = ZipWriter::new(dest_file);
    let options = FileOptions::default().compression_method(CompressionMethod::Stored);

    writer
        .start_file("manifest.cbor", options)
        .expect("start manifest entry");
    writer
        .write_all(&manifest_cbor)
        .expect("write manifest entry");

    writer
        .start_file("sbom.json", options)
        .expect("start sbom entry");
    writer.write_all(&sbom_bytes).expect("write sbom entry");

    writer.finish().expect("finish pack");
}
