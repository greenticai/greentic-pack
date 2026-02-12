use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use greentic_types::cbor::canonical;
use greentic_types::decode_pack_manifest;
use greentic_types::schemas::common::schema_ir::{AdditionalProperties, SchemaIr};
use greentic_types::schemas::component::v0_6_0::{
    ComponentDescribe, ComponentInfo, ComponentOperation, ComponentRunInput, ComponentRunOutput,
    schema_hash,
};
use serde_json::Value;
use std::collections::BTreeMap;
use tempfile::TempDir;
use walkdir::WalkDir;
use zip::ZipArchive;

const COMPONENT_ID: &str = "ai.greentic.component-templates";
const COMPONENT_VERSION: &str = "0.1.2";
const COMPONENT_WORLD: &str = "greentic:component/component@0.5.0";
const COMPONENT_DIGEST: &str =
    "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn fixture_dir(name: &str) -> PathBuf {
    workspace_root()
        .join("crates")
        .join("packc")
        .join("tests")
        .join("fixtures")
        .join("packs")
        .join(name)
}

fn copy_fixture_dir(src: &Path, dest: &Path) {
    for entry in WalkDir::new(src).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        let rel = path.strip_prefix(src).expect("fixture path strip");
        let dest_path = dest.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&dest_path).expect("create fixture dir");
        } else {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent).expect("create fixture file dir");
            }
            fs::copy(path, &dest_path).expect("copy fixture file");
        }
    }
}

fn write_pack_fixture(temp: &TempDir, include_manifest: bool) -> PathBuf {
    let pack_dir = temp.path().join("hello2-pack");
    let fixture = fixture_dir("hello2-pack");
    copy_fixture_dir(&fixture, &pack_dir);

    if include_manifest {
        let manifest = serde_json::json!({
            "id": COMPONENT_ID,
            "version": COMPONENT_VERSION,
            "supports": ["messaging"],
            "world": COMPONENT_WORLD,
            "profiles": {
                "default": "stateless",
                "supported": ["stateless"]
            },
            "capabilities": {
                "wasi": {
                    "random": false,
                    "clocks": false
                },
                "host": {}
            },
            "operations": [
                {
                    "name": "handle_message",
                    "input_schema": {},
                    "output_schema": {}
                }
            ]
        });
        let manifest_path = pack_dir
            .join("components")
            .join(COMPONENT_ID)
            .join("component.manifest.json");
        if let Some(parent) = manifest_path.parent() {
            fs::create_dir_all(parent).expect("manifest dir");
        }
        fs::write(
            manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("manifest json"),
        )
        .expect("write component manifest");
    }

    pack_dir
}

fn write_pack_with_manifest_mismatch(temp: &TempDir) -> PathBuf {
    let pack_dir = temp.path().join("manifest-mismatch-pack");
    fs::create_dir_all(&pack_dir).expect("pack dir");

    let components_dir = pack_dir.join("components").join("hello-world");
    fs::create_dir_all(&components_dir).expect("components dir");
    let fixture_wasm = fixture_dir("valid-minimal")
        .join("components")
        .join("fixture.wasm");
    fs::copy(&fixture_wasm, components_dir.join("fixture.wasm")).expect("copy wasm");

    let manifest = serde_json::json!({
        "id": "ai.greentic.hello-world",
        "version": "0.1.0",
        "supports": ["messaging"],
        "world": COMPONENT_WORLD,
        "profiles": {
            "default": "stateless",
            "supported": ["stateless"]
        },
        "capabilities": {
            "wasi": {
                "random": false,
                "clocks": false
            },
            "host": {}
        },
        "operations": [
            {
                "name": "handle_message",
                "input_schema": {},
                "output_schema": {}
            }
        ]
    });
    fs::write(
        components_dir.join("component.manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("manifest json"),
    )
    .expect("write component manifest");

    fs::create_dir_all(pack_dir.join("flows")).expect("flows dir");
    fs::write(
        pack_dir.join("flows/main.ygtc"),
        "id: main\ntype: messaging\nnodes: {}\n",
    )
    .expect("flow file");
    fs::write(
        pack_dir.join("flows/main.ygtc.resolve.summary.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "flow": "main.ygtc",
            "nodes": {}
        }))
        .expect("summary json"),
    )
    .expect("summary file");

    let pack_yaml = r#"pack_id: dev.local.manifest-mismatch
version: 0.1.0
kind: application
publisher: Greentic
components:
  - id: hello-world
    version: 0.1.0
    world: greentic:component/component@0.5.0
    supports: [messaging]
    profiles:
      default: stateless
      supported: [stateless]
    capabilities:
      wasi:
        random: false
        clocks: false
      host: {}
    wasm: components/hello-world/fixture.wasm
flows:
  - id: main
    file: flows/main.ygtc
    tags: [default]
    entrypoints: [default]
dependencies: []
assets: []
"#;
    fs::write(pack_dir.join("pack.yaml"), pack_yaml).expect("pack.yaml");

    pack_dir
}

fn cache_component(cache_dir: &Path, digest: &str, include_manifest: bool) {
    let dir = cache_dir.join(digest.trim_start_matches("sha256:"));
    fs::create_dir_all(&dir).expect("cache dir");
    let wasm_path = dir.join("component.wasm");
    fs::write(&wasm_path, b"cached-component").expect("write wasm");
    write_describe_sidecar(&wasm_path, COMPONENT_ID);
    if include_manifest {
        let manifest = serde_json::json!({
            "id": COMPONENT_ID,
            "version": COMPONENT_VERSION,
            "supports": ["messaging"],
            "world": COMPONENT_WORLD,
            "profiles": {
                "default": "stateless",
                "supported": ["stateless"]
            },
            "capabilities": {
                "wasi": {
                    "random": false,
                    "clocks": false
                },
                "host": {}
            },
            "operations": [
                {
                    "name": "handle_message",
                    "input_schema": {},
                    "output_schema": {}
                }
            ]
        });
        fs::write(
            dir.join("component.manifest.json"),
            serde_json::to_vec_pretty(&manifest).expect("manifest json"),
        )
        .expect("write component.manifest.json");
    }
}

fn write_describe_sidecar(wasm_path: &Path, component_id: &str) {
    let input_schema = SchemaIr::String {
        min_len: None,
        max_len: None,
        regex: None,
        format: None,
    };
    let output_schema = SchemaIr::String {
        min_len: None,
        max_len: None,
        regex: None,
        format: None,
    };
    let config_schema = SchemaIr::Object {
        properties: BTreeMap::new(),
        required: Vec::new(),
        additional: AdditionalProperties::Forbid,
    };
    let hash = schema_hash(&input_schema, &output_schema, &config_schema).expect("schema hash");
    let operation = ComponentOperation {
        id: "run".to_string(),
        display_name: None,
        input: ComponentRunInput {
            schema: input_schema,
        },
        output: ComponentRunOutput {
            schema: output_schema,
        },
        defaults: BTreeMap::new(),
        redactions: Vec::new(),
        constraints: BTreeMap::new(),
        schema_hash: hash,
    };
    let describe = ComponentDescribe {
        info: ComponentInfo {
            id: component_id.to_string(),
            version: COMPONENT_VERSION.to_string(),
            role: "tool".to_string(),
            display_name: None,
        },
        provided_capabilities: Vec::new(),
        required_capabilities: Vec::new(),
        metadata: BTreeMap::new(),
        operations: vec![operation],
        config_schema,
    };
    let bytes = canonical::to_canonical_cbor_allow_floats(&describe).expect("encode describe");
    let describe_path = PathBuf::from(format!("{}.describe.cbor", wasm_path.display()));
    fs::write(describe_path, bytes).expect("write describe cache");
}

fn read_lock_digest(pack_dir: &Path) -> String {
    let flow_dir = pack_dir.join("flows");
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
                    return digest.to_string();
                }
            }
        }
    }
    COMPONENT_DIGEST.to_string()
}

fn build_pack(pack_dir: &Path, cache_dir: &Path, require_manifests: bool) -> std::process::Output {
    let manifest_out = pack_dir.join("dist/manifest.cbor");
    let gtpack_out = pack_dir.join("dist/pack.gtpack");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"));
    cmd.current_dir(pack_dir)
        .env("GREENTIC_DIST_CACHE_DIR", cache_dir)
        .env("GREENTIC_DIST_OFFLINE", "1")
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
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
    let pack_dir = write_pack_fixture(&temp, true);
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
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "doctor",
            "--pack",
            "dist/pack.gtpack",
            "--json",
            "--no-flow-doctor",
            "--no-component-doctor",
        ])
        .output()
        .expect("run doctor");
    assert!(
        doctor.status.success(),
        "doctor failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&doctor.stdout),
        String::from_utf8_lossy(&doctor.stderr)
    );
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
fn build_rejects_component_manifest_id_mismatch() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = write_pack_with_manifest_mismatch(&temp);
    let manifest_out = pack_dir.join("dist/manifest.cbor");
    let gtpack_out = pack_dir.join("dist/pack.gtpack");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "--offline",
            "build",
            "--in",
            pack_dir.to_str().unwrap(),
            "--manifest",
            manifest_out.to_str().unwrap(),
            "--gtpack-out",
            gtpack_out.to_str().unwrap(),
            "--no-update",
        ])
        .output()
        .expect("run build");

    assert!(
        !output.status.success(),
        "build should fail on manifest id mismatch"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "component manifest id ai.greentic.hello-world does not match pack.yaml id hello-world"
        ),
        "expected id mismatch error, stderr:\n{}",
        stderr
    );
}

#[test]
fn build_warns_when_manifest_missing() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = write_pack_fixture(&temp, false);

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
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "doctor",
            "--pack",
            "dist/pack.gtpack",
            "--json",
            "--no-flow-doctor",
            "--no-component-doctor",
        ])
        .output()
        .expect("run doctor");
    assert!(
        doctor.status.success(),
        "doctor failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&doctor.stdout),
        String::from_utf8_lossy(&doctor.stderr)
    );
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
    let pack_dir = write_pack_fixture(&temp, false);

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
