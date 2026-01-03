use crate::path_safety::normalize_under_root;
use anyhow::{Context, Result, bail};
use greentic_types::pack_manifest::ExtensionInline;
use greentic_types::provider::{PROVIDER_EXTENSION_ID, ProviderDecl, ProviderExtensionInline};
use greentic_types::{
    ComponentCapabilities, ComponentProfiles, ExtensionRef, FlowKind, ResourceHints,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const PROVIDER_RUNTIME_WORLD: &str = "greentic:provider/schema-core@1.0.0";
const LEGACY_PROVIDER_EXTENSION_KIND: &str = "greentic.ext.provider";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PackConfig {
    pub pack_id: String,
    pub version: String,
    pub kind: String,
    pub publisher: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap: Option<BootstrapConfig>,
    #[serde(default)]
    pub components: Vec<ComponentConfig>,
    #[serde(default)]
    pub dependencies: Vec<DependencyConfig>,
    #[serde(default)]
    pub flows: Vec<FlowConfig>,
    #[serde(default)]
    pub assets: Vec<AssetConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, ExtensionRef>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ComponentConfig {
    pub id: String,
    pub version: String,
    pub world: String,
    #[serde(default)]
    pub supports: Vec<FlowKindLabel>,
    pub profiles: ComponentProfiles,
    pub capabilities: ComponentCapabilities,
    pub wasm: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<ComponentOperationConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceHints>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configurators: Option<ComponentConfiguratorConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ComponentOperationConfig {
    pub name: String,
    pub input_schema: JsonValue,
    pub output_schema: JsonValue,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FlowConfig {
    pub id: String,
    pub file: PathBuf,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub entrypoints: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DependencyConfig {
    pub alias: String,
    pub pack_id: String,
    pub version_req: String,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AssetConfig {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BootstrapConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_flow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrade_flow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installer_component: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ComponentConfiguratorConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FlowKindLabel {
    Messaging,
    Event,
    #[serde(
        rename = "componentconfig",
        alias = "component-config",
        alias = "component_config"
    )]
    ComponentConfig,
    Job,
    Http,
}

impl FlowKindLabel {
    pub fn to_kind(&self) -> FlowKind {
        match self {
            FlowKindLabel::Messaging => FlowKind::Messaging,
            FlowKindLabel::Event => FlowKind::Event,
            FlowKindLabel::ComponentConfig => FlowKind::ComponentConfig,
            FlowKindLabel::Job => FlowKind::Job,
            FlowKindLabel::Http => FlowKind::Http,
        }
    }
}

pub fn load_pack_config(root: &Path) -> Result<PackConfig> {
    let manifest_path = normalize_under_root(root, Path::new("pack.yaml"))?;
    let contents = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let mut cfg: PackConfig = serde_yaml_bw::from_str(&contents)
        .with_context(|| format!("{} is not a valid pack.yaml", manifest_path.display()))?;

    // Normalize relative paths to be under the pack root so downstream logic can treat them as absolute.
    for component in cfg.components.iter_mut() {
        component.wasm = normalize_under_root(root, &component.wasm)?;
    }
    for flow in cfg.flows.iter_mut() {
        flow.file = normalize_under_root(root, &flow.file)?;
    }
    for asset in cfg.assets.iter_mut() {
        asset.path = normalize_under_root(root, &asset.path)?;
    }

    validate_extensions(cfg.extensions.as_ref(), strict_extensions())?;

    Ok(cfg)
}

fn strict_extensions() -> bool {
    matches!(
        std::env::var("GREENTIC_PACK_STRICT_EXTENSIONS")
            .unwrap_or_default()
            .as_str(),
        "1" | "true" | "TRUE"
    )
}

fn validate_extensions(
    extensions: Option<&BTreeMap<String, ExtensionRef>>,
    strict: bool,
) -> Result<()> {
    let Some(exts) = extensions else {
        return Ok(());
    };

    for (key, ext) in exts {
        if ext.kind.trim().is_empty() {
            bail!("extensions[{key}] kind must not be empty");
        }
        if ext.version.trim().is_empty() {
            bail!("extensions[{key}] version must not be empty");
        }
        if ext.kind != *key {
            bail!(
                "extensions[{key}] kind `{}` must match the extension key",
                ext.kind
            );
        }
        if strict && let Some(location) = ext.location.as_deref() {
            let digest_missing = ext
                .digest
                .as_ref()
                .map(|d| d.trim().is_empty())
                .unwrap_or(true);
            if digest_missing {
                bail!("extensions[{key}] location requires digest in strict mode");
            }
            let allowed = location.starts_with("oci://")
                || location.starts_with("file://")
                || location.starts_with("https://");
            if !allowed {
                bail!(
                    "extensions[{key}] location `{location}` uses an unsupported scheme; allowed: oci://, file://, https://"
                );
            }
        }

        if ext.kind == PROVIDER_EXTENSION_ID || ext.kind == LEGACY_PROVIDER_EXTENSION_KIND {
            validate_provider_extension(key, ext)?;
        }
    }

    Ok(())
}

fn validate_provider_extension(key: &str, ext: &ExtensionRef) -> Result<()> {
    let inline = ext
        .inline
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("extensions[{key}] inline payload is required"))?;
    let providers = match inline {
        ExtensionInline::Provider(value) => value.providers.clone(),
        ExtensionInline::Other(value) => {
            serde_json::from_value::<ProviderExtensionInline>(value.clone())
                .with_context(|| {
                    format!("extensions[{key}].inline is not a valid provider extension")
                })?
                .providers
        }
    };
    if providers.is_empty() {
        bail!("extensions[{key}].inline.providers must not be empty");
    }

    for (idx, provider) in providers.iter().enumerate() {
        validate_provider_decl(provider, key, idx)?;
    }

    Ok(())
}

fn validate_provider_decl(provider: &ProviderDecl, key: &str, idx: usize) -> Result<()> {
    if provider.provider_type.trim().is_empty() {
        bail!("extensions[{key}].inline.providers[{idx}].provider_type must not be empty");
    }
    if provider.config_schema_ref.trim().is_empty() {
        bail!("extensions[{key}].inline.providers[{idx}].config_schema_ref must not be empty");
    }
    if provider.runtime.world != PROVIDER_RUNTIME_WORLD {
        bail!(
            "extensions[{key}].inline.providers[{idx}].runtime.world must be `{}`",
            PROVIDER_RUNTIME_WORLD
        );
    }
    if provider.runtime.component_ref.trim().is_empty() || provider.runtime.export.trim().is_empty()
    {
        bail!(
            "extensions[{key}].inline.providers[{idx}].runtime component_ref/export must not be empty"
        );
    }
    validate_string_vec(&provider.capabilities, "capabilities", key, idx)?;
    validate_string_vec(&provider.ops, "ops", key, idx)?;
    Ok(())
}

fn validate_string_vec(entries: &[String], field: &str, key: &str, idx: usize) -> Result<()> {
    if entries.is_empty() {
        bail!("extensions[{key}].inline.providers[{idx}].{field} must not be empty");
    }
    for (entry_idx, entry) in entries.iter().enumerate() {
        if entry.trim().is_empty() {
            bail!(
                "extensions[{key}].inline.providers[{idx}].{field}[{entry_idx}] must be a non-empty string"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn provider_extension_inline() -> JsonValue {
        json!({
            "providers": [
                {
                    "provider_type": "messaging.telegram.bot",
                    "capabilities": ["send", "receive"],
                    "ops": ["send", "reply"],
                    "config_schema_ref": "schemas/messaging/telegram/config.schema.json",
                    "state_schema_ref": "schemas/messaging/telegram/state.schema.json",
                    "runtime": {
                        "component_ref": "telegram-provider",
                        "export": "provider",
                        "world": PROVIDER_RUNTIME_WORLD
                    },
                    "docs_ref": "schemas/messaging/telegram/README.md"
                }
            ]
        })
    }

    #[test]
    fn provider_extension_validates() {
        let mut extensions = BTreeMap::new();
        extensions.insert(
            PROVIDER_EXTENSION_ID.to_string(),
            ExtensionRef {
                kind: PROVIDER_EXTENSION_ID.into(),
                version: "1.0.0".into(),
                digest: Some("sha256:abc123".into()),
                location: None,
                inline: Some(
                    serde_json::from_value(provider_extension_inline()).expect("inline parse"),
                ),
            },
        );
        validate_extensions(Some(&extensions), false).expect("provider extension should validate");
    }

    #[test]
    fn provider_extension_missing_required_fields_fails() {
        let mut extensions = BTreeMap::new();
        extensions.insert(
            PROVIDER_EXTENSION_ID.to_string(),
            ExtensionRef {
                kind: PROVIDER_EXTENSION_ID.into(),
                version: "1.0.0".into(),
                digest: None,
                location: None,
                inline: Some(
                    serde_json::from_value(json!({
                        "providers": [{
                            "provider_type": "",
                            "capabilities": [],
                            "ops": ["send"],
                            "config_schema_ref": "",
                            "state_schema_ref": "schemas/state.json",
                            "runtime": {
                                "component_ref": "",
                                "export": "",
                                "world": "greentic:provider/schema-core@1.0.0"
                            }
                        }]
                    }))
                    .expect("inline parse"),
                ),
            },
        );
        assert!(
            validate_extensions(Some(&extensions), false).is_err(),
            "missing fields should fail validation"
        );
    }

    #[test]
    fn strict_mode_requires_digest_for_remote_extension() {
        let mut extensions = BTreeMap::new();
        extensions.insert(
            "greentic.ext.provider".to_string(),
            ExtensionRef {
                kind: PROVIDER_EXTENSION_ID.into(),
                version: "1.0.0".into(),
                digest: None,
                location: Some("oci://registry/extensions/provider".into()),
                inline: None,
            },
        );
        assert!(
            validate_extensions(Some(&extensions), true).is_err(),
            "strict mode should require digest when location is set"
        );
    }

    #[test]
    fn unknown_extensions_are_allowed() {
        let mut extensions = BTreeMap::new();
        extensions.insert(
            "acme.ext.logging".to_string(),
            ExtensionRef {
                kind: "acme.ext.logging".into(),
                version: "0.1.0".into(),
                digest: None,
                location: None,
                inline: None,
            },
        );
        validate_extensions(Some(&extensions), false).expect("unknown extensions should pass");
    }
}
