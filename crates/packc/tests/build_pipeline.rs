use std::fs;
use std::path::Path;

use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::tempdir;
use zip::ZipArchive;

fn write_stub_wasm(path: &Path) {
    const STUB: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, STUB).expect("write stub wasm");
}

#[test]
fn build_does_not_copy_component_directory() {
    let temp = tempdir().unwrap();
    let pack_dir = temp.path();

    let component_dir = pack_dir.join("components/dev.hello-world");
    fs::create_dir_all(component_dir.join("src")).unwrap();
    fs::create_dir_all(component_dir.join("tmp")).unwrap();
    write_stub_wasm(&component_dir.join("dev.hello-world.component.wasm"));
    fs::write(
        component_dir.join("component.json"),
        r#"{
            "id": "dev.hello-world",
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
    fs::write(component_dir.join("README.md"), "ignore me").unwrap();
    fs::write(component_dir.join("src/main.rs"), "// ignore").unwrap();
    fs::write(component_dir.join("tmp/foo.txt"), "ignore").unwrap();

    fs::create_dir_all(pack_dir.join("flows")).unwrap();
    fs::write(
        pack_dir.join("flows/main.ygtc"),
        r#"id: main
type: messaging
start: call
nodes:
  call:
    component.exec:
      component: dev.hello-world
      operation: handle_message
      input:
        text: "hi"
    routing:
      - out: true
"#,
    )
    .unwrap();
    let wasm_path = component_dir.join("dev.hello-world.component.wasm");

    fs::write(
        pack_dir.join("pack.yaml"),
        r#"pack_id: dev.local.dir-pack
version: 0.1.0
kind: application
publisher: Test
components:
  - id: "dev.hello-world"
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
    wasm: "components/dev.hello-world"
flows:
  - id: main
    file: flows/main.ygtc
    tags: [default]
    entrypoints: [main]
"#,
    )
    .unwrap();

    let digest = format!("sha256:{:x}", Sha256::digest(fs::read(&wasm_path).unwrap()));
    let sidecar = json!({
        "schema_version": 1,
        "flow": "flows/main.ygtc",
        "nodes": {
            "call": {
                "source": {
                    "kind": "local",
                    "path": "../components/dev.hello-world/dev.hello-world.component.wasm",
                    "digest": digest
                }
            }
        }
    });
    fs::write(
        pack_dir.join("flows/main.ygtc.resolve.json"),
        serde_json::to_vec_pretty(&sidecar).unwrap(),
    )
    .unwrap();

    let manifest_out = pack_dir.join("dist/manifest.cbor");
    let gtpack_out = pack_dir.join("dist/pack.gtpack");

    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(pack_dir)
        .args([
            "build",
            "--in",
            pack_dir.to_str().unwrap(),
            "--manifest",
            manifest_out.to_str().unwrap(),
            "--gtpack-out",
            gtpack_out.to_str().unwrap(),
        ])
        .output()
        .expect("run packc build");
    assert!(
        output.status.success(),
        "packc build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let file = fs::File::open(&gtpack_out).expect("open gtpack");
    let mut archive = ZipArchive::new(file).expect("zip archive");
    let mut names = Vec::new();
    for i in 0..archive.len() {
        names.push(archive.by_index(i).unwrap().name().to_string());
    }
    names.sort();

    assert!(
        names.contains(&"components/dev.hello-world.wasm".to_string()),
        "wasm should be packaged"
    );
    assert!(
        names.contains(&"components/dev.hello-world.manifest.cbor".to_string()),
        "manifest cbor should be packaged"
    );
    assert_eq!(
        names.iter().filter(|n| n.ends_with(".wasm")).count(),
        1,
        "only one wasm should be packaged"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.contains("README") || n.contains("src/") || n.contains("tmp/")),
        "non-runtime files must not be packaged: {names:?}"
    );
}

#[test]
fn build_runs_update_by_default() {
    let temp = tempdir().unwrap();
    let pack_dir = temp.path();
    fs::create_dir_all(pack_dir.join("components")).unwrap();
    let wasm_path = pack_dir.join("components/auto.wasm");
    write_stub_wasm(&wasm_path);
    fs::create_dir_all(pack_dir.join("flows")).unwrap();
    fs::write(
        pack_dir.join("flows/main.ygtc"),
        r#"id: main
type: messaging
start: call
nodes:
  call:
    component.exec:
      component: dev.auto
      operation: handle_message
      input:
        text: "hi"
    routing:
      - out: true
"#,
    )
    .unwrap();

    let digest = format!("sha256:{:x}", Sha256::digest(fs::read(&wasm_path).unwrap()));
    let sidecar = json!({
        "schema_version": 1,
        "flow": "flows/main.ygtc",
        "nodes": {
            "call": {
                "source": {
                    "kind": "local",
                    "path": "../components/auto.wasm",
                    "digest": digest
                }
            }
        }
    });
    fs::write(
        pack_dir.join("flows/main.ygtc.resolve.json"),
        serde_json::to_vec_pretty(&sidecar).unwrap(),
    )
    .unwrap();

    let initial_pack = r#"pack_id: dev.local.auto
version: 0.1.0
kind: application
publisher: Test
components: []
flows: []
"#;
    fs::write(pack_dir.join("pack.yaml"), initial_pack).unwrap();

    let gtpack_out = pack_dir.join("dist/pack.gtpack");
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(pack_dir)
        .args([
            "build",
            "--in",
            pack_dir.to_str().unwrap(),
            "--gtpack-out",
            gtpack_out.to_str().unwrap(),
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

    let updated: serde_yaml_bw::Value =
        serde_yaml_bw::from_str(&fs::read_to_string(pack_dir.join("pack.yaml")).unwrap()).unwrap();
    let components = updated
        .get("components")
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.len())
        .unwrap_or(0);
    let flows = updated
        .get("flows")
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.len())
        .unwrap_or(0);
    assert_eq!(components, 1, "update should add discovered component");
    assert_eq!(flows, 1, "update should sync flows");

    let file = fs::File::open(&gtpack_out).expect("open gtpack");
    let mut archive = ZipArchive::new(file).expect("zip archive");
    let names: Vec<_> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n.ends_with(".wasm")),
        "gtpack should include component wasm"
    );
}

#[test]
fn build_no_update_skips_update() {
    let temp = tempdir().unwrap();
    let pack_dir = temp.path();
    fs::create_dir_all(pack_dir.join("components")).unwrap();
    write_stub_wasm(&pack_dir.join("components/manual.wasm"));
    fs::create_dir_all(pack_dir.join("flows")).unwrap();
    fs::write(
        pack_dir.join("flows/main.ygtc"),
        r#"id: main
type: messaging
start: call
nodes:
  call:
    component.exec:
      component: dev.manual
      operation: handle_message
      input:
        text: "hi"
    routing:
      - out: true
"#,
    )
    .unwrap();

    let initial_pack = r#"pack_id: dev.local.manual
version: 0.1.0
kind: application
publisher: Test
components:
  - id: "dev.manual"
    version: "0.1.0"
    world: "greentic:component/component@0.5.0"
    supports: ["messaging"]
    profiles:
      default: "stateless"
      supported: ["stateless"]
    capabilities:
      wasi: {}
      host: {}
    operations: []
    wasm: "components/manual.wasm"
flows: []
"#;
    fs::write(pack_dir.join("pack.yaml"), initial_pack).unwrap();

    let pack_yaml_before = fs::read_to_string(pack_dir.join("pack.yaml")).unwrap();
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(pack_dir)
        .args([
            "build",
            "--in",
            pack_dir.to_str().unwrap(),
            "--no-update",
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
    let pack_yaml_after = fs::read_to_string(pack_dir.join("pack.yaml")).unwrap();
    assert_eq!(
        pack_yaml_before, pack_yaml_after,
        "pack.yaml should not be modified when --no-update is set"
    );
}
