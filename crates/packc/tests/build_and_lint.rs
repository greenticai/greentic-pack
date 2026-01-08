use assert_cmd::prelude::*;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn write_weather_sidecar(pack_dir: &Path) {
    let sidecar_path = pack_dir.join("flows/weather_bot.ygtc.resolve.json");
    let parent = sidecar_path
        .parent()
        .expect("sidecar parent exists")
        .to_path_buf();
    std::fs::create_dir_all(&parent).expect("create flows dir");
    // Use digest-pinned repo refs to satisfy resolver without network.
    let doc = json!({
        "schema_version": 1,
        "flow": "flows/weather_bot.ygtc",
        "nodes": {
            "collect_location": { "source": { "kind": "repo", "ref": "io.3bridges.components.qa@1.0.0", "digest": "sha256:deadbeef" } },
            "forecast_weather": { "source": { "kind": "repo", "ref": "io.3bridges.components.mcp@1.0.0", "digest": "sha256:deadbeef" } },
            "weather_text": { "source": { "kind": "local", "path": "../components/templating.handlebars.wasm", "digest": "sha256:deadbeef" } },
        }
    });
    std::fs::write(sidecar_path, serde_json::to_vec_pretty(&doc).unwrap()).expect("write sidecar");
}

#[test]
fn build_weather_demo_dry_run() {
    let pack_dir = workspace_root().join("examples/weather-demo");
    write_weather_sidecar(&pack_dir);
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
