#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::Args;
use greentic_distributor_client::{DistClient, DistOptions};
use greentic_pack::pack_lock::{LockedComponent, PackLockV1, write_pack_lock};
use greentic_types::flow_resolve::{ComponentSourceRefV1, FlowResolveV1};
use sha2::{Digest, Sha256};

use crate::config::load_pack_config;
use crate::flow_resolve::discover_flow_resolves;
use crate::runtime::{NetworkPolicy, RuntimeContext};

#[derive(Debug, Args)]
pub struct ResolveArgs {
    /// Pack root directory containing pack.yaml.
    #[arg(long = "in", value_name = "DIR", default_value = ".")]
    pub input: PathBuf,

    /// Output path for pack.lock.json (default: pack.lock.json under pack root).
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
    let resolves = discover_flow_resolves(&pack_dir, &config.flows);

    for entry in &resolves {
        if let Some(msg) = &entry.warning {
            eprintln!("warning: {msg}");
        }
    }

    let mut entries: Vec<LockedComponent> = Vec::new();
    let dist = new_dist_client(runtime);

    for sidecar in resolves {
        let Some(doc) = sidecar.document else {
            continue;
        };
        collect_from_sidecar(
            &pack_dir,
            &sidecar.flow_id,
            &doc,
            &dist,
            runtime,
            &mut entries,
        )
        .await?;
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
        None => pack_dir.join("pack.lock.json"),
    }
}

fn new_dist_client(runtime: &RuntimeContext) -> DistClient {
    let opts = DistOptions {
        cache_dir: runtime.cache_dir(),
        allow_tags: true,
        offline: runtime.network_policy() == NetworkPolicy::Offline,
    };
    DistClient::new(opts)
}

async fn collect_from_sidecar(
    pack_dir: &Path,
    flow_id: &str,
    doc: &FlowResolveV1,
    dist: &DistClient,
    runtime: &RuntimeContext,
    out: &mut Vec<LockedComponent>,
) -> Result<()> {
    let mut seen: BTreeMap<String, (String, String)> = BTreeMap::new();

    for (node, resolve) in &doc.nodes {
        let name = format!("{flow_id}::{node}");
        let source_ref = &resolve.source;
        let (reference, digest) = match source_ref {
            ComponentSourceRefV1::Local { path, digest } => {
                let abs = normalize_local(pack_dir, &doc.flow, path)?;
                let digest = match digest {
                    Some(d) => d.clone(),
                    None => compute_sha256(&abs)?,
                };
                (path.clone(), digest)
            }
            ComponentSourceRefV1::Oci { r#ref, digest }
            | ComponentSourceRefV1::Repo { r#ref, digest }
            | ComponentSourceRefV1::Store { r#ref, digest, .. } => {
                if let Some(d) = digest {
                    (format_reference(source_ref), d.clone())
                } else {
                    runtime.require_online("resolve component refs")?;
                    let resolved = dist
                        .resolve_ref(&format_reference(source_ref))
                        .await
                        .map_err(|err| anyhow!("resolve {} failed: {}", r#ref, err))?;
                    (format_reference(source_ref), resolved.digest)
                }
            }
        };
        seen.insert(name, (reference, digest));
    }

    for (name, (reference, digest)) in seen {
        out.push(LockedComponent {
            name,
            r#ref: reference,
            digest,
        });
    }

    Ok(())
}

fn normalize_local(pack_dir: &Path, flow_file: &str, rel: &str) -> Result<PathBuf> {
    let flow_path = pack_dir.join(flow_file);
    let parent = flow_path
        .parent()
        .ok_or_else(|| anyhow!("flow path {} has no parent", flow_file))?;
    Ok(parent.join(rel))
}

fn compute_sha256(path: &Path) -> Result<String> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read local component {}", path.display()))?;
    let mut sha = Sha256::new();
    sha.update(&bytes);
    Ok(format!("sha256:{:x}", sha.finalize()))
}

fn format_reference(source: &ComponentSourceRefV1) -> String {
    match source {
        ComponentSourceRefV1::Local { path, .. } => path.clone(),
        ComponentSourceRefV1::Oci { r#ref, .. } => {
            if r#ref.contains("://") {
                r#ref.clone()
            } else {
                format!("oci://{}", r#ref)
            }
        }
        ComponentSourceRefV1::Repo { r#ref, .. } => {
            if r#ref.contains("://") {
                r#ref.clone()
            } else {
                format!("repo://{}", r#ref)
            }
        }
        ComponentSourceRefV1::Store { r#ref, .. } => {
            if r#ref.contains("://") {
                r#ref.clone()
            } else {
                format!("store://{}", r#ref)
            }
        }
    }
}
