use std::collections::BTreeMap;
use std::path::PathBuf;

use greentic_types::flow_resolve::{
    ComponentSourceRefV1, FlowResolveV1, NodeResolveV1, write_flow_resolve,
};
use packc::config::FlowConfig;
use packc::flow_resolve::discover_flow_resolves;
use tempfile::TempDir;

fn flow_config(name: &str) -> FlowConfig {
    FlowConfig {
        id: name.to_string(),
        file: PathBuf::from(format!("flows/{name}.ygtc")),
        tags: Vec::new(),
        entrypoints: Vec::new(),
    }
}

#[test]
fn discovers_and_parses_sidecars() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path();
    let flows_dir = pack_dir.join("flows");
    std::fs::create_dir_all(&flows_dir).unwrap();

    let fixtures = vec![
        (
            "local",
            ComponentSourceRefV1::Local {
                path: "../components/demo.wasm".into(),
                digest: Some("sha256:1111".into()),
            },
        ),
        (
            "oci",
            ComponentSourceRefV1::Oci {
                r#ref: "ghcr.io/demo/component:1.0.0".into(),
                digest: Some("sha256:2222".into()),
            },
        ),
        (
            "repo",
            ComponentSourceRefV1::Repo {
                r#ref: "repo://example/component?rev=main".into(),
                digest: None,
            },
        ),
        (
            "store",
            ComponentSourceRefV1::Store {
                r#ref: "store://component/paid".into(),
                digest: Some("sha256:3333".into()),
                license_hint: Some("commercial".into()),
                meter: Some(true),
            },
        ),
    ];

    let mut flow_configs = Vec::new();
    for (name, source) in &fixtures {
        let flow_path = flows_dir.join(format!("{name}.ygtc"));
        std::fs::write(&flow_path, "# flow stub").unwrap();

        let mut nodes = BTreeMap::new();
        nodes.insert(
            "node".into(),
            NodeResolveV1 {
                source: source.clone(),
                mode: None,
            },
        );
        let doc = FlowResolveV1 {
            schema_version: 1,
            flow: format!("{name}.ygtc"),
            nodes,
        };
        let sidecar_path = flow_path.with_file_name(format!("{name}.ygtc.resolve.json"));
        write_flow_resolve(&sidecar_path, &doc).expect("write sidecar");

        flow_configs.push(flow_config(name));
    }

    let discovered = discover_flow_resolves(pack_dir, &flow_configs);
    assert_eq!(discovered.len(), fixtures.len());
    for entry in &discovered {
        assert!(entry.warning.is_none(), "expected no warning");
        let doc = entry.document.as_ref().expect("sidecar parsed");
        assert_eq!(doc.schema_version, 1);
        assert!(doc.nodes.contains_key("node"));
    }
}

#[test]
fn missing_sidecar_emits_warning() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path();
    let flows_dir = pack_dir.join("flows");
    std::fs::create_dir_all(&flows_dir).unwrap();

    let flow_path = flows_dir.join("missing.ygtc");
    std::fs::write(&flow_path, "# flow stub").unwrap();

    let flows = vec![flow_config("missing")];
    let discovered = discover_flow_resolves(pack_dir, &flows);
    assert_eq!(discovered.len(), 1);
    let entry = &discovered[0];
    assert!(entry.document.is_none());
    assert!(entry.warning.is_some());
}
