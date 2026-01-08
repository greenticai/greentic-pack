use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use assert_cmd::Command;
use greentic_types::flow_resolve::read_flow_resolve;
use greentic_types::flow_resolve::{FlowResolveV1, write_flow_resolve};
use tempfile::TempDir;

fn write_stub_wasm(path: &Path) {
    const STUB: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, STUB).expect("write stub wasm");
}

#[test]
fn update_creates_missing_sidecar() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path();

    fs::create_dir_all(pack_dir.join("components")).unwrap();
    write_stub_wasm(&pack_dir.join("components/demo.wasm"));
    fs::create_dir_all(pack_dir.join("flows")).unwrap();
    fs::write(
        pack_dir.join("flows/main.ygtc"),
        r#"id: main
type: messaging
start: call
nodes:
  call:
    component.exec:
      component: dev.demo
      operation: handle_message
      input:
        text: "hi"
    routing:
      - out: true
"#,
    )
    .unwrap();

    fs::write(
        pack_dir.join("pack.yaml"),
        r#"pack_id: dev.local.sidecar
version: 0.1.0
kind: application
publisher: Test
components: []
flows: []
"#,
    )
    .unwrap();

    Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(pack_dir)
        .args([
            "update",
            "--in",
            pack_dir.to_str().unwrap(),
            "--log",
            "warn",
        ])
        .assert()
        .success();

    let sidecar_path = pack_dir.join("flows/main.ygtc.resolve.json");
    assert!(sidecar_path.exists(), "sidecar should be created");
    let doc = read_flow_resolve(&sidecar_path).expect("read sidecar");
    assert!(doc.nodes.is_empty(), "new sidecar should start empty");
}

#[test]
fn build_fails_when_nodes_unmapped() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path();

    let component_dir = pack_dir.join("components");
    fs::create_dir_all(&component_dir).unwrap();
    write_stub_wasm(&component_dir.join("demo.component.wasm"));
    fs::write(
        component_dir.join("component.json"),
        r#"{
            "id": "dev.demo",
            "version": "0.1.0",
            "supports": ["messaging"],
            "world": "greentic:component/component@0.5.0",
            "profiles": { "default": "stateless", "supported": ["stateless"] },
            "capabilities": { "wasi": {}, "host": {} },
            "operations": [ { "name": "handle_message", "input_schema": {}, "output_schema": {} } ],
            "resources": {}
        }"#,
    )
    .unwrap();

    fs::create_dir_all(pack_dir.join("flows")).unwrap();
    fs::write(
        pack_dir.join("flows/main.ygtc"),
        r#"id: main
type: messaging
start: call
nodes:
  call:
    component.exec:
      component: dev.demo
      operation: handle_message
      input:
        text: "hi"
    routing:
      - out: true
"#,
    )
    .unwrap();

    // Write an empty sidecar (no node mappings) so build must fail.
    let sidecar_path = pack_dir.join("flows/main.ygtc.resolve.json");
    let empty = FlowResolveV1 {
        schema_version: 1,
        flow: "flows/main.ygtc".into(),
        nodes: BTreeMap::new(),
    };
    write_flow_resolve(&sidecar_path, &empty).expect("write empty sidecar");

    fs::write(
        pack_dir.join("pack.yaml"),
        r#"pack_id: dev.local.unmapped
version: 0.1.0
kind: application
publisher: Test
components:
  - id: "dev.demo"
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
    wasm: "components/demo.component.wasm"
flows:
  - id: main
    file: flows/main.ygtc
    tags: [default]
    entrypoints: [main]
"#,
    )
    .unwrap();

    let output = Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(pack_dir)
        .args([
            "build",
            "--in",
            pack_dir.to_str().unwrap(),
            "--no-update",
            "--gtpack-out",
            pack_dir.join("dist/pack.gtpack").to_str().unwrap(),
            "--log",
            "warn",
        ])
        .output()
        .expect("run packc build");

    assert!(
        !output.status.success(),
        "build should fail when resolve sidecar lacks node mappings"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing resolve entries") || stderr.contains("requires a resolve sidecar"),
        "expected actionable error, got stderr={}",
        stderr
    );
}
