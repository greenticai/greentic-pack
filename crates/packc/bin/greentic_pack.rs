use clap::Parser;
use packc::cli::{self, Cli};
use tokio::runtime::Builder;

fn main() -> anyhow::Result<()> {
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    // Accept `inspect` as a compatibility alias for one release.
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
        packc::telemetry::install("greentic-pack")?;
        packc::telemetry::with_task_local(
            async move { cli::run_with_cli(cli, inspect_alias).await },
        )
        .await
    })
}
