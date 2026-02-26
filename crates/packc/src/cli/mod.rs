#![forbid(unsafe_code)]

use std::{convert::TryFrom, path::PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use greentic_types::{EnvId, TenantCtx, TenantId};
use tokio::runtime::Runtime;

pub mod add_extension;
pub mod components;
pub mod config;
pub mod gui;
pub mod input;
pub mod inspect;
pub mod inspect_lock;
pub mod lint;
pub mod plan;
pub mod providers;
pub mod qa;
pub mod resolve;
pub mod sign;
pub mod update;
pub mod verify;
pub mod wizard;

use crate::telemetry::set_current_tenant_ctx;
use crate::{build, new, runtime};

#[derive(Debug, Parser)]
#[command(name = "greentic-pack", about = "Greentic pack CLI", version)]
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

    /// Locale used for CLI messages (fallback: LC_ALL/LC_MESSAGES/LANG/system/en)
    #[arg(long, global = true)]
    pub locale: Option<String>,

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
    /// Diagnose a pack archive (.gtpack) or source directory (runs validation)
    Doctor(self::inspect::InspectArgs),
    /// Deprecated alias for `doctor`
    Inspect(self::inspect::InspectArgs),
    /// Inspect pack.lock.cbor (stable JSON to stdout)
    InspectLock(self::inspect_lock::InspectLockArgs),
    /// Run component QA and store answers.
    Qa(self::qa::QaArgs),
    /// Inspect resolved configuration (provenance and warnings)
    Config(self::config::ConfigArgs),
    /// Generate a DeploymentPlan from a pack archive or source directory.
    Plan(self::plan::PlanArgs),
    /// Legacy provider-extension helpers (schema-core path).
    #[command(subcommand)]
    Providers(self::providers::ProvidersCommand),
    /// Add data to pack extensions (provider extension path is legacy/schema-core).
    #[command(subcommand)]
    AddExtension(self::add_extension::AddExtensionCommand),
    /// Pack wizard helpers.
    #[command(subcommand)]
    Wizard(self::wizard::WizardCommand),
    /// Resolve component references and write pack.lock.cbor
    Resolve(self::resolve::ResolveArgs),
}

#[derive(Debug, Clone, Parser)]
pub struct BuildArgs {
    /// Root directory of the pack (must contain pack.yaml)
    #[arg(long = "in", value_name = "DIR")]
    pub input: PathBuf,

    /// Skip running `packc update` before building (default: update first)
    #[arg(long = "no-update", default_value_t = false)]
    pub no_update: bool,

    /// Output path for the built Wasm component (legacy; writes a stub)
    #[arg(long = "out", value_name = "FILE")]
    pub component_out: Option<PathBuf>,

    /// Output path for the generated manifest (CBOR); defaults to dist/manifest.cbor
    #[arg(long, value_name = "FILE")]
    pub manifest: Option<PathBuf>,

    /// Output path for the generated SBOM (legacy; writes a stub JSON)
    #[arg(long, value_name = "FILE")]
    pub sbom: Option<PathBuf>,

    /// Output path for the generated & canonical .gtpack archive (default: dist/<pack_dir>.gtpack)
    #[arg(long = "gtpack-out", value_name = "FILE")]
    pub gtpack_out: Option<PathBuf>,

    /// Optional path to pack.lock.cbor (default: <pack_dir>/pack.lock.cbor)
    #[arg(long = "lock", value_name = "FILE")]
    pub lock: Option<PathBuf>,

    /// Bundle strategy for component artifacts (cache=embed wasm, none=refs only)
    #[arg(long = "bundle", value_enum, default_value = "cache")]
    pub bundle: crate::build::BundleMode,

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

    /// Require manifest metadata for flow-referenced components
    #[arg(long, default_value_t = false)]
    pub require_component_manifests: bool,

    /// Skip auto-including extra directories (e.g. schemas/, templates/)
    #[arg(long = "no-extra-dirs", default_value_t = false)]
    pub no_extra_dirs: bool,

    /// Include source files (pack.yaml, flows) inside the generated .gtpack for debugging
    #[arg(long = "dev", default_value_t = false)]
    pub dev: bool,

    /// Migration-only escape hatch: allow deriving component manifest/schema from pack.yaml.
    #[arg(long = "allow-pack-schema", default_value_t = false)]
    pub allow_pack_schema: bool,
}

pub fn run() -> Result<()> {
    Runtime::new()?.block_on(run_with_cli(Cli::parse(), false))
}

pub fn print_top_level_help() {
    println!("{}", crate::cli_i18n::t("cli.help.title"));
    println!();
    println!("{}", crate::cli_i18n::t("cli.help.usage"));
    println!();
    println!("{}", crate::cli_i18n::t("cli.help.commands_header"));
    println!("{}", crate::cli_i18n::t("cli.help.command.build"));
    println!("{}", crate::cli_i18n::t("cli.help.command.lint"));
    println!("{}", crate::cli_i18n::t("cli.help.command.components"));
    println!("{}", crate::cli_i18n::t("cli.help.command.update"));
    println!("{}", crate::cli_i18n::t("cli.help.command.new"));
    println!("{}", crate::cli_i18n::t("cli.help.command.sign"));
    println!("{}", crate::cli_i18n::t("cli.help.command.verify"));
    println!("{}", crate::cli_i18n::t("cli.help.command.gui"));
    println!("{}", crate::cli_i18n::t("cli.help.command.doctor"));
    println!("{}", crate::cli_i18n::t("cli.help.command.inspect"));
    println!("{}", crate::cli_i18n::t("cli.help.command.inspect_lock"));
    println!("{}", crate::cli_i18n::t("cli.help.command.qa"));
    println!("{}", crate::cli_i18n::t("cli.help.command.config"));
    println!("{}", crate::cli_i18n::t("cli.help.command.plan"));
    println!("{}", crate::cli_i18n::t("cli.help.command.providers"));
    println!("{}", crate::cli_i18n::t("cli.help.command.add_extension"));
    println!("{}", crate::cli_i18n::t("cli.help.command.wizard"));
    println!("{}", crate::cli_i18n::t("cli.help.command.resolve"));
    println!("{}", crate::cli_i18n::t("cli.help.command.help"));
    println!();
    println!("{}", crate::cli_i18n::t("cli.help.options_header"));
    println!("{}", crate::cli_i18n::t("cli.help.option.log"));
    println!("{}", crate::cli_i18n::t("cli.help.option.offline"));
    println!("{}", crate::cli_i18n::t("cli.help.option.cache_dir"));
    println!("{}", crate::cli_i18n::t("cli.help.option.config_override"));
    println!("{}", crate::cli_i18n::t("cli.help.option.json"));
    println!("{}", crate::cli_i18n::t("cli.help.option.locale"));
    println!("{}", crate::cli_i18n::t("cli.help.option.help"));
    println!("{}", crate::cli_i18n::t("cli.help.option.version"));
}

pub fn print_help_for_path(path: &[String]) -> bool {
    let key = match path {
        [] => "cli.help.page.root",
        [a] if a == "build" => "cli.help.page.build",
        [a] if a == "lint" => "cli.help.page.lint",
        [a] if a == "components" => "cli.help.page.components",
        [a] if a == "update" => "cli.help.page.update",
        [a] if a == "new" => "cli.help.page.new",
        [a] if a == "sign" => "cli.help.page.sign",
        [a] if a == "verify" => "cli.help.page.verify",
        [a] if a == "gui" => "cli.help.page.gui",
        [a] if a == "doctor" => "cli.help.page.doctor",
        [a] if a == "inspect" => "cli.help.page.inspect",
        [a] if a == "inspect-lock" => "cli.help.page.inspect_lock",
        [a] if a == "qa" => "cli.help.page.qa",
        [a] if a == "config" => "cli.help.page.config",
        [a] if a == "plan" => "cli.help.page.plan",
        [a] if a == "providers" => "cli.help.page.providers",
        [a] if a == "add-extension" => "cli.help.page.add_extension",
        [a] if a == "wizard" => "cli.help.page.wizard",
        [a] if a == "resolve" => "cli.help.page.resolve",
        [a, b] if a == "gui" && b == "loveable-convert" => "cli.help.page.gui_loveable_convert",
        [a, b] if a == "providers" && b == "list" => "cli.help.page.providers_list",
        [a, b] if a == "providers" && b == "info" => "cli.help.page.providers_info",
        [a, b] if a == "providers" && b == "validate" => "cli.help.page.providers_validate",
        [a, b] if a == "add-extension" && b == "provider" => "cli.help.page.add_extension_provider",
        [a, b] if a == "add-extension" && b == "capability" => {
            "cli.help.page.add_extension_capability"
        }
        [a, b] if a == "wizard" && b == "new-app" => "cli.help.page.wizard_new_app",
        [a, b] if a == "wizard" && b == "new-extension" => "cli.help.page.wizard_new_extension",
        [a, b] if a == "wizard" && b == "add-component" => "cli.help.page.wizard_add_component",
        _ => return false,
    };

    if !crate::cli_i18n::has(key) {
        return false;
    }
    println!("{}", crate::cli_i18n::t(key));
    true
}

/// Resolve the logging filter to use for telemetry initialisation.
pub fn resolve_env_filter(cli: &Cli) -> String {
    std::env::var("PACKC_LOG").unwrap_or_else(|_| cli.verbosity.clone())
}

/// Execute the CLI using a pre-parsed argument set.
pub async fn run_with_cli(cli: Cli, warn_inspect_alias: bool) -> Result<()> {
    crate::cli_i18n::init_locale(cli.locale.as_deref());

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
        Command::Build(args) => {
            build::run(&build::BuildOptions::from_args(args, &runtime)?).await?
        }
        Command::Lint(args) => self::lint::handle(args, cli.json)?,
        Command::Components(args) => self::components::handle(args, cli.json)?,
        Command::Update(args) => self::update::handle(args, cli.json)?,
        Command::New(args) => new::handle(args, cli.json, &runtime).await?,
        Command::Sign(args) => self::sign::handle(args, cli.json)?,
        Command::Verify(args) => self::verify::handle(args, cli.json)?,
        Command::Gui(cmd) => self::gui::handle(cmd, cli.json, &runtime).await?,
        Command::Inspect(args) | Command::Doctor(args) => {
            if warn_inspect_alias {
                eprintln!("{}", crate::cli_i18n::t("cli.warn.inspect_deprecated"));
            }
            self::inspect::handle(args, cli.json, &runtime).await?
        }
        Command::InspectLock(args) => self::inspect_lock::handle(args)?,
        Command::Qa(args) => self::qa::handle(args, &runtime)?,
        Command::Config(args) => self::config::handle(args, cli.json, &runtime)?,
        Command::Plan(args) => self::plan::handle(&args)?,
        Command::Providers(cmd) => self::providers::run(cmd)?,
        Command::AddExtension(cmd) => self::add_extension::handle(cmd)?,
        Command::Wizard(cmd) => self::wizard::handle(cmd, &runtime)?,
        Command::Resolve(args) => self::resolve::handle(args, &runtime, true).await?,
    }

    Ok(())
}
