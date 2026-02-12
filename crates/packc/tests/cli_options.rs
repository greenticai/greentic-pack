use assert_cmd::prelude::*;
use greentic_types::cbor::canonical;
use greentic_types::schemas::common::schema_ir::{AdditionalProperties, SchemaIr};
use greentic_types::schemas::component::v0_6_0::{
    ComponentDescribe, ComponentInfo, ComponentOperation, ComponentRunInput, ComponentRunOutput,
    schema_hash,
};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn write_describe_sidecar(wasm_path: &Path, component_id: &str, version: &str) {
    let input_schema = SchemaIr::String {
        min_len: None,
        max_len: None,
        regex: None,
        format: None,
    };
    let output_schema = SchemaIr::String {
        min_len: None,
        max_len: None,
        regex: None,
        format: None,
    };
    let config_schema = SchemaIr::Object {
        properties: BTreeMap::new(),
        required: Vec::new(),
        additional: AdditionalProperties::Forbid,
    };
    let hash = schema_hash(&input_schema, &output_schema, &config_schema).expect("schema hash");
    let operation = ComponentOperation {
        id: "run".to_string(),
        display_name: None,
        input: ComponentRunInput {
            schema: input_schema,
        },
        output: ComponentRunOutput {
            schema: output_schema,
        },
        defaults: BTreeMap::new(),
        redactions: Vec::new(),
        constraints: BTreeMap::new(),
        schema_hash: hash,
    };
    let describe = ComponentDescribe {
        info: ComponentInfo {
            id: component_id.to_string(),
            version: version.to_string(),
            role: "tool".to_string(),
            display_name: None,
        },
        provided_capabilities: Vec::new(),
        required_capabilities: Vec::new(),
        metadata: BTreeMap::new(),
        operations: vec![operation],
        config_schema,
    };
    let bytes = canonical::to_canonical_cbor_allow_floats(&describe).expect("encode describe");
    let describe_path = format!("{}.describe.cbor", wasm_path.display());
    std::fs::write(describe_path, bytes).expect("write describe cache");
}

fn write_weather_summary(pack_dir: &Path, _cache_dir: &Path) {
    let summary_path = pack_dir.join("flows/weather_bot.ygtc.resolve.summary.json");
    let parent = summary_path
        .parent()
        .expect("summary parent exists")
        .to_path_buf();
    std::fs::create_dir_all(&parent).expect("create flows dir");

    let qa_path = pack_dir.join("components/qa.process/component.wasm");
    write_describe_sidecar(&qa_path, "qa.process", "0.1.0");
    let qa_digest = format!(
        "sha256:{:x}",
        Sha256::digest(std::fs::read(&qa_path).unwrap())
    );

    let mcp_path = pack_dir.join("components/mcp.exec/component.wasm");
    write_describe_sidecar(&mcp_path, "mcp.exec", "0.1.0");
    let mcp_digest = format!(
        "sha256:{:x}",
        Sha256::digest(std::fs::read(&mcp_path).unwrap())
    );

    let templating_path = pack_dir.join("components/templating.handlebars/component.wasm");
    write_describe_sidecar(&templating_path, "templating.handlebars", "0.1.0");
    let templating_digest = format!(
        "sha256:{:x}",
        Sha256::digest(std::fs::read(&templating_path).unwrap())
    );

    let doc = serde_json::json!({
        "schema_version": 1,
        "flow": "weather_bot.ygtc",
        "nodes": {
            "collect_location": {
                "component_id": "qa.process",
                "source": { "kind": "local", "path": "../components/qa.process/component.wasm" },
                "digest": qa_digest
            },
            "forecast_weather": {
                "component_id": "mcp.exec",
                "source": { "kind": "local", "path": "../components/mcp.exec/component.wasm" },
                "digest": mcp_digest
            },
            "weather_text": {
                "component_id": "templating.handlebars",
                "source": { "kind": "local", "path": "../components/templating.handlebars/component.wasm" },
                "digest": templating_digest
            }
        }
    });
    std::fs::write(summary_path, serde_json::to_vec_pretty(&doc).unwrap()).expect("write summary");
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

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"));
    cmd.current_dir(workspace_root());
    let pack_dir = workspace_root().join("crates/packc/tests/fixtures/packs/valid-minimal");
    let fixture_wasm = pack_dir.join("components/fixture.wasm");
    write_describe_sidecar(&fixture_wasm, "dev.local.component", "0.1.0");
    cmd.args([
        "build",
        "--in",
        pack_dir.to_str().unwrap(),
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
    cmd.env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1");
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

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"));
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

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"));
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

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
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
fn greentic_pack_inspect_alias_warns_and_runs() {
    let temp = TempDir::new().expect("temp dir");
    let gtpack_out = temp.path().join("demo.gtpack");
    let cache_dir = temp.path().join("cache");
    let pack_dir = workspace_root().join("examples/weather-demo");
    write_weather_summary(&pack_dir, &cache_dir);

    let mut build_cmd = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"));
    build_cmd.current_dir(workspace_root()).args([
        "build",
        "--in",
        "examples/weather-demo",
        "--offline",
        "--cache-dir",
        cache_dir.to_str().unwrap(),
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
    build_cmd.env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1");
    build_cmd.assert().success();

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .args([
            "inspect",
            "--pack",
            gtpack_out.to_str().unwrap(),
            "--no-flow-doctor",
            "--no-component-doctor",
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

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
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
