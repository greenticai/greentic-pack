use std::fs;
use std::path::PathBuf;
use std::process::Command;

use greentic_types::decode_pack_manifest;
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const COMPONENT_ID: &str = "ai.greentic.fixture";
const OPERATION: &str = "handle_message";

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

#[test]
fn component_exec_operation_is_preserved_in_manifest() {
    let temp = tempdir().expect("temp dir");
    let pack_root = temp.path();

    let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("component_exec_pack");
    fs::create_dir_all(pack_root.join("flows")).expect("flows dir");
    fs::create_dir_all(pack_root.join("components")).expect("components dir");
    fs::copy(fixture_root.join("pack.yaml"), pack_root.join("pack.yaml")).expect("copy pack.yaml");
    fs::copy(
        fixture_root.join("flows/main.ygtc"),
        pack_root.join("flows/main.ygtc"),
    )
    .expect("copy flow");
    let wasm_path = pack_root.join("components/fixture.wasm");
    write_stub_wasm(&wasm_path);
    let digest = format!("sha256:{:x}", Sha256::digest(fs::read(&wasm_path).unwrap()));
    let sidecar = json!({
        "schema_version": 1,
        "flow": "flows/main.ygtc",
        "nodes": {
            "hello": {
                "source": {
                    "kind": "local",
                    "path": "../components/fixture.wasm",
                    "digest": digest
                }
            }
        }
    });
    fs::write(
        pack_root.join("flows/main.ygtc.resolve.json"),
        serde_json::to_vec_pretty(&sidecar).unwrap(),
    )
    .expect("write sidecar");

    let manifest_path = pack_root.join("dist/manifest.cbor");
    let output = Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
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

    let manifest_bytes = fs::read(&manifest_path).expect("manifest bytes");
    let manifest = decode_pack_manifest(&manifest_bytes).expect("decode manifest.cbor");

    let flow = manifest
        .flows
        .iter()
        .find(|f| f.id.as_str() == "main")
        .expect("main flow present");
    let (_, node) = flow
        .flow
        .nodes
        .iter()
        .find(|(id, _)| id.as_str() == "hello")
        .expect("hello node present");

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
    assert!(
        node.component.pack_alias.is_none(),
        "component.exec should not synthesize pack aliases"
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

    let component = manifest
        .components
        .iter()
        .find(|c| c.id.as_str() == COMPONENT_ID)
        .expect("component present");
    assert!(
        component
            .operations
            .iter()
            .any(|op| op.name.as_str() == OPERATION),
        "component operations should include declared operation"
    );
}
