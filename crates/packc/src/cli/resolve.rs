#![forbid(unsafe_code)]

use std::collections::{BTreeMap, btree_map::Entry};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::Args;
use greentic_distributor_client::{DistClient, DistOptions};
use greentic_pack::pack_lock::{LockedComponent, PackLockV1, write_pack_lock};
use greentic_pack::resolver::{ComponentResolver, ResolveReq, ResolvedComponent};
use greentic_types::cbor::canonical;
use greentic_types::flow_resolve_summary::{FlowResolveSummarySourceRefV1, FlowResolveSummaryV1};
use greentic_types::schemas::component::v0_6_0::{ComponentDescribe, schema_hash};
use hex;
use sha2::{Digest, Sha256};
use tokio::runtime::Handle;
use wasmtime::Engine;
use wasmtime::component::{Component as WasmtimeComponent, Linker};

use crate::component_host_stubs::{DescribeHostState, add_describe_host_imports};
use crate::config::load_pack_config;
use crate::flow_resolve::{read_flow_resolve_summary_for_flow, strip_file_uri_prefix};
use crate::runtime::RuntimeContext;

#[derive(Debug, Args)]
pub struct ResolveArgs {
    /// Pack root directory containing pack.yaml.
    #[arg(long = "in", value_name = "DIR", default_value = ".")]
    pub input: PathBuf,

    /// Output path for pack.lock.cbor (default: pack.lock.cbor under pack root).
    #[arg(long = "lock", value_name = "FILE")]
    pub lock: Option<PathBuf>,
}

pub async fn handle(args: ResolveArgs, runtime: &RuntimeContext, emit_path: bool) -> Result<()> {
    let pack_dir = args
        .input
        .canonicalize()
        .with_context(|| format!("failed to resolve pack dir {}", args.input.display()))?;
    let lock_path = resolve_lock_path(&pack_dir, args.lock.as_deref());

    let config = load_pack_config(&pack_dir)?;
    let mut entries: BTreeMap<String, LockedComponent> = BTreeMap::new();
    for flow in &config.flows {
        let summary = read_flow_resolve_summary_for_flow(&pack_dir, flow)?;
        collect_from_summary(&pack_dir, flow, &summary, &mut entries)?;
    }

    if !entries.is_empty() {
        let resolver = PackResolver::new(runtime)?;
        let engine = Engine::default();
        for component in entries.values_mut() {
            populate_component_contract(&engine, &resolver, component).await?;
        }
    }

    let lock = PackLockV1::new(entries);
    write_pack_lock(&lock_path, &lock)?;
    if emit_path {
        eprintln!("wrote {}", lock_path.display());
    }

    Ok(())
}

fn resolve_lock_path(pack_dir: &Path, override_path: Option<&Path>) -> PathBuf {
    match override_path {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => pack_dir.join(path),
        None => pack_dir.join("pack.lock.cbor"),
    }
}

fn collect_from_summary(
    pack_dir: &Path,
    flow: &crate::config::FlowConfig,
    doc: &FlowResolveSummaryV1,
    out: &mut BTreeMap<String, LockedComponent>,
) -> Result<()> {
    let mut seen: BTreeMap<String, LockedComponent> = BTreeMap::new();

    for resolve in doc.nodes.values() {
        let source_ref = &resolve.source;
        let (reference, digest) = match source_ref {
            FlowResolveSummarySourceRefV1::Local { path } => {
                let abs = normalize_local(pack_dir, flow, path)?;
                (
                    format!("file://{}", abs.to_string_lossy()),
                    resolve.digest.clone(),
                )
            }
            FlowResolveSummarySourceRefV1::Oci { .. }
            | FlowResolveSummarySourceRefV1::Repo { .. }
            | FlowResolveSummarySourceRefV1::Store { .. } => {
                (format_reference(source_ref), resolve.digest.clone())
            }
        };
        let component_id = resolve.component_id.clone();
        let key = component_id.as_str().to_string();
        let reference_for_insert = reference.clone();
        let digest_for_insert = digest.clone();
        let world = resolve.manifest.as_ref().map(|meta| meta.world.clone());
        let component_version = resolve
            .manifest
            .as_ref()
            .map(|meta| meta.version.to_string());
        match seen.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(LockedComponent {
                    component_id: component_id.as_str().to_string(),
                    r#ref: Some(reference_for_insert),
                    abi_version: "0.6.0".to_string(),
                    resolved_digest: digest_for_insert,
                    describe_hash: String::new(),
                    operations: Vec::new(),
                    world,
                    component_version,
                    role: None,
                });
            }
            Entry::Occupied(entry) => {
                let existing = entry.get();
                if existing.r#ref.as_deref() != Some(reference.as_str())
                    || existing.resolved_digest != digest
                {
                    bail!(
                        "component {} resolved by nodes points to different artifacts ({}@{} vs {}@{})",
                        component_id.as_str(),
                        existing.r#ref.as_deref().unwrap_or("unknown-ref"),
                        existing.resolved_digest,
                        reference,
                        digest
                    );
                }
            }
        }
    }

    out.extend(seen);

    Ok(())
}

async fn populate_component_contract(
    engine: &Engine,
    resolver: &dyn ComponentResolver,
    component: &mut LockedComponent,
) -> Result<()> {
    if is_builtin_component(component.component_id.as_str()) {
        component.describe_hash = "0".repeat(64);
        component.operations.clear();
        component.role = Some("builtin".to_string());
        if component.component_version.is_none() {
            component.component_version = Some("0.0.0".to_string());
        }
        return Ok(());
    }

    let reference = component
        .r#ref
        .as_ref()
        .ok_or_else(|| anyhow!("component {} missing ref", component.component_id))?;
    let resolved = resolver.resolve(ResolveReq {
        component_id: component.component_id.clone(),
        reference: reference.clone(),
        expected_digest: component.resolved_digest.clone(),
        abi_version: component.abi_version.clone(),
        world: component.world.clone(),
        component_version: component.component_version.clone(),
    })?;
    let bytes = resolved.bytes;
    let use_describe_cache =
        std::env::var("GREENTIC_PACK_USE_DESCRIBE_CACHE").is_ok() || cfg!(test);
    let describe = match describe_component(engine, &bytes) {
        Ok(describe) => describe,
        Err(err) => {
            if let Some(describe) = load_describe_from_cache_path(resolved.source_path.as_deref())?
            {
                describe
            } else if use_describe_cache {
                return Err(err).context("describe failed and no describe cache present");
            } else {
                return Err(err);
            }
        }
    };

    if describe.info.id != component.component_id {
        bail!(
            "component {} describe id mismatch: {}",
            component.component_id,
            describe.info.id
        );
    }

    let describe_hash = compute_describe_hash(&describe)?;
    let mut operations: Vec<_> = describe
        .operations
        .iter()
        .map(|op| {
            let hash = schema_hash(&op.input.schema, &op.output.schema, &describe.config_schema)
                .map_err(|err| anyhow!("schema_hash for {}: {}", op.id, err))?;
            Ok((op.id.clone(), hash))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .map(
            |(operation_id, schema_hash)| greentic_pack::pack_lock::LockedOperation {
                operation_id,
                schema_hash,
            },
        )
        .collect();
    operations.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));

    component.describe_hash = describe_hash;
    component.operations = operations;
    component.role = Some(describe.info.role);
    component.component_version = Some(describe.info.version);
    Ok(())
}

fn is_builtin_component(component_id: &str) -> bool {
    matches!(
        component_id,
        "session.wait" | "flow.call" | "provider.invoke"
    ) || component_id.starts_with("emit.")
}

struct PackResolver {
    runtime: RuntimeContext,
    dist: DistClient,
}

impl PackResolver {
    fn new(runtime: &RuntimeContext) -> Result<Self> {
        let dist = DistClient::new(DistOptions {
            cache_dir: runtime.cache_dir(),
            allow_tags: true,
            offline: runtime.network_policy() == crate::runtime::NetworkPolicy::Offline,
            allow_insecure_local_http: false,
            ..DistOptions::default()
        });
        Ok(Self {
            runtime: runtime.clone(),
            dist,
        })
    }
}

impl ComponentResolver for PackResolver {
    fn resolve(&self, req: ResolveReq) -> Result<ResolvedComponent> {
        if req.reference.starts_with("file://") {
            let path = strip_file_uri_prefix(&req.reference);
            let bytes = fs::read(path).with_context(|| format!("read {}", path))?;
            return Ok(ResolvedComponent {
                bytes,
                resolved_digest: req.expected_digest,
                component_id: req.component_id,
                abi_version: req.abi_version,
                world: req.world,
                component_version: req.component_version,
                source_path: Some(PathBuf::from(path)),
            });
        }

        let handle =
            Handle::try_current().context("component resolution requires a Tokio runtime")?;
        let resolved = if self.runtime.network_policy() == crate::runtime::NetworkPolicy::Offline {
            block_on(&handle, self.dist.ensure_cached(&req.expected_digest))
                .map_err(|err| anyhow!("offline cache miss for {}: {}", req.reference, err))?
        } else {
            block_on(&handle, self.dist.resolve_ref(&req.reference))
                .map_err(|err| anyhow!("resolve {}: {}", req.reference, err))?
        };
        let path = resolved
            .cache_path
            .ok_or_else(|| anyhow!("resolved component missing path for {}", req.reference))?;
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        Ok(ResolvedComponent {
            bytes,
            resolved_digest: req.expected_digest,
            component_id: req.component_id,
            abi_version: req.abi_version,
            world: req.world,
            component_version: req.component_version,
            source_path: Some(path),
        })
    }
}

fn block_on<F, T, E>(handle: &Handle, fut: F) -> std::result::Result<T, E>
where
    F: Future<Output = std::result::Result<T, E>>,
{
    tokio::task::block_in_place(|| handle.block_on(fut))
}

fn describe_component(engine: &Engine, bytes: &[u8]) -> Result<ComponentDescribe> {
    describe_component_untyped(engine, bytes)
}

fn describe_component_untyped(engine: &Engine, bytes: &[u8]) -> Result<ComponentDescribe> {
    let component =
        WasmtimeComponent::from_binary(engine, bytes).context("decode component bytes")?;
    let mut store = wasmtime::Store::new(engine, DescribeHostState::default());
    let mut linker = Linker::new(engine);
    add_describe_host_imports(&mut linker)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .context("instantiate component root world")?;

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
        .context("lookup component-descriptor.describe")?;
    let (describe_bytes,) = describe_func
        .call(&mut store, ())
        .context("call component-descriptor.describe")?;
    canonical::from_cbor(&describe_bytes).context("decode ComponentDescribe")
}

fn load_describe_from_cache_path(path: Option<&Path>) -> Result<Option<ComponentDescribe>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let describe_path = PathBuf::from(format!("{}.describe.cbor", path.display()));
    if !describe_path.exists() {
        return Ok(None);
    }
    let bytes =
        fs::read(&describe_path).with_context(|| format!("read {}", describe_path.display()))?;
    canonical::ensure_canonical(&bytes).context("describe cache must be canonical")?;
    let describe = canonical::from_cbor(&bytes).context("decode ComponentDescribe from cache")?;
    Ok(Some(describe))
}

fn compute_describe_hash(describe: &ComponentDescribe) -> Result<String> {
    let bytes =
        canonical::to_canonical_cbor_allow_floats(describe).context("canonicalize describe")?;
    let digest = Sha256::digest(bytes.as_slice());
    Ok(hex::encode(digest))
}

fn normalize_local(
    pack_dir: &Path,
    flow: &crate::config::FlowConfig,
    rel: &str,
) -> Result<PathBuf> {
    let flow_path = if flow.file.is_absolute() {
        flow.file.clone()
    } else {
        pack_dir.join(&flow.file)
    };
    let parent = flow_path
        .parent()
        .ok_or_else(|| anyhow!("flow path {} has no parent", flow_path.display()))?;
    let rel = strip_file_uri_prefix(rel);
    Ok(parent.join(rel))
}

fn format_reference(source: &FlowResolveSummarySourceRefV1) -> String {
    match source {
        FlowResolveSummarySourceRefV1::Local { path } => path.clone(),
        FlowResolveSummarySourceRefV1::Oci { r#ref } => {
            if r#ref.contains("://") {
                r#ref.clone()
            } else {
                format!("oci://{}", r#ref)
            }
        }
        FlowResolveSummarySourceRefV1::Repo { r#ref } => {
            if r#ref.contains("://") {
                r#ref.clone()
            } else {
                format!("repo://{}", r#ref)
            }
        }
        FlowResolveSummarySourceRefV1::Store { r#ref } => {
            if r#ref.contains("://") {
                r#ref.clone()
            } else {
                format!("store://{}", r#ref)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greentic_types::ComponentId;
    use greentic_types::flow_resolve_summary::{
        FlowResolveSummarySourceRefV1, FlowResolveSummaryV1, NodeResolveSummaryV1,
    };
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn sample_flow() -> crate::config::FlowConfig {
        crate::config::FlowConfig {
            id: "meetingPrep".to_string(),
            file: PathBuf::from("flows/main.ygtc"),
            tags: Vec::new(),
            entrypoints: Vec::new(),
        }
    }

    #[test]
    fn collect_from_summary_dedups_duplicate_component_ids() {
        let flow = sample_flow();
        let pack_dir = PathBuf::from("/tmp");
        let component_id =
            ComponentId::new("ai.greentic.component-adaptive-card").expect("valid component id");

        let mut nodes = BTreeMap::new();
        for name in ["node_one", "node_two"] {
            nodes.insert(
                name.to_string(),
                NodeResolveSummaryV1 {
                    component_id: component_id.clone(),
                    source: FlowResolveSummarySourceRefV1::Oci {
                        r#ref:
                            "oci://ghcr.io/greentic-ai/components/component-adaptive-card:latest"
                                .to_string(),
                    },
                    digest: format!("sha256:{}", "a".repeat(64)),
                    manifest: None,
                },
            );
        }

        let summary = FlowResolveSummaryV1 {
            schema_version: 1,
            flow: "main.ygtc".to_string(),
            nodes,
        };

        let mut entries = BTreeMap::new();
        collect_from_summary(&pack_dir, &flow, &summary, &mut entries).expect("collect entries");

        assert_eq!(entries.len(), 1);
        let entry = entries.get(component_id.as_str()).expect("component entry");
        assert_eq!(entry.component_id, component_id.as_str());
    }

    #[test]
    fn collect_from_summary_rejects_conflicting_lock_data() {
        let flow = sample_flow();
        let pack_dir = PathBuf::from("/tmp");
        let component_id =
            ComponentId::new("ai.greentic.component-adaptive-card").expect("valid component id");

        let mut nodes = BTreeMap::new();
        nodes.insert(
            "alpha".to_string(),
            NodeResolveSummaryV1 {
                component_id: component_id.clone(),
                source: FlowResolveSummarySourceRefV1::Oci {
                    r#ref: "oci://ghcr.io/greentic-ai/components/component-adaptive-card:latest"
                        .to_string(),
                },
                digest: format!("sha256:{}", "b".repeat(64)),
                manifest: None,
            },
        );
        nodes.insert(
            "beta".to_string(),
            NodeResolveSummaryV1 {
                component_id: component_id.clone(),
                source: FlowResolveSummarySourceRefV1::Oci {
                    r#ref: "oci://ghcr.io/greentic-ai/components/component-adaptive-card:latest"
                        .to_string(),
                },
                digest: format!("sha256:{}", "c".repeat(64)),
                manifest: None,
            },
        );

        let summary = FlowResolveSummaryV1 {
            schema_version: 1,
            flow: "main.ygtc".to_string(),
            nodes,
        };

        let mut entries = BTreeMap::new();
        let err = collect_from_summary(&pack_dir, &flow, &summary, &mut entries).unwrap_err();
        assert!(err.to_string().contains("points to different artifacts"));
    }
}
