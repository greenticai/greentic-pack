use greentic_types::cbor::canonical;
use greentic_types::schemas::common::schema_ir::{AdditionalProperties, SchemaIr};
use greentic_types::schemas::component::v0_6_0::{
    ComponentDescribe, ComponentInfo, ComponentOperation, ComponentRunInput, ComponentRunOutput,
    schema_hash,
};
use greentic_types::{decode_pack_manifest, pack_manifest::PackManifest};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const COMPONENT_WASM_BASENAME: &str = "shared.component";
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

fn greentic_component_cmd() -> Command {
    let mut cmd = Command::new("greentic-component");
    cmd.env_remove("CARGO_TARGET_DIR");
    cmd
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
        .join(format!("{COMPONENT_WASM_BASENAME}.wasm"));
    fs::create_dir_all(dest.parent().unwrap()).expect("components dir");
    if dest.exists() {
        fs::remove_file(&dest).expect("remove existing link");
    }
    fs::hard_link(wasm_src, &dest).unwrap_or_else(|_| {
        fs::copy(wasm_src, &dest).expect("copy shared component");
    });
    dest
}

fn write_pack_files(pack_dir: &Path, wasm_src: &Path, pack_name: &str, component_id: &str) {
    fs::create_dir_all(pack_dir.join("flows")).expect("pack flow dir");
    let component_path = link_shared_component(wasm_src, pack_dir);
    write_describe_sidecar(&component_path, component_id);
    write_summary(pack_dir, wasm_src, component_id);

    let pack_yaml = format!(
        r#"pack_id: dev.local.{pack_name}
version: 0.0.1
kind: application
publisher: Test

components:
  - id: "{component_id}"
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
    {OPERATION}:
      text: "hi from {pack_name}"
    routing:
      - out: true
"#
    );
    fs::write(pack_dir.join("flows").join("main.ygtc"), flow).expect("flow file");
}

fn write_describe_sidecar(wasm_path: &Path, component_id: &str) {
    let input_schema = SchemaIr::Object {
        properties: BTreeMap::new(),
        required: Vec::new(),
        additional: AdditionalProperties::Forbid,
    };
    let output_schema = SchemaIr::Object {
        properties: BTreeMap::new(),
        required: Vec::new(),
        additional: AdditionalProperties::Forbid,
    };
    let config_schema = SchemaIr::Object {
        properties: BTreeMap::new(),
        required: Vec::new(),
        additional: AdditionalProperties::Forbid,
    };
    let hash = schema_hash(&input_schema, &output_schema, &config_schema).expect("schema hash");
    let operation = ComponentOperation {
        id: OPERATION.to_string(),
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
    let describe_path = format!("{}.describe.cbor", wasm_path.display());
    fs::write(describe_path, bytes).expect("write describe cache");
}

fn write_summary(pack_dir: &Path, wasm_src: &Path, component_id: &str) {
    let summary_path = pack_dir.join("flows/main.ygtc.resolve.summary.json");
    let parent = summary_path.parent().expect("summary parent");
    fs::create_dir_all(parent).expect("summary dir");
    let digest = format!(
        "sha256:{:x}",
        Sha256::digest(fs::read(wasm_src).expect("read wasm"))
    );
    // flows/.. relative path to components/
    let rel_path = "../components/shared.component.wasm";
    let doc = serde_json::json!({
        "schema_version": 1,
        "flow": "main.ygtc",
        "nodes": {
            "call": {
                "component_id": component_id,
                "source": {
                    "kind": "local",
                    "path": rel_path
                },
                "digest": digest
            }
        }
    });
    fs::write(
        &summary_path,
        serde_json::to_vec_pretty(&doc).expect("serialize summary"),
    )
    .expect("write summary");
}

fn read_component_id(manifest_path: &Path) -> String {
    let raw = fs::read_to_string(manifest_path).expect("read component manifest");
    let json: serde_json::Value = serde_json::from_str(&raw).expect("parse component manifest");
    json.get("id")
        .and_then(|v| v.as_str())
        .expect("component manifest missing id")
        .to_string()
}

fn build_pack(pack_dir: &Path, gtpack_name: &str) -> (PathBuf, PathBuf) {
    let gtpack_out = pack_dir.join("dist").join(gtpack_name);
    let manifest_out = pack_dir.join("dist").join("manifest.cbor");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"));
    cmd.current_dir(workspace_root());
    cmd.env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1");
    cmd.args([
        "build",
        "--in",
        pack_dir.to_str().unwrap(),
        "--allow-pack-schema",
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

    let output = greentic_component_cmd()
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

    let output = greentic_component_cmd()
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
    let component_id = read_component_id(&shared_component_dir.join("component.manifest.json"));

    let wasm_path = shared_component_dir.join("target/wasm32-wasip2/release/shared_component.wasm");
    assert!(
        wasm_path.exists(),
        "shared component wasm missing at {}",
        wasm_path.display()
    );

    let pack_a = temp.path().join("pack-a");
    let pack_b = temp.path().join("pack-b");
    write_pack_files(&pack_a, &wasm_path, "pack-a", &component_id);
    write_pack_files(&pack_b, &wasm_path, "pack-b", &component_id);

    build_pack(&pack_a, "pack-a.gtpack");
    let (manifest_b, gtpack_b) = build_pack(&pack_b, "pack-b.gtpack");

    assert_operation_in_payload(&manifest_b, &gtpack_b);
}
