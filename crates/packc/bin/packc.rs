use clap::Parser;
use packc::cli::{self, Cli};
use tokio::runtime::Builder;

fn main() -> anyhow::Result<()> {
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    // Detect `inspect` alias and rewrite to `doctor` for compatibility warnings.
    let mut inspect_alias = false;
    for arg in args.iter_mut().skip(1) {
        if let Some(stripped) = arg.to_str() {
            if stripped.starts_with('-') {
                continue;
            }
            if stripped == "inspect" {
                *arg = std::ffi::OsString::from("doctor");
                inspect_alias = true;
            }
            break;
        }
    }
    let cli = Cli::parse_from(args);
    let env_filter = cli::resolve_env_filter(&cli);

    if std::env::var_os("RUST_LOG").is_none() {
        unsafe {
            std::env::set_var("RUST_LOG", &env_filter);
        }
    }

    let rt = Builder::new_multi_thread().enable_all().build()?;

    rt.block_on(async move {
        packc::telemetry::install("packc")?;
        if std::env::var("GREENTIC_SILENCE_DEPRECATION_WARNINGS")
            .map(|v| v == "1")
            .unwrap_or(false)
        {
            // no-op
        } else {
            eprintln!("WARNING: `packc` is deprecated and will be removed in the next release. Use `greentic-pack` instead.");
        }
        packc::telemetry::with_task_local(async move {
            cli::run_with_cli(cli, inspect_alias).await
        })
        .await
    })
}
