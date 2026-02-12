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
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn write_summary(path: &Path, flow: &str, nodes: &[(&str, serde_json::Value)]) {
    let summary_path = path.join(flow);
    if let Some(parent) = summary_path.parent() {
        fs::create_dir_all(parent).expect("create summary dir");
    }
    let nodes_json: serde_json::Map<_, _> = nodes
        .iter()
        .map(|(name, node)| ((*name).to_string(), node.clone()))
        .collect();
    let flow_name = summary_path.file_name().unwrap().to_string_lossy();
    let flow_name = flow_name
        .strip_suffix(".resolve.summary.json")
        .unwrap_or(&flow_name);
    let doc = json!({
        "schema_version": 1,
        "flow": flow_name,
        "nodes": nodes_json
    });
    fs::write(
        summary_path,
        serde_json::to_vec_pretty(&doc).expect("serialize summary"),
    )
    .expect("write summary");
}

fn digest_for(path: &Path) -> String {
    let bytes = fs::read(path).expect("read artifact for digest");
    format!("sha256:{:x}", Sha256::digest(bytes))
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
    fs::write(describe_path, bytes).expect("write describe cache");
}

fn cache_component(cache_dir: &Path, component_id: &str) -> String {
    let bytes = format!("component-{component_id}").into_bytes();
    let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
    let dir = cache_dir.join(digest.trim_start_matches("sha256:"));
    fs::create_dir_all(&dir).expect("cache dir");
    let wasm_path = dir.join("component.wasm");
    fs::write(&wasm_path, &bytes).expect("write cached component");
    write_describe_sidecar(&wasm_path, component_id, "0.1.0");
    digest
}

#[test]
fn build_all_examples_manifest_only() {
    let packs = [
        "examples/weather-demo",
        "examples/qa-demo",
        "examples/billing-demo",
        "examples/search-demo",
        "examples/reco-demo",
    ];
    let temp = tempfile::tempdir().expect("cache dir");
    let cache_dir = temp.path().join("cache");

    for pack in packs {
        let pack_dir = workspace_root().join(pack);
        match pack {
            "examples/weather-demo" => {
                let summary_path = PathBuf::from("flows/weather_bot.ygtc.resolve.summary.json");
                let templating = pack_dir.join("components/templating.handlebars/component.wasm");
                write_describe_sidecar(&templating, "templating.handlebars", "0.1.0");
                let qa = pack_dir.join("components/qa.process/component.wasm");
                write_describe_sidecar(&qa, "qa.process", "0.1.0");
                let qa_digest = digest_for(&qa);
                let mcp = pack_dir.join("components/mcp.exec/component.wasm");
                write_describe_sidecar(&mcp, "mcp.exec", "0.1.0");
                let mcp_digest = digest_for(&mcp);
                write_summary(
                    &pack_dir,
                    summary_path.to_str().unwrap(),
                    &[
                        (
                            "collect_location",
                            json!({
                                "component_id": "qa.process",
                                "source": {"kind": "local", "path": "../components/qa.process/component.wasm"},
                                "digest": qa_digest
                            }),
                        ),
                        (
                            "forecast_weather",
                            json!({
                                "component_id": "mcp.exec",
                                "source": {"kind": "local", "path": "../components/mcp.exec/component.wasm"},
                                "digest": mcp_digest
                            }),
                        ),
                        (
                            "weather_text",
                            json!({
                                "component_id": "templating.handlebars",
                                "source": {"kind": "local", "path": "../components/templating.handlebars/component.wasm"},
                                "digest": digest_for(&templating)
                            }),
                        ),
                    ],
                );
            }
            "examples/qa-demo" => {
                let llm_digest = cache_component(&cache_dir, "llm.openai.chat");
                let flow_return_digest = cache_component(&cache_dir, "flow.return");
                let qa_digest = cache_component(&cache_dir, "qa.process");
                let state_digest = cache_component(&cache_dir, "state.get");
                let flow_call_digest = cache_component(&cache_dir, "flow.call");
                let messaging_digest = cache_component(&cache_dir, "messaging.emit");
                write_summary(
                    &pack_dir,
                    "flows/qa_answer_flow.ygtc.resolve.summary.json",
                    &[
                        (
                            "call_llm",
                            json!({
                                "component_id": "llm.openai.chat",
                                "source": {"kind": "repo", "ref": "io.3bridges.components.llm.openai@1.0.0"},
                                "digest": llm_digest
                            }),
                        ),
                        (
                            "package_messages",
                            json!({
                                "component_id": "flow.return",
                                "source": {"kind": "repo", "ref": "io.3bridges.components.flow@1.0.0"},
                                "digest": flow_return_digest
                            }),
                        ),
                    ],
                );
                write_summary(
                    &pack_dir,
                    "flows/qa_orchestrator.ygtc.resolve.summary.json",
                    &[
                        (
                            "collect_question",
                            json!({
                                "component_id": "qa.process",
                                "source": {"kind": "repo", "ref": "io.3bridges.components.qa@1.0.0"},
                                "digest": qa_digest
                            }),
                        ),
                        (
                            "load_profile",
                            json!({
                                "component_id": "state.get",
                                "source": {"kind": "repo", "ref": "io.3bridges.components.state@1.0.0"},
                                "digest": state_digest
                            }),
                        ),
                        (
                            "call_specialist",
                            json!({
                                "component_id": "flow.call",
                                "source": {"kind": "repo", "ref": "io.3bridges.components.flow@1.0.0"},
                                "digest": flow_call_digest
                            }),
                        ),
                        (
                            "respond_to_user",
                            json!({
                                "component_id": "messaging.emit",
                                "source": {"kind": "repo", "ref": "io.3bridges.components.messaging@1.0.0"},
                                "digest": messaging_digest
                            }),
                        ),
                    ],
                );
            }
            "examples/billing-demo" => {
                let templating = pack_dir.join("components/templating.handlebars.wasm");
                write_describe_sidecar(&templating, "templating.handlebars", "0.1.0");
                write_summary(
                    &pack_dir,
                    "flows/main.ygtc.resolve.summary.json",
                    &[(
                        "start",
                        json!({
                            "component_id": "templating.handlebars",
                            "source": {"kind": "local", "path": "../components/templating.handlebars.wasm"},
                            "digest": digest_for(&templating)
                        }),
                    )],
                );
            }
            "examples/search-demo" => {
                let templating = pack_dir.join("components/templating.handlebars.wasm");
                write_describe_sidecar(&templating, "templating.handlebars", "0.1.0");
                write_summary(
                    &pack_dir,
                    "flows/main.ygtc.resolve.summary.json",
                    &[(
                        "start",
                        json!({
                            "component_id": "templating.handlebars",
                            "source": {"kind": "local", "path": "../components/templating.handlebars.wasm"},
                            "digest": digest_for(&templating)
                        }),
                    )],
                );
            }
            "examples/reco-demo" => {
                let templating = pack_dir.join("components/templating.handlebars.wasm");
                write_describe_sidecar(&templating, "templating.handlebars", "0.1.0");
                write_summary(
                    &pack_dir,
                    "flows/main.ygtc.resolve.summary.json",
                    &[(
                        "start",
                        json!({
                            "component_id": "templating.handlebars",
                            "source": {"kind": "local", "path": "../components/templating.handlebars.wasm"},
                            "digest": digest_for(&templating)
                        }),
                    )],
                );
            }
            _ => {}
        }
        let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"));
        cmd.current_dir(workspace_root());
        cmd.env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1");
        cmd.args([
            "build",
            "--in",
            pack,
            "--dry-run",
            "--offline",
            "--cache-dir",
            cache_dir.to_str().unwrap(),
            "--log",
            "warn",
        ]);
        cmd.assert().success();
    }
}
