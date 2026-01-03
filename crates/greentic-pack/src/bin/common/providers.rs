use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use greentic_types::pack_manifest::{PackManifest, PackSignatures};
use greentic_types::provider::{ProviderDecl, ProviderExtensionInline};
use greentic_types::{PackId, PackKind, decode_pack_manifest};
use tempfile::TempDir;
use zip::ZipArchive;

use crate::input::materialize_pack_path;

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
        println!("No providers declared.");
        return Ok(());
    }

    println!("{:<24} {:<28} {:<16} DETAILS", "ID", "RUNTIME", "KIND");
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
        None => bail!("provider extension not present"),
    };
    let Some(provider) = inline
        .providers
        .iter()
        .find(|p| p.provider_type == args.provider_id)
    else {
        bail!("provider `{}` not found", args.provider_id);
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
                    "status": "ok",
                    "providers_present": false,
                    "warnings": [],
                }))?
            );
        } else {
            println!("providers valid (extension not present)");
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
                "status": "ok",
                "providers_present": true,
                "warnings": warnings,
            }))?
        );
    } else if warnings.is_empty() {
        println!("providers valid");
    } else {
        println!("providers valid with warnings:");
        for warning in warnings {
            println!("  - {warning}");
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
        .by_name("manifest.cbor")
        .context("manifest.cbor missing from archive")?;
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
        .flat_map(|part| part.split('/'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}
