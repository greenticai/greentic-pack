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

    let locale = extract_locale_arg(&args);
    if let Some(help_path) = extract_help_path(&args) {
        packc::cli_i18n::init_locale(locale.as_deref());
        if cli::print_help_for_path(&help_path) {
            return Ok(());
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

fn extract_help_path(args: &[std::ffi::OsString]) -> Option<Vec<String>> {
    let mut has_help_flag = false;
    let mut path: Vec<String> = Vec::new();
    let mut idx = 1usize;

    while idx < args.len() {
        let text = args[idx].to_str()?;
        if text == "--help" || text == "-h" {
            has_help_flag = true;
            idx += 1;
            continue;
        }
        if is_global_option_with_inline_value(text) {
            idx += 1;
            continue;
        }
        if is_global_option_with_value(text) {
            idx += 2;
            continue;
        }
        if text.starts_with('-') {
            idx += 1;
            continue;
        }

        if text == "help" {
            // `greentic-pack help <path...>`
            idx += 1;
            while idx < args.len() {
                let t = args[idx].to_str()?;
                if t == "--help" || t == "-h" {
                    idx += 1;
                    continue;
                }
                if is_global_option_with_inline_value(t) {
                    idx += 1;
                    continue;
                }
                if is_global_option_with_value(t) {
                    idx += 2;
                    continue;
                }
                if !t.starts_with('-') {
                    push_known_help_segment(&mut path, t);
                }
                idx += 1;
            }
            return Some(path);
        }

        push_known_help_segment(&mut path, text);
        idx += 1;
    }

    if has_help_flag {
        return Some(path);
    }
    None
}

fn is_global_option_with_inline_value(text: &str) -> bool {
    text.starts_with("--locale=")
        || text.starts_with("--log=")
        || text.starts_with("--cache-dir=")
        || text.starts_with("--config-override=")
}

fn is_global_option_with_value(text: &str) -> bool {
    text == "--locale" || text == "--log" || text == "--cache-dir" || text == "--config-override"
}

fn push_known_help_segment(path: &mut Vec<String>, segment: &str) {
    if path.is_empty() {
        if is_known_root_command(segment) {
            path.push(segment.to_string());
        }
        return;
    }

    if path.len() == 1 && is_known_child_command(&path[0], segment) {
        path.push(segment.to_string());
    }
}

fn is_known_root_command(command: &str) -> bool {
    matches!(
        command,
        "build"
            | "lint"
            | "components"
            | "update"
            | "new"
            | "sign"
            | "verify"
            | "gui"
            | "doctor"
            | "inspect"
            | "inspect-lock"
            | "qa"
            | "config"
            | "plan"
            | "providers"
            | "add-extension"
            | "wizard"
            | "resolve"
    )
}

fn is_known_child_command(root: &str, child: &str) -> bool {
    matches!(
        (root, child),
        ("gui", "loveable-convert")
            | ("providers", "list")
            | ("providers", "info")
            | ("providers", "validate")
            | ("add-extension", "provider")
            | ("add-extension", "capability")
            | ("wizard", "new-app")
            | ("wizard", "new-extension")
            | ("wizard", "add-component")
    )
}

fn extract_locale_arg(args: &[std::ffi::OsString]) -> Option<String> {
    let mut idx = 1usize;
    while idx < args.len() {
        let text = args[idx].to_str()?;
        if let Some((_, value)) = text.split_once("--locale=")
            && !value.trim().is_empty()
        {
            return Some(value.to_string());
        }
        if text == "--locale" {
            let next = args.get(idx + 1)?.to_str()?;
            if !next.trim().is_empty() {
                return Some(next.to_string());
            }
        }
        idx += 1;
    }
    None
}
