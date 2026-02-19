use crate::{DistributorEnvironmentId, TenantCtx};
use greentic_config_types::GreenticConfig;
use std::{collections::HashMap, time::Duration};

/// Configuration for distributor clients.
///
/// NOTE: `base_url`, `auth_token`, and header fields are used when the
/// `http-runtime` feature is enabled. Without that feature, the WIT client is
/// the primary implementation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DistributorClientConfig {
    pub base_url: Option<String>,
    pub environment_id: DistributorEnvironmentId,
    pub tenant: TenantCtx,
    pub auth_token: Option<String>,
    pub extra_headers: Option<HashMap<String, String>>,
    pub request_timeout: Option<Duration>,
}

impl DistributorClientConfig {
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Builds a distributor client config from a resolved GreenticConfig and tenant context.
    ///
    /// This keeps greentic-config resolution in the host while allowing consumers to
    /// map the shared schema into the distributor client.
    pub fn from_greentic(cfg: &GreenticConfig, mut tenant: TenantCtx) -> Self {
        // Ensure tenant context env matches the shared config.
        tenant.env = cfg.environment.env_id.clone();
        tenant.tenant_id = tenant.tenant.clone();

        let environment_id = DistributorEnvironmentId::from(cfg.environment.env_id.as_str());
        let request_timeout = cfg.network.read_timeout_ms.map(Duration::from_millis);

        Self {
            base_url: None,
            environment_id,
            tenant,
            auth_token: None,
            extra_headers: None,
            request_timeout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greentic_config_types::{
        ConfigVersion, EnvironmentConfig, NetworkConfig, PathsConfig, RuntimeConfig,
        SecretsBackendRefConfig, TelemetryConfig,
    };
    use greentic_types::{EnvId, TenantCtx, TenantId};
    use std::path::PathBuf;

    fn greentic_config() -> GreenticConfig {
        GreenticConfig {
            schema_version: ConfigVersion::default(),
            environment: EnvironmentConfig {
                env_id: EnvId::try_from("dev").unwrap(),
                deployment: None,
                connection: None,
                region: None,
            },
            paths: PathsConfig {
                greentic_root: PathBuf::from("/workspace"),
                state_dir: PathBuf::from("/workspace/state"),
                cache_dir: PathBuf::from("/workspace/cache"),
                logs_dir: PathBuf::from("/workspace/logs"),
            },
            packs: None,
            services: None,
            events: None,
            runtime: RuntimeConfig::default(),
            telemetry: TelemetryConfig::default(),
            network: NetworkConfig::default(),
            deployer: None,
            secrets: SecretsBackendRefConfig::default(),
            dev: None,
        }
    }

    #[test]
    fn maps_request_timeout_from_network_read_timeout() {
        let mut cfg = greentic_config();
        cfg.network.read_timeout_ms = Some(2500);

        let tenant = TenantCtx::new(
            EnvId::try_from("ignored").unwrap(),
            TenantId::try_from("t1").unwrap(),
        );
        let mapped = DistributorClientConfig::from_greentic(&cfg, tenant);

        assert_eq!(mapped.request_timeout, Some(Duration::from_millis(2500)));
        assert_eq!(mapped.environment_id.as_str(), "dev");
    }
}
