#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result, anyhow};
use greentic_pack::plan::infer_base_deployment_plan;
use greentic_pack::reader::{PackLoad, SigningPolicy, open_pack};
use greentic_types::component::ComponentManifest;
use greentic_types::{EnvId, SecretRequirement, TenantCtx, TenantId};
use zip::ZipArchive;

use crate::cli::input::materialize_pack_path;

#[derive(Debug, clap::Args)]
pub struct PlanArgs {
    /// Path to a .gtpack archive or pack source directory.
    #[arg(value_name = "PATH")]
    pub input: std::path::PathBuf,

    /// Tenant identifier to embed in the plan.
    #[arg(long, default_value = "tenant-local")]
    pub tenant: String,

    /// Environment identifier to embed in the plan.
    #[arg(long, default_value = "local")]
    pub environment: String,

    /// Emit compact JSON output instead of pretty-printing.
    #[arg(long)]
    pub json: bool,

    /// When set, print additional diagnostics (for directory builds).
    #[arg(long)]
    pub verbose: bool,
}

pub fn handle(args: &PlanArgs) -> Result<()> {
    let (temp, pack_path) = materialize_pack_path(&args.input, args.verbose)?;
    let tenant_ctx = build_tenant_ctx(&args.environment, &args.tenant)?;
    let plan = plan_for_pack(&pack_path, &tenant_ctx, &args.environment)?;

    if args.json {
        println!("{}", serde_json::to_string(&plan)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    }

    drop(temp);
    Ok(())
}

fn plan_for_pack(
    path: &Path,
    tenant: &TenantCtx,
    environment: &str,
) -> Result<greentic_types::deployment::DeploymentPlan> {
    let load = open_pack(path, SigningPolicy::DevOk).map_err(|err| anyhow!(err.message))?;
    let connectors = load.manifest.meta.annotations.get("connectors");
    let components = load_component_manifests(&load)?;
    let secret_requirements = load_secret_requirements(path).unwrap_or(None);

    Ok(infer_base_deployment_plan(
        &load.manifest.meta,
        &load.manifest.flows,
        connectors,
        &components,
        secret_requirements,
        tenant,
        environment,
    ))
}

fn build_tenant_ctx(environment: &str, tenant: &str) -> Result<TenantCtx> {
    let env_id = EnvId::from_str(environment)
        .with_context(|| format!("invalid environment id `{}`", environment))?;
    let tenant_id =
        TenantId::from_str(tenant).with_context(|| format!("invalid tenant id `{}`", tenant))?;
    Ok(TenantCtx::new(env_id, tenant_id))
}

fn load_component_manifests(load: &PackLoad) -> Result<HashMap<String, ComponentManifest>> {
    let mut manifests = HashMap::new();
    for component in &load.manifest.components {
        let id = &component.name;
        if let Some(manifest) = load
            .get_component_manifest_prefer_file(id)
            .with_context(|| format!("failed to load manifest for component `{id}`"))?
        {
            manifests.insert(component.name.clone(), manifest);
        }
    }
    Ok(manifests)
}

fn load_secret_requirements(path: &Path) -> Result<Option<Vec<SecretRequirement>>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut archive = ZipArchive::new(file)
        .with_context(|| format!("{} is not a valid gtpack archive", path.display()))?;

    for name in [
        "assets/secret-requirements.json",
        "secret-requirements.json",
    ] {
        if let Ok(mut entry) = archive.by_name(name) {
            let mut buf = String::new();
            entry
                .read_to_string(&mut buf)
                .context("failed to read secret requirements file")?;
            let reqs: Vec<SecretRequirement> =
                serde_json::from_str(&buf).context("secret requirements file is invalid JSON")?;
            return Ok(Some(reqs));
        }
    }

    Ok(None)
}
