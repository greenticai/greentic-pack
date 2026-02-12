use std::fs;
use std::path::{Path, PathBuf};

use greentic_types::cbor::canonical;
use greentic_types::schemas::common::schema_ir::{AdditionalProperties, SchemaIr};
use greentic_types::schemas::component::v0_6_0::{
    ComponentDescribe, ComponentInfo, ComponentOperation, ComponentRunInput, ComponentRunOutput,
    schema_hash,
};
use serde_yaml_bw::Value as YamlValue;
use std::collections::BTreeMap;
use tempfile::tempdir;
use walkdir::WalkDir;

fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    for entry in WalkDir::new(src) {
        let entry = entry?;
        let src_path = entry.path();
        let rel = src_path.strip_prefix(src).expect("relative path");
        let dest_path = dest.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&dest_path)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(src_path, &dest_path)?;
        }
    }
    Ok(())
}

fn example_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
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
    let pack_yaml = fs::read_to_string(pack_dir.join("pack.yaml")).expect("read pack.yaml");
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
fn cli_build_smoke() {
    let temp = tempdir().expect("temp dir");
    let pack_dir = temp.path().join("pack");
    copy_dir(&example_path("billing-demo"), &pack_dir).expect("copy example");
    write_describe_sidecars_from_pack(&pack_dir);

    let manifest_out = pack_dir.join("dist/manifest.cbor");
    let gtpack_out = pack_dir.join("dist/pack.gtpack");

    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(&pack_dir)
        .env("GREENTIC_DIST_OFFLINE", "1")
        .env("GREENTIC_PACK_USE_DESCRIBE_CACHE", "1")
        .args([
            "build",
            "--in",
            pack_dir.to_str().expect("pack dir"),
            "--manifest",
            manifest_out.to_str().expect("manifest path"),
            "--gtpack-out",
            gtpack_out.to_str().expect("gtpack path"),
        ])
        .output()
        .expect("run greentic-pack build");

    assert!(
        output.status.success(),
        "greentic-pack build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(manifest_out.exists(), "manifest should be written");
    assert!(gtpack_out.exists(), "gtpack should be written");
}
