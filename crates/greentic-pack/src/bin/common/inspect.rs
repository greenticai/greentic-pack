use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use clap::ValueEnum;
use greentic_pack::{
    ComponentManifestIndexState, ManifestFileVerificationReport, SigningPolicy, VerifyReport,
    builder::PackManifest, open_pack,
};
use greentic_types::SecretRequirement;
use serde::Deserialize;
use serde_json::json;
use zip::ZipArchive;

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum PolicyArg {
    Devok,
    Strict,
}

impl From<PolicyArg> for SigningPolicy {
    fn from(value: PolicyArg) -> Self {
        match value {
            PolicyArg::Devok => SigningPolicy::DevOk,
            PolicyArg::Strict => SigningPolicy::Strict,
        }
    }
}

pub fn run(path: &Path, policy: PolicyArg, json: bool, verify_manifest_files: bool) -> Result<()> {
    let load = open_pack(path, policy.into()).map_err(|err| anyhow!(err.message))?;
    let gui = load_gui_summary(path).ok();
    let secrets = load_secret_requirements(path).ok();
    let index_state = load.component_manifest_index_v1();
    let manifest_verify = if verify_manifest_files {
        Some(load.verify_component_manifest_files())
    } else {
        None
    };
    if verify_manifest_files {
        if !index_state.ok() {
            bail!(
                "component manifest index invalid: {}",
                index_state
                    .error
                    .as_deref()
                    .unwrap_or("unknown error decoding extension")
            );
        }
        if let Some(report) = manifest_verify.as_ref()
            && !report.ok()
        {
            let reason = report
                .first_error()
                .unwrap_or_else(|| "verification failed".to_string());
            bail!("component manifest file verification failed: {reason}");
        }
    }
    if json {
        print_json(
            &load.manifest,
            &load.report,
            &load.sbom,
            &index_state,
            manifest_verify.as_ref(),
            gui,
            secrets,
        )?;
    } else {
        print_human(
            &load.manifest,
            &load.report,
            &load.sbom,
            &index_state,
            manifest_verify.as_ref(),
            gui.as_ref(),
            secrets.as_deref(),
        );
    }
    Ok(())
}

fn print_human(
    manifest: &PackManifest,
    report: &VerifyReport,
    sbom: &[greentic_pack::builder::SbomEntry],
    index_state: &ComponentManifestIndexState,
    manifest_verify: Option<&ManifestFileVerificationReport>,
    gui: Option<&GuiSummary>,
    secrets: Option<&[SecretRequirement]>,
) {
    println!(
        "Pack: {} ({})",
        manifest.meta.pack_id, manifest.meta.version
    );
    println!("Flows: {}", manifest.flows.len());
    println!("Components: {}", manifest.components.len());
    println!("SBOM entries: {}", sbom.len());
    println!("Signature OK: {}", report.signature_ok);
    println!("SBOM OK: {}", report.sbom_ok);
    if report.warnings.is_empty() {
        println!("Warnings: none");
    } else {
        println!("Warnings:");
        for warning in &report.warnings {
            println!("  - {}", warning);
        }
    }

    println!("Component manifest index:");
    if !index_state.present {
        println!("  not present");
    } else if let Some(err) = &index_state.error {
        println!("  invalid: {err}");
    } else if let Some(index) = &index_state.index {
        if index.entries.is_empty() {
            println!("  present (no entries)");
        } else {
            println!("  entries:");
            for entry in &index.entries {
                let hash = entry.content_hash.as_deref().unwrap_or("no content_hash");
                println!(
                    "    - {} -> {} ({hash})",
                    entry.component_id, entry.manifest_file
                );
            }
        }
    }

    if let Some(report) = manifest_verify {
        if !report.extension_present {
            println!("Component manifest verification: index not present (skipped)");
        } else if let Some(err) = &report.extension_error {
            println!("Component manifest verification: FAILED - {err}");
        } else {
            let status = if report.ok() { "ok" } else { "FAILED" };
            println!("Component manifest verification: {}", status);
            for entry in &report.entries {
                let summary = entry.error.as_deref().unwrap_or("verified");
                println!(
                    "  - {} -> {} [{}]",
                    entry.component_id, entry.manifest_file, summary
                );
            }
        }
    }
    if let Some(secrets) = secrets {
        println!("Secrets:");
        if secrets.is_empty() {
            println!("  none");
        } else {
            for req in secrets {
                let scope = req
                    .scope
                    .as_ref()
                    .map(|s| {
                        format!(
                            "{}/{}{}",
                            &s.env,
                            &s.tenant,
                            s.team
                                .as_deref()
                                .map(|t| format!("/{}", t))
                                .unwrap_or_default()
                        )
                    })
                    .unwrap_or_else(|| "unspecified".to_string());
                let fmt = req
                    .format
                    .as_ref()
                    .map(|f| format!("{:?}", f))
                    .unwrap_or_else(|| "unspecified".to_string());
                println!(
                    "  - {} @ {} ({})",
                    <String as From<_>>::from(req.key.clone()),
                    scope,
                    fmt
                );
            }
        }
    }
    if let Some(gui) = gui {
        println!("GUI:");
        println!("  kind: {}", gui.kind);
        if gui.routes.is_empty() {
            println!("  routes: none");
        } else {
            println!("  routes:");
            for r in &gui.routes {
                println!(
                    "    - {} -> {}{}{}",
                    r.path,
                    r.html,
                    if r.public { " (public)" } else { "" },
                    if r.authenticated { " (auth)" } else { "" }
                );
            }
        }
        if gui.digital_workers.is_empty() {
            println!("  workers: none");
        } else {
            println!("  workers:");
            for w in &gui.digital_workers {
                println!("    - {} ({}) @ {}", w.id, w.worker_id, w.selector);
            }
        }
        if gui.fragments.is_empty() {
            println!("  fragments: none");
        } else {
            println!("  fragments:");
            for f in &gui.fragments {
                println!(
                    "    - {} -> {} ({}) @ {}",
                    f.id, f.component_name, f.component_world, f.selector
                );
            }
        }
        println!(
            "  assets: {} files, {} bytes",
            gui.assets.files, gui.assets.total_bytes
        );
    }
}

fn print_json(
    manifest: &PackManifest,
    report: &VerifyReport,
    sbom: &[greentic_pack::builder::SbomEntry],
    index_state: &ComponentManifestIndexState,
    manifest_verify: Option<&ManifestFileVerificationReport>,
    gui: Option<GuiSummary>,
    secrets: Option<Vec<SecretRequirement>>,
) -> Result<()> {
    let payload = json!({
        "manifest": {
            "pack_id": manifest.meta.pack_id,
            "version": manifest.meta.version,
            "flows": manifest.flows.len(),
            "components": manifest.components.len(),
        },
        "report": {
            "signature_ok": report.signature_ok,
            "sbom_ok": report.sbom_ok,
            "warnings": report.warnings,
        },
        "sbom": sbom,
        "component_manifest_index": {
            "present": index_state.present,
            "error": index_state.error,
            "entries": index_state.index.as_ref().map(|idx| &idx.entries),
        },
        "component_manifest_verification": manifest_verify.map(|report| json!({
            "ok": report.ok(),
            "extension_present": report.extension_present,
            "extension_error": report.extension_error,
            "entries": report.entries.iter().map(|entry| json!({
                "component_id": entry.component_id,
                "manifest_file": entry.manifest_file,
                "encoding": entry.encoding,
                "content_hash": entry.content_hash,
                "file_present": entry.file_present,
                "hash_ok": entry.hash_ok,
                "decoded": entry.decoded,
                "inline_match": entry.inline_match,
                "error": entry.error,
            })).collect::<Vec<_>>(),
        })),
        "gui": gui,
        "secret_requirements": secrets,
    });
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

#[derive(Debug, Deserialize)]
struct GuiManifest {
    kind: String,
    #[serde(default)]
    routes: Vec<GuiRoute>,
    #[serde(default)]
    digital_workers: Vec<GuiWorker>,
    #[serde(default)]
    fragments: Vec<GuiFragment>,
}

#[derive(Debug, Deserialize)]
struct GuiRoute {
    path: String,
    html: String,
    #[serde(default)]
    public: bool,
    #[serde(default)]
    authenticated: bool,
}

#[derive(Debug, Deserialize)]
struct GuiWorker {
    id: String,
    worker_id: String,
    #[serde(default)]
    attach: Option<GuiAttach>,
    #[allow(dead_code)]
    #[serde(default)]
    routes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GuiAttach {
    #[allow(dead_code)]
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    selector: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GuiFragment {
    id: String,
    selector: String,
    component_world: String,
    component_name: String,
}

#[derive(Debug, serde::Serialize)]
struct GuiSummary {
    kind: String,
    routes: Vec<GuiRouteSummary>,
    digital_workers: Vec<GuiWorkerSummary>,
    fragments: Vec<GuiFragmentSummary>,
    assets: GuiAssetsSummary,
}

#[derive(Debug, serde::Serialize)]
struct GuiRouteSummary {
    path: String,
    html: String,
    public: bool,
    authenticated: bool,
}

#[derive(Debug, serde::Serialize)]
struct GuiWorkerSummary {
    id: String,
    worker_id: String,
    selector: String,
}

#[derive(Debug, serde::Serialize)]
struct GuiFragmentSummary {
    id: String,
    selector: String,
    component_world: String,
    component_name: String,
}

#[derive(Debug, serde::Serialize)]
struct GuiAssetsSummary {
    files: usize,
    total_bytes: u64,
}

fn load_secret_requirements(path: &Path) -> Result<Vec<SecretRequirement>> {
    let file =
        std::fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut archive = ZipArchive::new(file)
        .with_context(|| format!("{} is not a valid gtpack archive", path.display()))?;

    let candidate_names = [
        "assets/secret-requirements.json",
        "secret-requirements.json",
    ];
    for name in candidate_names {
        if let Ok(mut entry) = archive.by_name(name) {
            let mut buf = String::new();
            entry
                .read_to_string(&mut buf)
                .context("failed to read secret requirements file")?;
            let reqs: Vec<SecretRequirement> =
                serde_json::from_str(&buf).context("secret requirements file is invalid JSON")?;
            return Ok(reqs);
        }
    }
    Err(anyhow!("secret requirements file not found in archive"))
}

fn load_gui_summary(path: &Path) -> Result<GuiSummary> {
    let file =
        std::fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut archive = ZipArchive::new(file)
        .with_context(|| format!("{} is not a valid gtpack archive", path.display()))?;

    let manifest: GuiManifest = {
        let mut manifest_file = archive
            .by_name("assets/gui/manifest.json")
            .context("gui manifest not found (assets/gui/manifest.json)")?;
        let mut buf = String::new();
        manifest_file
            .read_to_string(&mut buf)
            .context("failed to read gui/manifest.json")?;
        serde_json::from_str(&buf).context("gui manifest is invalid JSON")?
    };

    let routes = manifest
        .routes
        .into_iter()
        .map(|r| GuiRouteSummary {
            path: r.path,
            html: r.html,
            public: r.public,
            authenticated: r.authenticated,
        })
        .collect();

    let digital_workers = manifest
        .digital_workers
        .into_iter()
        .map(|w| GuiWorkerSummary {
            id: w.id,
            worker_id: w.worker_id,
            selector: w
                .attach
                .as_ref()
                .and_then(|a| a.selector.clone())
                .unwrap_or_else(|| "[data-greentic-worker]".to_string()),
        })
        .collect();

    let fragments = manifest
        .fragments
        .into_iter()
        .map(|f| GuiFragmentSummary {
            id: f.id,
            selector: f.selector,
            component_world: f.component_world,
            component_name: f.component_name,
        })
        .collect();

    let mut files = 0usize;
    let mut bytes = 0u64;
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        if name.starts_with("assets/gui/assets/") {
            files += 1;
            bytes += entry.size();
        }
    }

    Ok(GuiSummary {
        kind: manifest.kind,
        routes,
        digital_workers,
        fragments,
        assets: GuiAssetsSummary {
            files,
            total_bytes: bytes,
        },
    })
}
