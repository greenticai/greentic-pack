use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use greentic_types::{
    ArtifactLocation, CacheInfo, ComponentDigest, ComponentStatus, DistributorEnvironmentId,
    ResolveComponentRequest, SecretFormat, SecretKey, SecretRequirement, SecretScope,
    SignatureSummary, TenantCtx,
};

pub use greentic_types::{ComponentId, EnvId, PackId, TenantId};

pub use semver::Version;

/// Resolve response that now includes secret requirements from the distributor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveComponentResponse {
    pub status: ComponentStatus,
    pub digest: ComponentDigest,
    pub artifact: ArtifactLocation,
    pub signature: SignatureSummary,
    pub cache: CacheInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_requirements: Option<Vec<SecretRequirement>>,
}

/// Typed pack status response that carries secret requirements.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackStatusResponse {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_requirements: Option<Vec<SecretRequirement>>,
    #[serde(default)]
    pub extra: Value,
}
