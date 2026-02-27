#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow};
use greentic_distributor_client::{DistClient, DistOptions};
use greentic_types::WizardStep;
use serde::Deserialize;
use tokio::runtime::Handle;

use crate::cli::wizard_i18n::WizardI18n;
use crate::runtime::{NetworkPolicy, RuntimeContext};

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ExtensionCatalog {
    pub(crate) extension_types: Vec<ExtensionType>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ExtensionType {
    pub(crate) id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    name_key: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    description_key: Option<String>,
    #[serde(default)]
    pub(crate) templates: Vec<ExtensionTemplate>,
    #[serde(default)]
    pub(crate) edit_questions: Vec<CatalogQuestion>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ExtensionTemplate {
    pub(crate) id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    name_key: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    description_key: Option<String>,
    #[serde(default)]
    pub(crate) plan: Vec<WizardStep>,
    #[serde(default)]
    pub(crate) qa_questions: Vec<CatalogQuestion>,
}

impl ExtensionType {
    pub(crate) fn display_name(&self, i18n: &WizardI18n) -> String {
        resolve_catalog_text(
            i18n,
            self.name_key.as_deref(),
            self.name.as_deref(),
            &self.id,
        )
    }

    pub(crate) fn display_description(&self, i18n: &WizardI18n) -> String {
        resolve_catalog_text(
            i18n,
            self.description_key.as_deref(),
            self.description.as_deref(),
            "",
        )
    }
}

impl ExtensionTemplate {
    pub(crate) fn display_name(&self, i18n: &WizardI18n) -> String {
        resolve_catalog_text(
            i18n,
            self.name_key.as_deref(),
            self.name.as_deref(),
            &self.id,
        )
    }

    pub(crate) fn display_description(&self, i18n: &WizardI18n) -> String {
        resolve_catalog_text(
            i18n,
            self.description_key.as_deref(),
            self.description.as_deref(),
            "",
        )
    }
}

fn resolve_catalog_text(
    i18n: &WizardI18n,
    key: Option<&str>,
    raw: Option<&str>,
    fallback: &str,
) -> String {
    if let Some(key) = key {
        let resolved = i18n.t(key);
        if !resolved.starts_with("??") {
            return resolved;
        }
    }
    raw.unwrap_or(fallback).to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CatalogQuestion {
    pub(crate) id: String,
    pub(crate) title_key: String,
    #[serde(default)]
    pub(crate) description_key: Option<String>,
    #[serde(default)]
    pub(crate) default: Option<String>,
    #[serde(default)]
    pub(crate) kind: CatalogQuestionKind,
    #[serde(default)]
    pub(crate) choices: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CatalogQuestionKind {
    #[default]
    String,
    Enum,
    Boolean,
    Integer,
}

pub(crate) fn load_extension_catalog(
    catalog_ref: &str,
    runtime: Option<&RuntimeContext>,
) -> Result<ExtensionCatalog> {
    if let Some(path) = catalog_ref.strip_prefix("fixture://") {
        let bytes = match path {
            "extensions.json" => FIXTURE_EXTENSIONS_JSON.as_bytes().to_vec(),
            other => return Err(anyhow!("unknown fixture catalog `{other}`")),
        };
        return parse_catalog_bytes(&bytes);
    }

    if let Some(path) = catalog_ref.strip_prefix("file://") {
        let bytes = std::fs::read(path).with_context(|| format!("read catalog file {path}"))?;
        return parse_catalog_bytes(&bytes);
    }

    if catalog_ref.starts_with("oci://") {
        let Some(runtime) = runtime else {
            return Err(anyhow!("runtime required for oci catalog loading"));
        };
        return load_catalog_from_oci(catalog_ref, runtime);
    }

    Err(anyhow!(
        "unsupported catalog ref scheme; expected fixture://, file://, or oci://"
    ))
}

fn parse_catalog_bytes(bytes: &[u8]) -> Result<ExtensionCatalog> {
    let mut catalog: ExtensionCatalog =
        serde_json::from_slice(bytes).context("decode extension catalog json")?;
    if !catalog
        .extension_types
        .iter()
        .any(|item| item.id == "custom-scaffold")
    {
        catalog.extension_types.push(ExtensionType {
            id: "custom-scaffold".to_string(),
            name: Some("Custom extension".to_string()),
            name_key: None,
            description: Some("Scaffold only".to_string()),
            description_key: None,
            templates: vec![default_custom_template()],
            edit_questions: vec![default_edit_question("entry_label", "custom")],
        });
    }
    for extension_type in &mut catalog.extension_types {
        if extension_type.templates.is_empty() {
            extension_type.templates.push(default_generic_template());
        }
        if extension_type.edit_questions.is_empty() {
            extension_type
                .edit_questions
                .push(default_edit_question("entry_label", &extension_type.id));
        }
    }
    Ok(catalog)
}

fn default_generic_template() -> ExtensionTemplate {
    ExtensionTemplate {
        id: "default".to_string(),
        name: Some("Default".to_string()),
        name_key: None,
        description: Some("Baseline scaffold".to_string()),
        description_key: None,
        plan: vec![
            WizardStep::EnsureDir {
                paths: vec!["flows".to_string(), "components".to_string(), "i18n".to_string()],
            },
            WizardStep::WriteFiles {
                files: BTreeMap::from([
                    (
                        "README.md".to_string(),
                        "# {{extension_type_name}} extension\n\n{{not_implemented}}\n".to_string(),
                    ),
                    (
                        "pack.yaml".to_string(),
                        "pack_id: {{extension_type_id}}.extension\nversion: 0.1.0\nkind: application\npublisher: Greentic\n\ncomponents: []\nflows: []\ndependencies: []\nassets: []\n".to_string(),
                    ),
                ]),
            },
        ],
        qa_questions: vec![CatalogQuestion {
            id: "display_name".to_string(),
            title_key: "wizard.qa.display_name".to_string(),
            description_key: None,
            default: Some("My extension".to_string()),
            kind: CatalogQuestionKind::String,
            choices: Vec::new(),
        }],
    }
}

fn default_custom_template() -> ExtensionTemplate {
    ExtensionTemplate {
        id: "custom-scaffold-template".to_string(),
        name: Some("Custom scaffold template".to_string()),
        name_key: None,
        description: Some("Create a minimal custom extension skeleton".to_string()),
        description_key: None,
        plan: vec![
            WizardStep::EnsureDir {
                paths: vec!["flows".to_string(), "components".to_string(), "i18n".to_string()],
            },
            WizardStep::WriteFiles {
                files: BTreeMap::from([
                    (
                        "README.md".to_string(),
                        "# {{extension_type_name}} ({{template_name}})\n\nCanonical key: {{canonical_extension_key}}\n\nNext steps:\n- Add components under ./components\n- Add flows under ./flows\n".to_string(),
                    ),
                    (
                        "pack.yaml".to_string(),
                        "pack_id: custom.extension\nversion: 0.1.0\nkind: application\npublisher: Greentic\n\ncomponents: []\nflows: []\ndependencies: []\nassets: []\n".to_string(),
                    ),
                    (
                        "flows/main.ygtc".to_string(),
                        "id: main\ntype: messaging\nnodes: {}\n".to_string(),
                    ),
                ]),
            },
        ],
        qa_questions: vec![CatalogQuestion {
            id: "display_name".to_string(),
            title_key: "wizard.qa.display_name".to_string(),
            description_key: None,
            default: Some("Custom extension".to_string()),
            kind: CatalogQuestionKind::String,
            choices: Vec::new(),
        }],
    }
}

fn default_edit_question(id: &str, default: &str) -> CatalogQuestion {
    CatalogQuestion {
        id: id.to_string(),
        title_key: "wizard.update_extension_pack.edit.entry_label".to_string(),
        description_key: None,
        default: Some(default.to_string()),
        kind: CatalogQuestionKind::String,
        choices: Vec::new(),
    }
}

fn load_catalog_from_oci(catalog_ref: &str, runtime: &RuntimeContext) -> Result<ExtensionCatalog> {
    let offline = runtime.network_policy() == NetworkPolicy::Offline;
    let cache_dir = runtime.cache_dir().join("wizard/catalogs");
    let handle = Handle::try_current().context("catalog resolution requires a Tokio runtime")?;
    let bytes = block_on(&handle, async {
        let dist = DistClient::new(DistOptions {
            cache_dir,
            allow_tags: true,
            offline,
            allow_insecure_local_http: false,
            ..DistOptions::default()
        });

        let resolved = if offline {
            dist.ensure_cached(catalog_ref)
                .await
                .context("catalog ref not cached in offline mode")?
        } else {
            dist.resolve_ref(catalog_ref)
                .await
                .context("failed to fetch catalog ref")?
        };
        let cache_path = resolved
            .cache_path
            .as_ref()
            .ok_or_else(|| anyhow!("catalog ref resolved without cache path"))?;
        std::fs::read(cache_path)
            .with_context(|| format!("failed to read catalog cache {}", cache_path.display()))
    })?;

    parse_catalog_bytes(&bytes)
}

fn block_on<F, T, E>(handle: &Handle, fut: F) -> std::result::Result<T, E>
where
    F: std::future::Future<Output = std::result::Result<T, E>>,
{
    tokio::task::block_in_place(|| handle.block_on(fut))
}

const FIXTURE_EXTENSIONS_JSON: &str = include_str!("../../tests/fixtures/wizard/extensions.json");
