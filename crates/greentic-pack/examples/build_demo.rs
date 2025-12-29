use std::fs;
use std::path::PathBuf;

use greentic_pack::builder::{
    ComponentArtifact, FlowBundle, PackBuilder, PackMeta, Provenance, Signing,
};
use semver::Version;
use serde_json::{Map, json};
use tempfile::tempdir;

fn main() -> anyhow::Result<()> {
    let out_path = resolve_out_path();
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let temp = tempdir()?;
    let wasm_path = temp.path().join("component.wasm");
    fs::write(&wasm_path, DEMO_WASM)?;

    let meta = PackMeta {
        pack_version: greentic_pack::builder::PACK_VERSION,
        pack_id: "ai.greentic.demo.local".into(),
        version: Version::parse("0.1.0").unwrap(),
        name: "Greentic Demo Pack".into(),
        kind: None,
        description: Some("Minimal pack built via PackBuilder".into()),
        authors: vec!["Greentic".into()],
        license: Some("MIT".into()),
        homepage: None,
        support: None,
        vendor: None,
        imports: vec![],
        entry_flows: vec!["demo".into()],
        created_at_utc: "2025-01-01T00:00:00Z".into(),
        events: None,
        repo: None,
        messaging: None,
        interfaces: Vec::new(),
        annotations: Map::new(),
        distribution: None,
        components: Vec::new(),
    };

    let flow_json = json!({
        "id": "demo",
        "kind": "flow/v1",
        "entry": "start",
        "nodes": []
    });
    let flow = FlowBundle {
        id: "demo".into(),
        kind: "flow/v1".into(),
        entry: "start".into(),
        yaml: "id: demo\nentry: start\n".into(),
        json: flow_json.clone(),
        hash_blake3: blake3::hash(&serde_json::to_vec(&flow_json)?)
            .to_hex()
            .to_string(),
        nodes: Vec::new(),
    };

    let component = ComponentArtifact {
        name: "demo-component".into(),
        version: Version::parse("1.0.0").unwrap(),
        wasm_path: wasm_path.clone(),
        schema_json: None,
        manifest_json: None,
        capabilities: None,
        world: Some("component:tool".into()),
        hash_blake3: None,
    };

    let provenance = Provenance {
        builder: "greentic-pack-demo".into(),
        git_commit: None,
        git_repo: None,
        toolchain: Some("rustc 1.85.0".into()),
        built_at_utc: "2025-01-01T00:00:00Z".into(),
        host: Some("local".into()),
        notes: Some("example build".into()),
    };

    let result = PackBuilder::new(meta)
        .with_flow(flow)
        .with_component(component)
        .with_signing(Signing::None)
        .with_provenance(provenance)
        .build(&out_path)?;

    println!("pack: {}", result.out_path.display());
    println!("manifest_blake3: {}", result.manifest_hash_blake3);

    Ok(())
}

fn resolve_out_path() -> PathBuf {
    let mut out = PathBuf::from("target/demo.gtpack");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => {
                let value = args.next().expect("--out requires a path argument");
                out = PathBuf::from(value);
            }
            "-h" | "--help" => {
                eprintln!(
                    "Usage: cargo run -p greentic-pack --example build_demo [-- --out <path>]"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("Unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    out
}

const DEMO_WASM: &[u8] = b"\0asm\x01\0\0\0";
