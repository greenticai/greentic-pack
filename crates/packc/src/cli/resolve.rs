#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::Args;
use greentic_pack::pack_lock::{LockedComponent, PackLockV1, write_pack_lock};
use greentic_types::ComponentId;
use greentic_types::flow_resolve_summary::{FlowResolveSummarySourceRefV1, FlowResolveSummaryV1};

use crate::config::load_pack_config;
use crate::flow_resolve::read_flow_resolve_summary_for_flow;
use crate::runtime::RuntimeContext;

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
    let mut entries: Vec<LockedComponent> = Vec::new();
    let _ = runtime;
    for flow in &config.flows {
        let summary = read_flow_resolve_summary_for_flow(&pack_dir, flow)?;
        collect_from_summary(&pack_dir, flow, &summary, &mut entries)?;
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

fn collect_from_summary(
    pack_dir: &Path,
    flow: &crate::config::FlowConfig,
    doc: &FlowResolveSummaryV1,
    out: &mut Vec<LockedComponent>,
) -> Result<()> {
    let mut seen: BTreeMap<String, (String, String, ComponentId)> = BTreeMap::new();

    for (node, resolve) in &doc.nodes {
        let name = format!("{}___{node}", flow.id);
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
        seen.insert(name, (reference, digest, resolve.component_id.clone()));
    }

    for (name, (reference, digest, component_id)) in seen {
        out.push(LockedComponent {
            name,
            r#ref: reference,
            digest,
            component_id: Some(component_id),
        });
    }

    Ok(())
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
    let rel = rel.strip_prefix("file://").unwrap_or(rel);
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
