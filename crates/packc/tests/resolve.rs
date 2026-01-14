use assert_cmd::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn write_pack(dir: &Path, wasm_contents: &[u8], sidecar_digest: Option<&str>) {
    let flows_dir = dir.join("flows");
    fs::create_dir_all(&flows_dir).unwrap();
    let flow_path = flows_dir.join("main.ygtc");
    fs::write(&flow_path, "id: main\nentry: start\n").unwrap();

    // Flow resolve sidecar.
    let wasm_path = dir.join("components").join("demo.wasm");
    fs::create_dir_all(wasm_path.parent().unwrap()).unwrap();
    fs::write(&wasm_path, wasm_contents).unwrap();

    let digest_json = sidecar_digest
        .map(|d| format!(r#", "digest": "{d}""#))
        .unwrap_or_default();
    let sidecar = format!(
        r#"{{
  "schema_version": 1,
  "flow": "flows/main.ygtc",
  "nodes": {{
    "call": {{
      "source": {{
        "kind": "oci",
        "ref": "file://{}"{digest_json}
      }}
    }}
  }}
}}"#,
        wasm_path.display()
    );
    fs::write(flow_path.with_extension("ygtc.resolve.json"), sidecar).unwrap();

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

fn write_pack_with_local_sidecar(dir: &Path, wasm_contents: &[u8]) {
    let flows_dir = dir.join("flows");
    fs::create_dir_all(&flows_dir).unwrap();
    let flow_path = flows_dir.join("main.ygtc");
    fs::write(&flow_path, "id: main\nentry: start\n").unwrap();

    let wasm_path = dir.join("components").join("demo.wasm");
    fs::create_dir_all(wasm_path.parent().unwrap()).unwrap();
    fs::write(&wasm_path, wasm_contents).unwrap();

    let sidecar = r#"{
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
}"#;
    fs::write(flow_path.with_extension("ygtc.resolve.json"), sidecar).unwrap();

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

fn write_pack_with_local_sidecar_file_uri(dir: &Path, wasm_contents: &[u8]) {
    let flows_dir = dir.join("flows");
    fs::create_dir_all(&flows_dir).unwrap();
    let flow_path = flows_dir.join("main.ygtc");
    fs::write(&flow_path, "id: main\nentry: start\n").unwrap();

    let wasm_path = dir.join("components").join("demo.wasm");
    fs::create_dir_all(wasm_path.parent().unwrap()).unwrap();
    fs::write(&wasm_path, wasm_contents).unwrap();

    let sidecar = r#"{
  "schema_version": 1,
  "flow": "main.ygtc",
  "nodes": {
    "call": {
      "source": {
        "kind": "local",
        "path": "file://../components/demo.wasm"
      }
    }
  }
}"#;
    fs::write(flow_path.with_extension("ygtc.resolve.json"), sidecar).unwrap();

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

    write_pack(&pack_dir, b"wasm-bytes", None);
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
    assert!(
        components[0]["digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
}

#[test]
fn offline_missing_digest_errors() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path().to_path_buf();

    write_pack(&pack_dir, b"wasm-bytes", None);

    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .args([
            "--offline",
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

    write_pack_with_local_sidecar(&pack_dir, b"wasm-bytes");

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

    write_pack_with_local_sidecar_file_uri(&pack_dir, b"wasm-bytes");

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
