use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use tempfile::TempDir;

pub const PACKC_ENV: &str = "GREENTIC_PACK_PLAN_PACKC";

pub fn materialize_pack_path(input: &Path, verbose: bool) -> Result<(Option<TempDir>, PathBuf)> {
    let metadata =
        fs::metadata(input).with_context(|| format!("unable to read input {}", input.display()))?;
    if metadata.is_file() {
        Ok((None, input.to_path_buf()))
    } else if metadata.is_dir() {
        let (temp, path) = build_pack_from_source(input, verbose)?;
        Ok((Some(temp), path))
    } else {
        bail!(
            "input {} is neither a file nor a directory",
            input.display()
        );
    }
}

fn build_pack_from_source(source: &Path, verbose: bool) -> Result<(TempDir, PathBuf)> {
    let temp = TempDir::new().context("failed to create temporary directory for pack build")?;
    let gtpack_path = temp.path().join("pack.gtpack");
    let wasm_path = temp.path().join("pack.wasm");
    let manifest_path = temp.path().join("manifest.cbor");
    let sbom_path = temp.path().join("sbom.cdx.json");
    let component_data = temp.path().join("data.rs");

    let packc_bin = std::env::var(PACKC_ENV).unwrap_or_else(|_| "packc".to_string());
    let mut cmd = Command::new(packc_bin);
    cmd.arg("build")
        .arg("--in")
        .arg(source)
        .arg("--out")
        .arg(&wasm_path)
        .arg("--manifest")
        .arg(&manifest_path)
        .arg("--sbom")
        .arg(&sbom_path)
        .arg("--gtpack-out")
        .arg(&gtpack_path)
        .arg("--component-data")
        .arg(&component_data)
        .arg("--log")
        .arg(if verbose { "info" } else { "warn" });

    if !verbose {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }

    let status = cmd
        .status()
        .context("failed to spawn packc to build temporary .gtpack")?;
    if !status.success() {
        bail!("packc build failed with status {}", status);
    }

    Ok((temp, gtpack_path))
}
