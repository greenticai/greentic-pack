#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use greentic_pack::archive_shape::{CANONICAL_MANIFEST_ENTRY, non_canonical_archive_message};
use greentic_types::pack_manifest::{PackManifest, PackSignatures};
use greentic_types::provider::{ProviderDecl, ProviderExtensionInline};
use greentic_types::{PackId, PackKind, decode_pack_manifest};
use tempfile::TempDir;
use zip::ZipArchive;

use crate::cli::input::materialize_pack_path;

#[derive(Debug, Subcommand)]
pub enum ProvidersCommand {
    /// List providers declared in the provider extension.
    List(ListArgs),
    /// Show details for a specific provider id.
    Info(InfoArgs),
    /// Validate provider extension contents.
    Validate(ValidateArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Path to a .gtpack archive or pack source directory (defaults to current dir).
    #[arg(long = "pack", value_name = "PATH")]
    pub pack: Option<PathBuf>,

    /// Emit JSON output
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct InfoArgs {
    /// Provider identifier to inspect.
    #[arg(value_name = "PROVIDER_ID")]
    pub provider_id: String,

    /// Path to a .gtpack archive or pack source directory (defaults to current dir).
    #[arg(long = "pack", value_name = "PATH")]
    pub pack: Option<PathBuf>,

    /// Emit JSON output
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// Path to a .gtpack archive or pack source directory (defaults to current dir).
    #[arg(long = "pack", value_name = "PATH")]
    pub pack: Option<PathBuf>,

    /// Treat warnings as errors (e.g. missing local references).
    #[arg(long)]
    pub strict: bool,

    /// Emit JSON output
    #[arg(long)]
    pub json: bool,
}

pub fn run(cmd: ProvidersCommand) -> Result<()> {
    match cmd {
        ProvidersCommand::List(args) => list(&args),
        ProvidersCommand::Info(args) => info(&args),
        ProvidersCommand::Validate(args) => validate(&args),
    }
}

pub fn list(args: &ListArgs) -> Result<()> {
    let pack = load_pack(args.pack.as_deref())?;
    let providers = providers_from_manifest(&pack.manifest);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&providers)?);
        return Ok(());
    }

    if providers.is_empty() {
        println!(
            "{}",
            crate::cli_i18n::t("cli.providers.no_providers_declared")
        );
        return Ok(());
    }

    println!("{}", crate::cli_i18n::t("cli.providers.table_header"));
    for provider in providers {
        let runtime = format!(
            "{}::{}",
            provider.runtime.component_ref, provider.runtime.export
        );
        let kind = provider_kind(&provider);
        let details = summarize_provider(&provider);
        println!(
            "{:<24} {:<28} {:<16} {}",
            provider.provider_type, runtime, kind, details
        );
    }

    Ok(())
}

pub fn info(args: &InfoArgs) -> Result<()> {
    let pack = load_pack(args.pack.as_deref())?;
    let inline = match pack.manifest.provider_extension_inline() {
        Some(value) => value,
        None => bail!(
            "{}",
            crate::cli_i18n::t("cli.providers.error.extension_not_present")
        ),
    };
    let Some(provider) = inline
        .providers
        .iter()
        .find(|p| p.provider_type == args.provider_id)
    else {
        bail!(
            "{}",
            crate::cli_i18n::tf(
                "cli.providers.error.provider_not_found",
                &[&args.provider_id]
            )
        );
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(provider)?);
    } else {
        let yaml = serde_yaml_bw::to_string(provider)?;
        println!("{yaml}");
    }

    Ok(())
}

pub fn validate(args: &ValidateArgs) -> Result<()> {
    let pack = load_pack(args.pack.as_deref())?;
    let Some(inline) = pack.manifest.provider_extension_inline() else {
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": crate::cli_i18n::t("cli.status.ok"),
                    "providers_present": false,
                    "warnings": [],
                }))?
            );
        } else {
            println!(
                "{}",
                crate::cli_i18n::t("cli.providers.valid_extension_not_present")
            );
        }
        return Ok(());
    };

    if let Err(err) = inline.validate_basic() {
        return Err(anyhow!(err.to_string()));
    }

    let warnings = validate_local_refs(inline, &pack);
    if args.strict && !warnings.is_empty() {
        let message = warnings.join("; ");
        return Err(anyhow!(message));
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": crate::cli_i18n::t("cli.status.ok"),
                "providers_present": true,
                "warnings": warnings,
            }))?
        );
    } else if warnings.is_empty() {
        println!("{}", crate::cli_i18n::t("cli.providers.valid"));
    } else {
        println!(
            "{}",
            crate::cli_i18n::t("cli.providers.valid_with_warnings")
        );
        for warning in warnings {
            println!(
                "{}",
                crate::cli_i18n::tf("cli.providers.warning_item", &[&warning])
            );
        }
    }

    Ok(())
}

#[derive(Debug)]
struct LoadedPack {
    manifest: PackManifest,
    root_dir: Option<PathBuf>,
    entries: HashSet<String>,
    _temp: Option<TempDir>,
}

fn load_pack(pack: Option<&Path>) -> Result<LoadedPack> {
    let input = pack.unwrap_or_else(|| Path::new("."));
    let root_dir = if input.is_dir() {
        Some(
            input
                .canonicalize()
                .with_context(|| format!("failed to canonicalize {}", input.display()))?,
        )
    } else {
        None
    };
    let (temp, pack_path) = materialize_pack_path(input, false)?;
    let (manifest, entries) = read_manifest(&pack_path)?;
    Ok(LoadedPack {
        manifest,
        root_dir,
        entries,
        _temp: temp,
    })
}

fn read_manifest(path: &Path) -> Result<(PackManifest, HashSet<String>)> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut archive = ZipArchive::new(file)
        .with_context(|| format!("{} is not a valid gtpack archive", path.display()))?;
    let mut entries = HashSet::new();
    for i in 0..archive.len() {
        let name = archive
            .by_index(i)
            .context("failed to read archive entry")?
            .name()
            .to_string();
        entries.insert(name);
    }

    let mut manifest_entry = archive
        .by_name(CANONICAL_MANIFEST_ENTRY)
        .with_context(|| non_canonical_archive_message(&entries.iter().cloned().collect()))?;
    let mut buf = Vec::new();
    manifest_entry.read_to_end(&mut buf)?;
    let manifest = match decode_pack_manifest(&buf) {
        Ok(manifest) => manifest,
        Err(err) => {
            // Fallback to legacy greentic-pack manifest to keep older packs usable.
            let legacy: greentic_pack::builder::PackManifest =
                serde_cbor::from_slice(&buf).map_err(|_| err)?;
            downgrade_legacy_manifest(&legacy)?
        }
    };

    Ok((manifest, entries))
}

fn downgrade_legacy_manifest(
    manifest: &greentic_pack::builder::PackManifest,
) -> Result<PackManifest> {
    let pack_id =
        PackId::new(manifest.meta.pack_id.clone()).context("legacy manifest pack_id is invalid")?;
    Ok(PackManifest {
        schema_version: "pack-v1".to_string(),
        pack_id,
        name: Some(manifest.meta.name.clone()),
        version: manifest.meta.version.clone(),
        kind: PackKind::Application,
        publisher: manifest.meta.authors.first().cloned().unwrap_or_default(),
        components: Vec::new(),
        flows: Vec::new(),
        dependencies: Vec::new(),
        capabilities: Vec::new(),
        secret_requirements: Vec::new(),
        signatures: PackSignatures::default(),
        bootstrap: None,
        extensions: None,
        agents: BTreeMap::new(),
    })
}

fn providers_from_manifest(manifest: &PackManifest) -> Vec<ProviderDecl> {
    let mut providers = manifest
        .provider_extension_inline()
        .map(|inline| inline.providers.clone())
        .unwrap_or_default();
    providers.sort_by(|a, b| a.provider_type.cmp(&b.provider_type));
    providers
}

fn provider_kind(provider: &ProviderDecl) -> String {
    provider
        .runtime
        .world
        .split('@')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn summarize_provider(provider: &ProviderDecl) -> String {
    let caps = provider.capabilities.len();
    let ops = provider.ops.len();
    let mut parts = vec![format!("caps:{caps}"), format!("ops:{ops}")];
    parts.push(format!("config:{}", provider.config_schema_ref));
    if let Some(docs) = provider.docs_ref.as_deref() {
        parts.push(format!("docs:{docs}"));
    }
    parts.join(" ")
}

fn validate_local_refs(inline: &ProviderExtensionInline, pack: &LoadedPack) -> Vec<String> {
    let mut warnings = Vec::new();
    for provider in &inline.providers {
        for (label, value) in referenced_paths(provider) {
            if !is_local_ref(value) {
                continue;
            }
            if !ref_exists(value, pack) {
                warnings.push(format!(
                    "provider `{}` {} reference `{}` missing",
                    provider.provider_type, label, value
                ));
            }
        }
    }
    warnings
}

fn referenced_paths(provider: &ProviderDecl) -> Vec<(&'static str, &str)> {
    let mut refs = Vec::new();
    refs.push(("config_schema_ref", provider.config_schema_ref.as_str()));
    if let Some(state) = provider.state_schema_ref.as_deref() {
        refs.push(("state_schema_ref", state));
    }
    if let Some(docs) = provider.docs_ref.as_deref() {
        refs.push(("docs_ref", docs));
    }
    refs
}

fn is_local_ref(value: &str) -> bool {
    !value.contains("://")
}

fn ref_exists(value: &str, pack: &LoadedPack) -> bool {
    if let Some(root) = pack.root_dir.as_ref() {
        let candidate = root.join(value);
        if candidate.exists() {
            return true;
        }
    }

    pack.entries.contains(&normalize_entry(value))
}

fn normalize_entry(value: &str) -> String {
    value
        .split(std::path::MAIN_SEPARATOR)
        .flat_map(|part| part.split(['/', '\\']))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use greentic_types::pack_manifest::{ExtensionInline, ExtensionRef};
    use greentic_types::provider::{PROVIDER_EXTENSION_ID, ProviderRuntimeRef};
    use semver::Version;

    fn provider(provider_type: &str) -> ProviderDecl {
        ProviderDecl {
            provider_type: provider_type.to_string(),
            capabilities: vec!["send".to_string(), "receive".to_string()],
            ops: vec!["send".to_string()],
            config_schema_ref: "schemas/provider.json".to_string(),
            state_schema_ref: Some("schemas/state.json".to_string()),
            runtime: ProviderRuntimeRef {
                component_ref: "provider.component".to_string(),
                export: "provider".to_string(),
                world: "greentic:provider/schema-core@1.0.0".to_string(),
            },
            docs_ref: Some("docs/provider.md".to_string()),
        }
    }

    fn manifest_with_providers(providers: Vec<ProviderDecl>) -> PackManifest {
        PackManifest {
            schema_version: "pack-v1".to_string(),
            pack_id: PackId::new("dev.local.providers").expect("pack id"),
            name: Some("providers".to_string()),
            version: Version::parse("0.1.0").expect("version"),
            kind: PackKind::Application,
            publisher: "test".to_string(),
            components: Vec::new(),
            flows: Vec::new(),
            dependencies: Vec::new(),
            capabilities: Vec::new(),
            secret_requirements: Vec::new(),
            signatures: PackSignatures::default(),
            bootstrap: None,
            extensions: Some(std::collections::BTreeMap::from([(
                PROVIDER_EXTENSION_ID.to_string(),
                ExtensionRef {
                    kind: PROVIDER_EXTENSION_ID.to_string(),
                    version: "1.0.0".to_string(),
                    digest: None,
                    location: None,
                    inline: Some(ExtensionInline::Provider(ProviderExtensionInline {
                        providers,
                        additional_fields: Default::default(),
                    })),
                },
            )])),
            agents: BTreeMap::new(),
        }
    }

    #[test]
    fn providers_from_manifest_returns_sorted_entries() {
        let manifest = manifest_with_providers(vec![provider("zeta"), provider("alpha")]);
        let sorted = providers_from_manifest(&manifest);
        assert_eq!(sorted[0].provider_type, "alpha");
        assert_eq!(sorted[1].provider_type, "zeta");
    }

    #[test]
    fn provider_helpers_summarize_runtime_and_docs() {
        let provider = provider("messaging.demo");
        assert_eq!(provider_kind(&provider), "greentic:provider/schema-core");

        let summary = summarize_provider(&provider);
        assert!(summary.contains("caps:2"));
        assert!(summary.contains("ops:1"));
        assert!(summary.contains("docs:docs/provider.md"));
    }

    #[test]
    fn validate_local_refs_reports_missing_local_files_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("schemas")).expect("create schemas dir");
        std::fs::write(temp.path().join("schemas/provider.json"), "{}").expect("write schema");
        let inline = ProviderExtensionInline {
            providers: vec![provider("messaging.demo")],
            additional_fields: Default::default(),
        };
        let pack = LoadedPack {
            manifest: manifest_with_providers(Vec::new()),
            root_dir: Some(temp.path().to_path_buf()),
            entries: HashSet::from(["docs/provider.md".to_string()]),
            _temp: None,
        };

        let warnings = validate_local_refs(&inline, &pack);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("state_schema_ref"));
        assert!(warnings[0].contains("schemas/state.json"));
    }

    #[test]
    fn normalize_entry_and_is_local_ref_handle_mixed_paths() {
        assert_eq!(
            normalize_entry("schemas\\\\provider.json"),
            "schemas/provider.json"
        );
        assert!(is_local_ref("docs/provider.md"));
        assert!(!is_local_ref("oci://registry/provider"));
    }
}
