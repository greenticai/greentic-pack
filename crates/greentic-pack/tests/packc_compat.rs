use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::prelude::*;
use greentic_pack::reader::{SigningPolicy, open_pack};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn write_stub_wasm(path: &Path) {
    const STUB: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, STUB).expect("write stub wasm");
}

#[test]
fn open_pack_accepts_packc_gtpack() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path().join("demo-pack");
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    // Create a minimal pack with a local component and sidecar mapping.
    fs::create_dir_all(pack_dir.join("flows")).expect("flows dir");
    fs::create_dir_all(pack_dir.join("components")).expect("components dir");
    let wasm_path = pack_dir.join("components/demo.wasm");
    write_stub_wasm(&wasm_path);
    let digest = format!(
        "sha256:{:x}",
        Sha256::digest(fs::read(&wasm_path).expect("read wasm"))
    );

    let pack_yaml = r#"pack_id: demo.test
version: 0.1.0
kind: application
publisher: Greentic

components:
  - id: "demo.component"
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
    wasm: "components/demo.wasm"

flows:
  - id: main
    file: flows/main.ygtc
    tags: [default]
    entrypoints: [main]
"#;
    fs::write(pack_dir.join("pack.yaml"), pack_yaml).expect("write pack.yaml");

    let flow_yaml = r#"id: main
type: messaging
start: call
nodes:
  call:
    component.exec:
      component: demo.component
      operation: handle_message
      input:
        text: "hi"
    routing:
      - out: true
"#;
    fs::write(pack_dir.join("flows/main.ygtc"), flow_yaml).expect("write flow");

    let sidecar = format!(
        r#"{{
  "schema_version": 1,
  "flow": "flows/main.ygtc",
  "nodes": {{
    "call": {{
      "source": {{
        "kind": "local",
        "path": "../components/demo.wasm",
        "digest": "{digest}"
      }}
    }}
  }}
}}"#
    );
    fs::write(pack_dir.join("flows/main.ygtc.resolve.json"), sidecar).expect("write sidecar");

    let pack_path = temp.path().join("pack.gtpack");
    Command::new("cargo")
        .current_dir(&workspace_root)
        .args([
            "run",
            "-p",
            "packc",
            "--bin",
            "packc",
            "--quiet",
            "--",
            "build",
            "--in",
            pack_dir.to_str().unwrap(),
            "--gtpack-out",
            pack_path.to_str().unwrap(),
            "--offline",
        ])
        .assert()
        .success();

    let load = open_pack(&pack_path, SigningPolicy::DevOk)
        .expect("packc-generated gtpack should be accepted");

    assert_eq!(load.manifest.meta.pack_id, "demo.test");
    assert!(!load.report.signature_ok);
    assert!(!load.report.sbom_ok);
    assert!(load.report.warnings.iter().any(|msg| msg.contains("sbom")));
    assert!(!load.sbom.is_empty());
}
