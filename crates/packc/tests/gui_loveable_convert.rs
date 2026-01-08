#![forbid(unsafe_code)]

use packc::cli::gui::loveable_convert::{self, Args, GuiPackKind};
use std::fs;
use std::io::Read;
use tempfile::TempDir;
use zip::ZipArchive;

#[test]
fn loveable_convert_assets_mode_produces_gtpack() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let assets_dir = temp.path().join("assets");
    fs::create_dir_all(&assets_dir)?;
    fs::write(
        assets_dir.join("index.html"),
        "<html><body>index</body></html>",
    )?;
    fs::write(
        assets_dir.join("invoices.html"),
        r#"<html data-greentic-worker="worker.billing"><body>invoices</body></html>"#,
    )?;

    let out = temp.path().join("demo.gtpack");
    let args = Args {
        pack_kind: GuiPackKind::Feature,
        pack_id: "greentic.demo.gui".into(),
        version: "0.1.0".into(),
        pack_manifest_kind: "application".into(),
        publisher: "greentic.gui".into(),
        name: None,
        repo_url: None,
        branch: "main".into(),
        dir: None,
        assets_dir: Some(assets_dir),
        package_dir: None,
        install_cmd: None,
        build_cmd: None,
        build_dir: None,
        spa: None,
        routes: Vec::new(),
        routes_flat: None,
        out: out.clone(),
    };

    let runtime = packc::runtime::resolve_runtime(Some(temp.path()), None, false, None)?;
    tokio::runtime::Runtime::new()?.block_on(loveable_convert::handle(args, false, &runtime))?;

    let mut archive = ZipArchive::new(std::fs::File::open(&out)?)?;

    let mut manifest_json = String::new();
    archive
        .by_name("assets/gui/manifest.json")?
        .read_to_string(&mut manifest_json)?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_json)?;

    let routes = manifest.get("routes").and_then(|r| r.as_array()).unwrap();
    let paths: Vec<_> = routes
        .iter()
        .filter_map(|r| r.get("path").and_then(|p| p.as_str()))
        .collect();
    assert!(paths.contains(&"/"));
    assert!(paths.contains(&"/invoices"));

    let workers = manifest
        .get("digital_workers")
        .and_then(|w| w.as_array())
        .unwrap();
    assert_eq!(workers.len(), 1);
    let worker_id = workers[0]
        .get("worker_id")
        .and_then(|w| w.as_str())
        .unwrap();
    assert_eq!(worker_id, "worker.billing");

    // Ensure core assets are present in the archive.
    let mut index_html = String::new();
    archive
        .by_name("assets/gui/assets/index.html")?
        .read_to_string(&mut index_html)?;
    assert!(index_html.contains("index"));

    Ok(())
}

#[test]
fn spa_inference_ignores_404_and_generates_single_route() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let assets_dir = temp.path().join("spa");
    fs::create_dir_all(&assets_dir)?;
    fs::write(
        assets_dir.join("index.html"),
        "<html><body>home</body></html>",
    )?;
    fs::write(assets_dir.join("404.html"), "<html>not found</html>")?;
    fs::write(assets_dir.join("robots.html"), "<html>robots</html>")?;

    let out = temp.path().join("spa.gtpack");
    let args = Args {
        pack_kind: GuiPackKind::Feature,
        pack_id: "greentic.demo.gui".into(),
        version: "0.1.0".into(),
        pack_manifest_kind: "application".into(),
        publisher: "greentic.gui".into(),
        name: None,
        repo_url: None,
        branch: "main".into(),
        dir: None,
        assets_dir: Some(assets_dir),
        package_dir: None,
        install_cmd: None,
        build_cmd: None,
        build_dir: None,
        spa: None,
        routes: Vec::new(),
        routes_flat: None,
        out: out.clone(),
    };

    let runtime = packc::runtime::resolve_runtime(Some(temp.path()), None, false, None)?;
    tokio::runtime::Runtime::new()?.block_on(loveable_convert::handle(args, false, &runtime))?;

    let mut archive = ZipArchive::new(std::fs::File::open(&out)?)?;
    let mut manifest_json = String::new();
    archive
        .by_name("assets/gui/manifest.json")?
        .read_to_string(&mut manifest_json)?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_json)?;

    let routes = manifest.get("routes").and_then(|r| r.as_array()).unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].get("path").and_then(|p| p.as_str()), Some("/"));
    assert_eq!(
        routes[0].get("html").and_then(|p| p.as_str()),
        Some("gui/assets/index.html")
    );

    Ok(())
}
