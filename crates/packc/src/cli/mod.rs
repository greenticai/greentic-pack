#![forbid(unsafe_code)]

use std::{convert::TryFrom, ffi::OsString, path::PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use greentic_types::{EnvId, TenantCtx, TenantId};
use tokio::runtime::Runtime;

pub mod add_extension;
pub mod components;
pub mod config;
pub(crate) mod doctor_dw;
pub mod ext_resolver;
pub mod extensions_lock;
pub(crate) mod flow_doctor;
pub mod gui;
pub mod info;
pub mod info_cmd;
pub mod input;
pub mod inspect;
pub mod inspect_lock;
pub mod lint;
pub mod plan;
pub mod providers;
pub mod publish_agent;
pub mod qa;
pub mod resolve;
pub mod sign;
pub mod update;
pub mod verify;
pub mod wizard;
mod wizard_catalog;
mod wizard_i18n;
mod wizard_ui;

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
    /// Describe a .gtpack: show name, version, components, and metadata.
    Info {
        /// Path to a .gtpack file.
        #[arg(value_name = "PATH")]
        path: std::path::PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = self::inspect::InspectFormat::Human)]
        format: self::inspect::InspectFormat,
        /// Fail if unsigned or signature invalid.
        #[arg(long, default_value_t = false)]
        strict: bool,
    },
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
    /// Resolve extension dependency refs into pack.extensions.lock.json
    ExtensionsLock(self::extensions_lock::ExtensionsLockArgs),
    /// Interactive pack wizard.
    Wizard(self::wizard::WizardArgs),
    /// Resolve component references and write pack.lock.cbor
    Resolve(self::resolve::ResolveArgs),
    /// Publish a built dw-application pack to the store as an AgenticWorker.
    PublishAgent(self::publish_agent::PublishAgentArgs),
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
    let cli = parse_cli_from_env();
    Runtime::new()?.block_on(run_with_cli(cli, false))
}

pub fn parse_cli_from_env() -> Cli {
    let args: Vec<OsString> = std::env::args_os().collect();
    parse_cli_from_args(args)
}

pub fn parse_cli_from_args(args: Vec<OsString>) -> Cli {
    let (rewritten, wizard_schema_requested) = rewrite_wizard_schema_flags(args);
    self::wizard::set_forced_schema_flag(wizard_schema_requested);
    Cli::parse_from(rewritten)
}

fn rewrite_wizard_schema_flags(args: Vec<OsString>) -> (Vec<OsString>, bool) {
    let mut saw_wizard = false;
    let mut schema_requested = false;
    let mut rewritten = Vec::with_capacity(args.len());

    for arg in args {
        if arg == "wizard" {
            saw_wizard = true;
            rewritten.push(arg);
            continue;
        }
        if saw_wizard && arg == "--schema" {
            schema_requested = true;
            continue;
        }
        rewritten.push(arg);
    }

    (rewritten, schema_requested)
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
    println!("{}", crate::cli_i18n::t("cli.help.command.extensions_lock"));
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
        [a] if a == "extensions-lock" => "cli.help.page.extensions_lock",
        [a] if a == "wizard" => "cli.help.page.wizard",
        [a, b] if a == "wizard" && b == "run" => "cli.help.page.wizard_run",
        [a, b] if a == "wizard" && b == "validate" => "cli.help.page.wizard_validate",
        [a, b] if a == "wizard" && b == "apply" => "cli.help.page.wizard_apply",
        [a] if a == "resolve" => "cli.help.page.resolve",
        [a, b] if a == "gui" && b == "loveable-convert" => "cli.help.page.gui_loveable_convert",
        [a, b] if a == "providers" && b == "list" => "cli.help.page.providers_list",
        [a, b] if a == "providers" && b == "info" => "cli.help.page.providers_info",
        [a, b] if a == "providers" && b == "validate" => "cli.help.page.providers_validate",
        [a, b] if a == "add-extension" && b == "provider" => "cli.help.page.add_extension_provider",
        [a, b] if a == "add-extension" && b == "capability" => {
            "cli.help.page.add_extension_capability"
        }
        [a, b] if a == "add-extension" && b == "deployer" => "cli.help.page.add_extension_deployer",
        [a, b] if a == "add-extension" && b == "dependency" => {
            "cli.help.page.add_extension_dependency"
        }
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
    let wizard_locale = cli.locale.clone();
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
        Command::Info {
            path,
            format,
            strict,
        } => {
            // Honour the global `--json` flag as a shortcut for `--format json`.
            let effective_format = if cli.json {
                self::inspect::InspectFormat::Json
            } else {
                format
            };
            match self::info_cmd::handle(&path, effective_format, strict) {
                Ok(()) => {}
                Err(err) => {
                    let msg = err.to_string();
                    let code = if msg.starts_with(self::info_cmd::ERR_NOT_A_PACK) {
                        2
                    } else if msg.starts_with(self::info_cmd::ERR_STRICT_UNSIGNED) {
                        3
                    } else {
                        1
                    };
                    eprintln!("{msg}");
                    std::process::exit(code);
                }
            }
        }
        Command::InspectLock(args) => self::inspect_lock::handle(args)?,
        Command::Qa(args) => self::qa::handle(args, &runtime)?,
        Command::Config(args) => self::config::handle(args, cli.json, &runtime)?,
        Command::Plan(args) => self::plan::handle(&args)?,
        Command::Providers(cmd) => self::providers::run(cmd)?,
        Command::AddExtension(cmd) => self::add_extension::handle(cmd)?,
        Command::ExtensionsLock(args) => {
            self::extensions_lock::handle(args, &runtime, true).await?
        }
        Command::Wizard(args) => self::wizard::handle(args, &runtime, wizard_locale.as_deref())?,
        Command::Resolve(args) => self::resolve::handle(args, &runtime, true).await?,
        Command::PublishAgent(args) => self::publish_agent::run(args).await?,
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parse_build_populates_defaults() {
        let cli = Cli::parse_from(["greentic-pack", "build", "--in", "demo-pack"]);
        assert_eq!(cli.verbosity, "info");
        assert!(!cli.offline);
        assert!(!cli.json);
        assert!(matches!(
            cli.command,
            Command::Build(BuildArgs {
                input,
                no_update: false,
                dry_run: false,
                allow_oci_tags: false,
                require_component_manifests: false,
                no_extra_dirs: false,
                dev: false,
                allow_pack_schema: false,
                ..
            }) if input.as_path() == std::path::Path::new("demo-pack")
        ));
    }

    #[test]
    fn cli_parse_nested_subcommands_and_globals() {
        let cli = Cli::parse_from([
            "greentic-pack",
            "--json",
            "--offline",
            "--locale",
            "nl",
            "providers",
            "validate",
        ]);
        assert!(cli.json);
        assert!(cli.offline);
        assert_eq!(cli.locale.as_deref(), Some("nl"));
        assert!(matches!(
            cli.command,
            Command::Providers(self::providers::ProvidersCommand::Validate(_))
        ));
    }

    #[test]
    fn print_help_for_known_paths_returns_true() {
        crate::cli_i18n::init_locale(Some("en"));

        assert!(print_help_for_path(&[]));
        assert!(print_help_for_path(&["build".to_string()]));
        assert!(print_help_for_path(&[
            "wizard".to_string(),
            "run".to_string()
        ]));
        assert!(print_help_for_path(&[
            "providers".to_string(),
            "validate".to_string()
        ]));
        assert!(print_help_for_path(&[
            "add-extension".to_string(),
            "dependency".to_string()
        ]));
    }

    #[test]
    fn print_help_for_unknown_paths_returns_false() {
        crate::cli_i18n::init_locale(Some("en"));
        assert!(!print_help_for_path(&["does-not-exist".to_string()]));
        assert!(!print_help_for_path(&[
            "wizard".to_string(),
            "missing".to_string()
        ]));
    }

    #[test]
    fn localized_wizard_help_mentions_schema_option() {
        let en_catalog: serde_json::Value =
            serde_json::from_str(include_str!("../../i18n/en.json")).expect("valid English i18n");
        let en_wizard = en_catalog["cli.help.page.wizard"]
            .as_str()
            .expect("English wizard help string");
        let en_run = en_catalog["cli.help.page.wizard_run"]
            .as_str()
            .expect("English wizard run help string");
        assert!(en_wizard.contains("--schema"));
        assert!(en_run.contains("--schema"));

        let nl_catalog: serde_json::Value =
            serde_json::from_str(include_str!("../../i18n/nl.json")).expect("valid Dutch i18n");
        let nl_wizard = nl_catalog["cli.help.page.wizard"]
            .as_str()
            .expect("Dutch wizard help string");
        let nl_run = nl_catalog["cli.help.page.wizard_run"]
            .as_str()
            .expect("Dutch wizard run help string");
        assert!(nl_wizard.contains("--schema"));
        assert!(nl_run.contains("--schema"));
        assert!(nl_wizard.contains("AnswerDocument-schema"));
        assert!(nl_run.contains("AnswerDocument-schema"));
    }

    #[test]
    fn print_top_level_help_does_not_panic() {
        crate::cli_i18n::init_locale(Some("en"));
        print_top_level_help();
    }

    #[test]
    fn resolve_env_filter_uses_cli_verbosity_when_env_missing() {
        let cli = Cli::parse_from(["greentic-pack", "--log", "debug", "build", "--in", "demo"]);
        assert_eq!(resolve_env_filter(&cli), "debug");
    }

    #[test]
    fn rewrite_wizard_schema_flags_strips_schema_after_wizard() {
        let (rewritten, schema_requested) = rewrite_wizard_schema_flags(vec![
            "greentic-pack".into(),
            "--locale".into(),
            "nl".into(),
            "wizard".into(),
            "run".into(),
            "--schema".into(),
            "--answers".into(),
            "answers.json".into(),
        ]);

        assert!(schema_requested);
        assert_eq!(
            rewritten,
            vec![
                OsString::from("greentic-pack"),
                OsString::from("--locale"),
                OsString::from("nl"),
                OsString::from("wizard"),
                OsString::from("run"),
                OsString::from("--answers"),
                OsString::from("answers.json"),
            ]
        );
    }

    #[test]
    fn rewrite_wizard_schema_flags_leaves_other_schema_flags_alone() {
        let (rewritten, schema_requested) = rewrite_wizard_schema_flags(vec![
            "greentic-pack".into(),
            "build".into(),
            "--schema".into(),
        ]);

        assert!(!schema_requested);
        assert_eq!(
            rewritten,
            vec![
                OsString::from("greentic-pack"),
                OsString::from("build"),
                OsString::from("--schema"),
            ]
        );
    }
}
