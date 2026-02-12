use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use greentic_types::cbor::canonical;
use greentic_types::schemas::common::schema_ir::{AdditionalProperties, SchemaIr};
use greentic_types::schemas::component::v0_6_0::{
    ComponentDescribe, ComponentInfo, ComponentOperation, ComponentRunInput, ComponentRunOutput,
    schema_hash,
};
use greentic_types::{decode_pack_manifest, flow::Routing};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use tempfile::tempdir;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn write_stub_wasm(path: &PathBuf) {
    const STUB: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("components dir");
    }
    fs::write(path, STUB).expect("stub wasm");
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

fn write_pack(routing: &str, flow_name: &str) -> (tempfile::TempDir, PathBuf) {
    let temp = tempdir().expect("temp dir");
    let pack_root = temp.path();

    fs::create_dir_all(pack_root.join("flows")).expect("flows dir");
    fs::create_dir_all(pack_root.join("components")).expect("components dir");

    let pack_yaml = r#"pack_id: dev.local.routing
version: 0.0.1
kind: application
publisher: Test
components:
  - id: "ai.greentic.fixture"
    version: "0.1.0"
    world: "greentic:component/component@0.5.0"
    supports: ["messaging"]
    profiles:
      default: "stateless"
      supported: ["stateless"]
    capabilities:
      wasi: {}
      host: {}
    operations:
      - name: "handle_message"
        input_schema: {}
        output_schema: {}
    wasm: "components/fixture.wasm"

flows:
  - id: main
    file: flows/main.ygtc
    tags: [default]
    entrypoints: [main]
"#;
    fs::write(pack_root.join("pack.yaml"), pack_yaml).expect("pack.yaml");

    let flow = format!(
        r#"id: main
type: messaging
start: {flow_name}
nodes:
  {flow_name}:
    handle_message:
      text: "hi"
    routing: {routing}
"#
    );
    fs::write(pack_root.join("flows/main.ygtc"), flow).expect("flow file");

    let wasm_path = pack_root.join("components/fixture.wasm");
    write_stub_wasm(&wasm_path);
    write_describe_sidecar(&wasm_path, "ai.greentic.fixture", "0.1.0");
    let digest = format!("sha256:{:x}", Sha256::digest(fs::read(&wasm_path).unwrap()));
    let summary = json!({
        "schema_version": 1,
        "flow": "main.ygtc",
        "nodes": {
            flow_name: {
                "component_id": "ai.greentic.fixture",
                "source": {
                    "kind": "local",
                    "path": "../components/fixture.wasm"
                },
                "digest": digest
            }
        }
    });
    fs::write(
        pack_root.join("flows/main.ygtc.resolve.summary.json"),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .expect("write summary");

    let manifest_path = pack_root.join("dist/manifest.cbor");
    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "build",
            "--in",
            pack_root.to_str().unwrap(),
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--log",
            "warn",
        ])
        .output()
        .expect("run packc build");
    assert!(
        output.status.success(),
        "packc build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    (temp, manifest_path)
}

#[test]
fn routing_shorthand_out_normalizes() {
    let (_temp, manifest_path) = write_pack("out", "fixture");
    let manifest = decode_pack_manifest(&fs::read(manifest_path).unwrap()).unwrap();

    let flow = manifest
        .flows
        .iter()
        .find(|f| f.id.as_str() == "main")
        .expect("main flow present");
    let (_id, node) = flow
        .flow
        .nodes
        .iter()
        .find(|(id, _)| id.as_str() == "fixture")
        .expect("fixture node");
    assert_eq!(
        node.component.id.as_str(),
        "ai.greentic.fixture",
        "node component id resolved from flow node name"
    );
    assert_eq!(
        node.component.operation.as_deref(),
        Some("handle_message"),
        "operation preserved"
    );
    match node.routing {
        Routing::End => {}
        ref other => panic!("routing shorthand 'out' should normalize to End, got {other:?}"),
    }
}

#[test]
fn routing_shorthand_reply_normalizes() {
    let (_temp, manifest_path) = write_pack("reply", "fixture");
    let manifest = decode_pack_manifest(&fs::read(manifest_path).unwrap()).unwrap();

    let flow = manifest
        .flows
        .iter()
        .find(|f| f.id.as_str() == "main")
        .expect("main flow present");
    let (_id, node) = flow
        .flow
        .nodes
        .iter()
        .find(|(id, _)| id.as_str() == "fixture")
        .expect("fixture node");
    assert_eq!(
        node.component.id.as_str(),
        "ai.greentic.fixture",
        "node component id resolved from flow node name"
    );
    assert_eq!(
        node.component.operation.as_deref(),
        Some("handle_message"),
        "operation preserved"
    );
    match node.routing {
        Routing::Reply => {}
        ref other => {
            panic!("routing shorthand 'reply' should normalize to Reply, got {other:?}")
        }
    }
}
