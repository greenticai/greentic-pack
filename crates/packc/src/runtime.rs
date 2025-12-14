#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use anyhow::Result;
use greentic_config::{CliOverrides, ConfigResolver, ResolvedConfig};
use greentic_types::ConnectionKind;
use std::sync::Arc;

pub struct RuntimeState {
    pub resolved: ResolvedConfig,
}

pub type RuntimeContext = Arc<RuntimeState>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPolicy {
    Online,
    Offline,
}

impl RuntimeState {
    pub fn cache_dir(&self) -> PathBuf {
        self.resolved.config.paths.cache_dir.clone()
    }

    pub fn network_policy(&self) -> NetworkPolicy {
        if self.resolved.config.network.offline
            || matches!(
                self.resolved.config.environment.connection,
                Some(ConnectionKind::Offline)
            )
        {
            NetworkPolicy::Offline
        } else {
            NetworkPolicy::Online
        }
    }

    pub fn require_online(&self, action: &str) -> Result<()> {
        match self.network_policy() {
            NetworkPolicy::Online => Ok(()),
            NetworkPolicy::Offline => Err(anyhow::anyhow!(
                "network operation blocked in offline mode: {}",
                action
            )),
        }
    }

    pub fn warnings(&self) -> &[String] {
        &self.resolved.warnings
    }
}

pub fn resolve_runtime(
    project_root: Option<&Path>,
    cli_cache_dir: Option<&Path>,
    cli_offline: bool,
    cli_override: Option<&Path>,
) -> Result<RuntimeContext> {
    let mut resolver = ConfigResolver::new();
    if let Some(root) = project_root {
        resolver = resolver.with_project_root(root.to_path_buf());
    }

    if let Some(path) = cli_override {
        resolver = resolver.with_cli_overrides(CliOverrides {
            config_path: Some(path.to_path_buf()),
            ..Default::default()
        });
    }

    let mut resolved = resolver.load()?;

    if cli_offline {
        resolved.config.environment.connection = Some(ConnectionKind::Offline);
        resolved.config.network.offline = true;
        resolved
            .warnings
            .push("offline forced by CLI --offline flag".to_string());
    }

    if let Some(cache_dir) = cli_cache_dir {
        resolved.config.paths.cache_dir = cache_dir.to_path_buf();
    }

    Ok(Arc::new(RuntimeState { resolved }))
}
