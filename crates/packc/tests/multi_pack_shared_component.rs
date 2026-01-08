use greentic_types::{decode_pack_manifest, pack_manifest::PackManifest};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::fs;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const COMPONENT_ID: &str = "shared.component";
const OPERATION: &str = "handle_message";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn tool_available(name: &str) -> bool {
    Command::new(name)
        .arg("--help")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn online() -> bool {
    if std::env::var("GREENTIC_PACK_OFFLINE").is_ok() {
        return false;
    }
    let addr = ("index.crates.io", 443)
        .to_socket_addrs()
        .ok()
        .and_then(|mut iter| iter.next());
    if let Some(addr) = addr {
        std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_ok()
    } else {
        false
    }
}

fn link_shared_component(wasm_src: &Path, pack_dir: &Path) -> PathBuf {
    let dest = pack_dir
        .join("components")
        .join(format!("{COMPONENT_ID}.wasm"));
    fs::create_dir_all(dest.parent().unwrap()).expect("components dir");
    if dest.exists() {
        fs::remove_file(&dest).expect("remove existing link");
    }
    fs::hard_link(wasm_src, &dest).unwrap_or_else(|_| {
        fs::copy(wasm_src, &dest).expect("copy shared component");
    });
    dest
}

fn write_pack_files(pack_dir: &Path, wasm_src: &Path, pack_name: &str) {
    fs::create_dir_all(pack_dir.join("flows")).expect("pack flow dir");
    let component_path = link_shared_component(wasm_src, pack_dir);
    write_sidecar(pack_dir, wasm_src);

    let pack_yaml = format!(
        r#"pack_id: dev.local.{pack_name}
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
      wasi:
        filesystem:
          mode: none
          mounts: []
        random: true
        clocks: true
      host:
        messaging:
          inbound: true
          outbound: true
        telemetry:
          scope: node
        secrets:
          required: []
    operations:
      - name: "{OPERATION}"
        input_schema: {{}}
        output_schema: {{}}
    wasm: "{}"

flows:
  - id: main
    file: flows/main.ygtc
    tags: [default]
    entrypoints: [main]
"#,
        component_path.strip_prefix(pack_dir).unwrap().display()
    );
    fs::write(pack_dir.join("pack.yaml"), pack_yaml).expect("pack.yaml");

    let flow = format!(
        r#"id: main
type: messaging
start: call
nodes:
  call:
    component.exec:
      component: {COMPONENT_ID}
      operation: {OPERATION}
      input:
        text: "hi from {pack_name}"
    routing:
      - out: true
"#
    );
    fs::write(pack_dir.join("flows").join("main.ygtc"), flow).expect("flow file");
}

fn write_sidecar(pack_dir: &Path, wasm_src: &Path) {
    let sidecar_path = pack_dir.join("flows/main.ygtc.resolve.json");
    let parent = sidecar_path.parent().expect("sidecar parent");
    fs::create_dir_all(parent).expect("sidecar dir");
    let digest = format!(
        "sha256:{:x}",
        Sha256::digest(fs::read(wasm_src).expect("read wasm"))
    );
    // flows/.. relative path to components/
    let rel_path = "../components/shared.component.wasm";
    let doc = serde_json::json!({
        "schema_version": 1,
        "flow": "flows/main.ygtc",
        "nodes": {
            "call": {
                "source": {
                    "kind": "local",
                    "path": rel_path,
                    "digest": digest,
                }
            }
        }
    });
    fs::write(
        &sidecar_path,
        serde_json::to_vec_pretty(&doc).expect("serialize sidecar"),
    )
    .expect("write sidecar");
}

fn build_pack(pack_dir: &Path, gtpack_name: &str) -> (PathBuf, PathBuf) {
    let gtpack_out = pack_dir.join("dist").join(gtpack_name);
    let manifest_out = pack_dir.join("dist").join("manifest.cbor");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    cmd.current_dir(workspace_root());
    cmd.args([
        "build",
        "--in",
        pack_dir.to_str().unwrap(),
        "--gtpack-out",
        gtpack_out.to_str().unwrap(),
        "--manifest",
        manifest_out.to_str().unwrap(),
        "--log",
        "warn",
    ]);
    let output = cmd.output().expect("run packc build");
    assert!(
        output.status.success(),
        "packc build failed for {}:\nstdout={}\nstderr={}",
        pack_dir.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (manifest_out, gtpack_out)
}

fn assert_operation_in_payload(manifest_path: &Path, gtpack_path: &Path) {
    let bytes = fs::read(manifest_path).expect("manifest bytes");
    let manifest: PackManifest =
        decode_pack_manifest(&bytes).expect("decode manifest.cbor into PackManifest");
    let flow = manifest
        .flows
        .iter()
        .find(|f| f.id.as_str() == "main")
        .expect("main flow present");
    let (node_id, node) = flow
        .flow
        .nodes
        .iter()
        .find(|(id, _)| id.as_str() == "call")
        .expect("call node present");

    let payload: &JsonValue = &node.input.mapping;
    let op_in_component = node
        .component
        .operation
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    assert!(
        !op_in_component.is_empty(),
        "pack {} ({}) missing component.operation binding for node {}: component_ref={}, payload={}",
        gtpack_path.display(),
        manifest_path.display(),
        node_id.as_str(),
        node.component.id.as_str(),
        payload
    );
}

#[test]
fn multi_pack_shared_component_has_operation_binding() {
    assert!(
        tool_available("greentic-component"),
        "greentic-component binary is required for this test"
    );
    if !online() {
        eprintln!(
            "skipping multi_pack_shared_component_has_operation_binding: offline environment"
        );
        return;
    }

    let temp = tempfile::tempdir().expect("temp dir");
    let shared_component_dir = temp.path().join("shared-component");
    fs::create_dir_all(&shared_component_dir).expect("shared component dir");

    let output = Command::new("greentic-component")
        .current_dir(&shared_component_dir)
        .args([
            "new",
            "--name",
            "shared-component",
            "--path",
            shared_component_dir.to_str().unwrap(),
            "--non-interactive",
            "--no-git",
            "--org",
            "dev.local",
            "--version",
            "0.1.0",
        ])
        .output()
        .expect("spawn greentic-component new");
    assert!(
        output.status.success(),
        "greentic-component new failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let output = Command::new("greentic-component")
        .current_dir(&shared_component_dir)
        .args(["build", "--manifest", "component.manifest.json", "--json"])
        .output()
        .expect("spawn greentic-component build");
    assert!(
        output.status.success(),
        "greentic-component build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let wasm_path = shared_component_dir.join("target/wasm32-wasip2/release/shared_component.wasm");
    assert!(
        wasm_path.exists(),
        "shared component wasm missing at {}",
        wasm_path.display()
    );

    let pack_a = temp.path().join("pack-a");
    let pack_b = temp.path().join("pack-b");
    write_pack_files(&pack_a, &wasm_path, "pack-a");
    write_pack_files(&pack_b, &wasm_path, "pack-b");

    build_pack(&pack_a, "pack-a.gtpack");
    let (manifest_b, gtpack_b) = build_pack(&pack_b, "pack-b.gtpack");

    assert_operation_in_payload(&manifest_b, &gtpack_b);
}
