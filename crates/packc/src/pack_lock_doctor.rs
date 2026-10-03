#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use greentic_distributor_client::{DistClient, DistOptions};
use greentic_flow::wizard_ops::{WizardMode, decode_component_qa_spec, fetch_wizard_spec};
use greentic_pack::PackLoad;
use greentic_pack::pack_lock::{LockedComponent, PackLockV1, read_pack_lock, validate_pack_lock};
use greentic_types::cbor::canonical;
use greentic_types::pack::extensions::component_sources::{
    ArtifactLocationV1, ComponentSourceEntryV1, ComponentSourcesV1,
};
use greentic_types::schemas::common::schema_ir::{AdditionalProperties, SchemaIr};
use greentic_types::schemas::component::v0_6_0::{ComponentDescribe, schema_hash};
use greentic_types::validate::{Diagnostic, Severity};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::runtime::Handle;
use wasmtime::Engine;
use wasmtime::component::{Component as WasmtimeComponent, Linker};

use crate::component_host_stubs::{
    DescribeHostState, add_describe_host_imports, stub_remaining_imports,
};
use crate::runtime::{NetworkPolicy, RuntimeContext};

pub struct PackLockDoctorInput<'a> {
    pub load: &'a PackLoad,
    pub pack_dir: Option<&'a Path>,
    pub runtime: &'a RuntimeContext,
    pub allow_oci_tags: bool,
    pub use_describe_cache: bool,
    pub online: bool,
}

pub struct PackLockDoctorOutput {
    pub diagnostics: Vec<Diagnostic>,
    pub has_errors: bool,
}

#[derive(Clone)]
struct ComponentDiagnostic {
    component_id: String,
    diagnostic: Diagnostic,
}

struct WasmSource {
    bytes: Vec<u8>,
    source_path: Option<PathBuf>,
    describe_bytes: Option<Vec<u8>>,
}

struct DescribeResolution {
    describe: ComponentDescribe,
    requires_typed_instance: bool,
}

pub fn run_pack_lock_doctor(input: PackLockDoctorInput<'_>) -> Result<PackLockDoctorOutput> {
    let mut diagnostics: Vec<ComponentDiagnostic> = Vec::new();
    let mut has_errors = false;

    let pack_lock = match load_pack_lock(input.load, input.pack_dir) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            diagnostics.push(ComponentDiagnostic {
                component_id: String::new(),
                diagnostic: Diagnostic {
                    severity: Severity::Warn,
                    code: "PACK_LOCK_MISSING".to_string(),
                    message: "pack.lock.cbor missing; skipping pack lock doctor checks".to_string(),
                    path: Some("pack.lock.cbor".to_string()),
                    hint: Some(
                        "run `greentic-pack resolve` to generate pack.lock.cbor".to_string(),
                    ),
                    data: Value::Null,
                },
            });
            return Ok(finish_diagnostics(diagnostics));
        }
        Err(err) => {
            diagnostics.push(ComponentDiagnostic {
                component_id: String::new(),
                diagnostic: Diagnostic {
                    severity: Severity::Error,
                    code: "PACK_LOCK_INVALID".to_string(),
                    message: format!("failed to load pack.lock.cbor: {err}"),
                    path: Some("pack.lock.cbor".to_string()),
                    hint: Some("regenerate the lock with `greentic-pack resolve`".to_string()),
                    data: Value::Null,
                },
            });
            return Ok(finish_diagnostics(diagnostics));
        }
    };

    let component_sources = match load_component_sources(input.load) {
        Ok(sources) => sources,
        Err(err) => {
            diagnostics.push(ComponentDiagnostic {
                component_id: String::new(),
                diagnostic: Diagnostic {
                    severity: Severity::Warn,
                    code: "PACK_LOCK_COMPONENT_SOURCES_INVALID".to_string(),
                    message: format!("component sources extension invalid: {err}"),
                    path: Some("manifest.cbor".to_string()),
                    hint: Some("rebuild the pack to refresh component sources".to_string()),
                    data: Value::Null,
                },
            });
            None
        }
    };

    let component_sources_map = build_component_sources_map(component_sources.as_ref());
    let manifest_map: HashMap<_, _> = input
        .load
        .manifest
        .components
        .iter()
        .map(|entry| (entry.name.clone(), entry))
        .collect();

    let engine = Engine::default();

    for (component_id, locked) in &pack_lock.components {
        if locked.abi_version != "0.6.0" {
            continue;
        }

        let wasm = match resolve_component_wasm(
            &input,
            &manifest_map,
            &component_sources_map,
            component_id,
            locked,
        ) {
            Ok(wasm) => wasm,
            Err(err) => {
                has_errors = true;
                diagnostics.push(component_diag(
                    component_id,
                    Severity::Error,
                    "PACK_LOCK_COMPONENT_WASM_MISSING",
                    format!("component wasm unavailable: {err}"),
                    Some(format!("components/{component_id}")),
                    Some(
                        "bundle artifacts into the pack or allow online resolution with --online"
                            .to_string(),
                    ),
                    Value::Null,
                ));
                continue;
            }
        };

        let digest = format!("sha256:{}", hex::encode(Sha256::digest(&wasm.bytes)));
        if digest != locked.resolved_digest {
            has_errors = true;
            diagnostics.push(component_diag(
                component_id,
                Severity::Error,
                "PACK_LOCK_COMPONENT_DIGEST_MISMATCH",
                "resolved_digest does not match component bytes".to_string(),
                Some(format!("components/{component_id}")),
                Some("re-run `greentic-pack resolve` after updating components".to_string()),
                json!({ "expected": locked.resolved_digest, "actual": digest }),
            ));
        }

        let describe_resolution = match describe_component_with_cache(
            &engine,
            &wasm,
            input.use_describe_cache,
            component_id,
        ) {
            Ok(describe) => describe,
            Err(err) => {
                has_errors = true;
                diagnostics.push(component_diag(
                    component_id,
                    Severity::Error,
                    "PACK_LOCK_COMPONENT_DESCRIBE_FAILED",
                    format!("describe() failed: {err}"),
                    Some(format!("components/{component_id}")),
                    Some("ensure the component exports greentic:component@0.6.0".to_string()),
                    Value::Null,
                ));
                continue;
            }
        };
        let describe = describe_resolution.describe;

        if describe.info.id != locked.component_id {
            has_errors = true;
            diagnostics.push(component_diag(
                component_id,
                Severity::Error,
                "PACK_LOCK_COMPONENT_ID_MISMATCH",
                "describe id does not match pack.lock component_id".to_string(),
                Some(format!("components/{component_id}")),
                None,
                json!({ "describe_id": describe.info.id, "component_id": locked.component_id }),
            ));
        }

        let describe_hash = compute_describe_hash(&describe)?;
        if describe_hash != locked.describe_hash {
            has_errors = true;
            diagnostics.push(component_diag(
                component_id,
                Severity::Error,
                "PACK_LOCK_DESCRIBE_HASH_MISMATCH",
                "describe_hash does not match describe() output".to_string(),
                Some(format!("components/{component_id}")),
                Some("re-run `greentic-pack resolve` after updating components".to_string()),
                json!({ "expected": locked.describe_hash, "actual": describe_hash }),
            ));
        }

        let mut describe_ops = BTreeMap::new();
        for op in &describe.operations {
            describe_ops.insert(op.id.clone(), op);
        }

        for op in &describe.operations {
            let recomputed =
                match schema_hash(&op.input.schema, &op.output.schema, &describe.config_schema) {
                    Ok(hash) => hash,
                    Err(err) => {
                        has_errors = true;
                        diagnostics.push(component_diag(
                            component_id,
                            Severity::Error,
                            "PACK_LOCK_SCHEMA_HASH_COMPUTE_FAILED",
                            format!("schema_hash failed for {}: {err}", op.id),
                            Some(format!(
                                "components/{component_id}/operations/{}/schema_hash",
                                op.id
                            )),
                            None,
                            Value::Null,
                        ));
                        continue;
                    }
                };

            if recomputed != op.schema_hash {
                has_errors = true;
                diagnostics.push(component_diag(
                    component_id,
                    Severity::Error,
                    "PACK_LOCK_SCHEMA_HASH_DESCRIBE_MISMATCH",
                    "schema_hash does not match describe() payload".to_string(),
                    Some(format!(
                        "components/{component_id}/operations/{}/schema_hash",
                        op.id
                    )),
                    None,
                    json!({ "expected": op.schema_hash, "actual": recomputed }),
                ));
            }

            match locked
                .operations
                .iter()
                .find(|entry| entry.operation_id == op.id)
            {
                Some(lock_op) => {
                    if recomputed != lock_op.schema_hash {
                        has_errors = true;
                        diagnostics.push(component_diag(
                            component_id,
                            Severity::Error,
                            "PACK_LOCK_SCHEMA_HASH_LOCK_MISMATCH",
                            "schema_hash does not match pack.lock entry".to_string(),
                            Some(format!(
                                "components/{component_id}/operations/{}/schema_hash",
                                op.id
                            )),
                            None,
                            json!({ "expected": lock_op.schema_hash, "actual": recomputed }),
                        ));
                    }
                }
                None => {
                    has_errors = true;
                    diagnostics.push(component_diag(
                        component_id,
                        Severity::Error,
                        "PACK_LOCK_OPERATION_MISSING",
                        "operation missing from pack.lock".to_string(),
                        Some(format!("components/{component_id}/operations/{}", op.id)),
                        Some(
                            "re-run `greentic-pack resolve` to refresh lock operations".to_string(),
                        ),
                        Value::Null,
                    ));
                }
            }

            validate_schema_ir(
                component_id,
                &op.input.schema,
                &format!(
                    "components/{component_id}/operations/{}/input.schema",
                    op.id
                ),
                &mut diagnostics,
                &mut has_errors,
            );
            validate_schema_ir(
                component_id,
                &op.output.schema,
                &format!(
                    "components/{component_id}/operations/{}/output.schema",
                    op.id
                ),
                &mut diagnostics,
                &mut has_errors,
            );
        }

        for lock_op in &locked.operations {
            if !describe_ops.contains_key(&lock_op.operation_id) {
                has_errors = true;
                diagnostics.push(component_diag(
                    component_id,
                    Severity::Error,
                    "PACK_LOCK_OPERATION_STALE",
                    "pack.lock operation not present in describe()".to_string(),
                    Some(format!(
                        "components/{component_id}/operations/{}",
                        lock_op.operation_id
                    )),
                    Some("re-run `greentic-pack resolve` to refresh lock operations".to_string()),
                    Value::Null,
                ));
            }
        }

        validate_schema_ir(
            component_id,
            &describe.config_schema,
            &format!("components/{component_id}/config_schema"),
            &mut diagnostics,
            &mut has_errors,
        );

        if let Err(err) = WasmtimeComponent::from_binary(&engine, &wasm.bytes) {
            has_errors = true;
            diagnostics.push(component_diag(
                component_id,
                Severity::Error,
                "PACK_LOCK_COMPONENT_DECODE_FAILED",
                format!("component bytes are not a valid component: {err}"),
                Some(format!("components/{component_id}")),
                Some("rebuild the pack with a valid component artifact".to_string()),
                Value::Null,
            ));
            continue;
        }

        if !describe_resolution.requires_typed_instance {
            diagnostics.push(component_diag(
                component_id,
                Severity::Warn,
                "PACK_LOCK_COMPONENT_WORLD_FALLBACK",
                "describe was resolved via fallback; qa/i18n contract checks were skipped"
                    .to_string(),
                Some(format!("components/{component_id}")),
                None,
                Value::Null,
            ));
        }

        for (mode, label) in [
            (WizardMode::Default, "default"),
            (WizardMode::Setup, "setup"),
            (WizardMode::Update, "update"),
            (WizardMode::Remove, "remove"),
        ] {
            let spec = match fetch_wizard_spec(&wasm.bytes, mode) {
                Ok(spec) => spec,
                Err(err) => {
                    has_errors = true;
                    diagnostics.push(component_diag(
                        component_id,
                        Severity::Error,
                        "PACK_LOCK_QA_SPEC_MISSING",
                        format!("wizard qa_spec fetch failed for {label}: {err}"),
                        Some(format!("components/{component_id}/qa/{label}")),
                        Some(
                            "ensure setup contract and setup.apply_answers are exported"
                                .to_string(),
                        ),
                        Value::Null,
                    ));
                    continue;
                }
            };
            if let Err(err) = decode_component_qa_spec(&spec.qa_spec_cbor, mode) {
                has_errors = true;
                diagnostics.push(component_diag(
                    component_id,
                    Severity::Error,
                    "PACK_LOCK_QA_SPEC_DECODE_FAILED",
                    format!("qa_spec decode failed for {label}: {err}"),
                    Some(format!("components/{component_id}/qa/{label}")),
                    Some("ensure qa_spec is valid canonical CBOR/legacy JSON".to_string()),
                    Value::Null,
                ));
            }
        }
    }

    Ok(finish_diagnostics(diagnostics))
}

fn finish_diagnostics(mut diagnostics: Vec<ComponentDiagnostic>) -> PackLockDoctorOutput {
    diagnostics.sort_by(|a, b| {
        let path_a = a.diagnostic.path.as_deref().unwrap_or_default();
        let path_b = b.diagnostic.path.as_deref().unwrap_or_default();
        a.component_id
            .cmp(&b.component_id)
            .then_with(|| a.diagnostic.code.cmp(&b.diagnostic.code))
            .then_with(|| path_a.cmp(path_b))
    });
    let mut has_errors = false;
    let diagnostics: Vec<Diagnostic> = diagnostics
        .into_iter()
        .map(|entry| {
            if matches!(entry.diagnostic.severity, Severity::Error) {
                has_errors = true;
            }
            entry.diagnostic
        })
        .collect();
    PackLockDoctorOutput {
        diagnostics,
        has_errors,
    }
}

fn load_pack_lock(load: &PackLoad, pack_dir: Option<&Path>) -> Result<Option<PackLockV1>> {
    if let Some(bytes) = load.files.get("pack.lock.cbor") {
        return read_pack_lock_from_bytes(bytes).map(Some);
    }
    let Some(pack_dir) = pack_dir else {
        return Ok(None);
    };
    let path = pack_dir.join("pack.lock.cbor");
    if !path.exists() {
        return Ok(None);
    }
    read_pack_lock(&path).map(Some)
}

fn read_pack_lock_from_bytes(bytes: &[u8]) -> Result<PackLockV1> {
    canonical::ensure_canonical(bytes).context("pack.lock.cbor must be canonical")?;
    let lock: PackLockV1 = canonical::from_cbor(bytes).context("decode pack.lock.cbor")?;
    validate_pack_lock(&lock)?;
    Ok(lock)
}

fn load_component_sources(load: &PackLoad) -> Result<Option<ComponentSourcesV1>> {
    let Some(manifest) = load.gpack_manifest.as_ref() else {
        return Ok(None);
    };
    manifest
        .get_component_sources_v1()
        .map_err(|err| anyhow!(err.to_string()))
}

fn build_component_sources_map(
    sources: Option<&ComponentSourcesV1>,
) -> HashMap<String, ComponentSourceEntryV1> {
    let mut map = HashMap::new();
    let Some(sources) = sources else {
        return map;
    };
    for entry in &sources.components {
        let key = entry
            .component_id
            .as_ref()
            .map(|id| id.to_string())
            .unwrap_or_else(|| entry.name.clone());
        map.insert(key, entry.clone());
    }
    map
}

fn resolve_component_wasm(
    input: &PackLockDoctorInput<'_>,
    manifest_map: &HashMap<String, &greentic_pack::builder::ComponentEntry>,
    component_sources_map: &HashMap<String, ComponentSourceEntryV1>,
    component_id: &str,
    locked: &LockedComponent,
) -> Result<WasmSource> {
    if let Some(entry) = manifest_map.get(component_id) {
        let logical = entry.file_wasm.clone();
        if let Some(bytes) = input.load.files.get(&logical) {
            return Ok(WasmSource {
                bytes: bytes.clone(),
                source_path: input
                    .pack_dir
                    .map(|dir| dir.join(&entry.file_wasm))
                    .filter(|path| path.exists()),
                describe_bytes: load_describe_sidecar_from_pack(input.load, &logical),
            });
        }
        if let Some(pack_dir) = input.pack_dir {
            let disk_path = pack_dir.join(&entry.file_wasm);
            if disk_path.exists() {
                let bytes = fs::read(&disk_path)
                    .with_context(|| format!("read {}", disk_path.display()))?;
                return Ok(WasmSource {
                    bytes,
                    source_path: Some(disk_path),
                    describe_bytes: None,
                });
            }
        }
    }

    if let Some(entry) = component_sources_map.get(component_id)
        && let ArtifactLocationV1::Inline { wasm_path, .. } = &entry.artifact
    {
        if let Some(bytes) = input.load.files.get(wasm_path) {
            return Ok(WasmSource {
                bytes: bytes.clone(),
                source_path: input
                    .pack_dir
                    .map(|dir| dir.join(wasm_path))
                    .filter(|path| path.exists()),
                describe_bytes: load_describe_sidecar_from_pack(input.load, wasm_path),
            });
        }
        if let Some(pack_dir) = input.pack_dir {
            let disk_path = pack_dir.join(wasm_path);
            if disk_path.exists() {
                let bytes = fs::read(&disk_path)
                    .with_context(|| format!("read {}", disk_path.display()))?;
                return Ok(WasmSource {
                    bytes,
                    source_path: Some(disk_path),
                    describe_bytes: None,
                });
            }
        }
    }

    if let Some(reference) = locked.r#ref.as_ref()
        && reference.starts_with("file://")
    {
        let rel = strip_file_uri_prefix(reference);
        // Root the (possibly relative) ref at the pack dir so portable locks
        // resolve; absolute legacy refs are left untouched by join().
        let path = input
            .pack_dir
            .map(|dir| dir.join(rel))
            .unwrap_or_else(|| PathBuf::from(rel));
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        return Ok(WasmSource {
            bytes,
            source_path: Some(path),
            describe_bytes: None,
        });
    }

    let reference = locked
        .r#ref
        .as_ref()
        .ok_or_else(|| anyhow!("component {} missing ref", component_id))?;
    if input.online {
        input
            .runtime
            .require_online("pack lock doctor component download")?;
    }
    let offline = !input.online || input.runtime.network_policy() == NetworkPolicy::Offline;
    let dist = DistClient::new(DistOptions {
        cache_dir: input.runtime.cache_dir(),
        allow_tags: input.allow_oci_tags,
        offline,
        allow_insecure_local_http: false,
        ..DistOptions::default()
    });

    let handle = Handle::try_current().context("component resolution requires a Tokio runtime")?;
    let resolved = if offline {
        dist.open_cached(&locked.resolved_digest)
            .map_err(|err| anyhow!("offline cache miss for {}: {}", reference, err))?
    } else {
        let source = dist
            .parse_source(reference)
            .map_err(|err| anyhow!("resolve {}: {}", reference, err))?;
        let descriptor = block_on(
            &handle,
            dist.resolve(source, greentic_distributor_client::ResolvePolicy),
        )
        .map_err(|err| anyhow!("resolve {}: {}", reference, err))?;
        block_on(
            &handle,
            dist.fetch(&descriptor, greentic_distributor_client::CachePolicy),
        )
        .map_err(|err| anyhow!("resolve {}: {}", reference, err))?
    };
    let path = resolved
        .cache_path
        .ok_or_else(|| anyhow!("resolved component missing path for {}", reference))?;
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(WasmSource {
        bytes,
        source_path: Some(path),
        describe_bytes: None,
    })
}

fn block_on<F, T, E>(handle: &Handle, fut: F) -> std::result::Result<T, E>
where
    F: std::future::Future<Output = std::result::Result<T, E>>,
{
    tokio::task::block_in_place(|| handle.block_on(fut))
}

fn describe_component_with_cache(
    engine: &Engine,
    wasm: &WasmSource,
    use_cache: bool,
    component_id: &str,
) -> Result<DescribeResolution> {
    match describe_component(engine, &wasm.bytes) {
        Ok(describe) => Ok(DescribeResolution {
            describe,
            requires_typed_instance: true,
        }),
        Err(err) => {
            if should_fallback_to_untyped_describe(&err)
                && let Ok(describe) = describe_component_untyped(engine, &wasm.bytes)
            {
                return Ok(DescribeResolution {
                    describe,
                    requires_typed_instance: false,
                });
            }
            if use_cache || should_fallback_to_describe_cache(&err) {
                if let Some(describe) = load_describe_from_cache(
                    wasm.describe_bytes.as_deref(),
                    wasm.source_path.as_deref(),
                )? {
                    return Ok(DescribeResolution {
                        describe,
                        requires_typed_instance: false,
                    });
                }
                bail!("describe failed and no describe cache found for {component_id}: {err}");
            }
            Err(err)
        }
    }
}

fn describe_component_untyped(engine: &Engine, bytes: &[u8]) -> Result<ComponentDescribe> {
    let component = WasmtimeComponent::from_binary(engine, bytes)
        .map_err(|err| anyhow!("decode component bytes: {err}"))?;
    let mut store = wasmtime::Store::new(engine, DescribeHostState::default());
    let mut linker = Linker::new(engine);
    add_describe_host_imports(&mut linker)?;
    // Stub any remaining imports (secrets-store, http-client, interfaces-types,
    // etc.) that the component needs but describe() never calls at runtime.
    stub_remaining_imports(&mut linker, &component)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .map_err(|err| anyhow!("instantiate component root world: {err}"))?;

    let descriptor = [
        "component-descriptor",
        "greentic:component/component-descriptor",
        "greentic:component/component-descriptor@0.6.0",
    ]
    .iter()
    .find_map(|name| instance.get_export_index(&mut store, None, name))
    .ok_or_else(|| anyhow!("missing exported descriptor instance"))?;
    let describe_export = [
        "describe",
        "greentic:component/component-descriptor@0.6.0#describe",
    ]
    .iter()
    .find_map(|name| instance.get_export_index(&mut store, Some(&descriptor), name))
    .ok_or_else(|| anyhow!("missing exported describe function"))?;
    let describe_func = instance
        .get_typed_func::<(), (Vec<u8>,)>(&mut store, &describe_export)
        .map_err(|err| anyhow!("lookup component-descriptor.describe: {err}"))?;
    let (describe_bytes,) = describe_func
        .call(&mut store, ())
        .map_err(|err| anyhow!("call component-descriptor.describe: {err}"))?;
    canonical::from_cbor(&describe_bytes).context("decode ComponentDescribe")
}

fn should_fallback_to_describe_cache(err: &anyhow::Error) -> bool {
    err.to_string().contains("instantiate component-v0-v6-v0")
}

fn should_fallback_to_untyped_describe(err: &anyhow::Error) -> bool {
    err.to_string().contains("instantiate component-v0-v6-v0")
}

fn describe_component(engine: &Engine, bytes: &[u8]) -> Result<ComponentDescribe> {
    describe_component_untyped(engine, bytes)
}

fn load_describe_from_cache(
    inline_bytes: Option<&[u8]>,
    source_path: Option<&Path>,
) -> Result<Option<ComponentDescribe>> {
    if let Some(bytes) = inline_bytes {
        canonical::ensure_canonical(bytes).context("describe cache must be canonical")?;
        let describe =
            canonical::from_cbor(bytes).context("decode ComponentDescribe from cache")?;
        return Ok(Some(describe));
    }
    if let Some(path) = source_path {
        let describe_path = PathBuf::from(format!("{}.describe.cbor", path.display()));
        if !describe_path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&describe_path)
            .with_context(|| format!("read {}", describe_path.display()))?;
        canonical::ensure_canonical(&bytes).context("describe cache must be canonical")?;
        let describe =
            canonical::from_cbor(&bytes).context("decode ComponentDescribe from cache")?;
        return Ok(Some(describe));
    }
    Ok(None)
}

fn load_describe_sidecar_from_pack(load: &PackLoad, logical_path: &str) -> Option<Vec<u8>> {
    let describe_path = format!("{logical_path}.describe.cbor");
    load.files.get(&describe_path).cloned()
}

fn compute_describe_hash(describe: &ComponentDescribe) -> Result<String> {
    let bytes =
        canonical::to_canonical_cbor_allow_floats(describe).context("canonicalize describe")?;
    let digest = Sha256::digest(bytes.as_slice());
    Ok(hex::encode(digest))
}

fn validate_schema_ir(
    component_id: &str,
    schema: &SchemaIr,
    path: &str,
    diagnostics: &mut Vec<ComponentDiagnostic>,
    has_errors: &mut bool,
) {
    match schema {
        SchemaIr::Object {
            properties,
            required,
            additional,
        } => {
            if properties.is_empty()
                && required.is_empty()
                && matches!(additional, AdditionalProperties::Allow)
            {
                let inside_variant = path.contains("/variants/");
                if !inside_variant {
                    *has_errors = true;
                }
                diagnostics.push(component_diag(
                    component_id,
                    if inside_variant {
                        Severity::Warn
                    } else {
                        Severity::Error
                    },
                    "PACK_LOCK_SCHEMA_UNCONSTRAINED_OBJECT",
                    "object schema is unconstrained".to_string(),
                    Some(path.to_string()),
                    Some("add properties or set additional=forbid".to_string()),
                    Value::Null,
                ));
            }
            for req in required {
                if !properties.contains_key(req) {
                    *has_errors = true;
                    diagnostics.push(component_diag(
                        component_id,
                        Severity::Error,
                        "PACK_LOCK_SCHEMA_REQUIRED_MISSING",
                        format!("required property `{req}` missing from properties"),
                        Some(path.to_string()),
                        None,
                        Value::Null,
                    ));
                }
            }
            for (name, prop) in properties {
                let child_path = format!("{path}/properties/{name}");
                validate_schema_ir(component_id, prop, &child_path, diagnostics, has_errors);
            }
            if let AdditionalProperties::Schema(schema) = additional {
                let child_path = format!("{path}/additional");
                validate_schema_ir(component_id, schema, &child_path, diagnostics, has_errors);
            }
        }
        SchemaIr::Array {
            items,
            min_items,
            max_items,
        } => {
            if let (Some(min), Some(max)) = (min_items, max_items)
                && min > max
            {
                *has_errors = true;
                diagnostics.push(component_diag(
                    component_id,
                    Severity::Error,
                    "PACK_LOCK_SCHEMA_ARRAY_BOUNDS",
                    format!("min_items {min} exceeds max_items {max}"),
                    Some(path.to_string()),
                    None,
                    Value::Null,
                ));
            }
            let child_path = format!("{path}/items");
            validate_schema_ir(component_id, items, &child_path, diagnostics, has_errors);
        }
        SchemaIr::String {
            min_len,
            max_len,
            regex,
            format,
        } => {
            if let (Some(min), Some(max)) = (min_len, max_len)
                && min > max
            {
                *has_errors = true;
                diagnostics.push(component_diag(
                    component_id,
                    Severity::Error,
                    "PACK_LOCK_SCHEMA_STRING_BOUNDS",
                    format!("min_len {min} exceeds max_len {max}"),
                    Some(path.to_string()),
                    None,
                    Value::Null,
                ));
            }
            if regex.is_some() || format.is_some() {
                diagnostics.push(component_diag(
                    component_id,
                    Severity::Warn,
                    "PACK_LOCK_SCHEMA_STRING_CONSTRAINT",
                    "string regex/format constraints are not validated by pack doctor".to_string(),
                    Some(path.to_string()),
                    None,
                    Value::Null,
                ));
            }
        }
        SchemaIr::Int { min, max } => {
            if let (Some(min), Some(max)) = (min, max)
                && min > max
            {
                *has_errors = true;
                diagnostics.push(component_diag(
                    component_id,
                    Severity::Error,
                    "PACK_LOCK_SCHEMA_INT_BOUNDS",
                    format!("min {min} exceeds max {max}"),
                    Some(path.to_string()),
                    None,
                    Value::Null,
                ));
            }
        }
        SchemaIr::Float { min, max } => {
            if let (Some(min), Some(max)) = (min, max)
                && min > max
            {
                *has_errors = true;
                diagnostics.push(component_diag(
                    component_id,
                    Severity::Error,
                    "PACK_LOCK_SCHEMA_FLOAT_BOUNDS",
                    format!("min {min} exceeds max {max}"),
                    Some(path.to_string()),
                    None,
                    Value::Null,
                ));
            }
        }
        SchemaIr::Enum { values } => {
            if values.is_empty() {
                *has_errors = true;
                diagnostics.push(component_diag(
                    component_id,
                    Severity::Error,
                    "PACK_LOCK_SCHEMA_ENUM_EMPTY",
                    "enum has no values".to_string(),
                    Some(path.to_string()),
                    None,
                    Value::Null,
                ));
            }
        }
        SchemaIr::OneOf { variants } => {
            if variants.is_empty() {
                *has_errors = true;
                diagnostics.push(component_diag(
                    component_id,
                    Severity::Error,
                    "PACK_LOCK_SCHEMA_ONEOF_EMPTY",
                    "oneOf has no variants".to_string(),
                    Some(path.to_string()),
                    None,
                    Value::Null,
                ));
            }
            for (idx, variant) in variants.iter().enumerate() {
                let child_path = format!("{path}/variants/{idx}");
                validate_schema_ir(component_id, variant, &child_path, diagnostics, has_errors);
            }
        }
        SchemaIr::Bool | SchemaIr::Null | SchemaIr::Bytes | SchemaIr::Ref { .. } => {}
    }
}

fn component_diag(
    component_id: &str,
    severity: Severity,
    code: &str,
    message: String,
    path: Option<String>,
    hint: Option<String>,
    data: Value,
) -> ComponentDiagnostic {
    ComponentDiagnostic {
        component_id: component_id.to_string(),
        diagnostic: Diagnostic {
            severity,
            code: code.to_string(),
            message,
            path,
            hint,
            data,
        },
    }
}

fn strip_file_uri_prefix(reference: &str) -> &str {
    reference.strip_prefix("file://").unwrap_or(reference)
}

#[cfg(test)]
mod tests {
    use super::*;
    use greentic_pack::PackLoad;
    use greentic_types::ComponentId;
    use greentic_types::component_source::ComponentSourceRef;
    use greentic_types::pack::extensions::component_sources::{
        ComponentSourceEntryV1, ResolvedComponentV1,
    };
    use greentic_types::schemas::common::schema_ir::AdditionalProperties;
    use tempfile::TempDir;

    #[test]
    fn finish_diagnostics_sorts_and_marks_errors() {
        let output = finish_diagnostics(vec![
            component_diag(
                "b.component",
                Severity::Warn,
                "WARN_CODE",
                "warn".to_string(),
                Some("z".to_string()),
                None,
                Value::Null,
            ),
            component_diag(
                "a.component",
                Severity::Error,
                "ERR_CODE",
                "err".to_string(),
                Some("a".to_string()),
                None,
                Value::Null,
            ),
        ]);

        assert!(output.has_errors);
        assert_eq!(output.diagnostics[0].code, "ERR_CODE");
        assert_eq!(output.diagnostics[1].code, "WARN_CODE");
    }

    #[test]
    fn build_component_sources_map_prefers_component_id_when_present() {
        let sources = ComponentSourcesV1::new(vec![
            ComponentSourceEntryV1 {
                name: "friendly".to_string(),
                component_id: Some(ComponentId::try_from("demo.component").expect("component id")),
                source: ComponentSourceRef::File("components/demo.wasm".to_string()),
                resolved: ResolvedComponentV1 {
                    digest: "sha256:abc".to_string(),
                    signature: None,
                    signed_by: None,
                },
                artifact: ArtifactLocationV1::Remote,
                licensing_hint: None,
                metering_hint: None,
            },
            ComponentSourceEntryV1 {
                name: "name.only".to_string(),
                component_id: None,
                source: ComponentSourceRef::File("components/name.wasm".to_string()),
                resolved: ResolvedComponentV1 {
                    digest: "sha256:def".to_string(),
                    signature: None,
                    signed_by: None,
                },
                artifact: ArtifactLocationV1::Remote,
                licensing_hint: None,
                metering_hint: None,
            },
        ]);

        let map = build_component_sources_map(Some(&sources));
        assert!(map.contains_key("demo.component"));
        assert!(map.contains_key("name.only"));
        assert!(!map.contains_key("friendly"));
    }

    #[test]
    fn validate_schema_ir_reports_unconstrained_objects_and_bad_bounds() {
        let mut diagnostics = Vec::new();
        let mut has_errors = false;

        validate_schema_ir(
            "demo.component",
            &SchemaIr::Object {
                properties: BTreeMap::new(),
                required: Vec::new(),
                additional: AdditionalProperties::Allow,
            },
            "config",
            &mut diagnostics,
            &mut has_errors,
        );
        validate_schema_ir(
            "demo.component",
            &SchemaIr::Array {
                items: Box::new(SchemaIr::Bool),
                min_items: Some(3),
                max_items: Some(1),
            },
            "config/list",
            &mut diagnostics,
            &mut has_errors,
        );

        assert!(has_errors);
        assert!(diagnostics.iter().any(|diag| {
            diag.diagnostic.code == "PACK_LOCK_SCHEMA_UNCONSTRAINED_OBJECT"
                && diag.diagnostic.path.as_deref() == Some("config")
        }));
        assert!(diagnostics.iter().any(|diag| {
            diag.diagnostic.code == "PACK_LOCK_SCHEMA_ARRAY_BOUNDS"
                && diag.diagnostic.path.as_deref() == Some("config/list")
        }));
    }

    #[test]
    fn validate_schema_ir_downgrades_variant_unconstrained_object_to_warning() {
        let mut diagnostics = Vec::new();
        let mut has_errors = false;

        validate_schema_ir(
            "demo.component",
            &SchemaIr::Object {
                properties: BTreeMap::new(),
                required: Vec::new(),
                additional: AdditionalProperties::Allow,
            },
            "config/variants/0",
            &mut diagnostics,
            &mut has_errors,
        );

        assert!(!has_errors);
        assert_eq!(diagnostics.len(), 1);
        assert!(matches!(diagnostics[0].diagnostic.severity, Severity::Warn));
    }

    #[test]
    fn strip_file_uri_prefix_handles_prefixed_and_plain_paths() {
        assert_eq!(
            strip_file_uri_prefix("file:///tmp/demo.wasm"),
            "/tmp/demo.wasm"
        );
        assert_eq!(
            strip_file_uri_prefix("components/demo.wasm"),
            "components/demo.wasm"
        );
    }

    #[test]
    fn validate_schema_ir_reports_string_constraints_as_warnings() {
        let mut diagnostics = Vec::new();
        let mut has_errors = false;

        validate_schema_ir(
            "demo.component",
            &SchemaIr::String {
                min_len: Some(1),
                max_len: Some(8),
                regex: Some("^demo$".to_string()),
                format: None,
            },
            "config/name",
            &mut diagnostics,
            &mut has_errors,
        );

        assert!(!has_errors);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].diagnostic.code,
            "PACK_LOCK_SCHEMA_STRING_CONSTRAINT"
        );
    }

    #[test]
    fn load_describe_from_cache_reads_inline_and_sidecar_bytes() {
        let temp = TempDir::new().expect("temp dir");
        let describe = greentic_types::schemas::component::v0_6_0::ComponentDescribe {
            info: greentic_types::schemas::component::v0_6_0::ComponentInfo {
                id: "demo.component".to_string(),
                version: "0.1.0".to_string(),
                role: "tool".to_string(),
                display_name: None,
            },
            provided_capabilities: Vec::new(),
            required_capabilities: Vec::new(),
            metadata: BTreeMap::new(),
            operations: Vec::new(),
            config_schema: SchemaIr::Bool,
            outcomes: Vec::new(),
        };
        let bytes = canonical::to_canonical_cbor_allow_floats(&describe).expect("describe bytes");
        let wasm_path = temp.path().join("component.wasm");
        std::fs::write(&wasm_path, b"\0asm").expect("wasm");
        std::fs::write(format!("{}.describe.cbor", wasm_path.display()), &bytes).expect("sidecar");

        let inline = load_describe_from_cache(Some(&bytes), None)
            .expect("inline load")
            .expect("inline describe");
        let sidecar = load_describe_from_cache(None, Some(&wasm_path))
            .expect("sidecar load")
            .expect("sidecar describe");

        assert_eq!(inline.info.id, "demo.component");
        assert_eq!(sidecar.info.id, "demo.component");
    }

    #[test]
    fn load_describe_sidecar_from_pack_uses_logical_suffix() {
        let manifest = greentic_pack::builder::PackManifest {
            meta: greentic_pack::builder::PackMeta {
                pack_version: 1,
                pack_id: "demo.pack".to_string(),
                version: semver::Version::parse("0.1.0").expect("version"),
                name: "Demo".to_string(),
                kind: None,
                description: None,
                authors: Vec::new(),
                license: None,
                homepage: None,
                support: None,
                vendor: None,
                imports: Vec::new(),
                entry_flows: Vec::new(),
                created_at_utc: "2026-01-01T00:00:00Z".to_string(),
                events: None,
                repo: None,
                messaging: None,
                interfaces: Vec::new(),
                annotations: serde_json::Map::new(),
                distribution: None,
                components: Vec::new(),
            },
            flows: Vec::new(),
            components: Vec::new(),
            distribution: None,
            component_descriptors: Vec::new(),
            extensions: None,
        };
        let mut load = PackLoad {
            manifest,
            report: Default::default(),
            sbom: Vec::new(),
            files: HashMap::new(),
            gpack_manifest: None,
        };
        load.files.insert(
            "components/demo.wasm.describe.cbor".to_string(),
            vec![1, 2, 3],
        );

        assert_eq!(
            load_describe_sidecar_from_pack(&load, "components/demo.wasm"),
            Some(vec![1, 2, 3])
        );
    }

    #[test]
    fn compute_describe_hash_is_stable_for_same_payload() {
        let describe = greentic_types::schemas::component::v0_6_0::ComponentDescribe {
            info: greentic_types::schemas::component::v0_6_0::ComponentInfo {
                id: "demo.component".to_string(),
                version: "0.1.0".to_string(),
                role: "tool".to_string(),
                display_name: None,
            },
            provided_capabilities: Vec::new(),
            required_capabilities: Vec::new(),
            metadata: BTreeMap::new(),
            operations: Vec::new(),
            config_schema: SchemaIr::Bool,
            outcomes: Vec::new(),
        };

        let first = compute_describe_hash(&describe).expect("first hash");
        let second = compute_describe_hash(&describe).expect("second hash");
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn fallback_heuristics_only_trigger_for_known_errors() {
        let fallback = anyhow!("failed to instantiate component-v0-v6-v0 during describe");
        let other = anyhow!("totally different error");

        assert!(should_fallback_to_describe_cache(&fallback));
        assert!(should_fallback_to_untyped_describe(&fallback));
        assert!(!should_fallback_to_describe_cache(&other));
        assert!(!should_fallback_to_untyped_describe(&other));
    }

    #[test]
    fn load_pack_lock_returns_none_when_no_bytes_or_disk_file_exist() {
        let manifest = greentic_pack::builder::PackManifest {
            meta: greentic_pack::builder::PackMeta {
                pack_version: 1,
                pack_id: "demo.pack".to_string(),
                version: semver::Version::parse("0.1.0").expect("version"),
                name: "Demo".to_string(),
                kind: None,
                description: None,
                authors: Vec::new(),
                license: None,
                homepage: None,
                support: None,
                vendor: None,
                imports: Vec::new(),
                entry_flows: Vec::new(),
                created_at_utc: "2026-01-01T00:00:00Z".to_string(),
                events: None,
                repo: None,
                messaging: None,
                interfaces: Vec::new(),
                annotations: serde_json::Map::new(),
                distribution: None,
                components: Vec::new(),
            },
            flows: Vec::new(),
            components: Vec::new(),
            distribution: None,
            component_descriptors: Vec::new(),
            extensions: None,
        };
        let load = PackLoad {
            manifest,
            report: Default::default(),
            sbom: Vec::new(),
            files: HashMap::new(),
            gpack_manifest: None,
        };
        let temp = TempDir::new().expect("temp dir");

        assert!(load_pack_lock(&load, None).expect("none").is_none());
        assert!(
            load_pack_lock(&load, Some(temp.path()))
                .expect("none")
                .is_none()
        );
    }

    #[test]
    fn read_pack_lock_from_bytes_rejects_invalid_lock_documents() {
        let invalid = PackLockV1::new(BTreeMap::from([(
            "demo.component".to_string(),
            LockedComponent {
                component_id: "demo.component".to_string(),
                r#ref: None,
                abi_version: "0.6.0".to_string(),
                resolved_digest: "sha256:abc".to_string(),
                describe_hash: "not-a-real-hash".to_string(),
                operations: Vec::new(),
                world: None,
                component_version: None,
                role: None,
            },
        )]));
        let bytes = canonical::to_canonical_cbor_allow_floats(&invalid).expect("cbor");

        let err = read_pack_lock_from_bytes(&bytes).expect_err("invalid lock should fail");
        assert!(err.to_string().contains("describe_hash"));
    }

    #[test]
    fn validate_schema_ir_reports_missing_required_and_empty_variants() {
        let mut diagnostics = Vec::new();
        let mut has_errors = false;

        validate_schema_ir(
            "demo.component",
            &SchemaIr::Object {
                properties: BTreeMap::new(),
                required: vec!["missing".to_string()],
                additional: AdditionalProperties::Forbid,
            },
            "config",
            &mut diagnostics,
            &mut has_errors,
        );
        validate_schema_ir(
            "demo.component",
            &SchemaIr::OneOf {
                variants: Vec::new(),
            },
            "config/choice",
            &mut diagnostics,
            &mut has_errors,
        );
        validate_schema_ir(
            "demo.component",
            &SchemaIr::Enum { values: Vec::new() },
            "config/enum",
            &mut diagnostics,
            &mut has_errors,
        );

        assert!(has_errors);
        assert!(
            diagnostics
                .iter()
                .any(|diag| { diag.diagnostic.code == "PACK_LOCK_SCHEMA_REQUIRED_MISSING" })
        );
        assert!(
            diagnostics
                .iter()
                .any(|diag| { diag.diagnostic.code == "PACK_LOCK_SCHEMA_ONEOF_EMPTY" })
        );
        assert!(
            diagnostics
                .iter()
                .any(|diag| { diag.diagnostic.code == "PACK_LOCK_SCHEMA_ENUM_EMPTY" })
        );
    }

    #[test]
    fn validate_schema_ir_reports_numeric_bound_inversions() {
        let mut diagnostics = Vec::new();
        let mut has_errors = false;

        validate_schema_ir(
            "demo.component",
            &SchemaIr::Int {
                min: Some(10),
                max: Some(1),
            },
            "config/int",
            &mut diagnostics,
            &mut has_errors,
        );
        validate_schema_ir(
            "demo.component",
            &SchemaIr::Float {
                min: Some(4.0),
                max: Some(2.0),
            },
            "config/float",
            &mut diagnostics,
            &mut has_errors,
        );

        assert!(has_errors);
        assert!(
            diagnostics
                .iter()
                .any(|diag| { diag.diagnostic.code == "PACK_LOCK_SCHEMA_INT_BOUNDS" })
        );
        assert!(
            diagnostics
                .iter()
                .any(|diag| { diag.diagnostic.code == "PACK_LOCK_SCHEMA_FLOAT_BOUNDS" })
        );
    }
}
