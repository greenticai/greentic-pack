use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Supported pack kinds.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PackKind {
    Application,
    SourceProvider,
    Scanner,
    Signing,
    Attestation,
    PolicyEngine,
    OciProvider,
    BillingProvider,
    SearchProvider,
    RecommendationProvider,
    DistributionBundle,
    /// Reserved for future phases; validation should reject this kind for now.
    RolloutStrategy,
}

impl PackKind {
    pub fn validate_allowed(&self) -> anyhow::Result<()> {
        if matches!(self, PackKind::RolloutStrategy) {
            anyhow::bail!("kind `rollout-strategy` is reserved and must not be used");
        }
        Ok(())
    }
}
