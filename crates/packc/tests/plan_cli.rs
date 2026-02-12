use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::prelude::*;
use greentic_types::cbor::canonical;
use greentic_types::schemas::common::schema_ir::{AdditionalProperties, SchemaIr};
use greentic_types::schemas::component::v0_6_0::{
    ComponentDescribe, ComponentInfo, ComponentOperation, ComponentRunInput, ComponentRunOutput,
    schema_hash,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
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
fn plan_outputs_json_for_gtpack() {
    let temp = TempDir::new().expect("temp dir");
    let gtpack = temp.path().join("demo.gtpack");
    let cache_dir = temp.path().join("cache");
    let pack_dir = workspace_root().join("examples/weather-demo");
    write_weather_summary(&pack_dir, &cache_dir);

    // Build a small pack to exercise the CLI.
    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "build",
            "--in",
            "examples/weather-demo",
            "--no-update",
            "--offline",
            "--cache-dir",
            cache_dir.to_str().unwrap(),
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
