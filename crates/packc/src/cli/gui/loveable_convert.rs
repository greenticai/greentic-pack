#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use clap::{ArgAction, Parser, ValueEnum};
use regex::Regex;
use semver::Version;
use serde::Serialize;
use serde_json::json;
use tempfile::TempDir;
use tracing::info;
use walkdir::WalkDir;

use crate::build;

#[derive(Debug, Clone, ValueEnum)]
pub enum GuiPackKind {
    Layout,
    Auth,
    Feature,
    Skin,
    Telemetry,
}

#[derive(Debug, Clone, Parser)]
pub struct Args {
    /// Logical GUI pack kind (maps to gui-<kind> in gui/manifest.json)
    #[arg(long = "pack-kind", value_enum)]
    pub pack_kind: GuiPackKind,

    /// Pack id to embed in pack.yaml
    #[arg(long = "id")]
    pub pack_id: String,

    /// Pack version (semver)
    #[arg(long = "version")]
    pub version: String,

    /// Pack manifest kind (application|provider|infrastructure|library)
    #[arg(long = "pack-manifest-kind", default_value = "application")]
    pub pack_manifest_kind: String,

    /// Pack publisher string (defaults to "greentic.gui")
    #[arg(long = "publisher", default_value = "greentic.gui")]
    pub publisher: String,

    /// Optional display name for the GUI pack
    #[arg(long)]
    pub name: Option<String>,

    /// Source repo URL to clone (mutually exclusive with --dir/--assets-dir)
    #[arg(long = "repo-url", conflicts_with_all = ["dir", "assets_dir"])]
    pub repo_url: Option<String>,

    /// Branch to checkout (only used with --repo-url)
    #[arg(long, requires = "repo_url", default_value = "main")]
    pub branch: String,

    /// Local source directory (mutually exclusive with --repo-url/--assets-dir)
    #[arg(long, conflicts_with_all = ["repo_url", "assets_dir"])]
    pub dir: Option<PathBuf>,

    /// Prebuilt assets directory (skips install/build)
    #[arg(long = "assets-dir", conflicts_with_all = ["repo_url", "dir"])]
    pub assets_dir: Option<PathBuf>,

    /// Subdirectory to build within the repo (monorepo support)
    #[arg(long = "package-dir")]
    pub package_dir: Option<PathBuf>,

    /// Override install command (default: auto-detect pnpm/yarn/npm)
    #[arg(long = "install-cmd")]
    pub install_cmd: Option<String>,

    /// Override build command (default: npm run build)
    #[arg(long = "build-cmd")]
    pub build_cmd: Option<String>,

    /// Build output directory override
    #[arg(long = "build-dir")]
    pub build_dir: Option<PathBuf>,

    /// Treat as SPA or MPA (override heuristic)
    #[arg(long = "spa")]
    pub spa: Option<bool>,

    /// Route overrides, can repeat (path:html)
    #[arg(long = "route", action = ArgAction::Append)]
    pub routes: Vec<String>,

    /// Routes convenience alias (comma-separated path:html entries)
    #[arg(long = "routes", value_name = "ROUTES")]
    pub routes_flat: Option<String>,

    /// Output .gtpack path
    #[arg(long = "out", alias = "output", value_name = "FILE")]
    pub out: PathBuf,
}

struct ConvertOptions {
    pack_kind: GuiPackKind,
    pack_id: String,
    version: Version,
    pack_manifest_kind: String,
    publisher: String,
    name: Option<String>,
    source: Source,
    package_dir: Option<PathBuf>,
    install_cmd: Option<String>,
    build_cmd: Option<String>,
    build_dir: Option<PathBuf>,
    spa: Option<bool>,
    routes: Vec<RouteOverride>,
    out: PathBuf,
}

#[derive(Debug, Clone)]
enum Source {
    Repo { url: String, branch: String },
    Dir(PathBuf),
    AssetsDir(PathBuf),
}

#[derive(Debug, Clone)]
struct RouteOverride {
    path: String,
    html: PathBuf,
}

#[derive(Debug, Serialize)]
struct Summary {
    pack_id: String,
    version: String,
    pack_kind: String,
    gui_kind: String,
    out: String,
    routes: Vec<String>,
    assets_copied: usize,
}

pub async fn handle(
    args: Args,
    json_out: bool,
    runtime: &crate::runtime::RuntimeContext,
) -> Result<()> {
    let opts = ConvertOptions::try_from(args)?;
    let staging = TempDir::new().context("failed to create staging dir")?;
    let staging_root = staging.path();
    let pack_root = staging_root
        .canonicalize()
        .context("failed to canonicalize staging dir")?;

    let mut _clone_guard: Option<TempDir> = None;
    let source_root = match &opts.source {
        Source::Repo { url, branch } => {
            runtime.require_online("git clone (packc gui loveable-convert --repo-url)")?;
            let (temp, repo_dir) = clone_repo(url, branch)?;
            let path = repo_dir
                .canonicalize()
                .context("failed to canonicalize cloned repo")?;
            _clone_guard = Some(temp);
            path
        }
        Source::Dir(p) => p.canonicalize().context("failed to canonicalize --dir")?,
        Source::AssetsDir(p) => p
            .canonicalize()
            .context("failed to canonicalize --assets-dir")?,
    };

    let build_root = opts
        .package_dir
        .as_ref()
        .map(|p| source_root.join(p))
        .unwrap_or_else(|| source_root.clone());

    let assets_dir = match opts.source {
        Source::AssetsDir(_) => build_root,
        _ => {
            runtime.require_online("install/build GUI assets")?;
            build_assets(&build_root, &opts)?
        }
    };

    let assets_dir = assets_dir
        .canonicalize()
        .with_context(|| format!("failed to canonicalize assets dir {}", assets_dir.display()))?;

    let staging_assets = staging_root.join("gui").join("assets");
    let copied = copy_assets(&assets_dir, &staging_assets)?;

    let gui_manifest = build_gui_manifest(&opts, &staging_assets)?;
    write_gui_manifest(&pack_root.join("gui").join("manifest.json"), &gui_manifest)?;

    write_pack_manifest(&opts, &pack_root, copied)?;

    let build_opts = build::BuildOptions {
        pack_dir: pack_root.clone(),
        component_out: None,
        manifest_out: pack_root.join("dist").join("manifest.cbor"),
        sbom_out: None,
        gtpack_out: Some(opts.out.clone()),
        lock_path: pack_root.join("pack.lock.json"),
        bundle: build::BundleMode::Cache,
        dry_run: false,
        secrets_req: None,
        default_secret_scope: None,
        allow_oci_tags: false,
        require_component_manifests: false,
        runtime: runtime.clone(),
        skip_update: false,
    };
    build::run(&build_opts).await?;

    if json_out {
        let summary = Summary {
            pack_id: opts.pack_id.clone(),
            version: opts.version.to_string(),
            pack_kind: opts.pack_manifest_kind.clone(),
            gui_kind: gui_kind_string(&opts.pack_kind),
            out: opts.out.display().to_string(),
            routes: extract_route_strings(&gui_manifest),
            assets_copied: copied,
        };
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        info!(
            pack_id = %opts.pack_id,
            version = %opts.version,
            gui_kind = gui_kind_string(&opts.pack_kind),
            out = %opts.out.display(),
            assets = copied,
            "gui pack conversion complete"
        );
    }

    Ok(())
}

impl TryFrom<Args> for ConvertOptions {
    type Error = anyhow::Error;

    fn try_from(args: Args) -> Result<Self> {
        if args.assets_dir.is_some() && args.package_dir.is_some() {
            return Err(anyhow!(
                "--package-dir cannot be combined with --assets-dir (assets are already built)"
            ));
        }

        let source = if let Some(url) = args.repo_url {
            Source::Repo {
                url,
                branch: args.branch,
            }
        } else if let Some(dir) = args.dir {
            Source::Dir(dir)
        } else if let Some(assets) = args.assets_dir {
            Source::AssetsDir(assets)
        } else {
            return Err(anyhow!(
                "one of --repo-url, --dir, or --assets-dir must be provided"
            ));
        };

        let routes = parse_routes(&args.routes, args.routes_flat.as_deref())?;
        let version =
            Version::parse(&args.version).context("invalid --version (expected semver)")?;
        let out = if args.out.is_absolute() {
            args.out
        } else {
            std::env::current_dir()
                .context("failed to resolve current dir")?
                .join(args.out)
        };

        Ok(Self {
            pack_kind: args.pack_kind,
            pack_id: args.pack_id,
            version,
            pack_manifest_kind: args.pack_manifest_kind.to_ascii_lowercase(),
            publisher: args.publisher,
            name: args.name,
            source,
            package_dir: args.package_dir,
            install_cmd: args.install_cmd,
            build_cmd: args.build_cmd,
            build_dir: args.build_dir,
            spa: args.spa,
            routes,
            out,
        })
    }
}

fn parse_routes(explicit: &[String], flat: Option<&str>) -> Result<Vec<RouteOverride>> {
    let mut entries = Vec::new();

    for raw in explicit {
        entries.push(parse_route_entry(raw)?);
    }

    if let Some(flat_raw) = flat {
        for part in flat_raw.split(',') {
            if part.trim().is_empty() {
                continue;
            }
            entries.push(parse_route_entry(part.trim())?);
        }
    }

    Ok(entries)
}

fn parse_route_entry(raw: &str) -> Result<RouteOverride> {
    let mut parts = raw.splitn(2, ':');
    let path = parts
        .next()
        .ok_or_else(|| anyhow!("invalid route entry: {}", raw))?;
    let html = parts
        .next()
        .ok_or_else(|| anyhow!("route entry must be path:html => {}", raw))?;

    let path = path.trim().to_string();
    if !path.starts_with('/') {
        return Err(anyhow!("route path must start with '/': {}", path));
    }

    let html_path = PathBuf::from(html.trim());
    if html_path.is_absolute() {
        return Err(anyhow!(
            "route html path must be relative to gui/assets: {}",
            html
        ));
    }

    Ok(RouteOverride {
        path,
        html: html_path,
    })
}

fn clone_repo(url: &str, branch: &str) -> Result<(TempDir, PathBuf)> {
    let temp = TempDir::new().context("failed to create temp dir for clone")?;
    let target = temp.path().join("repo");

    let status = Command::new("git")
        .arg("clone")
        .arg("--branch")
        .arg(branch)
        .arg("--depth")
        .arg("1")
        .arg(url)
        .arg(&target)
        .status()
        .with_context(|| format!("failed to execute git clone for {}", url))?;

    if !status.success() {
        return Err(anyhow!("git clone failed with status {}", status));
    }

    Ok((temp, target))
}

fn build_assets(build_root: &Path, opts: &ConvertOptions) -> Result<PathBuf> {
    let install_cmd = opts
        .install_cmd
        .clone()
        .unwrap_or_else(|| default_install_command(build_root));
    let build_cmd = opts
        .build_cmd
        .clone()
        .unwrap_or_else(|| "npm run build".to_string());

    run_shell(&install_cmd, build_root, "install dependencies")?;
    run_shell(&build_cmd, build_root, "build GUI assets")?;

    if let Some(dir) = &opts.build_dir {
        return Ok(build_root.join(dir));
    }

    let dist = build_root.join("dist");
    if dist.is_dir() {
        return Ok(dist);
    }

    let build = build_root.join("build");
    if build.is_dir() {
        return Ok(build);
    }

    Err(anyhow!(
        "unable to detect build output; specify --build-dir"
    ))
}

fn default_install_command(root: &Path) -> String {
    if root.join("pnpm-lock.yaml").exists() {
        "pnpm install".to_string()
    } else if root.join("yarn.lock").exists() {
        "yarn install".to_string()
    } else {
        "npm install".to_string()
    }
}

fn run_shell(cmd: &str, cwd: &Path, why: &str) -> Result<()> {
    info!(command = %cmd, cwd = %cwd.display(), "running {}", why);
    let status = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .status()
        .with_context(|| format!("failed to run command: {}", cmd))?;

    if !status.success() {
        return Err(anyhow!("command failed ({}) with status {}", why, status));
    }

    Ok(())
}

fn copy_assets(src: &Path, dest: &Path) -> Result<usize> {
    let mut count = 0usize;
    for entry in WalkDir::new(src)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let rel = entry
            .path()
            .strip_prefix(src)
            .expect("walkdir provided prefix");
        let target = dest.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::copy(entry.path(), &target).with_context(|| {
            format!(
                "failed to copy {} to {}",
                entry.path().display(),
                target.display()
            )
        })?;
        count += 1;
    }

    Ok(count)
}

fn build_gui_manifest(opts: &ConvertOptions, assets_root: &Path) -> Result<serde_json::Value> {
    let html_files = discover_html_files(assets_root);
    if html_files.is_empty()
        && !matches!(opts.pack_kind, GuiPackKind::Skin | GuiPackKind::Telemetry)
    {
        return Err(anyhow!(
            "no HTML files found in assets dir {}",
            assets_root.display()
        ));
    }

    match opts.pack_kind {
        GuiPackKind::Layout => {
            let entry = select_entrypoint(&html_files);
            let spa = opts.spa.unwrap_or_else(|| infer_spa(&html_files, &entry));
            Ok(json!({
                "kind": "gui-layout",
                "layout": {
                    "slots": ["header","menu","main","footer"],
                    "entrypoint_html": format!("gui/assets/{}", to_unix_path(&entry)),
                    "spa": spa,
                    "slot_selectors": {
                        "header": "#app-header",
                        "menu": "#app-menu",
                        "main": "#app-main",
                        "footer": "#app-footer"
                    }
                }
            }))
        }
        GuiPackKind::Auth => {
            let routes = build_auth_routes(&html_files);
            Ok(json!({
                "kind": "gui-auth",
                "routes": routes,
                "ui_bindings": {
                    "login_form_selector": "#login-form",
                    "login_buttons": [
                        { "provider": "microsoft", "selector": "#login-ms" },
                        { "provider": "google", "selector": "#login-google" }
                    ]
                }
            }))
        }
        GuiPackKind::Feature => {
            let routes = build_feature_routes(opts, &html_files);
            let workers = detect_workers(assets_root, &html_files)?;
            Ok(json!({
                "kind": "gui-feature",
                "routes": routes,
                "digital_workers": workers,
                "fragments": []
            }))
        }
        GuiPackKind::Skin => {
            let theme_css_path = find_theme_css(assets_root);
            let theme_css = theme_css_path.map(|p| format!("gui/assets/{}", to_unix_path(&p)));
            Ok(json!({
                "kind": "gui-skin",
                "skin": {
                    "theme_css": theme_css
                }
            }))
        }
        GuiPackKind::Telemetry => Ok(json!({
            "kind": "gui-telemetry",
            "telemetry": {}
        })),
    }
}

fn write_gui_manifest(path: &Path, value: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let data = serde_json::to_vec_pretty(value)?;
    fs::write(path, data).with_context(|| format!("failed to write {}", path.display()))
}

#[derive(Debug, Serialize)]
struct PackManifestYaml<'a> {
    pack_id: &'a str,
    version: &'a str,
    kind: &'a str,
    publisher: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    components: Vec<()>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    dependencies: Vec<()>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    flows: Vec<()>,
    assets: Vec<AssetEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct AssetEntry {
    path: String,
}

fn write_pack_manifest(opts: &ConvertOptions, root: &Path, assets_copied: usize) -> Result<()> {
    if assets_copied == 0 {
        return Err(anyhow!("no assets copied; cannot build GUI pack"));
    }

    let mut assets = Vec::new();
    assets.push(AssetEntry {
        path: "gui/manifest.json".to_string(),
    });

    let assets_root = root.join("gui").join("assets");
    for entry in WalkDir::new(&assets_root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let rel = entry.path().strip_prefix(root).expect("walkdir prefix");
        assets.push(AssetEntry {
            path: to_unix_path(rel),
        });
    }

    assets.sort_by(|a, b| a.path.cmp(&b.path));

    let yaml = PackManifestYaml {
        pack_id: &opts.pack_id,
        version: &opts.version.to_string(),
        kind: &opts.pack_manifest_kind,
        publisher: &opts.publisher,
        components: Vec::new(),
        dependencies: Vec::new(),
        flows: Vec::new(),
        assets,
        name: opts.name.as_deref(),
    };

    let manifest_path = root.join("pack.yaml");
    let contents = serde_yaml_bw::to_string(&yaml)?;
    fs::write(&manifest_path, contents)
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;

    Ok(())
}

fn discover_html_files(assets_root: &Path) -> Vec<PathBuf> {
    WalkDir::new(assets_root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "html")
                .unwrap_or(false)
        })
        .map(|e| {
            e.path()
                .strip_prefix(assets_root)
                .unwrap_or(e.path())
                .to_path_buf()
        })
        .collect()
}

fn select_entrypoint(html_files: &[PathBuf]) -> PathBuf {
    html_files
        .iter()
        .find(|p| p.file_name().map(|n| n == "index.html").unwrap_or(false))
        .cloned()
        .unwrap_or_else(|| html_files[0].clone())
}

fn infer_spa(html_files: &[PathBuf], entry: &Path) -> bool {
    let real_pages = html_files.iter().filter(|p| is_real_page(p)).count();
    real_pages <= 1
        && entry
            .file_name()
            .map(|n| n == "index.html")
            .unwrap_or(false)
}

fn is_real_page(path: &Path) -> bool {
    let ignore = ["404", "robots"];
    path.extension().map(|ext| ext == "html").unwrap_or(false)
        && !ignore
            .iter()
            .any(|ig| path.file_stem().and_then(OsStr::to_str) == Some(ig))
}

fn build_auth_routes(html_files: &[PathBuf]) -> Vec<serde_json::Value> {
    let mut routes = Vec::new();
    let login = html_files
        .iter()
        .find(|p| p.file_name().and_then(OsStr::to_str) == Some("login.html"))
        .or_else(|| html_files.first());

    if let Some(login) = login {
        routes.push(json!({
            "path": "/login",
            "html": format!("gui/assets/{}", to_unix_path(login)),
            "public": true
        }));
    }

    routes
}

fn build_feature_routes(opts: &ConvertOptions, html_files: &[PathBuf]) -> Vec<serde_json::Value> {
    if !opts.routes.is_empty() {
        return opts
            .routes
            .iter()
            .map(|r| {
                json!({
                    "path": r.path,
                    "html": format!("gui/assets/{}", to_unix_path(&r.html)),
                    "authenticated": true
                })
            })
            .collect();
    }

    let entry = select_entrypoint(html_files);
    let spa = opts.spa.unwrap_or_else(|| infer_spa(html_files, &entry));

    let mut routes = Vec::new();
    if spa {
        routes.push(json!({
            "path": "/",
            "html": format!("gui/assets/{}", to_unix_path(&entry)),
            "authenticated": true
        }));
        return routes;
    }

    for page in html_files.iter().filter(|p| is_real_page(p)) {
        let route = route_from_path(page);
        routes.push(json!({
            "path": route,
            "html": format!("gui/assets/{}", to_unix_path(page)),
            "authenticated": true
        }));
    }

    routes
}

fn route_from_path(path: &Path) -> String {
    let mut parts = Vec::new();
    if let Some(parent) = path.parent()
        && parent != Path::new("")
    {
        parts.push(to_unix_path(parent));
    }
    if path.file_stem().and_then(OsStr::to_str) != Some("index") {
        parts.push(
            path.file_stem()
                .and_then(OsStr::to_str)
                .unwrap_or_default()
                .to_string(),
        );
    }

    let combined = parts.join("/");
    if combined.is_empty() {
        "/".to_string()
    } else if combined.starts_with('/') {
        combined
    } else {
        format!("/{}", combined)
    }
}

fn detect_workers(assets_root: &Path, html_files: &[PathBuf]) -> Result<Vec<serde_json::Value>> {
    let worker_re = Regex::new(r#"data-greentic-worker\s*=\s*"([^"]+)""#)?;
    let slot_re = Regex::new(r#"data-greentic-worker-slot\s*=\s*"([^"]+)""#)?;
    let mut seen = BTreeSet::new();
    let mut workers = Vec::new();

    for rel in html_files {
        let abs = assets_root.join(rel);
        let contents = fs::read_to_string(&abs).with_context(|| {
            format!(
                "failed to read HTML for worker detection: {}",
                abs.display()
            )
        })?;

        for caps in worker_re.captures_iter(&contents) {
            let worker_id = caps
                .get(1)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            if worker_id.is_empty() || !seen.insert(worker_id.clone()) {
                continue;
            }

            let slot = slot_re
                .captures(&contents)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string());

            let selector = slot
                .as_ref()
                .map(|s| format!("#{}", s))
                .unwrap_or_else(|| format!(r#"[data-greentic-worker="{}"]"#, worker_id));

            workers.push(json!({
                "id": worker_id.split('.').next_back().unwrap_or(&worker_id),
                "worker_id": worker_id,
                "attach": { "mode": "selector", "selector": selector },
                "routes": ["/*"]
            }));
        }
    }

    Ok(workers)
}

fn extract_route_strings(manifest: &serde_json::Value) -> Vec<String> {
    manifest
        .get("routes")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    r.get("path")
                        .and_then(|p| p.as_str())
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn to_unix_path(path: &Path) -> String {
    path.iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn gui_kind_string(kind: &GuiPackKind) -> String {
    match kind {
        GuiPackKind::Layout => "gui-layout",
        GuiPackKind::Auth => "gui-auth",
        GuiPackKind::Feature => "gui-feature",
        GuiPackKind::Skin => "gui-skin",
        GuiPackKind::Telemetry => "gui-telemetry",
    }
    .to_string()
}

fn find_theme_css(assets_root: &Path) -> Option<PathBuf> {
    let candidates = ["theme.css", "styles.css"];
    for candidate in candidates {
        let path = assets_root.join(candidate);
        if path.exists() {
            return Some(PathBuf::from(candidate));
        }
    }
    None
}
