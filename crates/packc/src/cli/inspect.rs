#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use greentic_pack::builder::PackManifest;
use greentic_pack::{SigningPolicy, VerifyReport, open_pack};
use tempfile::TempDir;

use crate::build;
use crate::runtime::RuntimeContext;

#[derive(Debug, Parser)]
pub struct InspectArgs {
    /// Path to a compiled .gtpack archive
    #[arg(long, value_name = "FILE", conflicts_with = "input")]
    pub pack: Option<PathBuf>,

    /// Path to a pack source directory containing pack.yaml
    #[arg(long = "in", value_name = "DIR", conflicts_with = "pack")]
    pub input: Option<PathBuf>,

    /// Allow OCI component refs in extensions to be tag-based (default requires sha256 digest)
    #[arg(long = "allow-oci-tags", default_value_t = false)]
    pub allow_oci_tags: bool,
}

pub fn handle(args: InspectArgs, json: bool, runtime: &RuntimeContext) -> Result<()> {
    let provided = args.pack.is_some() as u8 + args.input.is_some() as u8;
    if provided != 1 {
        bail!("exactly one of --pack or --in must be supplied");
    }

    let (manifest, report) = if let Some(path) = args.pack {
        inspect_pack_file(&path)?
    } else {
        inspect_source_dir(args.input.as_ref().unwrap(), runtime, args.allow_oci_tags)?
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&manifest)?);
        return Ok(());
    }

    print_human(&manifest, &report);
    Ok(())
}

fn inspect_pack_file(path: &Path) -> Result<(PackManifest, VerifyReport)> {
    let load = open_pack(path, SigningPolicy::DevOk)
        .map_err(|err| anyhow!(err.message))
        .with_context(|| format!("failed to open pack {}", path.display()))?;
    Ok((load.manifest, load.report))
}

fn inspect_source_dir(
    dir: &Path,
    runtime: &RuntimeContext,
    allow_oci_tags: bool,
) -> Result<(PackManifest, VerifyReport)> {
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
        dry_run: false,
        secrets_req: None,
        default_secret_scope: None,
        allow_oci_tags,
        runtime: runtime.clone(),
    };

    build::run(&opts)?;

    inspect_pack_file(&gtpack_out)
}

fn print_human(manifest: &PackManifest, report: &VerifyReport) {
    println!(
        "Pack: {} ({})",
        manifest.meta.pack_id, manifest.meta.version
    );
    println!("Name: {}", manifest.meta.name);
    println!("Components: {}", manifest.components.len());

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
