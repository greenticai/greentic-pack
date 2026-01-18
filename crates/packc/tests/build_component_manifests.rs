use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use greentic_types::decode_pack_manifest;
use serde_json::Value;
use tempfile::TempDir;
use walkdir::WalkDir;
use zip::ZipArchive;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn hello2_fixture() -> PathBuf {
    workspace_root()
        .join("..")
        .join("tests")
        .join("hello2-pack")
}

fn copy_fixture(temp: &TempDir) -> PathBuf {
    let dest = temp.path().join("hello2-pack");
    let src = hello2_fixture();
    for entry in WalkDir::new(&src) {
        let entry = entry.expect("walk fixture");
        let path = entry.path();
        let rel = path.strip_prefix(&src).expect("relative path");
        let target = dest.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).expect("create dir");
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).expect("create parent dir");
            }
            fs::copy(path, &target).expect("copy file");
        }
    }
    dest
}

fn cache_component(cache_dir: &Path, digest: &str, include_manifest: bool) {
    let dir = cache_dir.join(digest.trim_start_matches("sha256:"));
    fs::create_dir_all(&dir).expect("cache dir");
    fs::write(dir.join("component.wasm"), b"cached-component").expect("write wasm");
    if include_manifest {
        let manifest_path = workspace_root()
            .join("..")
            .join("component-templates")
            .join("component.manifest.json");
        fs::copy(manifest_path, dir.join("component.manifest.json"))
            .expect("write component.manifest.json");
    }
}

fn read_resolve_digest(pack_dir: &Path) -> Option<String> {
    let flow_dir = pack_dir.join("flows");
    if !flow_dir.exists() {
        return None;
    }
    for entry in WalkDir::new(&flow_dir).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !entry.file_type().is_file()
            || !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".resolve.summary.json"))
        {
            continue;
        }
        let data = fs::read(path).expect("read resolve summary");
        let payload: Value = serde_json::from_slice(&data).expect("parse resolve summary");
        let nodes = payload.get("nodes").and_then(|val| val.as_object());
        if let Some(nodes) = nodes {
            for node in nodes.values() {
                if let Some(digest) = node.get("digest").and_then(|val| val.as_str()) {
                    return Some(digest.to_string());
                }
            }
        }
    }
    None
}

fn read_lock_digest(pack_dir: &Path) -> String {
    if let Some(digest) = read_resolve_digest(pack_dir) {
        return digest;
    }
    let lock_path = pack_dir.join("pack.lock.json");
    let data = fs::read(&lock_path).expect("read pack.lock.json");
    let payload: Value = serde_json::from_slice(&data).expect("parse pack.lock.json");
    payload
        .get("components")
        .and_then(|val| val.as_array())
        .and_then(|list| list.first())
        .and_then(|val| val.get("digest"))
        .and_then(|val| val.as_str())
        .expect("digest")
        .to_string()
}

fn build_pack(pack_dir: &Path, cache_dir: &Path, require_manifests: bool) -> std::process::Output {
    let manifest_out = pack_dir.join("dist/manifest.cbor");
    let gtpack_out = pack_dir.join("dist/pack.gtpack");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"));
    cmd.current_dir(pack_dir)
        .env("GREENTIC_DIST_CACHE_DIR", cache_dir)
        .env("GREENTIC_DIST_OFFLINE", "1")
        .args([
            "--offline",
            "--cache-dir",
            cache_dir.to_str().unwrap(),
            "build",
            "--in",
            pack_dir.to_str().unwrap(),
            "--manifest",
            manifest_out.to_str().unwrap(),
            "--gtpack-out",
            gtpack_out.to_str().unwrap(),
        ]);
    if require_manifests {
        cmd.arg("--require-component-manifests");
    }
    cmd.output().expect("run build")
}

#[test]
fn build_materializes_component_manifest_when_available() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = copy_fixture(&temp);
    let cache_dir = temp.path().join("cache");
    let digest = read_lock_digest(&pack_dir);
    cache_component(&cache_dir, &digest, false);

    let output = build_pack(&pack_dir, &cache_dir, true);
    assert!(
        output.status.success(),
        "build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let manifest_bytes = fs::read(pack_dir.join("dist/manifest.cbor")).expect("manifest");
    let manifest = decode_pack_manifest(&manifest_bytes).expect("decode manifest");
    assert_eq!(manifest.components.len(), 1);

    let doctor = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(&pack_dir)
        .args(["doctor", "--pack", "dist/pack.gtpack", "--json"])
        .output()
        .expect("run doctor");
    assert!(doctor.status.success(), "doctor failed");
    let payload: Value = serde_json::from_slice(&doctor.stdout).expect("doctor json");
    let diagnostics = payload
        .get("validation")
        .and_then(|val| val.get("diagnostics"))
        .and_then(|val| val.as_array())
        .expect("diagnostics");
    assert!(
        diagnostics.iter().all(|diag| {
            diag.get("code") != Some(&Value::String("PACK_COMPONENT_NOT_EXPLICIT".into()))
        }),
        "unexpected PACK_COMPONENT_NOT_EXPLICIT warning"
    );

    let pack_file = fs::File::open(pack_dir.join("dist/pack.gtpack")).expect("open pack");
    let mut archive = ZipArchive::new(pack_file).expect("read pack");
    assert!(
        archive
            .by_name("components/ai.greentic.component-templates.wasm")
            .is_ok(),
        "expected aliased component wasm in pack"
    );
}

#[test]
fn build_warns_when_manifest_missing() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = copy_fixture(&temp);
    let manifest_path = pack_dir
        .join("components")
        .join("ai.greentic.component-templates")
        .join("component.manifest.json");
    if manifest_path.exists() {
        fs::remove_file(&manifest_path).expect("remove manifest");
    }

    let cache_dir = temp.path().join("cache");
    let digest = read_lock_digest(&pack_dir);
    cache_component(&cache_dir, &digest, false);

    let output = build_pack(&pack_dir, &cache_dir, false);
    assert!(
        output.status.success(),
        "build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let manifest_bytes = fs::read(pack_dir.join("dist/manifest.cbor")).expect("manifest");
    let manifest = decode_pack_manifest(&manifest_bytes).expect("decode manifest");
    assert_eq!(manifest.components.len(), 0);

    let doctor = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(&pack_dir)
        .args(["doctor", "--pack", "dist/pack.gtpack", "--json"])
        .output()
        .expect("run doctor");
    assert!(doctor.status.success(), "doctor failed");
    let payload: Value = serde_json::from_slice(&doctor.stdout).expect("doctor json");
    let diagnostics = payload
        .get("validation")
        .and_then(|val| val.get("diagnostics"))
        .and_then(|val| val.as_array())
        .expect("diagnostics");
    assert!(
        diagnostics.iter().any(|diag| {
            diag.get("code")
                .and_then(|val| val.as_str())
                .map(|code| code == "PACK_COMPONENT_NOT_EXPLICIT")
                .unwrap_or(false)
        }),
        "expected PACK_COMPONENT_NOT_EXPLICIT warning"
    );
}

#[test]
fn build_fails_when_manifest_missing_in_strict_mode() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = copy_fixture(&temp);
    let manifest_path = pack_dir
        .join("components")
        .join("ai.greentic.component-templates")
        .join("component.manifest.json");
    if manifest_path.exists() {
        fs::remove_file(&manifest_path).expect("remove manifest");
    }

    let cache_dir = temp.path().join("cache");
    let digest = read_lock_digest(&pack_dir);
    cache_component(&cache_dir, &digest, false);

    let output = build_pack(&pack_dir, &cache_dir, true);
    assert!(
        !output.status.success(),
        "expected build to fail without component manifest metadata"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("component manifest metadata missing"),
        "unexpected stderr: {stderr}"
    );
}
