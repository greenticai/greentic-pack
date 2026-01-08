use std::path::PathBuf;
use std::process::Command;

use assert_cmd::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

#[test]
fn plan_outputs_json_for_gtpack() {
    let temp = TempDir::new().expect("temp dir");
    let gtpack = temp.path().join("demo.gtpack");

    // Build a small pack to exercise the CLI.
    Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
        .args([
            "build",
            "--in",
            "examples/weather-demo",
            "--no-update",
            "--manifest",
            temp.path().join("manifest.cbor").to_str().unwrap(),
            "--out",
            temp.path().join("component.wasm").to_str().unwrap(),
            "--sbom",
            temp.path().join("sbom.cdx.json").to_str().unwrap(),
            "--gtpack-out",
            gtpack.to_str().unwrap(),
            "--log",
            "warn",
        ])
        .assert()
        .success();

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .args([
            "plan",
            gtpack.to_str().unwrap(),
            "--json",
            "--tenant",
            "demo-tenant",
            "--environment",
            "demo-env",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("plan json");
    assert_eq!(parsed["tenant"], "demo-tenant");
    assert_eq!(parsed["environment"], "demo-env");
}

#[test]
fn plan_errors_for_missing_pack() {
    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .args(["plan", "missing.gtpack"])
        .assert()
        .failure();
}
