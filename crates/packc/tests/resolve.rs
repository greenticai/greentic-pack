use assert_cmd::prelude::*;
use greentic_types::cbor::canonical;
use greentic_types::schemas::common::schema_ir::{AdditionalProperties, SchemaIr};
use greentic_types::schemas::component::v0_6_0::{
    ComponentDescribe, ComponentInfo, ComponentOperation, ComponentRunInput, ComponentRunOutput,
    schema_hash,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn write_pack(dir: &Path, wasm_contents: &[u8]) {
    let flows_dir = dir.join("flows");
    fs::create_dir_all(&flows_dir).unwrap();
    let flow_path = flows_dir.join("main.ygtc");
    fs::write(&flow_path, "id: main\nentry: start\n").unwrap();

    let wasm_path = dir.join("components").join("demo.wasm");
    fs::create_dir_all(wasm_path.parent().unwrap()).unwrap();
    fs::write(&wasm_path, wasm_contents).unwrap();
    write_describe_sidecar(&wasm_path, "demo.component");
    let digest = format!("sha256:{:x}", Sha256::digest(fs::read(&wasm_path).unwrap()));
    let summary = format!(
        r#"{{
  "schema_version": 1,
  "flow": "main.ygtc",
  "nodes": {{
    "call": {{
      "component_id": "demo.component",
      "source": {{
        "kind": "local",
        "path": "../components/demo.wasm"
      }},
      "digest": "{digest}"
    }}
  }}
}}"#
    );
    fs::write(
        flow_path.with_extension("ygtc.resolve.summary.json"),
        summary,
    )
    .unwrap();

    let pack_yaml = r#"pack_id: demo.pack
version: 0.1.0
kind: application
publisher: demo
flows:
  - id: main
    file: flows/main.ygtc
"#;
    fs::write(dir.join("pack.yaml"), pack_yaml).unwrap();
}

fn write_pack_with_local_summary(dir: &Path, wasm_contents: &[u8]) {
    let flows_dir = dir.join("flows");
    fs::create_dir_all(&flows_dir).unwrap();
    let flow_path = flows_dir.join("main.ygtc");
    fs::write(&flow_path, "id: main\nentry: start\n").unwrap();

    let wasm_path = dir.join("components").join("demo.wasm");
    fs::create_dir_all(wasm_path.parent().unwrap()).unwrap();
    fs::write(&wasm_path, wasm_contents).unwrap();
    write_describe_sidecar(&wasm_path, "demo.component");

    let digest = format!("sha256:{:x}", Sha256::digest(fs::read(&wasm_path).unwrap()));
    let summary = serde_json::json!({
        "schema_version": 1,
        "flow": "main.ygtc",
        "nodes": {
            "call": {
                "component_id": "demo.component",
                "source": {
                    "kind": "local",
                    "path": "../components/demo.wasm"
                },
                "digest": digest
            }
        }
    });
    fs::write(
        flow_path.with_extension("ygtc.resolve.summary.json"),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .unwrap();

    let pack_yaml = r#"pack_id: demo.pack
version: 0.1.0
kind: application
publisher: demo
flows:
  - id: main
    file: flows/main.ygtc
"#;
    fs::write(dir.join("pack.yaml"), pack_yaml).unwrap();
}

fn write_pack_with_local_summary_file_uri(dir: &Path, wasm_contents: &[u8]) {
    let flows_dir = dir.join("flows");
    fs::create_dir_all(&flows_dir).unwrap();
    let flow_path = flows_dir.join("main.ygtc");
    fs::write(&flow_path, "id: main\nentry: start\n").unwrap();

    let wasm_path = dir.join("components").join("demo.wasm");
    fs::create_dir_all(wasm_path.parent().unwrap()).unwrap();
    fs::write(&wasm_path, wasm_contents).unwrap();
    write_describe_sidecar(&wasm_path, "demo.component");

    let digest = format!("sha256:{:x}", Sha256::digest(fs::read(&wasm_path).unwrap()));
    let summary = serde_json::json!({
        "schema_version": 1,
        "flow": "main.ygtc",
        "nodes": {
            "call": {
                "component_id": "demo.component",
                "source": {
                    "kind": "local",
                    "path": "file://../components/demo.wasm"
                },
                "digest": digest
            }
        }
    });
    fs::write(
        flow_path.with_extension("ygtc.resolve.summary.json"),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .unwrap();

    let pack_yaml = r#"pack_id: demo.pack
version: 0.1.0
kind: application
publisher: demo
flows:
  - id: main
    file: flows/main.ygtc
"#;
    fs::write(dir.join("pack.yaml"), pack_yaml).unwrap();
}

#[test]
fn resolve_writes_lockfile_with_digest() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path().to_path_buf();

    write_pack(&pack_dir, b"wasm-bytes");
    let lock_path = pack_dir.join("pack.lock.cbor");

    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "resolve",
            "--in",
            pack_dir.to_str().unwrap(),
            "--lock",
            lock_path.to_str().unwrap(),
            "--log",
            "warn",
        ])
        .assert()
        .success();

    let lock = greentic_pack::pack_lock::read_pack_lock(&lock_path).expect("lock file");
    assert_eq!(lock.version, 1);
    assert_eq!(lock.components.len(), 1);
    let entry = lock
        .components
        .get("demo.component")
        .expect("component entry");
    assert_eq!(entry.component_id, "demo.component");
    assert!(entry.resolved_digest.starts_with("sha256:"));
}

#[test]
fn missing_summary_without_sidecar_errors() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path().to_path_buf();

    let flows_dir = pack_dir.join("flows");
    fs::create_dir_all(&flows_dir).unwrap();
    fs::write(flows_dir.join("main.ygtc"), "id: main\nentry: start\n").unwrap();
    fs::write(
        pack_dir.join("pack.yaml"),
        r#"pack_id: demo.pack
version: 0.1.0
kind: application
publisher: demo
flows:
  - id: main
    file: flows/main.ygtc
"#,
    )
    .unwrap();

    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "resolve",
            "--in",
            pack_dir.to_str().unwrap(),
            "--log",
            "warn",
        ])
        .assert()
        .failure();
}

#[test]
fn resolve_local_paths_relative_to_flow() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path().to_path_buf();

    write_pack_with_local_summary(&pack_dir, b"wasm-bytes");

    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "resolve",
            "--in",
            pack_dir.to_str().unwrap(),
            "--log",
            "warn",
        ])
        .assert()
        .success();

    let lock_path = pack_dir.join("pack.lock.cbor");
    let lock = greentic_pack::pack_lock::read_pack_lock(&lock_path).expect("lock file");
    let entry = lock
        .components
        .get("demo.component")
        .expect("component entry");
    assert!(entry.resolved_digest.starts_with("sha256:"));
}

fn write_describe_sidecar(wasm_path: &Path, component_id: &str) {
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
            version: "0.1.0".to_string(),
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
    let describe_path = wasm_path.to_string_lossy().to_string() + ".describe.cbor";
    fs::write(describe_path, bytes).expect("write describe cache");
}

#[test]
fn resolve_local_file_uri_paths_relative_to_flow() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path().to_path_buf();

    write_pack_with_local_summary_file_uri(&pack_dir, b"wasm-bytes");

    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "resolve",
            "--in",
            pack_dir.to_str().unwrap(),
            "--log",
            "warn",
        ])
        .assert()
        .success();

    let lock_path = pack_dir.join("pack.lock.cbor");
    let lock = greentic_pack::pack_lock::read_pack_lock(&lock_path).expect("lock file");
    let entry = lock
        .components
        .get("demo.component")
        .expect("component entry");
    assert!(entry.resolved_digest.starts_with("sha256:"));
}

#[test]
fn resolve_falls_back_when_manifest_artifact_mismatch() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path().to_path_buf();

    let flows_dir = pack_dir.join("flows");
    fs::create_dir_all(&flows_dir).unwrap();
    let flow_path = flows_dir.join("main.ygtc");
    fs::write(&flow_path, "id: main\nentry: start\n").unwrap();

    let wasm_path = pack_dir.join("components").join("demo.wasm");
    fs::create_dir_all(wasm_path.parent().unwrap()).unwrap();
    fs::write(&wasm_path, b"wasm-bytes").unwrap();
    write_describe_sidecar(&wasm_path, "ai.greentic.demo-comp");

    let sidecar = serde_json::json!({
        "schema_version": 1,
        "flow": "main.ygtc",
        "nodes": {
            "call": {
                "source": {
                    "kind": "local",
                    "path": "../components/demo.wasm"
                }
            }
        }
    });
    fs::write(
        flow_path.with_extension("ygtc.resolve.json"),
        serde_json::to_vec_pretty(&sidecar).unwrap(),
    )
    .unwrap();

    let manifest = serde_json::json!({
        "id": "ai.greentic.demo-comp",
        "version": "0.1.0",
        "world": "greentic:component/component@0.5.0",
        "artifacts": {
            "component_wasm": "component.wasm"
        }
    });
    fs::write(
        pack_dir.join("component.manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let pack_yaml = r#"pack_id: demo.pack
version: 0.1.0
kind: application
publisher: demo
flows:
  - id: main
    file: flows/main.ygtc
"#;
    fs::write(pack_dir.join("pack.yaml"), pack_yaml).unwrap();

    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "resolve",
            "--in",
            pack_dir.to_str().unwrap(),
            "--log",
            "warn",
        ])
        .assert()
        .success();
}
