use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use greentic_pack::reader::{SigningPolicy, open_pack};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const COMPONENT_ID: &str = "demo.component";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn write_stub_wasm(path: &Path) {
    const STUB: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("components dir");
    }
    fs::write(path, STUB).expect("stub wasm");
}

fn write_pack_files(pack_dir: &Path) {
    fs::create_dir_all(pack_dir.join("flows")).expect("flows dir");
    let wasm_path = pack_dir.join("components/demo.wasm");
    write_stub_wasm(&wasm_path);

    let pack_yaml = format!(
        r#"pack_id: dev.local.manifest-demo
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
      - name: "handle_message"
        input_schema: {{}}
        output_schema: {{}}
    wasm: "components/demo.wasm"

flows:
  - id: main
    file: flows/main.ygtc
    tags: [default]
    entrypoints: [main]
"#
    );
    fs::write(pack_dir.join("pack.yaml"), pack_yaml).expect("pack.yaml");

    let wasm_bytes = fs::read(&wasm_path).expect("read stub wasm");
    let digest = format!("sha256:{:x}", Sha256::digest(&wasm_bytes));

    let flow = format!(
        r#"id: main
type: messaging
start: call
nodes:
  call:
    component.exec:
      component: {COMPONENT_ID}
      operation: handle_message
      input:
        text: "hi"
    routing:
      - out: true
"#
    );
    fs::write(pack_dir.join("flows/main.ygtc"), flow).expect("flow file");

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
}

fn build_pack_with_packc() -> (TempDir, PathBuf, greentic_pack::reader::PackLoad) {
    let temp = TempDir::new().expect("temp dir");
    write_pack_files(temp.path());

    let manifest_out = temp.path().join("dist/manifest.cbor");
    let pack_path = temp.path().join("dist/pack.gtpack");

    let output = Command::new("cargo")
        .current_dir(workspace_root())
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
            temp.path().to_str().unwrap(),
            "--manifest",
            manifest_out.to_str().unwrap(),
            "--gtpack-out",
            pack_path.to_str().unwrap(),
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

    let load = open_pack(&pack_path, SigningPolicy::DevOk).expect("pack should open");
    (temp, pack_path, load)
}

#[test]
fn component_manifest_index_verifies_ok() {
    let (_temp, _pack_path, load) = build_pack_with_packc();
    let state = load.component_manifest_index_v1();
    assert!(state.present, "index should be present");
    assert!(state.error.is_none(), "index decodes cleanly");
    let index = state.index.expect("index payload");
    assert!(
        !index.entries.is_empty(),
        "index should include at least one component"
    );

    let report = load.verify_component_manifest_files();
    assert!(
        report.extension_present,
        "verification should inspect the index"
    );
    assert!(report.ok(), "verification should succeed");
    assert!(
        report.entries.iter().all(|entry| entry.is_ok()),
        "all entries should be clean"
    );
}

#[test]
fn component_manifest_index_reports_missing_file() {
    let (_temp, _pack_path, mut load) = build_pack_with_packc();
    let state = load.component_manifest_index_v1();
    let index = state.index.expect("index payload");
    let missing_path = &index.entries[0].manifest_file;
    load.files.remove(missing_path);

    let report = load.verify_component_manifest_files();
    assert!(report.extension_present);
    assert!(!report.ok(), "verification should fail for missing files");
    let entry = report
        .entries
        .iter()
        .find(|entry| &entry.manifest_file == missing_path)
        .expect("entry in report");
    assert_eq!(
        entry.error.as_deref(),
        Some("manifest file missing from archive")
    );
}

#[test]
fn component_manifest_index_detects_hash_mismatch() {
    let (_temp, _pack_path, mut load) = build_pack_with_packc();
    let state = load.component_manifest_index_v1();
    let index = state.index.expect("index payload");
    let target_path = &index.entries[0].manifest_file;
    if let Some(bytes) = load.files.get_mut(target_path) {
        bytes.push(0u8);
    }

    let report = load.verify_component_manifest_files();
    assert!(report.extension_present);
    assert!(!report.ok(), "tampered manifest should fail verification");
    let entry = report
        .entries
        .iter()
        .find(|entry| &entry.manifest_file == target_path)
        .expect("entry in report");
    assert_eq!(entry.hash_ok, Some(false));
    assert!(
        entry
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("hash mismatch"),
        "expected hash mismatch error, got {:?}",
        entry.error
    );
}
