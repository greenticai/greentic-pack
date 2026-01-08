#![forbid(unsafe_code)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use greentic_pack::{PackLoad, SigningPolicy, open_pack};
use greentic_types::pack::extensions::component_sources::{
    ArtifactLocationV1, ComponentSourcesV1, EXT_COMPONENT_SOURCES_V1,
};
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
}

pub async fn handle(args: InspectArgs, json: bool, runtime: &RuntimeContext) -> Result<()> {
    let mode = resolve_mode(&args)?;

    let load = match mode {
        InspectMode::Archive(path) => inspect_pack_file(&path)?,
        InspectMode::Source(path) => {
            inspect_source_dir(&path, runtime, args.allow_oci_tags).await?
        }
    };

    if json {
        let payload = serde_json::json!({
            "manifest": load.manifest,
            "report": {
                "signature_ok": load.report.signature_ok,
                "sbom_ok": load.report.sbom_ok,
                "warnings": load.report.warnings,
            },
            "sbom": load.sbom,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    print_human(&load);
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
        runtime: runtime.clone(),
        skip_update: false,
    };

    build::run(&opts).await?;

    inspect_pack_file(&gtpack_out)
}

fn print_human(load: &PackLoad) {
    let manifest = &load.manifest;
    let report = &load.report;
    println!(
        "Pack: {} ({})",
        manifest.meta.pack_id, manifest.meta.version
    );
    println!("Name: {}", manifest.meta.name);
    println!("Components: {}", manifest.components.len());
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
        for entry in &cs.components {
            match entry.artifact {
                ArtifactLocationV1::Inline { .. } => inline += 1,
                ArtifactLocationV1::Remote => remote += 1,
            }
        }
        println!(
            "Component sources: {} inline, {} remote (digests required)",
            inline, remote
        );
    }

    if let Some(msg) = manifest
        .meta
        .messaging
        .as_ref()
        .and_then(|m| m.adapters.as_ref())
    {
        if msg.is_empty() {
            println!("Messaging adapters: none");
        } else {
            println!("Messaging adapters:");
            for adapter in msg {
                let flow = adapter
                    .default_flow
                    .as_deref()
                    .or(adapter.custom_flow.as_deref())
                    .unwrap_or("-");
                println!(
                    "  - {} ({:?}) component={} flow={}",
                    adapter.name, adapter.kind, adapter.component, flow
                );
            }
        }
    } else {
        println!("Messaging adapters: none");
    }

    if !report.warnings.is_empty() {
        println!("Warnings:");
        for warning in &report.warnings {
            println!("  - {}", warning);
        }
    }
}
