#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Canonical pack lock format (v1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackLockV1 {
    pub schema_version: u32,
    pub components: Vec<LockedComponent>,
}

/// Locked component entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedComponent {
    pub name: String,
    pub r#ref: String,
    pub digest: String,
}

impl PackLockV1 {
    pub fn new(components: Vec<LockedComponent>) -> Self {
        Self {
            schema_version: 1,
            components,
        }
    }
}

/// Validate a pack.lock document.
pub fn validate_pack_lock(lock: &PackLockV1) -> Result<()> {
    if lock.schema_version != 1 {
        anyhow::bail!("pack.lock schema_version must be 1");
    }

    for component in &lock.components {
        if component.name.trim().is_empty() {
            anyhow::bail!("pack.lock component name must not be empty");
        }
        if component.r#ref.trim().is_empty() {
            anyhow::bail!("pack.lock component ref must not be empty");
        }
        if !component.digest.starts_with("sha256:") || component.digest.len() <= 7 {
            anyhow::bail!(
                "pack.lock component digest for {} must start with sha256:<hex>",
                component.name
            );
        }
    }

    Ok(())
}

/// Read a pack.lock.json file from disk.
pub fn read_pack_lock(path: &Path) -> Result<PackLockV1> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let lock: PackLockV1 = serde_json::from_str(&raw).context("failed to parse pack.lock.json")?;
    validate_pack_lock(&lock)?;
    Ok(lock)
}

/// Write a pack.lock.json file to disk with deterministic ordering.
pub fn write_pack_lock(path: &Path, lock: &PackLockV1) -> Result<()> {
    validate_pack_lock(lock)?;
    let mut normalized = lock.clone();
    normalized
        .components
        .sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.r#ref.cmp(&b.r#ref)));

    let json =
        serde_json::to_string_pretty(&normalized).context("failed to serialize pack.lock.json")?;
    fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}
