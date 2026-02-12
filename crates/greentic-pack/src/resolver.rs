#![forbid(unsafe_code)]

use std::path::PathBuf;

use anyhow::Result;
use anyhow::{Context, anyhow};
use sha2::{Digest, Sha256};
use std::fs;

/// Resolve request for a component reference.
#[derive(Clone, Debug)]
pub struct ResolveReq {
    pub component_id: String,
    pub reference: String,
    pub expected_digest: String,
    pub abi_version: String,
    pub world: Option<String>,
    pub component_version: Option<String>,
}

/// Resolved component data + minimal typed metadata.
#[derive(Clone, Debug)]
pub struct ResolvedComponent {
    pub bytes: Vec<u8>,
    pub resolved_digest: String,
    pub component_id: String,
    pub abi_version: String,
    pub world: Option<String>,
    pub component_version: Option<String>,
    pub source_path: Option<PathBuf>,
}

/// Host-side resolver for component references.
pub trait ComponentResolver {
    fn resolve(&self, req: ResolveReq) -> Result<ResolvedComponent>;
}

/// Fixture-backed resolver for offline tests.
#[derive(Clone, Debug)]
pub struct FixtureResolver {
    root: PathBuf,
}

impl FixtureResolver {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn resolve_component_path(&self, component_id: &str) -> PathBuf {
        self.root
            .join("components")
            .join(component_id)
            .join("component.wasm")
    }
}

impl ComponentResolver for FixtureResolver {
    fn resolve(&self, req: ResolveReq) -> Result<ResolvedComponent> {
        let component_id = req.component_id.clone();
        let path = self.resolve_component_path(&component_id);
        if !path.exists() {
            return Err(anyhow!(
                "fixture component {} not found at {}",
                component_id,
                path.display()
            ));
        }
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
        if !req.expected_digest.is_empty() && req.expected_digest != digest {
            return Err(anyhow!(
                "fixture component digest mismatch for {} (expected {}, got {})",
                component_id,
                req.expected_digest,
                digest
            ));
        }
        Ok(ResolvedComponent {
            bytes,
            resolved_digest: digest,
            component_id,
            abi_version: req.abi_version,
            world: req.world,
            component_version: req.component_version,
            source_path: Some(path),
        })
    }
}
