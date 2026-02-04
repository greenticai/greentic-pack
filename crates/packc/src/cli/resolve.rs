#![forbid(unsafe_code)]

use std::collections::{BTreeMap, btree_map::Entry};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::Args;
use greentic_pack::pack_lock::{LockedComponent, PackLockV1, write_pack_lock};
use greentic_types::flow_resolve_summary::{FlowResolveSummarySourceRefV1, FlowResolveSummaryV1};

use crate::config::load_pack_config;
use crate::flow_resolve::{read_flow_resolve_summary_for_flow, strip_file_uri_prefix};
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
    let mut seen: BTreeMap<String, LockedComponent> = BTreeMap::new();

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
        let component_id = resolve.component_id.clone();
        let key = component_id.as_str().to_string();
        let name_for_insert = name.clone();
        let reference_for_insert = reference.clone();
        let digest_for_insert = digest.clone();

        match seen.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(LockedComponent {
                    name: name_for_insert,
                    r#ref: reference_for_insert,
                    digest: digest_for_insert,
                    component_id: Some(component_id.clone()),
                    bundled: false,
                    bundled_path: None,
                    wasm_sha256: None,
                    resolved_digest: None,
                });
            }
            Entry::Occupied(entry) => {
                let existing = entry.get();
                if existing.r#ref != reference || existing.digest != digest {
                    bail!(
                        "component {} resolved by nodes {} and {} points to different artifacts ({}@{} vs {}@{})",
                        component_id.as_str(),
                        existing.name,
                        name,
                        existing.r#ref,
                        existing.digest,
                        reference,
                        digest
                    );
                }
            }
        }
    }

    out.extend(seen.into_values());

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
                    digest: "sha256:abcd".to_string(),
                    manifest: None,
                },
            );
        }

        let summary = FlowResolveSummaryV1 {
            schema_version: 1,
            flow: "main.ygtc".to_string(),
            nodes,
        };

        let mut entries = Vec::new();
        collect_from_summary(&pack_dir, &flow, &summary, &mut entries).expect("collect entries");

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.name, "meetingPrep___node_one");
        assert_eq!(
            entry.component_id.as_ref().map(|id| id.as_str()),
            Some(component_id.as_str())
        );
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
                digest: "sha256:abcd".to_string(),
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
                digest: "sha256:dcba".to_string(),
                manifest: None,
            },
        );

        let summary = FlowResolveSummaryV1 {
            schema_version: 1,
            flow: "main.ygtc".to_string(),
            nodes,
        };

        let mut entries = Vec::new();
        let err = collect_from_summary(&pack_dir, &flow, &summary, &mut entries).unwrap_err();
        assert!(err.to_string().contains("points to different artifacts"));
    }
}
