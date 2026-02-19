use greentic_distributor_client::dist_cli;

#[tokio::main]
async fn main() {
    if let Err(err) = dist_cli::run_from_env().await {
        eprintln!("{}", err.message);
        std::process::exit(err.code);
    }
}
