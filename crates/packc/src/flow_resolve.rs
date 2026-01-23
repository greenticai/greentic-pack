#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use greentic_flow::resolve_summary::write_flow_resolve_summary_for_flow;
use greentic_types::ComponentId;
use greentic_types::Flow;
use greentic_types::error::ErrorCode;
use greentic_types::flow_resolve::{
    ComponentSourceRefV1, FlowResolveV1, read_flow_resolve, sidecar_path_for_flow,
    write_flow_resolve,
};
use greentic_types::flow_resolve_summary::{
    FLOW_RESOLVE_SUMMARY_SCHEMA_VERSION, FlowResolveSummaryManifestV1,
    FlowResolveSummarySourceRefV1, FlowResolveSummaryV1, NodeResolveSummaryV1,
    read_flow_resolve_summary, resolve_summary_path_for_flow, write_flow_resolve_summary,
};
use semver::Version;
use sha2::{Digest, Sha256};

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

/// Ensure a flow resolve summary exists, generating it from the resolve sidecar if missing.
pub fn load_flow_resolve_summary(
    pack_dir: &Path,
    flow: &FlowConfig,
    compiled: &Flow,
) -> Result<FlowResolveSummaryV1> {
    let flow_path = resolve_flow_path(pack_dir, flow);
    let summary = read_or_write_flow_resolve_summary(&flow_path, flow)?;
    enforce_summary_mappings(flow, compiled, &summary, &flow_path)?;
    Ok(summary)
}

/// Read or generate a flow resolve summary for a flow (no node enforcement).
pub fn read_flow_resolve_summary_for_flow(
    pack_dir: &Path,
    flow: &FlowConfig,
) -> Result<FlowResolveSummaryV1> {
    let flow_path = resolve_flow_path(pack_dir, flow);
    read_or_write_flow_resolve_summary(&flow_path, flow)
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
    let flow_path = resolve_flow_path(pack_dir, flow);
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

fn resolve_flow_path(pack_dir: &Path, flow: &FlowConfig) -> PathBuf {
    if flow.file.is_absolute() {
        flow.file.clone()
    } else {
        pack_dir.join(&flow.file)
    }
}

fn read_or_write_flow_resolve_summary(
    flow_path: &Path,
    flow: &FlowConfig,
) -> Result<FlowResolveSummaryV1> {
    let summary_path = resolve_summary_path_for_flow(flow_path);
    if !summary_path.exists() {
        let sidecar_path = sidecar_path_for_flow(flow_path);
        let sidecar = read_flow_resolve(&sidecar_path).map_err(|err| {
            anyhow!(
                "flow {} requires a resolve sidecar to generate summary; expected {}: {}",
                flow.id,
                sidecar_path.display(),
                err
            )
        })?;
        write_flow_resolve_summary_safe(flow_path, &sidecar).with_context(|| {
            format!(
                "failed to generate flow resolve summary for {}",
                flow_path.display()
            )
        })?;
    }

    read_flow_resolve_summary(&summary_path).map_err(|err| {
        anyhow!(
            "failed to read flow resolve summary for {}: {}",
            flow.id,
            err
        )
    })
}

fn write_flow_resolve_summary_safe(flow_path: &Path, sidecar: &FlowResolveV1) -> Result<PathBuf> {
    let result = if tokio::runtime::Handle::try_current().is_ok() {
        let flow_path = flow_path.to_path_buf();
        let sidecar = sidecar.clone();
        let join =
            std::thread::spawn(move || write_flow_resolve_summary_for_flow(&flow_path, &sidecar));
        join.join()
            .map_err(|_| anyhow!("flow resolve summary generation panicked"))?
    } else {
        write_flow_resolve_summary_for_flow(flow_path, sidecar)
    };

    match result {
        Ok(path) => Ok(path),
        Err(err) => {
            if sidecar
                .nodes
                .values()
                .all(|node| matches!(node.source, ComponentSourceRefV1::Local { .. }))
            {
                let summary = build_flow_resolve_summary_fallback(flow_path, sidecar)?;
                let summary_path = resolve_summary_path_for_flow(flow_path);
                write_flow_resolve_summary(&summary_path, &summary)
                    .map_err(|e| anyhow!(e.to_string()))?;
                return Ok(summary_path);
            }
            Err(err)
        }
    }
}

fn enforce_summary_mappings(
    flow: &FlowConfig,
    compiled: &Flow,
    summary: &FlowResolveSummaryV1,
    flow_path: &Path,
) -> Result<()> {
    let missing = missing_summary_node_mappings(compiled, summary);
    if !missing.is_empty() {
        let summary_path = resolve_summary_path_for_flow(flow_path);
        anyhow::bail!(
            "flow {} is missing resolve summary entries for nodes {} (summary {}). Regenerate the summary and rerun build.",
            flow.id,
            missing.join(", "),
            summary_path.display()
        );
    }
    Ok(())
}

fn missing_summary_node_mappings(flow: &Flow, doc: &FlowResolveSummaryV1) -> Vec<String> {
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

fn build_flow_resolve_summary_fallback(
    flow_path: &Path,
    sidecar: &FlowResolveV1,
) -> Result<FlowResolveSummaryV1> {
    let mut nodes = BTreeMap::new();
    for (node_id, entry) in &sidecar.nodes {
        let summary = summarize_node_fallback(flow_path, node_id, &entry.source)?;
        nodes.insert(node_id.clone(), summary);
    }
    Ok(FlowResolveSummaryV1 {
        schema_version: FLOW_RESOLVE_SUMMARY_SCHEMA_VERSION,
        flow: flow_name_from_path(flow_path),
        nodes,
    })
}

fn summarize_node_fallback(
    flow_path: &Path,
    node_id: &str,
    source: &ComponentSourceRefV1,
) -> Result<NodeResolveSummaryV1> {
    let ComponentSourceRefV1::Local { path, .. } = source else {
        anyhow::bail!(
            "flow resolve fallback only supports local sources (node {})",
            node_id
        );
    };
    let source_ref = FlowResolveSummarySourceRefV1::Local {
        path: strip_file_uri_prefix(path).to_string(),
    };
    let wasm_path = local_path_from_sidecar(path, flow_path);
    let digest = compute_sha256(&wasm_path)?;
    let manifest_path = find_manifest_for_wasm_loose(&wasm_path).with_context(|| {
        format!(
            "component.manifest.json not found for node '{}' ({})",
            node_id,
            wasm_path.display()
        )
    })?;
    let (component_id, manifest) = read_manifest_metadata(&manifest_path).with_context(|| {
        format!(
            "failed to read component.manifest.json for node '{}' ({})",
            node_id,
            manifest_path.display()
        )
    })?;

    Ok(NodeResolveSummaryV1 {
        component_id,
        source: source_ref,
        digest,
        manifest,
    })
}

fn find_manifest_for_wasm_loose(wasm_path: &Path) -> Result<PathBuf> {
    let wasm_abs = fs::canonicalize(wasm_path)
        .with_context(|| format!("resolve wasm path {}", wasm_path.display()))?;
    let mut current = wasm_abs.parent();
    let mut fallback = None;
    while let Some(dir) = current {
        let candidate = dir.join("component.manifest.json");
        if candidate.exists() {
            if manifest_matches_wasm_loose(&candidate, &wasm_abs)? {
                return Ok(candidate);
            }
            if fallback.is_none() {
                fallback = Some(candidate);
            }
        }
        current = dir.parent();
    }

    if let Some(candidate) = fallback {
        return Ok(candidate);
    }

    anyhow::bail!(
        "component.manifest.json not found for wasm {}",
        wasm_abs.display()
    );
}

fn manifest_matches_wasm_loose(manifest_path: &Path, wasm_abs: &Path) -> Result<bool> {
    let raw = fs::read_to_string(manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&raw).context("parse component.manifest.json")?;
    let Some(rel) = json
        .get("artifacts")
        .and_then(|v| v.get("component_wasm"))
        .and_then(|v| v.as_str())
    else {
        return Ok(false);
    };
    let manifest_dir = manifest_path
        .parent()
        .ok_or_else(|| anyhow!("manifest path {} has no parent", manifest_path.display()))?;
    let sanitized = strip_file_uri_prefix(rel);
    let Ok(abs) = fs::canonicalize(manifest_dir.join(sanitized)) else {
        return Ok(false);
    };
    Ok(abs == *wasm_abs)
}

fn read_manifest_metadata(
    manifest_path: &Path,
) -> Result<(ComponentId, Option<FlowResolveSummaryManifestV1>)> {
    let raw = fs::read_to_string(manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&raw).context("parse component.manifest.json")?;
    let id = json
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("manifest missing id"))?;
    let component_id =
        ComponentId::new(id).with_context(|| format!("invalid component id {}", id))?;
    let world = json.get("world").and_then(|v| v.as_str());
    let version = json.get("version").and_then(|v| v.as_str());
    let manifest = match (world, version) {
        (Some(world), Some(version)) => {
            let parsed = Version::parse(version)
                .with_context(|| format!("invalid semver version {}", version))?;
            Some(FlowResolveSummaryManifestV1 {
                world: world.to_string(),
                version: parsed,
            })
        }
        _ => None,
    };
    Ok((component_id, manifest))
}

fn flow_name_from_path(flow_path: &Path) -> String {
    flow_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "flow.ygtc".to_string())
}

pub(crate) fn strip_file_uri_prefix(path: &str) -> &str {
    path.strip_prefix("file://")
        .or_else(|| path.strip_prefix("file:/"))
        .or_else(|| path.strip_prefix("file:"))
        .unwrap_or(path)
}

fn local_path_from_sidecar(path: &str, flow_path: &Path) -> PathBuf {
    let trimmed = strip_file_uri_prefix(path);
    let raw = PathBuf::from(trimmed);
    if raw.is_absolute() {
        raw
    } else {
        flow_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(raw)
    }
}

fn compute_sha256(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read wasm at {}", path.display()))?;
    let mut sha = Sha256::new();
    sha.update(bytes);
    Ok(format!("sha256:{:x}", sha.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn strip_file_uri_prefix_removes_scheme_variants() {
        assert_eq!(strip_file_uri_prefix("file:///tmp/foo"), "/tmp/foo");
        assert_eq!(strip_file_uri_prefix("file:/tmp/foo"), "tmp/foo");
        assert_eq!(strip_file_uri_prefix("file://bar/baz"), "bar/baz");
        assert_eq!(strip_file_uri_prefix("file:relative/path"), "relative/path");
        assert_eq!(
            strip_file_uri_prefix("../components/foo"),
            "../components/foo"
        );
    }

    #[test]
    fn manifest_matches_wasm_loose_handles_relative_file_uri_paths() {
        let temp = tempdir().expect("alloc temp dir");
        let components = temp.path().join("components");
        fs::create_dir_all(&components).expect("create components dir");
        let wasm_path = components.join("component.wasm");
        fs::write(&wasm_path, b"wasm-bytes").expect("write wasm");
        let manifest_path = components.join("component.manifest.json");
        let manifest = json!({
            "artifacts": {
                "component_wasm": "file://component.wasm"
            }
        });
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("encode manifest"),
        )
        .expect("write manifest");
        let wasm_abs = fs::canonicalize(&wasm_path).expect("canonicalize wasm");
        assert!(manifest_matches_wasm_loose(&manifest_path, &wasm_abs).expect("manifest lookup"));

        let parent_manifest = json!({
            "artifacts": {
                "component_wasm": "file://../component.wasm"
            }
        });
        let parent_dir = components.join("child");
        fs::create_dir_all(&parent_dir).expect("create child dir");
        let child_manifest_path = parent_dir.join("component.manifest.json");
        fs::write(
            &child_manifest_path,
            serde_json::to_vec_pretty(&parent_manifest).unwrap(),
        )
        .expect("write child manifest");
        assert!(
            manifest_matches_wasm_loose(&child_manifest_path, &wasm_abs)
                .expect("manifest matches child")
        );
    }

    #[test]
    fn manifest_matches_wasm_loose_handles_absolute_file_uri_paths() {
        let temp = tempdir().expect("alloc temp dir");
        let components = temp.path().join("components");
        fs::create_dir_all(&components).expect("create components dir");
        let wasm_path = components.join("component.wasm");
        fs::write(&wasm_path, b"bytes").expect("write wasm");
        let wasm_abs = fs::canonicalize(&wasm_path).expect("canonicalize wasm");
        let manifest_path = components.join("component.manifest.json");
        let manifest = json!({
            "artifacts": {
                "component_wasm": format!("file://{}", wasm_abs.display())
            }
        });
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("encode manifest"),
        )
        .expect("write manifest");
        assert!(manifest_matches_wasm_loose(&manifest_path, &wasm_abs).expect("manifest lookup"));
    }
}
