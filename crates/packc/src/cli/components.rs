#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use greentic_types::{ComponentCapabilities, ComponentProfiles};
use tracing::{info, warn};
use wit_component::DecodedWasm;

use crate::config::{ComponentConfig, FlowKindLabel, PackConfig};
use crate::path_safety::normalize_under_root;

#[derive(Debug, Clone, Copy, Default)]
pub struct ComponentUpdateStats {
    pub added: usize,
    pub removed: usize,
    pub total: usize,
}

#[derive(Debug)]
struct DiscoveredComponent {
    rel_wasm_path: PathBuf,
    abs_wasm_path: PathBuf,
    id_hint: String,
}

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

    let stats = sync_components(&mut config, &components_dir)?;

    let serialized = serde_yaml_bw::to_string(&config)?;
    fs::write(&pack_yaml, serialized)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "ok",
                "pack_dir": pack_dir,
                "components": {
                    "added": stats.added,
                    "removed": stats.removed,
                    "total": stats.total,
                }
            }))?
        );
    } else {
        info!(
            added = stats.added,
            removed = stats.removed,
            total = stats.total,
            "updated pack components"
        );
        println!(
            "components updated (added: {}, removed: {}, total: {})",
            stats.added, stats.removed, stats.total
        );
    }

    Ok(())
}

pub fn sync_components(
    config: &mut PackConfig,
    components_dir: &Path,
) -> Result<ComponentUpdateStats> {
    let discovered = discover_components(components_dir)?;
    let initial_components = config.components.len();
    let mut preserved = 0usize;
    let mut added = 0usize;

    let (mut existing_by_id, existing_by_path) =
        index_components(std::mem::take(&mut config.components));
    let mut updated = Vec::new();

    for discovered in discovered {
        let rel_path = discovered.rel_wasm_path;
        let path_key = path_key(&rel_path);
        let stem = discovered.id_hint;
        let chosen_id = existing_by_path
            .get(&path_key)
            .cloned()
            .unwrap_or_else(|| stem.to_string());

        let mut component = if let Some(existing) = existing_by_path
            .get(&path_key)
            .and_then(|id| existing_by_id.remove(id))
        {
            preserved += 1;
            existing
        } else if let Some(existing) = existing_by_id.remove(&chosen_id) {
            preserved += 1;
            existing
        } else {
            added += 1;
            default_component(chosen_id.clone(), rel_path.clone())
        };

        if let Some(world) = infer_component_world(&discovered.abs_wasm_path)
            && (component.world.trim().is_empty() || component.world == "greentic:component/stub")
        {
            component.world = world;
        }

        component.id = chosen_id;
        component.wasm = rel_path;
        updated.push(component);
    }

    updated.sort_by(|a, b| a.id.cmp(&b.id));
    config.components = updated;

    let removed = initial_components.saturating_sub(preserved);

    Ok(ComponentUpdateStats {
        added,
        removed,
        total: config.components.len(),
    })
}

fn discover_components(dir: &Path) -> Result<Vec<DiscoveredComponent>> {
    let mut components = Vec::new();

    if dir.exists() {
        for entry in fs::read_dir(dir)
            .with_context(|| format!("failed to list components in {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;

            if file_type.is_file() {
                if path.extension() != Some(std::ffi::OsStr::new("wasm")) {
                    continue;
                }
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| anyhow!("invalid component filename: {}", path.display()))?;
                components.push(DiscoveredComponent {
                    rel_wasm_path: PathBuf::from("components").join(
                        path.file_name()
                            .ok_or_else(|| anyhow!("invalid component filename"))?,
                    ),
                    abs_wasm_path: path.clone(),
                    id_hint: stem.to_string(),
                });
                continue;
            }

            if file_type.is_dir() {
                let mut wasm_files = collect_wasm_files(&path)?;
                if wasm_files.is_empty() {
                    continue;
                }

                let chosen = wasm_files.iter().find(|p| {
                    p.file_name()
                        .map(|n| n == std::ffi::OsStr::new("component.wasm"))
                        .unwrap_or(false)
                });
                let wasm_path = chosen.cloned().unwrap_or_else(|| {
                    if wasm_files.len() == 1 {
                        wasm_files[0].clone()
                    } else {
                        wasm_files.sort();
                        wasm_files[0].clone()
                    }
                });

                let dir_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| anyhow!("invalid component directory name: {}", path.display()))?
                    .to_string();

                let wasm_file_name = wasm_path
                    .strip_prefix(dir)
                    .unwrap_or(&wasm_path)
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");

                components.push(DiscoveredComponent {
                    rel_wasm_path: PathBuf::from("components").join(&wasm_file_name),
                    abs_wasm_path: wasm_path,
                    id_hint: dir_name,
                });
            }
        }
    }

    components.sort_by(|a, b| a.id_hint.cmp(&b.id_hint));
    Ok(components)
}

fn collect_wasm_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut wasm_files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current)
            .with_context(|| format!("failed to list components in {}", current.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if file_type.is_file() && path.extension() == Some(std::ffi::OsStr::new("wasm")) {
                wasm_files.push(path);
            }
        }
    }

    Ok(wasm_files)
}

fn infer_component_world(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let decoded = match wit_component::decode(&bytes) {
        Ok(decoded) => decoded,
        Err(err) => {
            warn!(
                path = %path.display(),
                "failed to decode component for world inference: {err}"
            );
            return None;
        }
    };

    let (resolve, world_id) = match decoded {
        DecodedWasm::Component(resolve, world) => (resolve, world),
        DecodedWasm::WitPackage(..) => return None,
    };

    let world = &resolve.worlds[world_id];
    let pkg_id = world.package?;
    let pkg = &resolve.packages[pkg_id];

    let mut label = format!("{}:{}/{}", pkg.name.namespace, pkg.name.name, world.name);
    if let Some(version) = &pkg.name.version {
        label.push('@');
        label.push_str(&version.to_string());
    }

    Some(label)
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
