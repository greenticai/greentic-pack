#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use greentic_types::{ComponentCapabilities, ComponentProfiles};
use tracing::info;

use crate::config::{ComponentConfig, FlowKindLabel, PackConfig};
use crate::path_safety::normalize_under_root;

#[derive(Debug, Parser)]
pub struct ComponentsArgs {
    /// Root directory of the pack (must contain pack.yaml)
    #[arg(long = "in", value_name = "DIR")]
    pub input: PathBuf,
}

pub fn handle(args: ComponentsArgs, json: bool) -> Result<()> {
    let pack_dir = normalize(args.input);
    let pack_yaml = normalize_under_root(&pack_dir, Path::new("pack.yaml"))?;
    let components_dir = normalize_under_root(&pack_dir, Path::new("components"))?;

    fs::create_dir_all(&components_dir)?;

    let mut config: PackConfig = serde_yaml_bw::from_str(
        &fs::read_to_string(&pack_yaml)
            .with_context(|| format!("failed to read {}", pack_yaml.display()))?,
    )
    .with_context(|| format!("{} is not a valid pack.yaml", pack_yaml.display()))?;

    let discovered = discover_components(&components_dir)?;
    let initial_components = config.components.len();
    let mut preserved = 0usize;
    let mut added = 0usize;

    let (mut existing_by_id, existing_by_path) = index_components(config.components);
    let mut updated = Vec::new();

    for file_name in discovered {
        let rel_path = PathBuf::from("components").join(&file_name);
        let path_key = path_key(&rel_path);
        let stem = Path::new(&file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                anyhow!(
                    "invalid component filename: {}",
                    Path::new(&file_name).display()
                )
            })?;

        let chosen_id = existing_by_path
            .get(&path_key)
            .cloned()
            .unwrap_or_else(|| stem.to_string());

        let mut component = if let Some(existing) = existing_by_id.remove(&chosen_id) {
            preserved += 1;
            existing
        } else {
            added += 1;
            default_component(chosen_id.clone(), rel_path.clone())
        };

        component.wasm = rel_path;
        updated.push(component);
    }

    updated.sort_by(|a, b| a.id.cmp(&b.id));
    config.components = updated;

    let removed = initial_components.saturating_sub(preserved);
    let serialized = serde_yaml_bw::to_string(&config)?;
    fs::write(&pack_yaml, serialized)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "ok",
                "pack_dir": pack_dir,
                "components": {
                    "added": added,
                    "removed": removed,
                    "total": config.components.len(),
                }
            }))?
        );
    } else {
        info!(
            added,
            removed,
            total = config.components.len(),
            "updated pack components"
        );
        println!(
            "components updated (added: {added}, removed: {removed}, total: {})",
            config.components.len()
        );
    }

    Ok(())
}

fn discover_components(dir: &Path) -> Result<Vec<std::ffi::OsString>> {
    let mut names = Vec::new();

    if dir.exists() {
        for entry in fs::read_dir(dir)
            .with_context(|| format!("failed to list components in {}", dir.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            if entry.path().extension() != Some(std::ffi::OsStr::new("wasm")) {
                continue;
            }
            names.push(entry.file_name());
        }
    }

    names.sort();
    Ok(names)
}

fn index_components(
    components: Vec<ComponentConfig>,
) -> (BTreeMap<String, ComponentConfig>, BTreeMap<String, String>) {
    let mut by_id = BTreeMap::new();
    let mut by_path = BTreeMap::new();

    for component in components {
        by_path.insert(path_key(&component.wasm), component.id.clone());
        by_id.insert(component.id.clone(), component);
    }

    (by_id, by_path)
}

fn path_key(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn default_component(id: String, wasm: PathBuf) -> ComponentConfig {
    ComponentConfig {
        id,
        version: "0.1.0".to_string(),
        world: "greentic:component/stub".to_string(),
        supports: vec![FlowKindLabel::Messaging],
        profiles: ComponentProfiles {
            default: Some("default".to_string()),
            supported: vec!["default".to_string()],
        },
        capabilities: ComponentCapabilities::default(),
        wasm,
        operations: Vec::new(),
        config_schema: None,
        resources: None,
        configurators: None,
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
