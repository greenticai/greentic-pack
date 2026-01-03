use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use assert_cmd::prelude::*;
use greentic_types::pack_manifest::{
    BootstrapSpec, ExtensionInline, ExtensionRef, PackManifest, PackSignatures,
};
use greentic_types::provider::{
    PROVIDER_EXTENSION_ID, ProviderDecl, ProviderExtensionInline, ProviderRuntimeRef,
};
use greentic_types::{PackId, PackKind, encode_pack_manifest};
use semver::Version;
use serde_json::Value;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

fn provider_decl(id: &str, config: &str) -> ProviderDecl {
    ProviderDecl {
        provider_type: id.to_string(),
        capabilities: vec!["cap".into()],
        ops: vec!["op".into()],
        config_schema_ref: config.to_string(),
        state_schema_ref: None,
        runtime: ProviderRuntimeRef {
            component_ref: format!("{id}.component"),
            export: "run".into(),
            world: "greentic:provider/schema-core@1.0.0".into(),
        },
        docs_ref: None,
    }
}

fn manifest_with_providers(providers: Vec<ProviderDecl>) -> PackManifest {
    let mut extensions = BTreeMap::new();
    extensions.insert(
        PROVIDER_EXTENSION_ID.to_string(),
        ExtensionRef {
            kind: PROVIDER_EXTENSION_ID.to_string(),
            version: "1.0.0".into(),
            digest: None,
            location: None,
            inline: Some(ExtensionInline::Provider(ProviderExtensionInline {
                providers,
                additional_fields: BTreeMap::new(),
            })),
        },
    );

    PackManifest {
        schema_version: "pack-v1".into(),
        pack_id: PackId::new("demo.providers").unwrap(),
        version: Version::parse("0.1.0").unwrap(),
        kind: PackKind::Application,
        publisher: "Greentic".into(),
        components: Vec::new(),
        flows: Vec::new(),
        dependencies: Vec::new(),
        capabilities: Vec::new(),
        secret_requirements: Vec::new(),
        signatures: PackSignatures::default(),
        bootstrap: Some(BootstrapSpec::default()),
        extensions: Some(extensions),
    }
}

fn write_pack(path: &Path, manifest: &PackManifest) {
    let file = std::fs::File::create(path).expect("pack file");
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let manifest_bytes = encode_pack_manifest(manifest).expect("manifest");
    zip.start_file("manifest.cbor", opts)
        .expect("start manifest");
    zip.write_all(&manifest_bytes).expect("write manifest");

    zip.finish().expect("finish zip");
}

#[test]
fn list_outputs_sorted_json() {
    let temp = TempDir::new().expect("temp dir");
    let pack_path = temp.path().join("providers.gtpack");
    write_pack(
        &pack_path,
        &manifest_with_providers(vec![
            provider_decl("beta", "schemas/b.json"),
            provider_decl("alpha", "schemas/a.json"),
        ]),
    );

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .args([
            "providers",
            "list",
            "--pack",
            pack_path.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Vec<Value> = serde_json::from_slice(&output).expect("json");
    let ids: Vec<_> = parsed
        .iter()
        .filter_map(|p| p.get("provider_type").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(ids, vec!["alpha", "beta"]);
}

#[test]
fn info_errors_for_unknown_provider() {
    let temp = TempDir::new().expect("temp dir");
    let pack_path = temp.path().join("providers.gtpack");
    write_pack(
        &pack_path,
        &manifest_with_providers(vec![provider_decl("alpha", "schemas/a.json")]),
    );

    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .args([
            "providers",
            "info",
            "missing",
            "--pack",
            pack_path.to_str().unwrap(),
        ])
        .assert()
        .failure();
}

#[test]
fn validate_rejects_duplicate_ids() {
    let temp = TempDir::new().expect("temp dir");
    let pack_path = temp.path().join("providers.gtpack");
    let provider = provider_decl("dup", "schemas/a.json");
    write_pack(
        &pack_path,
        &manifest_with_providers(vec![provider.clone(), provider]),
    );

    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .args([
            "providers",
            "validate",
            "--pack",
            pack_path.to_str().unwrap(),
        ])
        .assert()
        .failure();
}

#[test]
fn validate_reports_missing_local_refs_in_strict_mode() {
    let temp = TempDir::new().expect("temp dir");
    let pack_path = temp.path().join("providers.gtpack");
    let mut provider = provider_decl("alpha", "schemas/missing.json");
    provider.docs_ref = Some("docs/provider.md".into());
    write_pack(&pack_path, &manifest_with_providers(vec![provider]));

    Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .args([
            "providers",
            "validate",
            "--pack",
            pack_path.to_str().unwrap(),
            "--strict",
        ])
        .assert()
        .failure();
}
