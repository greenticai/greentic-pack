//! A flow built from the designer's Deep-worker node (`operala.call` with a
//! sibling `operation: <worker>`) must build with no resolve mapping and keep
//! the runtime-native kind in the manifest. It failed with "missing resolve
//! summary entries" while greentic-flow lowered it to `component.exec`.

use std::fs;

use assert_cmd::Command;
use greentic_types::{decode_pack_manifest, pack_manifest::PackManifest};
use tempfile::TempDir;

const FLOW: &str = r#"id: deep
schema_version: 2
type: messaging
start: plan
nodes:
  plan:
    operala.call:
      goal: "{{in.text}}"
    operation: w1
    routing:
      - to: lookup
  lookup:
    sorla.call:
      query: "{{in.text}}"
    operation: orders
    routing:
      - to: gate
  gate:
    approval.call:
      reason: "confirm"
    routing: out
"#;

const PACK_YAML: &str = r#"pack_id: dev.local.deep
version: 0.1.0
kind: application
publisher: Test
components: []
flows:
  - id: deep
    file: flows/deep.ygtc
    tags: [messaging]
"#;

#[test]
fn build_keeps_runtime_native_call_nodes_without_resolve_mapping() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path();
    fs::create_dir_all(pack_dir.join("flows")).expect("flows dir");
    fs::write(pack_dir.join("flows/deep.ygtc"), FLOW).expect("write flow");
    fs::write(pack_dir.join("pack.yaml"), PACK_YAML).expect("write pack.yaml");

    let manifest_path = pack_dir.join("dist/manifest.cbor");
    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(pack_dir)
        .args([
            "build",
            "--in",
            pack_dir.to_str().expect("utf-8 path"),
            "--allow-pack-schema",
            "--no-update",
            "--manifest",
            manifest_path.to_str().expect("utf-8 path"),
            "--gtpack-out",
            pack_dir
                .join("dist/deep.gtpack")
                .to_str()
                .expect("utf-8 path"),
            "--log",
            "warn",
        ])
        .output()
        .expect("run packc build");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "runtime-native call nodes must not require resolve mappings:\nstdout={}\nstderr={stderr}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        !stderr.contains("missing resolve summary entries"),
        "unexpected resolve-summary complaint: {stderr}"
    );

    let manifest: PackManifest =
        decode_pack_manifest(&fs::read(&manifest_path).expect("manifest bytes"))
            .expect("decode manifest.cbor");
    let flow = manifest
        .flows
        .iter()
        .find(|f| f.id.as_str() == "deep")
        .expect("deep flow present");
    let node = |id: &str| {
        flow.flow
            .nodes
            .iter()
            .find(|(node_id, _)| node_id.as_str() == id)
            .map(|(_, node)| node)
            .unwrap_or_else(|| panic!("node {id} present"))
    };

    assert_eq!(node("plan").component.id.as_str(), "operala.call");
    assert_eq!(node("plan").component.operation.as_deref(), Some("w1"));
    assert_eq!(node("lookup").component.id.as_str(), "sorla.call");
    assert_eq!(
        node("lookup").component.operation.as_deref(),
        Some("orders")
    );
    assert_eq!(node("gate").component.id.as_str(), "approval.call");
    assert!(
        manifest.components.is_empty(),
        "no pack component may be demanded for a runtime-native node: {:?}",
        manifest
            .components
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>()
    );
}
