use assert_cmd::prelude::*;
use greentic_types::cbor::canonical;
use greentic_types::schemas::common::schema_ir::{AdditionalProperties, SchemaIr};
use greentic_types::schemas::component::v0_6_0::{
    ComponentDescribe, ComponentInfo, ComponentOperation, ComponentRunInput, ComponentRunOutput,
    schema_hash,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

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

    let doc = json!({
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
fn readme_demo_build_and_doctor() {
    let pack_dir = workspace_root().join("examples/weather-demo");
    let cache_dir = tempfile::tempdir().expect("cache dir");
    write_weather_summary(&pack_dir, cache_dir.path());
    let temp = tempfile::tempdir().expect("temp dir");
    let gtpack_out = temp.path().join("weather-demo.gtpack");

    let mut build_cmd = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"));
    build_cmd.current_dir(workspace_root()).args([
        "build",
        "--in",
        "examples/weather-demo",
        "--offline",
        "--cache-dir",
        cache_dir.path().to_str().unwrap(),
        "--gtpack-out",
        gtpack_out.to_str().unwrap(),
        "--log",
        "warn",
    ]);
    build_cmd.env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1");
    build_cmd.assert().success();

    let mut doctor_cmd = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"));
    doctor_cmd.current_dir(workspace_root()).args([
        "doctor",
        gtpack_out.to_str().unwrap(),
        "--json",
        "--no-flow-doctor",
        "--no-component-doctor",
    ]);
    doctor_cmd.assert().success();
}
