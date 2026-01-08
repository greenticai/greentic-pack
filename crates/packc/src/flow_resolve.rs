#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use greentic_types::Flow;
use greentic_types::error::ErrorCode;
use greentic_types::flow_resolve::{
    FlowResolveV1, read_flow_resolve, sidecar_path_for_flow, write_flow_resolve,
};

use crate::config::FlowConfig;

#[derive(Clone, Debug)]
pub struct FlowResolveSidecar {
    pub flow_id: String,
    pub flow_path: PathBuf,
    pub sidecar_path: PathBuf,
    pub document: Option<FlowResolveV1>,
    pub warning: Option<String>,
}

/// Discover flow resolve sidecars for the configured flows.
///
/// Missing or unreadable sidecars produce warnings but do not fail.
pub fn discover_flow_resolves(pack_dir: &Path, flows: &[FlowConfig]) -> Vec<FlowResolveSidecar> {
    flows
        .iter()
        .map(|flow| {
            let flow_path = if flow.file.is_absolute() {
                flow.file.clone()
            } else {
                pack_dir.join(&flow.file)
            };
            let sidecar_path = sidecar_path_for_flow(&flow_path);

            let (document, warning) = match read_flow_resolve(&sidecar_path) {
                Ok(doc) => (Some(doc), None),
                Err(err) if err.code == ErrorCode::NotFound => (
                    None,
                    Some(format!(
                        "flow resolve sidecar missing for {} ({})",
                        flow.id,
                        sidecar_path.display()
                    )),
                ),
                Err(err) => (
                    None,
                    Some(format!(
                        "failed to read flow resolve sidecar for {}: {}",
                        flow.id, err
                    )),
                ),
            };

            FlowResolveSidecar {
                flow_id: flow.id.clone(),
                flow_path,
                sidecar_path,
                document,
                warning,
            }
        })
        .collect()
}

/// Ensure a resolve sidecar exists for a flow and optionally enforce node mappings.
///
/// When the sidecar is missing, an empty document is created. Missing node mappings
/// emit warnings, or become errors when `strict` is set.
pub fn ensure_sidecar_exists(
    pack_dir: &Path,
    flow: &FlowConfig,
    compiled: &Flow,
    strict: bool,
) -> Result<()> {
    let flow_path = if flow.file.is_absolute() {
        flow.file.clone()
    } else {
        pack_dir.join(&flow.file)
    };
    let sidecar_path = sidecar_path_for_flow(&flow_path);

    let doc = match read_flow_resolve(&sidecar_path) {
        Ok(doc) => doc,
        Err(err) if err.code == ErrorCode::NotFound => {
            let doc = FlowResolveV1 {
                schema_version: 1,
                flow: flow.file.to_string_lossy().into_owned(),
                nodes: BTreeMap::new(),
            };
            if let Some(parent) = sidecar_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            write_flow_resolve(&sidecar_path, &doc)
                .with_context(|| format!("failed to write {}", sidecar_path.display()))?;
            doc
        }
        Err(err) => {
            return Err(anyhow!(
                "failed to read flow resolve sidecar for {}: {}",
                flow.id,
                err
            ));
        }
    };

    let missing = missing_node_mappings(compiled, &doc);
    if !missing.is_empty() {
        if strict {
            anyhow::bail!(
                "flow {} is missing resolve entries for nodes {} (sidecar {}). Add mappings to the sidecar, then rerun `greentic-pack resolve` followed by `greentic-pack build`.",
                flow.id,
                missing.join(", "),
                sidecar_path.display()
            );
        } else {
            eprintln!(
                "warning: flow {} has no resolve entries for nodes {} ({}); add mappings to the sidecar and rerun `greentic-pack resolve`",
                flow.id,
                missing.join(", "),
                sidecar_path.display()
            );
        }
    }

    Ok(())
}

/// Require that a resolve sidecar exists and covers every node in the compiled flow.
pub fn enforce_sidecar_mappings(pack_dir: &Path, flow: &FlowConfig, compiled: &Flow) -> Result<()> {
    let flow_path = if flow.file.is_absolute() {
        flow.file.clone()
    } else {
        pack_dir.join(&flow.file)
    };
    let sidecar_path = sidecar_path_for_flow(&flow_path);
    let doc = read_flow_resolve(&sidecar_path).map_err(|err| {
        anyhow!(
            "flow {} requires a resolve sidecar; expected {}: {}",
            flow.id,
            sidecar_path.display(),
            err
        )
    })?;

    let missing = missing_node_mappings(compiled, &doc);
    if !missing.is_empty() {
        anyhow::bail!(
            "flow {} is missing resolve entries for nodes {} (sidecar {}). Add mappings to the sidecar, then rerun `greentic-pack resolve` followed by `greentic-pack build`.",
            flow.id,
            missing.join(", "),
            sidecar_path.display()
        );
    }

    Ok(())
}

/// Compute which nodes in a flow lack resolve entries.
pub fn missing_node_mappings(flow: &Flow, doc: &FlowResolveV1) -> Vec<String> {
    flow.nodes
        .keys()
        .filter_map(|node| {
            let id = node.to_string();
            if doc.nodes.contains_key(id.as_str()) {
                None
            } else {
                Some(id)
            }
        })
        .collect()
}
