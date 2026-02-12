use std::fs;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use greentic_types::cbor::canonical;
use greentic_types::schemas::common::schema_ir::{AdditionalProperties, SchemaIr};
use greentic_types::schemas::component::v0_6_0::{
    ComponentDescribe, ComponentInfo, ComponentOperation, ComponentRunInput, ComponentRunOutput,
    schema_hash,
};
use greentic_types::{PackManifest, decode_pack_manifest, encode_pack_manifest};
use serde_json::{Value, json};
use serde_yaml_bw::Value as YamlValue;
use std::collections::BTreeMap;
use walkdir::WalkDir;
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

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

fn validators_fixture_dir() -> PathBuf {
    workspace_root()
        .join("crates")
        .join("packc")
        .join("tests")
        .join("fixtures")
        .join("validators")
}

fn copy_fixture_to_temp(name: &str) -> (tempfile::TempDir, PathBuf) {
    let src = fixture_dir(name);
    let temp = tempfile::tempdir().expect("temp dir");
    let dest = temp.path().join(name);
    fs::create_dir_all(&dest).expect("create fixture root");
    for entry in WalkDir::new(&src).into_iter().filter_map(Result::ok) {
        let rel = entry.path().strip_prefix(&src).expect("relative path");
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = dest.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).expect("create fixture dir");
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).expect("create fixture parent");
            }
            fs::copy(entry.path(), &target).expect("copy fixture file");
        }
    }
    write_describe_sidecars_from_pack(&dest);
    (temp, dest)
}

fn write_describe_sidecar(wasm_path: &Path, component_id: &str, version: &str) {
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
            version: version.to_string(),
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
    let describe_path = format!("{}.describe.cbor", wasm_path.display());
    fs::write(describe_path, bytes).expect("write describe cache");
}

fn write_describe_sidecars_from_pack(pack_dir: &Path) {
    let pack_yaml_path = pack_dir.join("pack.yaml");
    if !pack_yaml_path.exists() {
        return;
    }
    let pack_yaml = fs::read_to_string(&pack_yaml_path).expect("read pack.yaml");
    let doc: YamlValue = serde_yaml_bw::from_str(&pack_yaml).expect("parse pack.yaml");
    let components = doc
        .get("components")
        .and_then(|val| val.as_sequence())
        .cloned()
        .unwrap_or_default();
    for comp in components {
        let id = comp
            .get("id")
            .and_then(|val| val.as_str())
            .unwrap_or_default();
        let version = comp
            .get("version")
            .and_then(|val| val.as_str())
            .unwrap_or("0.1.0");
        let wasm = comp
            .get("wasm")
            .and_then(|val| val.as_str())
            .unwrap_or_default();
        if id.is_empty() || wasm.is_empty() {
            continue;
        }
        let path = if Path::new(wasm).is_absolute() {
            PathBuf::from(wasm)
        } else {
            pack_dir.join(wasm)
        };
        write_describe_sidecar(&path, id, version);
    }
}

#[test]
fn doctor_json_includes_validation() {
    let temp = tempfile::tempdir().expect("temp dir");
    let (_pack_temp, pack_dir) = copy_fixture_to_temp("valid-minimal");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "doctor",
            pack_dir.to_str().unwrap(),
            "--json",
            "--validators-root",
            temp.path().to_str().unwrap(),
            "--no-flow-doctor",
            "--no-component-doctor",
        ])
        .output()
        .expect("run doctor");
    assert!(output.status.success(), "doctor should succeed");

    let payload: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(
        payload.get("validation").is_some(),
        "doctor --json should include validation report"
    );
}

#[test]
fn doctor_fails_on_missing_provider_schema() {
    let (_pack_temp, pack_dir) = copy_fixture_to_temp("missing-provider-schema");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "doctor",
            pack_dir.to_str().unwrap(),
            "--json",
            "--no-flow-doctor",
            "--no-component-doctor",
        ])
        .output()
        .expect("run doctor");
    assert!(
        !output.status.success(),
        "doctor should fail when validation errors exist"
    );

    let payload: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let diagnostics = payload
        .get("validation")
        .and_then(|val| val.get("diagnostics"))
        .and_then(|val| val.as_array())
        .expect("validation diagnostics present");
    assert!(
        diagnostics.iter().any(|diag| {
            diag.get("code")
                .and_then(|val| val.as_str())
                .map(|code| code == "PACK_MISSING_FILE")
                .unwrap_or(false)
                && diag
                    .get("path")
                    .and_then(|val| val.as_str())
                    .map(|path| path == "schemas/messaging/demo/config.schema.json")
                    .unwrap_or(false)
        }),
        "expected missing provider schema diagnostic"
    );
}

#[test]
fn doctor_reports_sbom_dangling_path() {
    let temp = tempfile::tempdir().expect("temp dir");
    let pack_path = temp.path().join("sbom-dangling.gtpack");
    write_gtpack_from_fixture(&fixture_dir("sbom-dangling"), &pack_path);

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "doctor",
            "--pack",
            pack_path.to_str().unwrap(),
            "--json",
            "--no-flow-doctor",
            "--no-component-doctor",
        ])
        .output()
        .expect("run doctor");
    assert!(
        !output.status.success(),
        "doctor should fail for dangling SBOM paths"
    );

    let payload: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let diagnostics = payload
        .get("validation")
        .and_then(|val| val.get("diagnostics"))
        .and_then(|val| val.as_array())
        .expect("validation diagnostics present");
    assert!(
        diagnostics.iter().any(|diag| {
            diag.get("code")
                .and_then(|val| val.as_str())
                .map(|code| code == "PACK_SBOM_DANGLING_PATH")
                .unwrap_or(false)
                && diag
                    .get("path")
                    .and_then(|val| val.as_str())
                    .map(|path| path == "missing.txt")
                    .unwrap_or(false)
        }),
        "expected dangling SBOM diagnostic"
    );
}

#[test]
fn doctor_reads_secret_requirements_from_manifest_only() {
    let (_pack_temp, pack_dir) = copy_fixture_to_temp("valid-minimal");
    let secrets = json!([{
        "key": "demo/token",
        "scope": { "env": "dev", "tenant": "demo" }
    }]);
    fs::write(
        pack_dir.join("secret-requirements.json"),
        serde_json::to_string_pretty(&secrets).expect("serialize secrets"),
    )
    .expect("write secret requirements");

    let temp = tempfile::tempdir().expect("temp dir");
    let pack_path = temp.path().join("manifest-secrets.gtpack");
    let build_output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "build",
            "--in",
            pack_dir.to_str().unwrap(),
            "--gtpack-out",
            pack_path.to_str().unwrap(),
            "--no-update",
        ])
        .output()
        .expect("run build command");
    assert!(build_output.status.success(), "building pack should work");

    let doctor_output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "doctor",
            "--pack",
            pack_path.to_str().unwrap(),
            "--json",
            "--no-flow-doctor",
            "--no-component-doctor",
        ])
        .output()
        .expect("run doctor command");
    assert!(
        doctor_output.status.success(),
        "doctor should succeed without JSON secrets artifacts"
    );
}

#[test]
fn doctor_fails_when_secret_scope_missing_in_manifest() {
    let (_pack_temp, pack_dir) = copy_fixture_to_temp("valid-minimal");
    let secrets = json!([{
        "key": "demo/token",
        "scope": { "env": "dev", "tenant": "demo" }
    }]);
    fs::write(
        pack_dir.join("secret-requirements.json"),
        serde_json::to_string_pretty(&secrets).expect("serialize secrets"),
    )
    .expect("write secret requirements");

    let temp = tempfile::tempdir().expect("temp dir");
    let pack_path = temp.path().join("bad-secrets.gtpack");
    let build_output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "build",
            "--in",
            pack_dir.to_str().unwrap(),
            "--gtpack-out",
            pack_path.to_str().unwrap(),
            "--no-update",
        ])
        .output()
        .expect("run build command");
    assert!(build_output.status.success(), "building pack should work");

    mutate_manifest(&pack_path, |manifest| {
        for requirement in manifest.secret_requirements.iter_mut() {
            requirement.scope = None;
        }
    });

    let doctor_output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "doctor",
            "--pack",
            pack_path.to_str().unwrap(),
            "--json",
            "--no-flow-doctor",
            "--no-component-doctor",
        ])
        .output()
        .expect("run doctor command");
    assert!(
        !doctor_output.status.success(),
        "doctor should fail when secret requirements lose their scope"
    );

    let payload: Value = serde_json::from_slice(&doctor_output.stdout).expect("doctor JSON output");
    let diagnostics = payload
        .get("validation")
        .and_then(|val| val.get("diagnostics"))
        .and_then(|val| val.as_array())
        .expect("validation diagnostics present");
    assert!(
        diagnostics.iter().any(|diag| {
            diag.get("code")
                .and_then(|val| val.as_str())
                .map(|code| code == "PACK_SECRET_REQUIREMENTS_INVALID")
                .unwrap_or(false)
        }),
        "expected secret requirements diagnostic"
    );
}

#[test]
fn doctor_loads_validator_pack_from_root() {
    let (_pack_temp, pack_dir) = copy_fixture_to_temp("valid-minimal");
    let validators_dir = validators_fixture_dir();

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "doctor",
            pack_dir.to_str().unwrap(),
            "--validators-root",
            validators_dir.to_str().unwrap(),
            "--json",
            "--no-flow-doctor",
            "--no-component-doctor",
        ])
        .output()
        .expect("run doctor");
    assert!(output.status.success(), "doctor should succeed");

    let payload: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let diagnostics = payload
        .get("validation")
        .and_then(|val| val.get("diagnostics"))
        .and_then(|val| val.as_array())
        .expect("validation diagnostics present");
    assert!(
        diagnostics.iter().any(|diag| {
            diag.get("code")
                .and_then(|val| val.as_str())
                .map(|code| code == "PACK_VALIDATOR_NOOP")
                .unwrap_or(false)
        }),
        "expected noop validator diagnostic"
    );
}

#[test]
fn doctor_blocks_unlisted_validator_oci_ref() {
    let (_pack_temp, pack_dir) = copy_fixture_to_temp("valid-minimal");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "doctor",
            pack_dir.to_str().unwrap(),
            "--validator-pack",
            "oci://example.com/validators/nope",
            "--validator-policy",
            "optional",
            "--json",
            "--no-flow-doctor",
            "--no-component-doctor",
        ])
        .output()
        .expect("run doctor");
    assert!(output.status.success(), "doctor should succeed");

    let payload: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let diagnostics = payload
        .get("validation")
        .and_then(|val| val.get("diagnostics"))
        .and_then(|val| val.as_array())
        .expect("validation diagnostics present");
    assert!(
        diagnostics.iter().any(|diag| {
            diag.get("code")
                .and_then(|val| val.as_str())
                .map(|code| code == "PACK_VALIDATOR_UNAVAILABLE")
                .unwrap_or(false)
        }),
        "expected allowlist warning diagnostic"
    );
}

#[test]
fn doctor_fails_when_required_validator_missing() {
    let (_pack_temp, pack_dir) = copy_fixture_to_temp("valid-minimal");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "doctor",
            pack_dir.to_str().unwrap(),
            "--validator-pack",
            "missing-validator.gtpack",
            "--validator-policy",
            "required",
            "--json",
            "--no-flow-doctor",
            "--no-component-doctor",
        ])
        .output()
        .expect("run doctor");
    assert!(
        !output.status.success(),
        "doctor should fail when required validator is missing"
    );
}

fn write_gtpack_from_fixture(fixture: &Path, dest: &Path) {
    let manifest_bytes = fs::read(fixture.join("manifest.json")).expect("read manifest fixture");
    let manifest: PackManifest =
        serde_json::from_slice(&manifest_bytes).expect("parse manifest fixture");
    let manifest_cbor = encode_pack_manifest(&manifest).expect("encode manifest");
    let sbom_bytes = fs::read(fixture.join("sbom.json")).expect("read sbom fixture");

    let dest_file = File::create(dest).expect("create pack");
    let mut writer = ZipWriter::new(dest_file);
    let options = FileOptions::<()>::default().compression_method(CompressionMethod::Stored);

    writer
        .start_file("manifest.cbor", options)
        .expect("start manifest entry");
    writer
        .write_all(&manifest_cbor)
        .expect("write manifest entry");

    writer
        .start_file("sbom.json", options)
        .expect("start sbom entry");
    writer.write_all(&sbom_bytes).expect("write sbom entry");

    writer.finish().expect("finish pack");
}

fn mutate_manifest<F>(pack_path: &Path, mutator: F)
where
    F: FnOnce(&mut PackManifest),
{
    let file = File::open(pack_path).expect("open pack for mutation");
    let mut archive = ZipArchive::new(file).expect("read pack archive");
    let mut entries = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).expect("read entry");
        let name = entry.name().to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).expect("read entry data");
        entries.push((name, data));
    }

    for (name, data) in entries.iter_mut() {
        if name == "manifest.cbor" {
            let mut manifest = decode_pack_manifest(data).expect("decode packaged manifest");
            mutator(&mut manifest);
            *data = encode_pack_manifest(&manifest).expect("encode manifest");
            break;
        }
    }

    let file = File::create(pack_path).expect("recreate pack archive");
    let mut writer = ZipWriter::new(file);
    let options = FileOptions::<()>::default().compression_method(CompressionMethod::Stored);
    for (name, data) in entries {
        writer.start_file(name, options).expect("start entry");
        writer.write_all(&data).expect("write entry");
    }
    writer.finish().expect("finish pack");
}
