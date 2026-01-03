use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

#[path = "common/input.rs"]
mod input;
#[path = "common/inspect.rs"]
mod inspect;
#[path = "common/plan.rs"]
mod plan_cmd;
#[path = "common/providers.rs"]
mod providers;

#[derive(Parser, Debug)]
#[command(name = "greentic-pack", version, about = "Greentic pack utilities")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Inspect a .gtpack archive and display verification info.
    Inspect(InspectArgs),
    /// Generate a DeploymentPlan from a pack archive or source directory.
    Plan(PlanArgs),
    /// Provider extension helpers.
    #[command(subcommand)]
    Providers(providers::ProvidersCommand),
}

#[derive(Args, Debug)]
struct InspectArgs {
    /// Path to the .gtpack file
    #[arg(value_name = "FILE")]
    path: PathBuf,

    /// Signature policy to enforce
    #[arg(long, value_enum, default_value_t = inspect::PolicyArg::Devok)]
    policy: inspect::PolicyArg,

    /// Emit JSON output
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct PlanArgs {
    /// Path to a .gtpack archive or pack source directory.
    #[arg(value_name = "PATH")]
    input: PathBuf,

    /// Tenant identifier to embed in the plan.
    #[arg(long, default_value = "tenant-local")]
    tenant: String,

    /// Environment identifier to embed in the plan.
    #[arg(long, default_value = "local")]
    environment: String,

    /// Emit compact JSON output instead of pretty-printing.
    #[arg(long)]
    json: bool,

    /// When set, print additional diagnostics (for directory builds).
    #[arg(long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Inspect(args) => inspect::run(&args.path, args.policy, args.json),
        Command::Plan(args) => plan_cmd::run(&args),
        Command::Providers(cmd) => providers::run(cmd),
    }
}
