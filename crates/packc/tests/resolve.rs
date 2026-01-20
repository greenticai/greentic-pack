use assert_cmd::prelude::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
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
    let lock_path = pack_dir.join("pack.lock.json");

    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
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

    let contents = fs::read_to_string(&lock_path).expect("lock file");
    let json: Value = serde_json::from_str(&contents).unwrap();
    assert_eq!(json["schema_version"], 1);
    let components = json["components"].as_array().unwrap();
    assert_eq!(components.len(), 1);
    assert_eq!(components[0]["name"].as_str().unwrap(), "main___call");
    assert!(
        components[0]["digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
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
        .args([
            "resolve",
            "--in",
            pack_dir.to_str().unwrap(),
            "--log",
            "warn",
        ])
        .assert()
        .success();

    let lock_path = pack_dir.join("pack.lock.json");
    let contents = fs::read_to_string(&lock_path).expect("lock file");
    let json: Value = serde_json::from_str(&contents).unwrap();
    let components = json["components"].as_array().unwrap();
    assert_eq!(components.len(), 1);
    assert!(
        components[0]["digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
}

#[test]
fn resolve_local_file_uri_paths_relative_to_flow() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path().to_path_buf();

    write_pack_with_local_summary_file_uri(&pack_dir, b"wasm-bytes");

    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .args([
            "resolve",
            "--in",
            pack_dir.to_str().unwrap(),
            "--log",
            "warn",
        ])
        .assert()
        .success();

    let lock_path = pack_dir.join("pack.lock.json");
    let contents = fs::read_to_string(&lock_path).expect("lock file");
    let json: Value = serde_json::from_str(&contents).unwrap();
    let components = json["components"].as_array().unwrap();
    assert_eq!(components.len(), 1);
    assert!(
        components[0]["digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
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
