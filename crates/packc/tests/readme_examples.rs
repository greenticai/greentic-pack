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
fn readme_demo_build_and_doctor() {
    let pack_dir = workspace_root().join("examples/weather-demo");
    write_weather_sidecar(&pack_dir);
    let temp = tempfile::tempdir().expect("temp dir");
    let gtpack_out = temp.path().join("weather-demo.gtpack");

    let mut build_cmd = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"));
    build_cmd.current_dir(workspace_root()).args([
        "build",
        "--in",
        "examples/weather-demo",
        "--gtpack-out",
        gtpack_out.to_str().unwrap(),
        "--log",
        "warn",
    ]);
    build_cmd.assert().success();

    let mut doctor_cmd = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"));
    doctor_cmd.current_dir(workspace_root()).args([
        "doctor",
        gtpack_out.to_str().unwrap(),
        "--json",
    ]);
    doctor_cmd.assert().success();
}
