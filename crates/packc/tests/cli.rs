use assert_cmd::prelude::*;
use serde_json::Value;
use std::fs;
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;
use walkdir::WalkDir;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

#[test]
fn dry_run_weather_demo_succeeds() {
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
fn dry_run_rejects_missing_manifest() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    cmd.current_dir(workspace_root());
    cmd.args(["build", "--in", "examples", "--dry-run"]);
    cmd.assert().failure();
}

#[test]
fn scaffold_minimal_pack_builds() {
    let temp = tempdir().expect("temp dir");
    let pack_dir = temp.path().join("demo-pack");

    let mut scaffold = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    scaffold.current_dir(workspace_root());
    scaffold.args([
        "new",
        "demo-pack",
        "--dir",
        pack_dir.to_str().unwrap(),
        "--log",
        "warn",
    ]);
    scaffold.assert().success();

    assert!(pack_dir.join("pack.yaml").exists(), "pack.yaml missing");
    assert!(
        pack_dir.join("flows").join("welcome.ygtc").exists(),
        "flow file missing"
    );

    let mut build = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    build.current_dir(workspace_root());
    build.args([
        "build",
        "--in",
        pack_dir.to_str().unwrap(),
        "--dry-run",
        "--log",
        "warn",
    ]);
    build.assert().success();
}

#[test]
fn scaffold_with_sign_generates_keys() {
    let temp = tempdir().expect("temp dir");
    let pack_dir = temp.path().join("signed-pack");

    let mut scaffold = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    scaffold.current_dir(workspace_root());
    scaffold.args([
        "new",
        "signed-pack",
        "--dir",
        pack_dir.to_str().unwrap(),
        "--sign",
        "--log",
        "warn",
    ]);
    scaffold.assert().success();

    let private_key =
        fs::read_to_string(pack_dir.join("keys/dev_ed25519.sk")).expect("private key present");
    assert!(
        private_key.contains("PRIVATE KEY"),
        "private key should be PEM"
    );

    let public_key =
        fs::read_to_string(pack_dir.join("keys/dev_ed25519.pk")).expect("public key present");
    assert!(
        public_key.contains("PUBLIC KEY"),
        "public key should be PEM"
    );
}

#[test]
fn scaffold_build_script_uses_pack_id() {
    let temp = tempdir().expect("temp dir");
    let pack_id = "demo-pack";
    let pack_dir = temp.path().join(pack_id);

    let mut scaffold = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    scaffold.current_dir(workspace_root());
    scaffold.args([
        "new",
        pack_id,
        "--dir",
        pack_dir.to_str().unwrap(),
        "--log",
        "warn",
    ]);
    scaffold.assert().success();

    let build_script = fs::read_to_string(pack_dir.join("scripts").join("build.sh"))
        .expect("build script present");
    assert!(
        build_script.contains("dist/demo-pack.wasm"),
        "build script should emit wasm named after pack id"
    );
    assert!(
        build_script.contains("dist/demo-pack.gtpack"),
        "build script should emit gtpack named after pack id"
    );
}
#[test]
fn build_outputs_gtpack_archive() {
    let temp = tempdir().expect("temp dir");
    let base = temp.path();
    if std::env::var("PACKC_OFFLINE").is_ok()
        || std::env::var("CARGO_NET_OFFLINE")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false)
        || !dns_resolves_index_crates()
    {
        eprintln!("skipping gtpack archive test; offline mode detected");
        return;
    }
    let wasm_target_installed = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|stdout| stdout.lines().any(|line| line.trim() == "wasm32-wasip2"))
        .unwrap_or(false);
    if !wasm_target_installed {
        eprintln!("skipping gtpack archive test; wasm32-wasip2 target missing");
        return;
    }
    let wasm = base.join("pack.wasm");
    let manifest = base.join("manifest.cbor");
    let sbom = base.join("sbom.cdx.json");
    let gtpack = base.join("pack.gtpack");
    let component_data = base.join("pack_component").join("src").join("data.rs");

    let mut build = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    build.current_dir(workspace_root());
    build.args([
        "build",
        "--in",
        "examples/weather-demo",
        "--out",
        wasm.to_str().unwrap(),
        "--manifest",
        manifest.to_str().unwrap(),
        "--sbom",
        sbom.to_str().unwrap(),
        "--gtpack-out",
        gtpack.to_str().unwrap(),
        "--component-data",
        component_data.to_str().unwrap(),
        "--log",
        "warn",
    ]);
    build.assert().success();

    let mut inspect = Command::new("cargo");
    inspect.current_dir(workspace_root());
    inspect.args([
        "run",
        "-p",
        "greentic-pack",
        "--bin",
        "gtpack-inspect",
        "--",
        "--policy",
        "devok",
        "--json",
        gtpack.to_str().unwrap(),
    ]);
    let output = inspect
        .output()
        .expect("gtpack-inspect should run successfully");
    assert!(output.status.success(), "gtpack-inspect failed");
    let report: Value =
        serde_json::from_slice(&output.stdout).expect("gtpack-inspect produced valid JSON");
    let sbom_entries = report
        .get("sbom")
        .and_then(Value::as_array)
        .expect("sbom array present");
    assert!(
        sbom_entries.iter().all(|entry| {
            entry
                .get("media_type")
                .and_then(Value::as_str)
                .map(|val| !val.is_empty())
                .unwrap_or(false)
        }),
        "sbom entries must expose media_type"
    );
}

fn dns_resolves_index_crates() -> bool {
    ("index.crates.io", 443)
        .to_socket_addrs()
        .map(|mut addrs| addrs.next().is_some())
        .unwrap_or(false)
}

#[test]
fn lint_accepts_valid_events_provider_block() {
    let temp = tempdir().expect("temp dir");
    let pack_dir = temp.path().join("weather-demo");
    copy_example_pack(&pack_dir);
    inject_events_section(&pack_dir, "broker");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    cmd.current_dir(workspace_root());
    cmd.args(["lint", "--in", pack_dir.to_str().unwrap(), "--log", "warn"]);
    cmd.assert().success();
}

#[test]
fn lint_rejects_invalid_events_kind() {
    let temp = tempdir().expect("temp dir");
    let pack_dir = temp.path().join("weather-demo");
    copy_example_pack(&pack_dir);
    inject_events_section(&pack_dir, "invalid-kind");

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
        .args(["lint", "--in", pack_dir.to_str().unwrap(), "--log", "warn"])
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_lowercase();
    assert!(
        stderr.contains("events") && stderr.contains("kind") || stderr.contains("unknown variant"),
        "stderr should mention invalid kind, got: {stderr}"
    );
}

#[test]
fn lint_accepts_valid_messaging_adapter_block() {
    let temp = tempdir().expect("temp dir");
    let pack_dir = temp.path().join("weather-demo");
    copy_example_pack(&pack_dir);
    inject_messaging_section(&pack_dir);

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    cmd.current_dir(workspace_root());
    cmd.args(["lint", "--in", pack_dir.to_str().unwrap(), "--log", "warn"]);
    cmd.assert().success();
}

#[test]
fn lint_accepts_valid_repo_scanner_pack() {
    let temp = tempdir().expect("temp dir");
    let pack_dir = temp.path().join("weather-demo");
    copy_example_pack(&pack_dir);
    inject_repo_section(
        &pack_dir,
        r#"
repo:
  kind: scanner
  capabilities:
    scan:
      - "sast"
      - "deps"
  bindings:
    scan:
      - package: "greentic:scan"
        world: "scanner"
        version: "1.0.0"
        component: "scanner-snyk"
        entrypoint: "scan"
"#,
    );

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    cmd.current_dir(workspace_root());
    cmd.args(["lint", "--in", pack_dir.to_str().unwrap(), "--log", "warn"]);
    cmd.assert().success();
}

#[test]
fn lint_accepts_valid_billing_pack() {
    let temp = tempdir().expect("temp dir");
    let pack_dir = temp.path().join("billing-demo");
    copy_example_pack(&pack_dir);
    inject_repo_section(
        &pack_dir,
        r#"
repo:
  kind: billing-provider
  capabilities:
    billing:
      - "metered"
  bindings:
    billing:
      - package: "greentic:billing"
        world: "provider"
        version: "1.0.0"
        component: "billing-generic"
        entrypoint: "serve"
interfaces:
  - package: "greentic:analytics"
    world: "tracker"
    version: "1.0.0"
"#,
    );

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    cmd.current_dir(workspace_root());
    cmd.args(["lint", "--in", pack_dir.to_str().unwrap(), "--log", "warn"]);
    cmd.assert().success();
}

#[test]
fn lint_rejects_repo_missing_binding() {
    let temp = tempdir().expect("temp dir");
    let pack_dir = temp.path().join("weather-demo");
    copy_example_pack(&pack_dir);
    inject_repo_section(
        &pack_dir,
        r#"
repo:
  kind: scanner
  capabilities:
    scan:
      - "sast"
  bindings: {}
"#,
    );

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
        .args(["lint", "--in", pack_dir.to_str().unwrap(), "--log", "warn"])
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_lowercase();
    assert!(
        stderr.contains("bindings") && stderr.contains("scanner"),
        "stderr should mention missing bindings for scanner, got: {stderr}"
    );
}

#[test]
fn lint_rejects_rollout_strategy_kind() {
    let temp = tempdir().expect("temp dir");
    let pack_dir = temp.path().join("weather-demo");
    copy_example_pack(&pack_dir);
    inject_repo_section(
        &pack_dir,
        r#"
repo:
  kind: rollout-strategy
  capabilities:
    policy:
      - "fake"
  bindings: {}
"#,
    );

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
        .args(["lint", "--in", pack_dir.to_str().unwrap(), "--log", "warn"])
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_lowercase();
    assert!(
        stderr.contains("rollout-strategy") || stderr.contains("unknown variant"),
        "stderr should mention rollout-strategy rejection, got: {stderr}"
    );
}

#[test]
fn lint_rejects_missing_pack_version() {
    let temp = tempdir().expect("temp dir");
    let pack_dir = temp.path().join("weather-demo");
    copy_example_pack(&pack_dir);
    strip_pack_version(&pack_dir);

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
        .args(["lint", "--in", pack_dir.to_str().unwrap(), "--log", "warn"])
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_lowercase();
    assert!(
        stderr.contains("packversion"),
        "stderr should mention missing packVersion, got: {stderr}"
    );
}

#[test]
fn lint_rejects_invalid_pack_version() {
    let temp = tempdir().expect("temp dir");
    let pack_dir = temp.path().join("weather-demo");
    copy_example_pack(&pack_dir);
    set_pack_version(&pack_dir, 99);

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
        .args(["lint", "--in", pack_dir.to_str().unwrap(), "--log", "warn"])
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_lowercase();
    assert!(
        stderr.contains("unsupported packversion") || stderr.contains("packversion"),
        "stderr should mention invalid packVersion, got: {stderr}"
    );
}

fn copy_example_pack(target: &std::path::Path) {
    let source = workspace_root().join("examples/weather-demo");
    for entry in WalkDir::new(&source)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let relative = entry.path().strip_prefix(&source).expect("strip prefix");
        let destination = target.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::copy(entry.path(), &destination).expect("copy fixture file");
    }
}

fn inject_events_section(pack_dir: &std::path::Path, kind: &str) {
    let path = pack_dir.join("pack.yaml");
    let original = fs::read_to_string(&path).expect("read pack.yaml");
    let events_block = format!(
        "\nevents:\n  providers:\n    - name: \"nats-core\"\n      kind: {kind}\n      component: \"nats-provider@1.0.0\"\n      default_flow: \"flows/events/nats/default.ygtc\"\n      custom_flow: \"flows/events/nats/custom.ygtc\"\n      capabilities:\n        transport: nats\n        reliability: at_least_once\n        ordering: per_key\n        topics:\n          - \"greentic.*\"\n"
    );
    fs::write(&path, format!("{original}{events_block}")).expect("write updated pack.yaml");
}

fn inject_repo_section(pack_dir: &std::path::Path, section: &str) {
    let path = pack_dir.join("pack.yaml");
    let original = fs::read_to_string(&path).expect("read pack.yaml");
    fs::write(&path, format!("{original}{section}\n")).expect("write updated pack.yaml");
}

fn inject_messaging_section(pack_dir: &std::path::Path) {
    let path = pack_dir.join("pack.yaml");
    let original = fs::read_to_string(&path).expect("read pack.yaml");
    let block = r#"
messaging:
  adapters:
    - name: "fake-adapter"
      kind: ingress-egress
      component: "fake-adapter@1.0.0"
      default_flow: "flows/messaging/fake/default.ygtc"
      custom_flow: "flows/messaging/fake/custom.ygtc"
      capabilities:
        direction: ["inbound", "outbound"]
        features: ["attachments", "threads"]
"#;
    fs::write(&path, format!("{original}{block}\n")).expect("write updated pack.yaml");
}

fn strip_pack_version(pack_dir: &std::path::Path) {
    let path = pack_dir.join("pack.yaml");
    let contents = fs::read_to_string(&path).expect("read pack.yaml");
    let filtered: String = contents
        .lines()
        .filter(|line| !line.trim_start().starts_with("packVersion"))
        .map(|line| format!("{line}\n"))
        .collect();
    fs::write(&path, filtered).expect("write pack.yaml without packVersion");
}

fn set_pack_version(pack_dir: &std::path::Path, version: u32) {
    let path = pack_dir.join("pack.yaml");
    let contents = fs::read_to_string(&path).expect("read pack.yaml");
    let mut lines: Vec<String> = contents.lines().map(|line| line.to_string()).collect();
    let mut replaced = false;
    for line in &mut lines {
        if line.trim_start().starts_with("packVersion") {
            *line = format!("packVersion: {version}");
            replaced = true;
            break;
        }
    }
    if !replaced {
        lines.insert(0, format!("packVersion: {version}"));
    }
    fs::write(&path, lines.join("\n") + "\n").expect("write pack.yaml with packVersion");
}

#[test]
fn lint_accepts_distribution_bundle_with_software_component() {
    let temp = tempfile::tempdir().expect("temp dir");
    let pack_dir = temp.path().join("bundle-pack");
    std::fs::create_dir_all(&pack_dir).expect("create pack dir");

    let manifest = r#"packVersion: 1
id: greentic.bundle.demo
version: 0.1.0
kind: distribution-bundle
entry_flows: []
flow_files: []
components:
  - component_id: install.app
    version: 1.2.3
    digest: sha256:deadbeef
    artifact_path: artifacts/app.bin
    kind: software
    artifact_type: binary/linux-x86_64
distribution:
  bundle_id: bundle-123
  tenant: {}
  environment_ref: env-prod
  desired_state_version: v1
  components:
    - component_id: install.app
      version: 1.2.3
      digest: sha256:deadbeef
      artifact_path: artifacts/app.bin
      kind: software
      artifact_type: binary/linux-x86_64
"#;
    std::fs::write(pack_dir.join("pack.yaml"), manifest).expect("write manifest");

    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin!("packc"));
    cmd.current_dir(workspace_root());
    cmd.args([
        "build",
        "--in",
        pack_dir.to_str().unwrap(),
        "--dry-run",
        "--log",
        "warn",
    ]);
    cmd.assert().success();
}
