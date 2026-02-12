#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use greentic_types::cbor::canonical;
use serde::{Deserialize, Serialize};

/// Canonical pack lock format (v1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackLockV1 {
    pub version: u32,
    pub components: BTreeMap<String, LockedComponent>,
}

/// Locked component entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedComponent {
    pub component_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    pub abi_version: String,
    pub resolved_digest: String,
    pub describe_hash: String,
    pub operations: Vec<LockedOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// Locked operation entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedOperation {
    pub operation_id: String,
    pub schema_hash: String,
}

impl PackLockV1 {
    pub fn new(components: BTreeMap<String, LockedComponent>) -> Self {
        Self {
            version: 1,
            components,
        }
    }
}

/// Validate a pack.lock document.
pub fn validate_pack_lock(lock: &PackLockV1) -> Result<()> {
    if lock.version != 1 {
        anyhow::bail!("pack.lock version must be 1");
    }

    for (key, component) in &lock.components {
        if key.trim().is_empty() {
            anyhow::bail!("pack.lock component key must not be empty");
        }
        if component.component_id.trim().is_empty() {
            anyhow::bail!("pack.lock component_id must not be empty");
        }
        if key != &component.component_id {
            anyhow::bail!(
                "pack.lock component key {} does not match component_id {}",
                key,
                component.component_id
            );
        }
        if let Some(reference) = component.r#ref.as_ref()
            && reference.trim().is_empty()
        {
            anyhow::bail!("pack.lock component ref must not be empty when set");
        }
        if component.abi_version.trim().is_empty() {
            anyhow::bail!("pack.lock abi_version must not be empty");
        }
        if !component.resolved_digest.starts_with("sha256:") || component.resolved_digest.len() <= 7
        {
            anyhow::bail!(
                "pack.lock component resolved_digest for {} must start with sha256:<hex>",
                component.component_id
            );
        }
        if !is_hex_64(&component.describe_hash) {
            anyhow::bail!(
                "pack.lock component describe_hash for {} must be 64 hex chars",
                component.component_id
            );
        }
        let mut sorted = component.operations.clone();
        sorted.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
        if component.operations != sorted {
            anyhow::bail!(
                "pack.lock component {} operations must be sorted by operation_id",
                component.component_id
            );
        }
        for op in &component.operations {
            if op.operation_id.trim().is_empty() {
                anyhow::bail!(
                    "pack.lock component {} operation_id must not be empty",
                    component.component_id
                );
            }
            if !is_hex_64(&op.schema_hash) {
                anyhow::bail!(
                    "pack.lock component {} schema_hash must be 64 hex chars",
                    component.component_id
                );
            }
        }
        if let Some(world) = component.world.as_ref()
            && world.trim().is_empty()
        {
            anyhow::bail!("pack.lock component world must not be empty when set");
        }
        if let Some(version) = component.component_version.as_ref()
            && version.trim().is_empty()
        {
            anyhow::bail!("pack.lock component_version must not be empty when set");
        }
        if let Some(role) = component.role.as_ref()
            && role.trim().is_empty()
        {
            anyhow::bail!("pack.lock role must not be empty when set");
        }
    }

    Ok(())
}

fn is_hex_64(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit())
}

/// Read a pack.lock.cbor file from disk.
pub fn read_pack_lock(path: &Path) -> Result<PackLockV1> {
    let raw = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let lock: PackLockV1 = canonical::from_cbor(&raw).context("failed to decode pack.lock.cbor")?;
    validate_pack_lock(&lock)?;
    Ok(lock)
}

/// Write a pack.lock.cbor file to disk with deterministic ordering.
pub fn write_pack_lock(path: &Path, lock: &PackLockV1) -> Result<()> {
    validate_pack_lock(lock)?;
    let mut normalized = lock.clone();
    for component in normalized.components.values_mut() {
        component
            .operations
            .sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
    }

    let bytes =
        canonical::to_canonical_cbor(&normalized).context("failed to encode pack.lock.cbor")?;
    fs::write(path, bytes).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}
