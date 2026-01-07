use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

#[path = "common/inspect.rs"]
mod inspect;

#[derive(Parser, Debug)]
#[command(
    name = "gtpack-inspect",
    version,
    about = "Inspect Greentic pack archives"
)]
struct Args {
    /// Path to the .gtpack file
    #[arg(value_name = "FILE")]
    path: PathBuf,

    /// Signature policy to enforce
    #[arg(long, value_enum, default_value_t = inspect::PolicyArg::Devok)]
    policy: inspect::PolicyArg,

    /// Emit JSON output
    #[arg(long)]
    json: bool,

    /// Verify component manifest files when present
    #[arg(long)]
    verify_manifest_files: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    inspect::run(
        &args.path,
        args.policy,
        args.json,
        args.verify_manifest_files,
    )
}
