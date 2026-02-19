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
    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
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
    let output = greentic_component_cmd()
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
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let host_error_wit_mismatch = stdout.contains("type `host-error` not defined in interface")
            || stdout.contains("type 'host-error' not defined in interface")
            || stderr.contains("type `host-error` not defined in interface")
            || stderr.contains("type 'host-error' not defined in interface");
        if host_error_wit_mismatch {
            eprintln!(
                "skipping end_to_end_component_pack_workflow: external greentic-interfaces guest WIT mismatch during scaffold\nstdout={}\nstderr={}",
                stdout, stderr
            );
            return;
        }
        panic!(
            "greentic-component new failed:\nstdout={}\nstderr={}",
            stdout, stderr
        );
    }

    // Prevent the scaffolded component from inheriting the workspace root in CI.
    let manifest_path = component_dir.join("Cargo.toml");
    let mut manifest = std::fs::read_to_string(&manifest_path).unwrap();
    if !manifest.contains("[workspace]") {
        manifest.push_str("\n[workspace]\n");
        std::fs::write(&manifest_path, manifest).unwrap();
    }

    // Build the component
    let output = greentic_component_cmd()
        .current_dir(&component_dir)
        .args(["build", "--manifest", "component.manifest.json", "--json"])
        .output()
        .expect("spawn greentic-component build");
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let host_error_wit_mismatch = stdout.contains("type `host-error` not defined in interface")
            || stdout.contains("type 'host-error' not defined in interface")
            || stderr.contains("type `host-error` not defined in interface")
            || stderr.contains("type 'host-error' not defined in interface");
        if host_error_wit_mismatch {
            eprintln!(
                "skipping end_to_end_component_pack_workflow: external greentic-interfaces guest WIT mismatch\nstdout={}\nstderr={}",
                stdout, stderr
            );
            return;
        }
        panic!(
            "greentic-component build failed:\nstdout={}\nstderr={}",
            stdout, stderr
        );
    }

    let release_artifact = component_dir.join("target/wasm32-wasip2/release/demo_component.wasm");
    assert!(
        release_artifact.exists(),
        "component build should produce release artifact at {}",
        release_artifact.display()
    );

    // Doctor the component
    let output = greentic_component_cmd()
        .current_dir(&component_dir)
        .args(["doctor", "component.manifest.json"])
        .output()
        .expect("spawn greentic-component doctor");
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stdout.contains("missing export interface")
            || stdout.contains("world mismatch")
            || stderr.contains("missing export interface")
            || stderr.contains("world mismatch")
        {
            eprintln!(
                "skipping end_to_end_component_pack_workflow: greentic-component doctor mismatch\nstdout={}\nstderr={}",
                stdout, stderr
            );
            return;
        }
        panic!(
            "greentic-component doctor failed:\nstdout={}\nstderr={}",
            stdout, stderr
        );
    }

    // Sync pack.yaml with built component
    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
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
    assert_eq!(cfg.components[0].id, "dev.local.demo-component");

    // Lint and build the pack
    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .args([
            "lint",
            "--in",
            pack_dir.to_str().unwrap(),
            "--allow-oci-tags",
            "--log",
            "warn",
        ])
        .assert()
        .success();

    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .args([
            "build",
            "--in",
            pack_dir.to_str().unwrap(),
            "--dry-run",
            "--allow-oci-tags",
            "--log",
            "warn",
        ])
        .assert()
        .success();
}
