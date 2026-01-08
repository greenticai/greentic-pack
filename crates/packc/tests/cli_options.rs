use assert_cmd::prelude::*;
use serde_json::Value as JsonValue;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

#[test]
fn build_writes_all_outputs_offline_with_cache_dir() {
    let temp = tempfile::Builder::new()
        .prefix("packc-cli-build")
        .tempdir_in(workspace_root())
        .expect("temp dir");
    let out_wasm = temp.path().join("out.wasm");
    let out_manifest = temp.path().join("manifest.cbor");
    let out_sbom = temp.path().join("sbom.cdx.json");
    let out_gtpack = temp.path().join("demo.gtpack");
    let cache_dir = temp.path().join("cache");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    cmd.current_dir(workspace_root());
    cmd.args([
        "build",
        "--in",
        "examples/weather-demo",
        "--out",
        out_wasm.to_str().unwrap(),
        "--manifest",
        out_manifest.to_str().unwrap(),
        "--sbom",
        out_sbom.to_str().unwrap(),
        "--gtpack-out",
        out_gtpack.to_str().unwrap(),
        "--cache-dir",
        cache_dir.to_str().unwrap(),
        "--offline",
        "--log",
        "warn",
    ]);
    cmd.assert().success();

    assert!(out_wasm.exists(), "component output missing");
    assert!(out_manifest.exists(), "manifest output missing");
    assert!(out_sbom.exists(), "sbom output missing");
    assert!(out_gtpack.exists(), "gtpack output missing");
}

#[test]
fn update_reports_counts_in_json() {
    let temp = tempfile::Builder::new()
        .prefix("packc-cli-update")
        .tempdir_in(workspace_root())
        .expect("temp dir");
    let pack_dir = temp.path().join("demo-pack");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    cmd.current_dir(workspace_root());
    cmd.args([
        "new",
        "demo-pack",
        "--dir",
        pack_dir.to_str().unwrap(),
        "--log",
        "warn",
    ]);
    cmd.assert().success();

    // Add one component and one flow.
    let components_dir = pack_dir.join("components");
    std::fs::write(components_dir.join("alpha.wasm"), [0x00, 0x61, 0x73, 0x6d]).unwrap();
    let flows_dir = pack_dir.join("flows");
    std::fs::write(
        flows_dir.join("extra.ygtc"),
        r#"id: extra
title: Extra
description: Extra flow
type: messaging
start: start

nodes:
  start:
    templating.handlebars:
      text: "hi"
    routing:
      - out: true
"#,
    )
    .unwrap();

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    cmd.current_dir(workspace_root());
    cmd.args([
        "update",
        "--in",
        pack_dir.to_str().unwrap(),
        "--json",
        "--log",
        "warn",
    ]);
    let output = cmd.output().expect("run packc update");
    assert!(
        output.status.success(),
        "update command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: JsonValue = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["components"]["added"], 1);
    assert_eq!(json["flows"]["added"], 1);
}

#[test]
fn config_respects_cli_override() {
    let temp = TempDir::new().expect("temp dir");
    let override_path = temp.path().join("override.toml");
    let custom_cache = temp.path().join("custom-cache");
    std::fs::write(
        &override_path,
        format!(
            r#" [paths]
cache_dir = "{}" "#,
            custom_cache.display()
        ),
    )
    .unwrap();

    let output = Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
        .args([
            "config",
            "--json",
            "--config-override",
            override_path.to_str().unwrap(),
        ])
        .output()
        .expect("run packc config");

    assert!(
        output.status.success(),
        "config command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let payload: JsonValue = serde_json::from_slice(&output.stdout).unwrap();
    let resolved_cache = payload
        .get("config")
        .and_then(|cfg| cfg.get("paths"))
        .and_then(|p| p.get("cache_dir"))
        .and_then(|c| c.as_str())
        .unwrap_or_default();
    assert!(
        resolved_cache.ends_with("custom-cache"),
        "cache_dir should reflect override"
    );
}

#[test]
fn packc_shim_emits_deprecation_warning() {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
        .args(["config", "--json", "--log", "warn"])
        .output()
        .expect("run packc config");

    assert!(
        output.status.success(),
        "packc config failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`packc` is deprecated"),
        "expected deprecation warning in stderr, got: {stderr}"
    );
}

#[test]
fn greentic_pack_inspect_alias_warns_and_runs() {
    let temp = TempDir::new().expect("temp dir");
    let gtpack_out = temp.path().join("demo.gtpack");

    let mut build_cmd = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    build_cmd.current_dir(workspace_root()).args([
        "build",
        "--in",
        "examples/weather-demo",
        "--no-update",
        "--out",
        temp.path().join("component.wasm").to_str().unwrap(),
        "--manifest",
        temp.path().join("manifest.cbor").to_str().unwrap(),
        "--sbom",
        temp.path().join("sbom.cdx.json").to_str().unwrap(),
        "--gtpack-out",
        gtpack_out.to_str().unwrap(),
        "--log",
        "warn",
    ]);
    build_cmd.assert().success();

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .args([
            "inspect",
            "--pack",
            gtpack_out.to_str().unwrap(),
            "--log",
            "warn",
        ])
        .output()
        .expect("run greentic-pack inspect");

    assert!(
        output.status.success(),
        "inspect alias failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`inspect` is deprecated"),
        "expected inspect alias warning in stderr, got: {stderr}"
    );
}

#[test]
fn gui_loveable_convert_cli_builds_gtpack() {
    let temp = TempDir::new().expect("temp dir");
    let assets_dir = temp.path().join("assets");
    std::fs::create_dir_all(&assets_dir).unwrap();
    std::fs::write(
        assets_dir.join("index.html"),
        "<html data-greentic-worker=\"worker.demo\">hi</html>",
    )
    .unwrap();

    let out = temp.path().join("demo.gtpack");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
        .args([
            "gui",
            "loveable-convert",
            "--pack-kind",
            "feature",
            "--id",
            "greentic.demo.gui",
            "--version",
            "0.1.0",
            "--pack-manifest-kind",
            "application",
            "--publisher",
            "greentic.gui",
            "--assets-dir",
            assets_dir.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--log",
            "warn",
        ])
        .output()
        .expect("run loveable-convert");

    assert!(
        output.status.success(),
        "loveable-convert CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out.exists(), "gtpack output missing");
}
