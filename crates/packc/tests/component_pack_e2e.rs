use assert_cmd::prelude::*;
use packc::config::PackConfig;
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

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

#[test]
fn end_to_end_component_pack_workflow() {
    if !tool_available("greentic-component") {
        panic!("greentic-component not installed; required for this E2E workflow test");
    }
    if !online() {
        eprintln!("skipping end_to_end_component_pack_workflow: offline environment");
        return;
    }

    let temp = tempfile::tempdir().expect("temp dir");
    let pack_dir = temp.path().join("demo-pack");

    // Scaffold pack
    Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
        .args([
            "new",
            "demo-pack",
            "--dir",
            pack_dir.to_str().unwrap(),
            "--log",
            "warn",
        ])
        .assert()
        .success();

    // Scaffold component inside pack
    let component_dir = pack_dir.join("components/demo-component");
    let output = Command::new("greentic-component")
        .current_dir(&pack_dir)
        .args([
            "new",
            "--name",
            "demo-component",
            "--path",
            component_dir.to_str().unwrap(),
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

    // Build the component
    let output = Command::new("greentic-component")
        .current_dir(&component_dir)
        .args(["build", "--manifest", "component.manifest.json", "--json"])
        .output()
        .expect("spawn greentic-component build");
    assert!(
        output.status.success(),
        "greentic-component build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let release_artifact = component_dir.join("target/wasm32-wasip2/release/demo_component.wasm");
    assert!(
        release_artifact.exists(),
        "component build should produce release artifact at {}",
        release_artifact.display()
    );

    // Doctor the component
    let output = Command::new("greentic-component")
        .current_dir(&component_dir)
        .args(["doctor", "component.manifest.json"])
        .output()
        .expect("spawn greentic-component doctor");
    assert!(
        output.status.success(),
        "greentic-component doctor failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Sync pack.yaml with built component
    Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
        .args([
            "components",
            "--in",
            pack_dir.to_str().unwrap(),
            "--log",
            "warn",
        ])
        .assert()
        .success();

    let cfg: PackConfig =
        serde_yaml_bw::from_str(&std::fs::read_to_string(pack_dir.join("pack.yaml")).unwrap())
            .unwrap();
    assert_eq!(cfg.components.len(), 1);
    assert_eq!(cfg.components[0].id, "demo-component");

    // Lint and build the pack
    Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
        .args(["lint", "--in", pack_dir.to_str().unwrap(), "--log", "warn"])
        .assert()
        .success();

    Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
        .args([
            "build",
            "--in",
            pack_dir.to_str().unwrap(),
            "--dry-run",
            "--log",
            "warn",
        ])
        .assert()
        .success();
}
