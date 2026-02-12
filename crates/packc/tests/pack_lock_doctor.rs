use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use greentic_pack::pack_lock::{LockedComponent, PackLockV1};
use greentic_pack::{SigningPolicy, open_pack};
use greentic_types::ComponentManifest;
use greentic_types::cbor::canonical;
use greentic_types::pack_manifest::{PackKind, PackManifest, PackSignatures};
use greentic_types::schemas::common::schema_ir::{AdditionalProperties, SchemaIr};
use greentic_types::schemas::component::v0_6_0::{
    ComponentDescribe, ComponentInfo, ComponentOperation, ComponentRunInput, ComponentRunOutput,
    schema_hash,
};
use greentic_types::{
    ComponentCapabilities, ComponentId, ComponentProfiles, PackId, ResourceHints,
    encode_pack_manifest,
};
use packc::pack_lock_doctor::{PackLockDoctorInput, run_pack_lock_doctor};
use packc::runtime::resolve_runtime;
use semver::Version;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipWriter};

fn minimal_core_wasm() -> Vec<u8> {
    // Valid core wasm module header + version.
    vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]
}

fn build_pack_manifest(component_id: &str) -> PackManifest {
    let component = ComponentManifest {
        id: ComponentId::try_from(component_id).expect("component id"),
        version: Version::parse("0.1.0").expect("component version"),
        supports: Vec::new(),
        world: "component-v0-v6-v0".to_string(),
        profiles: ComponentProfiles::default(),
        capabilities: ComponentCapabilities::default(),
        configurators: None,
        operations: Vec::new(),
        config_schema: None,
        resources: ResourceHints::default(),
        dev_flows: BTreeMap::new(),
    };

    PackManifest {
        schema_version: "pack-v1".to_string(),
        pack_id: PackId::try_from("dev.local.pack").expect("pack id"),
        name: None,
        version: Version::parse("0.1.0").expect("pack version"),
        kind: PackKind::Library,
        publisher: "test".to_string(),
        components: vec![component],
        flows: Vec::new(),
        dependencies: Vec::new(),
        capabilities: Vec::new(),
        secret_requirements: Vec::new(),
        signatures: PackSignatures::default(),
        bootstrap: None,
        extensions: None,
    }
}

fn write_pack_archive(
    dest: &Path,
    manifest_cbor: &[u8],
    component_id: &str,
    component_bytes: &[u8],
    pack_lock_bytes: Option<&[u8]>,
    describe_bytes: Option<&[u8]>,
) {
    let file = File::create(dest).expect("create pack archive");
    let mut writer = ZipWriter::new(file);
    let options = FileOptions::<()>::default().compression_method(CompressionMethod::Stored);

    writer
        .start_file("manifest.cbor", options)
        .expect("start manifest");
    writer.write_all(manifest_cbor).expect("write manifest");

    let wasm_path = format!("components/{component_id}.wasm");
    writer.start_file(&wasm_path, options).expect("start wasm");
    writer.write_all(component_bytes).expect("write wasm");

    if let Some(describe_bytes) = describe_bytes {
        let describe_path = format!("{wasm_path}.describe.cbor");
        writer
            .start_file(&describe_path, options)
            .expect("start describe");
        writer.write_all(describe_bytes).expect("write describe");
    }

    if let Some(lock_bytes) = pack_lock_bytes {
        writer
            .start_file("pack.lock.cbor", options)
            .expect("start lock");
        writer.write_all(lock_bytes).expect("write lock");
    }

    writer.finish().expect("finish pack");
}

fn build_pack_lock(component_id: &str, component_bytes: &[u8], describe_hash: &str) -> Vec<u8> {
    let digest = format!("sha256:{:x}", Sha256::digest(component_bytes));
    let component = LockedComponent {
        component_id: component_id.to_string(),
        r#ref: None,
        abi_version: "0.6.0".to_string(),
        resolved_digest: digest,
        describe_hash: describe_hash.to_string(),
        operations: Vec::new(),
        world: None,
        component_version: None,
        role: None,
    };

    let mut components = BTreeMap::new();
    components.insert(component_id.to_string(), component);
    let lock = PackLockV1::new(components);

    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("pack.lock.cbor");
    greentic_pack::pack_lock::write_pack_lock(&path, &lock).expect("write lock");
    std::fs::read(&path).expect("read lock bytes")
}

fn has_diag(diagnostics: &[greentic_types::validate::Diagnostic], code: &str) -> bool {
    diagnostics.iter().any(|diag| diag.code == code)
}

fn build_describe_cache(component_id: &str) -> (Vec<u8>, String) {
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
            version: "0.1.0".to_string(),
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
    let digest = format!("{:x}", Sha256::digest(&bytes));
    (bytes, digest)
}

#[test]
fn pack_lock_doctor_reports_invalid_component() {
    let temp = TempDir::new().expect("temp dir");
    let component_id = "dev.local.component";
    let wasm_bytes = minimal_core_wasm();
    let (describe_bytes, describe_hash) = build_describe_cache(component_id);
    let manifest = build_pack_manifest(component_id);
    let manifest_cbor = encode_pack_manifest(&manifest).expect("encode manifest");
    let lock_bytes = build_pack_lock(component_id, &wasm_bytes, &describe_hash);
    let pack_path = temp.path().join("pack.gtpack");

    write_pack_archive(
        &pack_path,
        &manifest_cbor,
        component_id,
        &wasm_bytes,
        Some(&lock_bytes),
        Some(&describe_bytes),
    );

    let load = open_pack(&pack_path, SigningPolicy::DevOk).expect("open pack");
    let runtime = resolve_runtime(Some(temp.path()), None, true, None).expect("resolve runtime");
    let output = run_pack_lock_doctor(PackLockDoctorInput {
        load: &load,
        pack_dir: None,
        runtime: &runtime,
        allow_oci_tags: false,
        use_describe_cache: true,
        online: false,
    })
    .expect("run doctor");

    assert!(output.has_errors, "expected errors");
    assert!(
        has_diag(&output.diagnostics, "PACK_LOCK_COMPONENT_DECODE_FAILED"),
        "expected decode failure diagnostic"
    );
}

#[test]
fn pack_lock_doctor_warns_on_missing_lock() {
    let temp = TempDir::new().expect("temp dir");
    let component_id = "dev.local.component";
    let wasm_bytes = minimal_core_wasm();
    let manifest = build_pack_manifest(component_id);
    let manifest_cbor = encode_pack_manifest(&manifest).expect("encode manifest");
    let pack_path = temp.path().join("pack.gtpack");

    write_pack_archive(
        &pack_path,
        &manifest_cbor,
        component_id,
        &wasm_bytes,
        None,
        None,
    );

    let load = open_pack(&pack_path, SigningPolicy::DevOk).expect("open pack");
    let runtime = resolve_runtime(Some(temp.path()), None, true, None).expect("resolve runtime");
    let output = run_pack_lock_doctor(PackLockDoctorInput {
        load: &load,
        pack_dir: None,
        runtime: &runtime,
        allow_oci_tags: false,
        use_describe_cache: false,
        online: false,
    })
    .expect("run doctor");

    assert!(!output.has_errors, "missing lock should not be fatal");
    assert!(
        has_diag(&output.diagnostics, "PACK_LOCK_MISSING"),
        "expected missing lock diagnostic"
    );
}
