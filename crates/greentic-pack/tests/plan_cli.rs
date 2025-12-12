use greentic_types::ResourceHints;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use assert_cmd::prelude::*;
use greentic_pack::builder::{ComponentArtifact, FlowBundle, PackBuilder, PackMeta};
use greentic_pack::builder::{ComponentPin, ImportRef, NodeRef};
use greentic_pack::events::{
    EventProviderCapabilities, EventProviderKind, EventProviderSpec, EventsSection, OrderingKind,
    ReliabilityKind, TransportKind,
};
use greentic_types::component::{
    ComponentCapabilities, ComponentManifest, ComponentProfiles, HostCapabilities,
    SecretsCapabilities, TelemetryCapabilities, WasiCapabilities,
};
use greentic_types::flow::FlowKind;
use semver::Version;
use serde_json::{Value, json};
use tempfile::TempDir;

fn sample_pack() -> TempDir {
    let temp = TempDir::new().expect("temp dir");
    let pack_path = temp.path().join("sample.gtpack");
    build_sample_pack(&pack_path);
    temp
}

fn build_sample_pack(out_path: &Path) {
    let component_manifest = ComponentManifest {
        id: "component.a".parse().unwrap(),
        version: Version::parse("1.0.0").unwrap(),
        supports: vec![FlowKind::Messaging],
        world: "greentic:demo@1.0.0".into(),
        profiles: ComponentProfiles {
            default: Some("default".into()),
            supported: vec!["default".into()],
        },
        capabilities: ComponentCapabilities {
            wasi: WasiCapabilities::default(),
            host: HostCapabilities {
                secrets: Some(SecretsCapabilities {
                    required: vec!["API_TOKEN".into()],
                }),
                telemetry: Some(TelemetryCapabilities {
                    scope: greentic_types::component::TelemetryScope::Tenant,
                }),
                ..Default::default()
            },
        },
        configurators: None,
        operations: Vec::new(),
        config_schema: None,
        resources: ResourceHints::default(),
        dev_flows: BTreeMap::new(),
    };

    let manifest_json = serde_json::to_string_pretty(&component_manifest).unwrap();

    let mut meta = PackMeta {
        pack_version: greentic_pack::builder::PACK_VERSION,
        pack_id: "demo.pack".into(),
        version: Version::parse("1.2.3").unwrap(),
        name: "Demo Pack".into(),
        kind: None,
        description: None,
        authors: vec!["Greentic".into()],
        license: Some("MIT".into()),
        homepage: None,
        support: None,
        vendor: None,
        imports: vec![ImportRef {
            pack_id: "dependency.pack".into(),
            version_req: "*".into(),
        }],
        entry_flows: vec!["flow.main".into()],
        created_at_utc: "2025-01-01T00:00:00Z".into(),
        events: Some(EventsSection {
            providers: vec![EventProviderSpec {
                name: "nats-core".into(),
                kind: EventProviderKind::Broker,
                component: "nats-provider@1.0.0".into(),
                default_flow: Some("flows/events/nats/default.ygtc".into()),
                custom_flow: None,
                capabilities: EventProviderCapabilities {
                    transport: Some(TransportKind::Nats),
                    reliability: Some(ReliabilityKind::AtLeastOnce),
                    ordering: Some(OrderingKind::PerKey),
                    topics: vec!["greentic.*".into()],
                },
            }],
        }),
        repo: None,
        messaging: None,
        interfaces: Vec::new(),
        annotations: serde_json::Map::new(),
        distribution: None,
        components: Vec::new(),
    };
    meta.annotations.insert(
        "connectors".into(),
        json!({
            "messaging": {
                "teams": {
                    "primary": {
                        "flow": "flow.main",
                        "name": "teams-primary"
                    }
                }
            }
        }),
    );

    let flow_json = json!({
        "id": "flow.main",
        "kind": "messaging",
        "entry": "start",
        "nodes": [],
    });
    let flow_yaml = "id: flow.main\nkind: messaging\nentry: start\n";
    let hash = blake3::hash(flow_json.to_string().as_bytes())
        .to_hex()
        .to_string();
    let flow = FlowBundle {
        id: "flow.main".into(),
        kind: "messaging".into(),
        entry: "start".into(),
        yaml: flow_yaml.into(),
        json: flow_json,
        hash_blake3: hash,
        nodes: vec![NodeRef {
            node_id: "start".into(),
            component: ComponentPin {
                name: "component.a".into(),
                version_req: "*".into(),
            },
            schema_id: None,
        }],
    };

    let wasm_dir = TempDir::new().expect("component temp dir");
    let wasm_path = wasm_dir.path().join("component.wasm");
    fs::write(&wasm_path, minimal_wasm()).unwrap();

    let component = ComponentArtifact {
        name: "component.a".into(),
        version: Version::parse("1.0.0").unwrap(),
        wasm_path: wasm_path.clone(),
        schema_json: None,
        manifest_json: Some(manifest_json),
        capabilities: Some(serde_json::to_value(component_manifest.capabilities).unwrap()),
        world: Some("greentic:demo@1.0.0".into()),
        hash_blake3: None,
    };

    let builder = PackBuilder::new(meta)
        .with_flow(flow)
        .with_component(component);
    builder.build(out_path).expect("build sample pack");
}

fn minimal_wasm() -> Vec<u8> {
    // precompiled empty module.
    vec![
        0x00, 0x61, 0x73, 0x6d, // magic
        0x01, 0x00, 0x00, 0x00, // version
    ]
}

#[test]
fn plan_from_gtpack_cli() {
    let pack = sample_pack();
    let path = pack.path().join("sample.gtpack");
    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .args([
            "plan",
            path.to_str().unwrap(),
            "--tenant",
            "tenant.demo",
            "--environment",
            "prod",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        value.get("pack_id").and_then(|v| v.as_str()),
        Some("demo.pack")
    );
    assert_eq!(
        value.get("environment").and_then(|v| v.as_str()),
        Some("prod")
    );
}

#[test]
fn plan_from_directory_uses_packc_stub() {
    let pack = sample_pack();
    let pack_path = pack.path().join("sample.gtpack");

    let fake_source = TempDir::new().expect("fake source");

    let stub_dir = TempDir::new().expect("stub dir");
    let stub_path = stub_dir.path().join("packc");
    create_stub_packc(&stub_path, &pack_path);

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .env(PACKC_ENV, &stub_path)
        .args([
            "plan",
            fake_source.path().to_str().unwrap(),
            "--tenant",
            "tenant.demo",
            "--environment",
            "dev",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        value.get("pack_id").and_then(|v| v.as_str()),
        Some("demo.pack")
    );
}

#[test]
fn events_list_shows_providers_in_table() {
    let pack = sample_pack();
    let path = pack.path().join("sample.gtpack");
    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .args(["events", "list", path.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);
    assert!(stdout.contains("nats-core"));
    assert!(stdout.contains("broker"));
    assert!(stdout.contains("nats"));
}

#[test]
fn events_list_supports_json_output() {
    let pack = sample_pack();
    let path = pack.path().join("sample.gtpack");
    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .args(["events", "list", "--format", "json", path.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).unwrap();
    let providers = value.as_array().expect("providers array");
    assert_eq!(providers.len(), 1);
    let provider = providers.first().unwrap();
    assert_eq!(
        provider.get("component").and_then(|val| val.as_str()),
        Some("nats-provider@1.0.0")
    );
}

const PACKC_ENV: &str = "GREENTIC_PACK_PLAN_PACKC";

fn create_stub_packc(stub_path: &Path, pack_path: &Path) {
    let mut file = fs::File::create(stub_path).expect("stub packc");
    writeln!(
        file,
        "#!/usr/bin/env bash\nset -euo pipefail\nout=\"\"\nwhile [[ $# -gt 0 ]]; do\n  if [[ \"$1\" == \"--gtpack-out\" ]]; then\n    out=\"$2\"\n    shift 2\n  else\n    shift\n  fi\ndone\ncp \"{}\" \"$out\"\n",
        pack_path.display()
    )
    .unwrap();
    drop(file);
    let mut perms = fs::metadata(stub_path).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
    }
    fs::set_permissions(stub_path, perms).unwrap();
}
