#![forbid(unsafe_code)]

use std::{convert::TryFrom, path::PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use greentic_types::{EnvId, TenantCtx, TenantId};

pub mod components;
pub mod config;
pub mod gui;
pub mod inspect;
pub mod lint;
pub mod sign;
pub mod update;
pub mod verify;

use crate::telemetry::set_current_tenant_ctx;
use crate::{build, new, runtime};

#[derive(Debug, Parser)]
#[command(name = "packc", about = "Greentic pack builder CLI", version)]
pub struct Cli {
    /// Logging filter (overrides PACKC_LOG)
    #[arg(long = "log", default_value = "info", global = true)]
    pub verbosity: String,

    /// Force offline mode (disables any network activity)
    #[arg(long, global = true)]
    pub offline: bool,

    /// Override cache directory (defaults to pack_dir/.packc or GREENTIC_PACK_CACHE_DIR)
    #[arg(long = "cache-dir", global = true)]
    pub cache_dir: Option<PathBuf>,

    /// Optional config overrides in TOML/JSON (greentic-config layer)
    #[arg(long = "config-override", value_name = "FILE", global = true)]
    pub config_override: Option<PathBuf>,

    /// Emit machine-readable JSON output where applicable
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Build a pack component and supporting artifacts
    Build(BuildArgs),
    /// Lint a pack manifest, flows, and templates
    Lint(self::lint::LintArgs),
    /// Sync pack.yaml components with files under components/
    Components(self::components::ComponentsArgs),
    /// Sync pack.yaml components and flows with files under the pack root
    Update(self::update::UpdateArgs),
    /// Scaffold a new pack directory
    New(new::NewArgs),
    /// Sign a pack manifest using an Ed25519 private key
    Sign(self::sign::SignArgs),
    /// Verify a pack's manifest signature
    Verify(self::verify::VerifyArgs),
    /// GUI-related tooling
    #[command(subcommand)]
    Gui(self::gui::GuiCommand),
    /// Inspect a pack manifest from a .gtpack or source directory
    Inspect(self::inspect::InspectArgs),
    /// Inspect resolved configuration (provenance and warnings)
    Config(self::config::ConfigArgs),
}

#[derive(Debug, Clone, Parser)]
pub struct BuildArgs {
    /// Root directory of the pack (must contain pack.yaml)
    #[arg(long = "in", value_name = "DIR")]
    pub input: PathBuf,

    /// Output path for the built Wasm component (legacy; writes a stub)
    #[arg(long = "out", value_name = "FILE")]
    pub component_out: Option<PathBuf>,

    /// Output path for the generated manifest (CBOR); defaults to dist/manifest.cbor
    #[arg(long, value_name = "FILE")]
    pub manifest: Option<PathBuf>,

    /// Output path for the generated SBOM (legacy; writes a stub JSON)
    #[arg(long, value_name = "FILE")]
    pub sbom: Option<PathBuf>,

    /// Output path for the generated & canonical .gtpack archive
    #[arg(long = "gtpack-out", value_name = "FILE")]
    pub gtpack_out: Option<PathBuf>,

    /// When set, the command validates input without writing artifacts
    #[arg(long)]
    pub dry_run: bool,

    /// Optional JSON file with additional secret requirements (migration bridge)
    #[arg(long = "secrets-req", value_name = "FILE")]
    pub secrets_req: Option<PathBuf>,

    /// Default secret scope to apply when missing (dev-only), format: env/tenant[/team]
    #[arg(long = "default-secret-scope", value_name = "ENV/TENANT[/TEAM]")]
    pub default_secret_scope: Option<String>,

    /// Allow OCI component refs in extensions to be tag-based (default requires sha256 digest)
    #[arg(long = "allow-oci-tags", default_value_t = false)]
    pub allow_oci_tags: bool,
}

pub fn run() -> Result<()> {
    run_with_cli(Cli::parse())
}

/// Resolve the logging filter to use for telemetry initialisation.
pub fn resolve_env_filter(cli: &Cli) -> String {
    std::env::var("PACKC_LOG").unwrap_or_else(|_| cli.verbosity.clone())
}

/// Execute the CLI using a pre-parsed argument set.
pub fn run_with_cli(cli: Cli) -> Result<()> {
    let runtime = runtime::resolve_runtime(
        Some(std::env::current_dir()?.as_path()),
        cli.cache_dir.as_deref(),
        cli.offline,
        cli.config_override.as_deref(),
    )?;

    // Install telemetry according to resolved config.
    crate::telemetry::install_with_config("packc", &runtime.resolved.config.telemetry)?;

    set_current_tenant_ctx(&TenantCtx::new(
        EnvId::try_from("local").expect("static env id"),
        TenantId::try_from("packc").expect("static tenant id"),
    ));

    match cli.command {
        Command::Build(args) => build::run(&build::BuildOptions::from_args(args, &runtime)?)?,
        Command::Lint(args) => self::lint::handle(args, cli.json)?,
        Command::Components(args) => self::components::handle(args, cli.json)?,
        Command::Update(args) => self::update::handle(args, cli.json)?,
        Command::New(args) => new::handle(args, cli.json)?,
        Command::Sign(args) => self::sign::handle(args, cli.json)?,
        Command::Verify(args) => self::verify::handle(args, cli.json)?,
        Command::Gui(cmd) => self::gui::handle(cmd, cli.json, &runtime)?,
        Command::Inspect(args) => self::inspect::handle(args, cli.json, &runtime)?,
        Command::Config(args) => self::config::handle(args, cli.json, &runtime)?,
    }

    Ok(())
}
