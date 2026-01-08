use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use greentic_types::{decode_pack_manifest, pack_manifest::PackManifest};
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const COMPONENT_ID: &str = "ai.greentic.hello-world";
const OPERATION: &str = "handle_message";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn write_stub_wasm(path: &Path) {
    const STUB: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    fs::create_dir_all(path.parent().unwrap()).expect("components dir");
    fs::write(path, STUB).expect("stub wasm");
}

#[test]
fn component_exec_operation_reaches_manifest() {
    let temp = tempdir().expect("temp dir");
    let pack_dir = temp.path();
    fs::create_dir_all(pack_dir.join("flows")).expect("flows dir");

    let wasm_path = pack_dir.join("components/hello-world.wasm");
    write_stub_wasm(&wasm_path);
    let digest = format!("sha256:{:x}", Sha256::digest(fs::read(&wasm_path).unwrap()));

    let pack_yaml = format!(
        r#"pack_id: dev.local.component-exec
version: 0.0.1
kind: application
publisher: Test
components:
  - id: "{COMPONENT_ID}"
    version: "0.1.0"
    world: "greentic:component/component@0.5.0"
    supports: ["messaging"]
    profiles:
      default: "stateless"
      supported: ["stateless"]
    capabilities:
      wasi: {{}}
      host: {{}}
    operations:
      - name: "{OPERATION}"
        input_schema: {{}}
        output_schema: {{}}
    wasm: "components/hello-world.wasm"

flows:
  - id: main
    file: flows/main.ygtc
    tags: [default]
    entrypoints: [main]
"#
    );
    fs::write(pack_dir.join("pack.yaml"), pack_yaml).expect("pack.yaml");

    let flow = format!(
        r#"id: main
type: messaging
start: hello-world
nodes:
  hello-world:
    component.exec:
      component: {COMPONENT_ID}
      operation: {OPERATION}
      input:
        input: "hi"
    routing:
      - out: true
"#
    );
    fs::write(pack_dir.join("flows").join("main.ygtc"), flow).expect("flow file");
    let sidecar = json!({
        "schema_version": 1,
        "flow": "flows/main.ygtc",
        "nodes": {
            "hello-world": {
                "source": {
                    "kind": "local",
                    "path": "../components/hello-world.wasm",
                    "digest": digest
                }
            }
        }
    });
    fs::write(
        pack_dir.join("flows/main.ygtc.resolve.json"),
        serde_json::to_vec_pretty(&sidecar).unwrap(),
    )
    .expect("write sidecar");

    let manifest_path = pack_dir.join("dist/manifest.cbor");
    let output = Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
        .args([
            "build",
            "--in",
            pack_dir.to_str().unwrap(),
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

    let manifest_bytes = fs::read(&manifest_path).expect("manifest bytes");
    let manifest: PackManifest =
        decode_pack_manifest(&manifest_bytes).expect("decode manifest.cbor");

    let flow = manifest
        .flows
        .iter()
        .find(|f| f.id.as_str() == "main")
        .expect("main flow present");
    let (_, node) = flow
        .flow
        .nodes
        .iter()
        .find(|(id, _)| id.as_str() == "hello-world")
        .expect("hello-world node present");

    assert_eq!(
        node.component.id.as_str(),
        COMPONENT_ID,
        "component.exec should populate node.component.id"
    );
    assert_eq!(
        node.component.operation.as_deref(),
        Some(OPERATION),
        "component.exec operation should populate node.component.operation"
    );
    let mapping_pretty = serde_json::to_string_pretty(&node.input.mapping).unwrap();
    let input_value = match node.input.mapping.get("input") {
        Some(serde_json::Value::String(s)) => Some(s.as_str()),
        Some(serde_json::Value::Object(obj)) => obj.get("input").and_then(|v| v.as_str()),
        _ => None,
    };
    assert_eq!(
        input_value,
        Some("hi"),
        "component.exec input should populate node.input.mapping (observed {})",
        mapping_pretty
    );

    let operations: Vec<_> = manifest
        .components
        .iter()
        .find(|c| c.id.as_str() == COMPONENT_ID)
        .expect("component present")
        .operations
        .iter()
        .map(|op| op.name.as_str())
        .collect();
    assert!(
        operations.contains(&OPERATION),
        "component operations missing expected entry: {:?}",
        operations
    );
}
