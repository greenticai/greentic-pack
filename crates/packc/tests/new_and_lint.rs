use assert_cmd::prelude::*;
use greentic_types::{ComponentCapabilities, ComponentProfiles};
use packc::config::{ComponentConfig, FlowKindLabel, PackConfig};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

#[test]
fn build_weather_demo_dry_run() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    cmd.current_dir(workspace_root());
    cmd.args([
        "build",
        "--in",
        "examples/weather-demo",
        "--dry-run",
        "--log",
        "warn",
    ]);
    cmd.assert().success();
}

#[test]
fn lint_weather_demo() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    cmd.current_dir(workspace_root());
    cmd.args(["lint", "--in", "examples/weather-demo", "--log", "warn"]);
    cmd.assert().success();
}

#[test]
fn new_pack_scaffold() {
    let temp = tempfile::tempdir().expect("temp dir");
    let pack_dir = temp.path().join("demo-pack");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    cmd.current_dir(workspace_root());
    cmd.args([
        "new",
        "demo-pack",
        "--dir",
        pack_dir.to_str().unwrap(),
        "--log",
        "warn",
    ]);
    cmd.assert().success();

    assert!(pack_dir.join("pack.yaml").exists(), "pack.yaml missing");
    assert!(
        pack_dir.join("flows").join("main.ygtc").exists(),
        "flow missing"
    );
    assert!(
        pack_dir.join("components").is_dir(),
        "components directory missing"
    );
    assert!(
        !pack_dir.join("components").join("stub.wasm").exists(),
        "stub component should not be scaffolded"
    );
}

#[test]
fn components_command_reflects_directory() {
    let temp = tempfile::tempdir().expect("temp dir");
    let pack_dir = temp.path().join("demo-pack");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    cmd.current_dir(workspace_root());
    cmd.args([
        "new",
        "demo-pack",
        "--dir",
        pack_dir.to_str().unwrap(),
        "--log",
        "warn",
    ]);
    cmd.assert().success();

    let components_dir = pack_dir.join("components");
    fs::write(components_dir.join("alpha.wasm"), [0x00, 0x61, 0x73, 0x6d]).unwrap();
    fs::write(components_dir.join("beta.wasm"), [0x00, 0x61, 0x73, 0x6d]).unwrap();

    let pack_yaml = pack_dir.join("pack.yaml");
    let mut cfg: PackConfig =
        serde_yaml_bw::from_str(&fs::read_to_string(&pack_yaml).unwrap()).unwrap();
    cfg.components.push(ComponentConfig {
        id: "old.component".to_string(),
        version: "0.1.0".to_string(),
        world: "greentic:component/stub".to_string(),
        supports: vec![FlowKindLabel::Messaging],
        profiles: ComponentProfiles {
            default: Some("default".to_string()),
            supported: vec!["default".to_string()],
        },
        capabilities: ComponentCapabilities::default(),
        wasm: PathBuf::from("components/old.wasm"),
        operations: Vec::new(),
        config_schema: None,
        resources: None,
        configurators: None,
    });
    fs::write(&pack_yaml, serde_yaml_bw::to_string(&cfg).unwrap()).unwrap();

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    cmd.current_dir(workspace_root());
    cmd.args([
        "components",
        "--in",
        pack_dir.to_str().unwrap(),
        "--log",
        "warn",
    ]);
    cmd.assert().success();

    let cfg: PackConfig =
        serde_yaml_bw::from_str(&fs::read_to_string(&pack_yaml).unwrap()).unwrap();
    let ids: Vec<_> = cfg.components.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, vec!["alpha", "beta"]);

    let wasm_paths: Vec<_> = cfg
        .components
        .iter()
        .map(|c| c.wasm.to_string_lossy().to_string())
        .collect();
    assert_eq!(
        wasm_paths,
        vec!["components/alpha.wasm", "components/beta.wasm"]
    );
}
