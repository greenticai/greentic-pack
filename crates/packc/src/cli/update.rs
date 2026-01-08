#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use greentic_flow::compile_ygtc_str;
use greentic_types::Flow;
use tracing::info;

use crate::cli::components::sync_components;
use crate::config::{FlowConfig, PackConfig};
use crate::path_safety::normalize_under_root;

#[derive(Debug, Parser)]
pub struct UpdateArgs {
    /// Root directory of the pack (must contain pack.yaml)
    #[arg(long = "in", value_name = "DIR")]
    pub input: PathBuf,

    /// Enforce that all flow nodes have resolve mappings; otherwise error.
    #[arg(long = "strict", default_value_t = false)]
    pub strict: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FlowUpdateStats {
    added: usize,
    removed: usize,
    total: usize,
}

#[derive(Debug)]
pub struct UpdateResult {
    pub pack_dir: PathBuf,
    pub config: PackConfig,
    pub component_stats: crate::cli::components::ComponentUpdateStats,
    pub flow_stats: FlowUpdateStats,
}

pub fn handle(args: UpdateArgs, json: bool) -> Result<()> {
    let result = update_pack(&args.input, args.strict)?;
    let pack_dir = result.pack_dir;
    let component_stats = result.component_stats;
    let flow_stats = result.flow_stats;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "ok",
                "pack_dir": pack_dir,
                "components": {
                    "added": component_stats.added,
                    "removed": component_stats.removed,
                    "total": component_stats.total,
                },
                "flows": {
                    "added": flow_stats.added,
                    "removed": flow_stats.removed,
                    "total": flow_stats.total,
                },
            }))?
        );
    } else {
        info!(
            components_added = component_stats.added,
            components_removed = component_stats.removed,
            components_total = component_stats.total,
            flows_added = flow_stats.added,
            flows_removed = flow_stats.removed,
            flows_total = flow_stats.total,
            "updated pack manifest"
        );
        println!(
            "pack.yaml updated (components: +{}, -{}, total {}; flows: +{}, -{}, total {})",
            component_stats.added,
            component_stats.removed,
            component_stats.total,
            flow_stats.added,
            flow_stats.removed,
            flow_stats.total
        );
    }

    Ok(())
}
pub fn update_pack(input: &Path, strict: bool) -> Result<UpdateResult> {
    let pack_dir = normalize(input.to_path_buf());
    let pack_yaml = normalize_under_root(&pack_dir, Path::new("pack.yaml"))?;
    let components_dir = normalize_under_root(&pack_dir, Path::new("components"))?;
    let flows_dir = normalize_under_root(&pack_dir, Path::new("flows"))?;

    fs::create_dir_all(&components_dir)?;
    fs::create_dir_all(&flows_dir)?;

    let mut config: PackConfig = serde_yaml_bw::from_str(
        &fs::read_to_string(&pack_yaml)
            .with_context(|| format!("failed to read {}", pack_yaml.display()))?,
    )
    .with_context(|| format!("{} is not a valid pack.yaml", pack_yaml.display()))?;

    let component_stats = sync_components(&mut config, &components_dir)?;
    let flow_stats = sync_flows(&mut config, &flows_dir, &pack_dir, strict)?;

    let serialized = serde_yaml_bw::to_string(&config)?;
    fs::write(&pack_yaml, serialized)?;

    Ok(UpdateResult {
        pack_dir,
        config,
        component_stats,
        flow_stats,
    })
}

fn sync_flows(
    config: &mut PackConfig,
    flows_dir: &Path,
    pack_dir: &Path,
    strict: bool,
) -> Result<FlowUpdateStats> {
    let discovered = discover_flows(flows_dir)?;
    let initial_flows = config.flows.len();
    let mut preserved = 0usize;
    let mut added = 0usize;

    let (mut existing_by_id, existing_by_path) = index_flows(std::mem::take(&mut config.flows));
    let mut updated = Vec::new();

    for file_name in discovered {
        let rel_path = PathBuf::from("flows").join(&file_name);
        let path_key = path_key(&rel_path);
        let file_path = flows_dir.join(&file_name);

        let flow = compile_flow(&file_path)?;
        let flow_id = flow.id.to_string();
        let entrypoints = flow_entrypoints(&flow);

        let mut cfg = if let Some(existing) = existing_by_path
            .get(&path_key)
            .and_then(|id| existing_by_id.remove(id))
        {
            preserved += 1;
            existing
        } else if let Some(existing) = existing_by_id.remove(&flow_id) {
            preserved += 1;
            existing
        } else {
            added += 1;
            default_flow(flow_id.clone(), rel_path.clone(), entrypoints.clone())
        };

        cfg.id = flow_id;
        cfg.file = rel_path;
        if cfg.entrypoints.is_empty() {
            cfg.entrypoints = entrypoints.clone();
        }
        if cfg.entrypoints.is_empty() {
            cfg.entrypoints = vec!["default".to_string()];
        }

        crate::flow_resolve::ensure_sidecar_exists(pack_dir, &cfg, &flow, strict)?;

        updated.push(cfg);
    }

    updated.sort_by(|a, b| a.id.cmp(&b.id));
    config.flows = updated;

    let removed = initial_flows.saturating_sub(preserved);

    Ok(FlowUpdateStats {
        added,
        removed,
        total: config.flows.len(),
    })
}

fn discover_flows(dir: &Path) -> Result<Vec<std::ffi::OsString>> {
    let mut names = Vec::new();

    if dir.exists() {
        for entry in fs::read_dir(dir)
            .with_context(|| format!("failed to list flows in {}", dir.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            if entry.path().extension() != Some(std::ffi::OsStr::new("ygtc")) {
                continue;
            }
            names.push(entry.file_name());
        }
    }

    names.sort();
    Ok(names)
}

fn index_flows(flows: Vec<FlowConfig>) -> (BTreeMap<String, FlowConfig>, BTreeMap<String, String>) {
    let mut by_id = BTreeMap::new();
    let mut by_path = BTreeMap::new();

    for flow in flows {
        by_path.insert(path_key(&flow.file), flow.id.clone());
        by_id.insert(flow.id.clone(), flow);
    }

    (by_id, by_path)
}

fn path_key(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn flow_entrypoints(flow: &Flow) -> Vec<String> {
    let mut entrypoints: Vec<_> = flow.entrypoints.keys().map(|key| key.to_string()).collect();
    entrypoints.sort();
    entrypoints
}

fn compile_flow(path: &Path) -> Result<Flow> {
    let yaml_src = fs::read_to_string(path)
        .with_context(|| format!("failed to read flow {}", path.display()))?;
    compile_ygtc_str(&yaml_src)
        .with_context(|| format!("failed to compile flow {}", path.display()))
}

fn default_flow(id: String, file: PathBuf, entrypoints: Vec<String>) -> FlowConfig {
    FlowConfig {
        id,
        file,
        tags: vec!["default".to_string()],
        entrypoints: if entrypoints.is_empty() {
            vec!["default".to_string()]
        } else {
            entrypoints
        },
    }
}

fn normalize(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}
