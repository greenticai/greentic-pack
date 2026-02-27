#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::ValueEnum;
use greentic_distributor_client::{DistClient, DistOptions};
use greentic_pack::{PackLoad, SigningPolicy, open_pack};
use greentic_types::pack_manifest::{ExtensionInline, PackManifest};
use greentic_types::provider::PROVIDER_EXTENSION_ID;
use greentic_types::validate::{Diagnostic, Severity};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::p2::add_to_linker_sync;
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::runtime::{NetworkPolicy, RuntimeContext};

const PACK_VALIDATOR_WORLDS: [&str; 2] = [
    "greentic:pack-validate@0.1.0/pack-validator",
    "greentic:pack-validate/pack-validator@0.1.0",
];
pub const DEFAULT_VALIDATOR_ALLOW: &str = "oci://ghcr.io/greenticai/validators/";
const DEFAULT_TIMEOUT_SECS: u64 = 2;
const DEFAULT_MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;

mod bindings {
    wasmtime::component::bindgen!({
        inline: r#"
        package greentic:pack-validate@0.1.0;

        interface validator {
          record diagnostic {
            severity: string,
            code: string,
            message: string,
            path: option<string>,
            hint: option<string>,
          }

          record pack-inputs {
            manifest-cbor: list<u8>,
            sbom-json: string,
            file-index: list<string>,
          }

          applies: func(inputs: pack-inputs) -> bool;
          validate: func(inputs: pack-inputs) -> list<diagnostic>;
        }

        world pack-validator {
          export validator;
        }
        "#,
    });
}

use bindings::PackValidator;
use bindings::exports::greentic::pack_validate::validator::{
    Diagnostic as WasmDiagnostic, PackInputs,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
pub enum ValidatorPolicy {
    Required,
    Optional,
}

impl ValidatorPolicy {
    pub fn is_required(self) -> bool {
        matches!(self, ValidatorPolicy::Required)
    }
}

#[derive(Clone, Debug)]
pub struct ValidatorConfig {
    pub validators_root: PathBuf,
    pub validator_packs: Vec<String>,
    pub validator_allow: Vec<String>,
    pub validator_cache_dir: PathBuf,
    pub policy: ValidatorPolicy,
    pub local_validators: Vec<LocalValidator>,
}

#[derive(Clone, Debug)]
pub struct LocalValidator {
    pub component_id: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
pub struct ValidatorSourceReport {
    pub reference: String,
    pub origin: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug)]
struct ValidatorRef {
    reference: String,
    digest: Option<String>,
    origin: String,
}

#[derive(Clone, Debug)]
struct ValidatorComponent {
    component_id: String,
    wasm: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct ValidatorRunResult {
    pub diagnostics: Vec<Diagnostic>,
    pub sources: Vec<ValidatorSourceReport>,
    pub missing_required: bool,
}

pub async fn run_wasm_validators(
    load: &PackLoad,
    config: &ValidatorConfig,
    runtime: &RuntimeContext,
) -> Result<ValidatorRunResult> {
    let inputs = build_pack_inputs(load)?;

    let mut result = ValidatorRunResult::default();
    let mut components = Vec::new();

    for local in &config.local_validators {
        let reference = format!("local:{}", local.path.display());
        match fs::read(&local.path) {
            Ok(bytes) => {
                components.push(ValidatorComponent {
                    component_id: local.component_id.clone(),
                    wasm: bytes,
                });
                result.sources.push(ValidatorSourceReport {
                    reference: reference.clone(),
                    origin: "local".to_string(),
                    status: "loaded".to_string(),
                    message: None,
                });
            }
            Err(err) => {
                let is_required = config.policy.is_required();
                if is_required {
                    result.missing_required = true;
                    result.diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "PACK_VALIDATOR_REQUIRED".to_string(),
                        message: format!(
                            "Validator {} is required but could not be loaded.",
                            reference
                        ),
                        path: None,
                        hint: Some(err.to_string()),
                        data: Value::Null,
                    });
                } else {
                    result.diagnostics.push(Diagnostic {
                        severity: Severity::Warn,
                        code: "PACK_VALIDATOR_UNAVAILABLE".to_string(),
                        message: format!("Validator {} could not be loaded; skipping.", reference),
                        path: None,
                        hint: Some(err.to_string()),
                        data: Value::Null,
                    });
                }
                result.sources.push(ValidatorSourceReport {
                    reference,
                    origin: "local".to_string(),
                    status: "failed".to_string(),
                    message: Some(err.to_string()),
                });
            }
        }
    }

    let refs = collect_validator_refs(load, config);
    if refs.is_empty() && components.is_empty() {
        return Ok(result);
    }

    for validator_ref in refs {
        match load_validator_components(&validator_ref, config, runtime).await {
            Ok(mut loaded) => {
                components.append(&mut loaded);
                result.sources.push(ValidatorSourceReport {
                    reference: validator_ref.reference.clone(),
                    origin: validator_ref.origin.clone(),
                    status: "loaded".to_string(),
                    message: None,
                });
            }
            Err(err) => {
                let is_required = config.policy.is_required();
                if is_required {
                    result.missing_required = true;
                    result.diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "PACK_VALIDATOR_REQUIRED".to_string(),
                        message: format!(
                            "Validator {} is required but could not be loaded.",
                            validator_ref.reference
                        ),
                        path: None,
                        hint: Some(err.to_string()),
                        data: Value::Null,
                    });
                }
                result.sources.push(ValidatorSourceReport {
                    reference: validator_ref.reference.clone(),
                    origin: validator_ref.origin.clone(),
                    status: "failed".to_string(),
                    message: Some(err.to_string()),
                });
                if !is_required {
                    result.diagnostics.push(Diagnostic {
                        severity: Severity::Warn,
                        code: "PACK_VALIDATOR_UNAVAILABLE".to_string(),
                        message: format!(
                            "Validator {} could not be loaded; skipping.",
                            validator_ref.reference
                        ),
                        path: None,
                        hint: Some(err.to_string()),
                        data: Value::Null,
                    });
                }
            }
        }
    }

    if components.is_empty() {
        return Ok(result);
    }

    let engine = build_engine()?;
    let mut linker = Linker::new(&engine);
    add_to_linker_sync(&mut linker)?;

    for component in components {
        let validator_result = run_component_validator(&engine, &mut linker, &component, &inputs);
        match validator_result {
            Ok(mut diags) => result.diagnostics.append(&mut diags),
            Err(err) => {
                result.diagnostics.push(Diagnostic {
                    severity: Severity::Warn,
                    code: "PACK_VALIDATOR_FAILED".to_string(),
                    message: format!(
                        "Validator component {} failed to execute.",
                        component.component_id
                    ),
                    path: None,
                    hint: Some(err.to_string()),
                    data: Value::Null,
                });
            }
        }
    }

    Ok(result)
}

fn build_engine() -> Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.epoch_interruption(true);
    Ok(Engine::new(&config)?)
}

fn run_component_validator(
    engine: &Engine,
    linker: &mut Linker<ValidatorCtx>,
    component: &ValidatorComponent,
    inputs: &PackInputs,
) -> Result<Vec<Diagnostic>> {
    let component = Component::from_binary(engine, &component.wasm)
        .map_err(|err| anyhow!("failed to load validator component: {err}"))?;

    let mut store = Store::new(engine, ValidatorCtx::new());
    store.limiter(|ctx| &mut ctx.limits);
    store.set_epoch_deadline(1);

    let validator = PackValidator::instantiate(&mut store, &component, linker)
        .map_err(|err| anyhow!("failed to instantiate validator component: {err}"))?;
    let guest = validator.greentic_pack_validate_validator();

    let engine = engine.clone();
    let timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);
    std::thread::spawn(move || {
        std::thread::sleep(timeout);
        engine.increment_epoch();
    });

    let applies = guest
        .call_applies(&mut store, inputs)
        .map_err(|err| anyhow!("validator applies call failed: {err}"))?;
    if !applies {
        return Ok(Vec::new());
    }

    let diags = guest
        .call_validate(&mut store, inputs)
        .map_err(|err| anyhow!("validator validate call failed: {err}"))?;
    Ok(convert_diagnostics(diags))
}

fn convert_diagnostics(diags: Vec<WasmDiagnostic>) -> Vec<Diagnostic> {
    diags
        .into_iter()
        .map(|diag| Diagnostic {
            severity: match diag.severity.as_str() {
                "info" => Severity::Info,
                "warn" => Severity::Warn,
                "error" => Severity::Error,
                _ => Severity::Warn,
            },
            code: diag.code,
            message: diag.message,
            path: diag.path,
            hint: diag.hint,
            data: Value::Null,
        })
        .collect()
}

fn build_pack_inputs(load: &PackLoad) -> Result<PackInputs> {
    let manifest_bytes = load.files.get("manifest.cbor").cloned().unwrap_or_default();

    let sbom_json = if let Some(bytes) = load.files.get("sbom.json") {
        String::from_utf8_lossy(bytes).to_string()
    } else if let Some(bytes) = load.files.get("sbom.cbor") {
        let value: Value = serde_cbor::from_slice(bytes).context("sbom.cbor is not valid CBOR")?;
        serde_json::to_string(&value).context("failed to serialize sbom json")?
    } else {
        serde_json::to_string(&serde_json::json!({"files": load.sbom}))
            .context("failed to serialize sbom json")?
    };

    let file_index = load.files.keys().cloned().collect();

    Ok(PackInputs {
        manifest_cbor: manifest_bytes,
        sbom_json,
        file_index,
    })
}

fn collect_validator_refs(load: &PackLoad, config: &ValidatorConfig) -> Vec<ValidatorRef> {
    let mut refs = Vec::new();

    for reference in &config.validator_packs {
        refs.push(ValidatorRef {
            reference: reference.clone(),
            digest: None,
            origin: "cli".to_string(),
        });
    }

    if config.validators_root.exists()
        && let Ok(entries) = std::fs::read_dir(&config.validators_root)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("gtpack") {
                refs.push(ValidatorRef {
                    reference: path.to_string_lossy().to_string(),
                    digest: None,
                    origin: "validators-root".to_string(),
                });
            }
        }
    }

    if let Some(manifest) = load.gpack_manifest.as_ref() {
        refs.extend(validator_refs_from_manifest(manifest));
    }
    refs.extend(validator_refs_from_annotations(load));

    let mut seen = BTreeSet::new();
    refs.retain(|r| seen.insert((r.reference.clone(), r.digest.clone())));
    refs
}

fn validator_refs_from_manifest(manifest: &PackManifest) -> Vec<ValidatorRef> {
    let mut refs = Vec::new();
    let Some(extensions) = manifest.extensions.as_ref() else {
        return refs;
    };
    let Some(extension) = extensions.get(PROVIDER_EXTENSION_ID) else {
        return refs;
    };
    let Some(inline) = extension.inline.as_ref() else {
        return refs;
    };

    let value = match inline {
        ExtensionInline::Other(value) => value.clone(),
        _ => serde_json::to_value(inline).unwrap_or(Value::Null),
    };

    if let Some(reference) = value.get("validator_ref").and_then(Value::as_str) {
        let digest = value
            .get("validator_digest")
            .and_then(Value::as_str)
            .map(|s| s.to_string());
        refs.push(ValidatorRef {
            reference: reference.to_string(),
            digest,
            origin: "provider-extension".to_string(),
        });
    }

    if let Some(values) = value.get("validator_refs").and_then(Value::as_array) {
        for entry in values {
            if let Some(reference) = entry.as_str() {
                refs.push(ValidatorRef {
                    reference: reference.to_string(),
                    digest: None,
                    origin: "provider-extension".to_string(),
                });
            }
        }
    }

    if let Some(providers) = value.get("providers").and_then(Value::as_array) {
        for provider in providers {
            if let Some(reference) = provider.get("validator_ref").and_then(Value::as_str) {
                let digest = provider
                    .get("validator_digest")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string());
                refs.push(ValidatorRef {
                    reference: reference.to_string(),
                    digest,
                    origin: "provider-extension".to_string(),
                });
            }
        }
    }

    refs
}

fn validator_refs_from_annotations(load: &PackLoad) -> Vec<ValidatorRef> {
    let mut refs = Vec::new();
    if let Some(value) = load.manifest.meta.annotations.get("greentic.validators") {
        match value {
            Value::String(reference) => refs.push(ValidatorRef {
                reference: reference.clone(),
                digest: None,
                origin: "annotations".to_string(),
            }),
            Value::Array(items) => {
                for item in items {
                    if let Some(reference) = item.as_str() {
                        refs.push(ValidatorRef {
                            reference: reference.to_string(),
                            digest: None,
                            origin: "annotations".to_string(),
                        });
                    } else if let Some(reference) = item.get("ref").and_then(Value::as_str) {
                        let digest = item
                            .get("digest")
                            .and_then(Value::as_str)
                            .map(|s| s.to_string());
                        refs.push(ValidatorRef {
                            reference: reference.to_string(),
                            digest,
                            origin: "annotations".to_string(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    refs
}

async fn load_validator_components(
    validator_ref: &ValidatorRef,
    config: &ValidatorConfig,
    runtime: &RuntimeContext,
) -> Result<Vec<ValidatorComponent>> {
    let reference = validator_ref.reference.as_str();
    if reference.starts_with("oci://") {
        return load_validator_components_from_oci(validator_ref, config, runtime).await;
    }

    let path = Path::new(reference);
    if path.exists() {
        if path.is_dir() {
            return load_validator_components_from_dir(path);
        }
        if path.extension().and_then(|ext| ext.to_str()) == Some("gtpack") {
            return load_validator_components_from_pack(path);
        }
        if path.extension().and_then(|ext| ext.to_str()) == Some("wasm") {
            let wasm = std::fs::read(path).with_context(|| {
                format!("failed to read validator component {}", path.display())
            })?;
            return Ok(vec![ValidatorComponent {
                component_id: path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("validator")
                    .to_string(),
                wasm,
            }]);
        }
    }

    Err(anyhow!(
        "validator reference {} could not be resolved",
        reference
    ))
}

fn load_validator_components_from_dir(path: &Path) -> Result<Vec<ValidatorComponent>> {
    let mut components = Vec::new();
    for entry in std::fs::read_dir(path)
        .with_context(|| format!("failed to read validators root {}", path.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("gtpack") {
            components.extend(load_validator_components_from_pack(&path)?);
        }
    }
    Ok(components)
}

fn load_validator_components_from_pack(path: &Path) -> Result<Vec<ValidatorComponent>> {
    let load = open_pack(path, SigningPolicy::DevOk)
        .map_err(|err| anyhow!(err.message))
        .with_context(|| format!("failed to open validator pack {}", path.display()))?;
    let mut components = Vec::new();

    if let Some(manifest) = load.gpack_manifest.as_ref() {
        for component in &manifest.components {
            if !PACK_VALIDATOR_WORLDS
                .iter()
                .any(|world| world == &component.world)
            {
                continue;
            }
            let wasm_paths = [
                format!(
                    "components/{}@{}/component.wasm",
                    component.id.as_str(),
                    component.version
                ),
                format!("components/{}.wasm", component.id.as_str()),
            ];
            let wasm = wasm_paths
                .iter()
                .find_map(|path| load.files.get(path).cloned())
                .ok_or_else(|| {
                    anyhow!(
                        "validator pack missing {} for component {}",
                        wasm_paths.join(" or "),
                        component.id.as_str()
                    )
                })?;
            components.push(ValidatorComponent {
                component_id: component.id.as_str().to_string(),
                wasm,
            });
        }
    } else {
        for component in &load.manifest.components {
            let Some(world) = component.world.as_deref() else {
                continue;
            };
            if !PACK_VALIDATOR_WORLDS.iter().any(|item| item == &world) {
                continue;
            }
            let Some(wasm) = load.files.get(&component.file_wasm).cloned() else {
                return Err(anyhow!(
                    "validator pack missing {} for component {}",
                    component.file_wasm,
                    component.name
                ));
            };
            components.push(ValidatorComponent {
                component_id: component.name.clone(),
                wasm,
            });
        }
    }

    if components.is_empty() {
        return Err(anyhow!(
            "validator pack {} contains no pack-validator components",
            path.display()
        ));
    }

    Ok(components)
}

async fn load_validator_components_from_oci(
    validator_ref: &ValidatorRef,
    config: &ValidatorConfig,
    runtime: &RuntimeContext,
) -> Result<Vec<ValidatorComponent>> {
    let allowed = if config.validator_allow.is_empty() {
        vec![DEFAULT_VALIDATOR_ALLOW.to_string()]
    } else {
        config.validator_allow.clone()
    };
    if !allowed
        .iter()
        .any(|prefix| validator_ref.reference.starts_with(prefix))
    {
        return Err(anyhow!(
            "validator ref {} is not in allowlist",
            validator_ref.reference
        ));
    }

    let dist = DistClient::new(DistOptions {
        cache_dir: config.validator_cache_dir.clone(),
        allow_tags: true,
        offline: runtime.network_policy() == NetworkPolicy::Offline,
        allow_insecure_local_http: false,
        ..DistOptions::default()
    });

    let resolved = if runtime.network_policy() == NetworkPolicy::Offline {
        dist.ensure_cached(&validator_ref.reference)
            .await
            .context("validator ref not cached")?
    } else {
        dist.resolve_ref(&validator_ref.reference)
            .await
            .context("failed to fetch validator ref")?
    };

    if let Some(expected) = validator_ref.digest.as_ref()
        && resolved.digest != *expected
    {
        return Err(anyhow!(
            "validator digest mismatch (expected {}, got {})",
            expected,
            resolved.digest
        ));
    }

    let cache_path = resolved
        .cache_path
        .as_ref()
        .ok_or_else(|| anyhow!("validator ref resolved without cache path"))?;
    let bytes = std::fs::read(cache_path)
        .with_context(|| format!("failed to read validator cache {}", cache_path.display()))?;

    if is_zip_archive(&bytes) {
        let temp = tempfile::NamedTempFile::new()?;
        std::fs::write(temp.path(), &bytes)?;
        return load_validator_components_from_pack(temp.path());
    }

    Ok(vec![ValidatorComponent {
        component_id: "validator".to_string(),
        wasm: bytes,
    }])
}

fn is_zip_archive(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0] == 0x50 && bytes[1] == 0x4b && bytes[2] == 0x03 && bytes[3] == 0x04
}

struct ValidatorCtx {
    table: ResourceTable,
    wasi: WasiCtx,
    limits: wasmtime::StoreLimits,
}

impl ValidatorCtx {
    fn new() -> Self {
        let limits = wasmtime::StoreLimitsBuilder::new()
            .memory_size(DEFAULT_MAX_MEMORY_BYTES)
            .build();
        let wasi = WasiCtxBuilder::new()
            .inherit_stdout()
            .inherit_stderr()
            .build();
        Self {
            table: ResourceTable::new(),
            wasi,
            limits,
        }
    }
}

impl WasiView for ValidatorCtx {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            table: &mut self.table,
            ctx: &mut self.wasi,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greentic_pack::builder::SbomEntry;
    use greentic_types::{
        ComponentCapabilities, ComponentId, ComponentManifest, ComponentProfiles, PackId, PackKind,
        PackManifest, PackSignatures, ResourceHints, encode_pack_manifest,
    };
    use semver::Version;
    use serde::Serialize;
    use std::collections::BTreeMap;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;
    use zip::write::FileOptions;
    use zip::{CompressionMethod, ZipWriter};

    #[derive(Serialize)]
    struct SbomDocument {
        format: String,
        files: Vec<SbomEntry>,
    }

    #[test]
    fn validator_pack_accepts_id_wasm_from_pack_manifest() {
        let temp = tempdir().expect("temp dir");
        let pack_path = temp.path().join("validator.gtpack");
        let component_id = ComponentId::new("messaging-validator").expect("component id");
        let component_version = Version::parse("0.1.0").expect("component version");
        let component = ComponentManifest {
            id: component_id.clone(),
            version: component_version.clone(),
            supports: Vec::new(),
            world: PACK_VALIDATOR_WORLDS[0].to_string(),
            profiles: ComponentProfiles::default(),
            capabilities: ComponentCapabilities::default(),
            configurators: None,
            operations: Vec::new(),
            config_schema: None,
            resources: ResourceHints::default(),
            dev_flows: BTreeMap::new(),
        };
        let manifest = PackManifest {
            schema_version: "pack-v1".to_string(),
            pack_id: PackId::new("dev.local.validator").expect("pack id"),
            name: None,
            version: component_version,
            kind: PackKind::Provider,
            publisher: "test".to_string(),
            components: vec![component],
            flows: Vec::new(),
            dependencies: Vec::new(),
            capabilities: Vec::new(),
            secret_requirements: Vec::new(),
            signatures: PackSignatures::default(),
            bootstrap: None,
            extensions: None,
        };
        let manifest_cbor = encode_pack_manifest(&manifest).expect("encode manifest");
        let wasm_bytes = b"validator wasm";
        let wasm_path = format!("components/{}.wasm", component_id.as_str());
        let sbom_entries = vec![
            SbomEntry {
                path: "manifest.cbor".to_string(),
                size: manifest_cbor.len() as u64,
                hash_blake3: blake3::hash(&manifest_cbor).to_hex().to_string(),
                media_type: "application/cbor".to_string(),
            },
            SbomEntry {
                path: wasm_path.clone(),
                size: wasm_bytes.len() as u64,
                hash_blake3: blake3::hash(wasm_bytes).to_hex().to_string(),
                media_type: "application/wasm".to_string(),
            },
        ];
        let sbom = SbomDocument {
            format: "greentic-sbom-v1".to_string(),
            files: sbom_entries,
        };
        let sbom_cbor = serde_cbor::to_vec(&sbom).expect("encode sbom");

        let file = File::create(&pack_path).expect("create pack");
        let mut writer = ZipWriter::new(file);
        let options = FileOptions::<()>::default().compression_method(CompressionMethod::Stored);
        writer
            .start_file("manifest.cbor", options)
            .expect("start manifest");
        writer.write_all(&manifest_cbor).expect("write manifest");
        writer.start_file(&wasm_path, options).expect("start wasm");
        writer.write_all(wasm_bytes).expect("write wasm");
        writer.start_file("sbom.cbor", options).expect("start sbom");
        writer.write_all(&sbom_cbor).expect("write sbom");
        writer.finish().expect("finish pack");

        let components =
            load_validator_components_from_pack(&pack_path).expect("load validator components");
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].component_id, "messaging-validator");
        assert_eq!(components[0].wasm, wasm_bytes);
    }
}

