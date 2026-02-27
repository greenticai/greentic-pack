use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::tempdir;
use walkdir::WalkDir;

const MSG_PROVIDER_MISSING: &str = "MSG_NO_PROVIDER_DECL";
const WORKSPACE_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn workspace_root() -> PathBuf {
    Path::new(WORKSPACE_ROOT).join("..").join("..")
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

fn copy_fixture(name: &str) -> (tempfile::TempDir, PathBuf) {
    let src = fixture_dir(name);
    let temp = tempdir().expect("tempdir");
    let dest = temp.path().join(name);
    fs::create_dir_all(&dest).expect("create destination");
    for entry in WalkDir::new(&src).into_iter().filter_map(Result::ok) {
        let relative = entry.path().strip_prefix(&src).expect("relative path");
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = dest.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).expect("create dir");
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::copy(entry.path(), &target).expect("copy file");
        }
    }
    (temp, dest)
}

fn add_schema(pack_dir: &Path, kind: &str, provider_id: &str) {
    let schema_dir = pack_dir.join("schemas").join(kind).join(provider_id);
    fs::create_dir_all(&schema_dir).expect("create schema dir");
    let schema_path = schema_dir.join("config.schema.json");
    fs::write(&schema_path, "{}").expect("write schema");
}

fn add_extension_and_run_validator(kind: &str, provider_id: &str, validator_ref: &str) {
    if std::env::var("GREENTIC_TEST_OCI").is_err() {
        eprintln!("GREENTIC_TEST_OCI not set; skipping extension validator smoke test");
        return;
    }

    let (_tmp, pack_dir) = copy_fixture("valid-minimal");
    add_schema(&pack_dir, kind, provider_id);

    let bin = assert_cmd::cargo::cargo_bin!("greentic-pack").to_path_buf();
    Command::new(bin.clone())
        .args([
            "add-extension",
            "provider",
            "--pack-dir",
            pack_dir.to_str().unwrap(),
            "--id",
            provider_id,
            "--kind",
            kind,
            "--title",
            "Smoke Provider",
            "--description",
            "Auto-added for validator tests",
            "--route",
            "diagnostics",
            "--flow",
            "main",
        ])
        .assert()
        .success();

    let mut doctor_cmd = Command::new(bin.clone());
    let output = doctor_cmd
        .args([
            "doctor",
            "--in",
            pack_dir.to_str().unwrap(),
            "--validate",
            "--validator-pack",
            validator_ref,
            "--no-flow-doctor",
            "--no-component-doctor",
            "--json",
        ])
        .output()
        .expect("run doctor");

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains(MSG_PROVIDER_MISSING),
        "provider error present:\n{combined}"
    );
}

#[test]
fn messaging_validator_smoke() {
    add_extension_and_run_validator(
        "messaging",
        "messaging.dummy",
        "oci://ghcr.io/greenticai/validators/messaging:latest",
    );
}

#[test]
fn events_validator_smoke() {
    add_extension_and_run_validator(
        "events",
        "events.dummy",
        "oci://ghcr.io/greenticai/validators/events:latest",
    );
}

#[test]
fn add_extension_updates_pack_yaml_in_place() {
    let (_tmp, pack_dir) = copy_fixture("valid-minimal");
    let bin = assert_cmd::cargo::cargo_bin!("greentic-pack");

    Command::new(bin)
        .args([
            "add-extension",
            "provider",
            "--pack-dir",
            pack_dir.to_str().unwrap(),
            "--id",
            "messaging.dummy",
            "--kind",
            "messaging",
            "--title",
            "Smoke Provider",
        ])
        .assert()
        .success();

    let updated = fs::read_to_string(pack_dir.join("pack.yaml")).expect("read pack.yaml");
    assert!(
        updated.contains("messaging.dummy"),
        "provider entry missing from pack.yaml"
    );
}

