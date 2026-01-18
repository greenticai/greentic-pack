#![forbid(unsafe_code)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use greentic_pack::validate::{
    ComponentReferencesExistValidator, ProviderReferencesExistValidator,
    ReferencedFilesExistValidator, SbomConsistencyValidator, ValidateCtx, run_validators,
};
use greentic_pack::{PackLoad, SigningPolicy, open_pack};
use greentic_types::component_source::ComponentSourceRef;
use greentic_types::pack::extensions::component_sources::{
    ArtifactLocationV1, ComponentSourcesV1, EXT_COMPONENT_SOURCES_V1,
};
use greentic_types::pack_manifest::PackManifest;
use greentic_types::provider::ProviderDecl;
use greentic_types::validate::{Diagnostic, Severity, ValidationReport};
use serde_json::Value;
use tempfile::TempDir;

use crate::build;
use crate::runtime::RuntimeContext;

#[derive(Debug, Parser)]
pub struct InspectArgs {
    /// Path to a pack (.gtpack or source dir). Defaults to current directory.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Path to a compiled .gtpack archive
    #[arg(long, value_name = "FILE", conflicts_with = "input")]
    pub pack: Option<PathBuf>,

    /// Path to a pack source directory containing pack.yaml
    #[arg(long = "in", value_name = "DIR", conflicts_with = "pack")]
    pub input: Option<PathBuf>,

    /// Force archive inspection (disables auto-detection)
    #[arg(long)]
    pub archive: bool,

    /// Force source inspection (disables auto-detection)
    #[arg(long)]
    pub source: bool,

    /// Allow OCI component refs in extensions to be tag-based (default requires sha256 digest)
    #[arg(long = "allow-oci-tags", default_value_t = false)]
    pub allow_oci_tags: bool,

    /// Output format
    #[arg(long, value_enum, default_value = "human")]
    pub format: InspectFormat,

    /// Enable validation (default)
    #[arg(long, default_value_t = true)]
    pub validate: bool,

    /// Disable validation
    #[arg(long = "no-validate", default_value_t = false)]
    pub no_validate: bool,
}

pub async fn handle(args: InspectArgs, json: bool, runtime: &RuntimeContext) -> Result<()> {
    let mode = resolve_mode(&args)?;
    let format = resolve_format(&args, json);
    let validate_enabled = if args.no_validate {
        false
    } else {
        args.validate
    };

    let load = match mode {
        InspectMode::Archive(path) => inspect_pack_file(&path)?,
        InspectMode::Source(path) => {
            inspect_source_dir(&path, runtime, args.allow_oci_tags).await?
        }
    };
    let validation = if validate_enabled {
        Some(run_pack_validation(&load))
    } else {
        None
    };

    match format {
        InspectFormat::Json => {
            let mut payload = serde_json::json!({
                "manifest": load.manifest,
                "report": {
                    "signature_ok": load.report.signature_ok,
                    "sbom_ok": load.report.sbom_ok,
                    "warnings": load.report.warnings,
                },
                "sbom": load.sbom,
            });
            if let Some(report) = validation.as_ref() {
                payload["validation"] = serde_json::to_value(report)?;
            }
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        InspectFormat::Human => {
            print_human(&load, validation.as_ref());
        }
    }

    if validate_enabled
        && validation
            .as_ref()
            .map(ValidationReport::has_errors)
            .unwrap_or(false)
    {
        bail!("pack validation failed");
    }

    Ok(())
}

fn inspect_pack_file(path: &Path) -> Result<PackLoad> {
    let load = open_pack(path, SigningPolicy::DevOk)
        .map_err(|err| anyhow!(err.message))
        .with_context(|| format!("failed to open pack {}", path.display()))?;
    Ok(load)
}

enum InspectMode {
    Archive(PathBuf),
    Source(PathBuf),
}

fn resolve_mode(args: &InspectArgs) -> Result<InspectMode> {
    if args.archive && args.source {
        bail!("--archive and --source are mutually exclusive");
    }
    if args.pack.is_some() && args.input.is_some() {
        bail!("exactly one of --pack or --in may be supplied");
    }

    if let Some(path) = &args.pack {
        return Ok(InspectMode::Archive(path.clone()));
    }
    if let Some(path) = &args.input {
        return Ok(InspectMode::Source(path.clone()));
    }
    if let Some(path) = &args.path {
        let meta =
            fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
        if args.archive || (path.extension() == Some(std::ffi::OsStr::new("gtpack"))) {
            return Ok(InspectMode::Archive(path.clone()));
        }
        if args.source || meta.is_dir() {
            return Ok(InspectMode::Source(path.clone()));
        }
        if meta.is_file() {
            return Ok(InspectMode::Archive(path.clone()));
        }
    }
    Ok(InspectMode::Source(
        std::env::current_dir().context("determine current directory")?,
    ))
}

async fn inspect_source_dir(
    dir: &Path,
    runtime: &RuntimeContext,
    allow_oci_tags: bool,
) -> Result<PackLoad> {
    let pack_dir = dir
        .canonicalize()
        .with_context(|| format!("failed to resolve pack dir {}", dir.display()))?;

    let temp = TempDir::new().context("failed to allocate temp dir for inspect")?;
    let manifest_out = temp.path().join("manifest.cbor");
    let gtpack_out = temp.path().join("pack.gtpack");

    let opts = build::BuildOptions {
        pack_dir,
        component_out: None,
        manifest_out,
        sbom_out: None,
        gtpack_out: Some(gtpack_out.clone()),
        lock_path: gtpack_out.with_extension("lock.json"), // use temp lock path under temp dir
        bundle: build::BundleMode::Cache,
        dry_run: false,
        secrets_req: None,
        default_secret_scope: None,
        allow_oci_tags,
        require_component_manifests: false,
        runtime: runtime.clone(),
        skip_update: false,
    };

    build::run(&opts).await?;

    inspect_pack_file(&gtpack_out)
}

fn print_human(load: &PackLoad, validation: Option<&ValidationReport>) {
    let manifest = &load.manifest;
    let report = &load.report;
    println!(
        "Pack: {} ({})",
        manifest.meta.pack_id, manifest.meta.version
    );
    println!("Name: {}", manifest.meta.name);
    println!("Flows: {}", manifest.flows.len());
    if manifest.flows.is_empty() {
        println!("Flows list: none");
    } else {
        println!("Flows list:");
        for flow in &manifest.flows {
            println!(
                "  - {} (entry: {}, kind: {})",
                flow.id, flow.entry, flow.kind
            );
        }
    }
    println!("Components: {}", manifest.components.len());
    if manifest.components.is_empty() {
        println!("Components list: none");
    } else {
        println!("Components list:");
        for component in &manifest.components {
            println!("  - {} ({})", component.name, component.version);
        }
    }
    if let Some(gmanifest) = load.gpack_manifest.as_ref()
        && let Some(value) = gmanifest
            .extensions
            .as_ref()
            .and_then(|m| m.get(EXT_COMPONENT_SOURCES_V1))
            .and_then(|ext| ext.inline.as_ref())
            .and_then(|inline| match inline {
                greentic_types::ExtensionInline::Other(v) => Some(v),
                _ => None,
            })
        && let Ok(cs) = ComponentSourcesV1::from_extension_value(value)
    {
        let mut inline = 0usize;
        let mut remote = 0usize;
        let mut oci = 0usize;
        let mut repo = 0usize;
        let mut store = 0usize;
        let mut file = 0usize;
        for entry in &cs.components {
            match entry.artifact {
                ArtifactLocationV1::Inline { .. } => inline += 1,
                ArtifactLocationV1::Remote => remote += 1,
            }
            match entry.source {
                ComponentSourceRef::Oci(_) => oci += 1,
                ComponentSourceRef::Repo(_) => repo += 1,
                ComponentSourceRef::Store(_) => store += 1,
                ComponentSourceRef::File(_) => file += 1,
            }
        }
        println!(
            "Component sources: {} total (origins: oci {}, repo {}, store {}, file {}; artifacts: inline {}, remote {})",
            cs.components.len(),
            oci,
            repo,
            store,
            file,
            inline,
            remote
        );
        if cs.components.is_empty() {
            println!("Component source entries: none");
        } else {
            println!("Component source entries:");
            for entry in &cs.components {
                println!(
                    "  - {} source={} artifact={}",
                    entry.name,
                    format_component_source(&entry.source),
                    format_component_artifact(&entry.artifact)
                );
            }
        }
    } else {
        println!("Component sources: none");
    }

    if let Some(gmanifest) = load.gpack_manifest.as_ref() {
        let providers = providers_from_manifest(gmanifest);
        if providers.is_empty() {
            println!("Providers: none");
        } else {
            println!("Providers:");
            for provider in providers {
                println!(
                    "  - {} ({}) {}",
                    provider.provider_type,
                    provider_kind(&provider),
                    summarize_provider(&provider)
                );
            }
        }
    } else {
        println!("Providers: none");
    }

    if !report.warnings.is_empty() {
        println!("Warnings:");
        for warning in &report.warnings {
            println!("  - {}", warning);
        }
    }

    if let Some(report) = validation {
        print_validation(report);
    }
}

fn run_pack_validation(load: &PackLoad) -> ValidationReport {
    let ctx = ValidateCtx::from_pack_load(load);
    let validators: Vec<Box<dyn greentic_types::validate::PackValidator>> = vec![
        Box::new(ReferencedFilesExistValidator::new(ctx.clone())),
        Box::new(SbomConsistencyValidator::new(ctx.clone())),
        Box::new(ProviderReferencesExistValidator::new(ctx.clone())),
        Box::new(ComponentReferencesExistValidator::default()),
    ];

    if let Some(manifest) = load.gpack_manifest.as_ref() {
        run_validators(manifest, &ctx, &validators)
    } else {
        ValidationReport {
            pack_id: None,
            pack_version: None,
            diagnostics: vec![Diagnostic {
                severity: Severity::Error,
                code: "PACK_MANIFEST_MISSING".to_string(),
                message: "Pack manifest could not be decoded for validation.".to_string(),
                path: Some("manifest.cbor".to_string()),
                hint: Some("Rebuild the pack to regenerate manifest.cbor.".to_string()),
                data: Value::Null,
            }],
        }
    }
}

fn print_validation(report: &ValidationReport) {
    let (info, warn, error) = validation_counts(report);
    println!("Validation:");
    println!("  Info: {info} Warn: {warn} Error: {error}");
    if report.diagnostics.is_empty() {
        println!("  - none");
        return;
    }
    for diag in &report.diagnostics {
        let sev = match diag.severity {
            Severity::Info => "INFO",
            Severity::Warn => "WARN",
            Severity::Error => "ERROR",
        };
        if let Some(path) = diag.path.as_deref() {
            println!("  - [{sev}] {} {} - {}", diag.code, path, diag.message);
        } else {
            println!("  - [{sev}] {} - {}", diag.code, diag.message);
        }
        if let Some(hint) = diag.hint.as_deref() {
            println!("    hint: {hint}");
        }
    }
}

fn validation_counts(report: &ValidationReport) -> (usize, usize, usize) {
    let mut info = 0;
    let mut warn = 0;
    let mut error = 0;
    for diag in &report.diagnostics {
        match diag.severity {
            Severity::Info => info += 1,
            Severity::Warn => warn += 1,
            Severity::Error => error += 1,
        }
    }
    (info, warn, error)
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum InspectFormat {
    Human,
    Json,
}

fn resolve_format(args: &InspectArgs, json: bool) -> InspectFormat {
    if json {
        InspectFormat::Json
    } else {
        args.format
    }
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

fn format_component_source(source: &ComponentSourceRef) -> String {
    match source {
        ComponentSourceRef::Oci(value) => format_source_ref("oci", value),
        ComponentSourceRef::Repo(value) => format_source_ref("repo", value),
        ComponentSourceRef::Store(value) => format_source_ref("store", value),
        ComponentSourceRef::File(value) => format_source_ref("file", value),
    }
}

fn format_source_ref(scheme: &str, value: &str) -> String {
    if value.contains("://") {
        value.to_string()
    } else {
        format!("{scheme}://{value}")
    }
}

fn format_component_artifact(artifact: &ArtifactLocationV1) -> String {
    match artifact {
        ArtifactLocationV1::Inline { wasm_path, .. } => format!("inline ({})", wasm_path),
        ArtifactLocationV1::Remote => "remote".to_string(),
    }
}
