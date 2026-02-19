use greentic_distributor_client::dist_cli;

#[tokio::main]
async fn main() {
    if std::env::var("GREENTIC_SILENCE_DEPRECATION_WARNINGS") != Ok("1".into()) {
        eprintln!(
            "warning: `greentic-distributor-client` CLI is deprecated; use `greentic-dist` instead"
        );
    }
    if let Err(err) = dist_cli::run_from_env().await {
        eprintln!("{}", err.message);
        std::process::exit(err.code);
    }
}
