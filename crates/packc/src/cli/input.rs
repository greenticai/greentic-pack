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
    let packc_bin = std::env::var(PACKC_ENV).unwrap_or_else(|_| "packc".to_string());
    build_pack_from_source_with_packc(source, verbose, &packc_bin)
}

fn build_pack_from_source_with_packc(
    source: &Path,
    verbose: bool,
    packc_bin: &str,
) -> Result<(TempDir, PathBuf)> {
    let temp = TempDir::new().context("failed to create temporary directory for pack build")?;
    let gtpack_path = temp.path().join("pack.gtpack");
    let wasm_path = temp.path().join("pack.wasm");
    let manifest_path = temp.path().join("manifest.cbor");
    let sbom_path = temp.path().join("sbom.cdx.json");
    let component_data = temp.path().join("data.rs");

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    #[test]
    fn materialize_pack_path_keeps_input_files() {
        let dir = tempdir().expect("tempdir");
        let file = dir.path().join("demo.gtpack");
        fs::write(&file, b"pack").expect("write file");

        let (temp, path) = materialize_pack_path(&file, false).expect("file input should work");

        assert!(temp.is_none());
        assert_eq!(path, file);
    }

    #[test]
    fn materialize_pack_path_reports_missing_input() {
        let dir = tempdir().expect("tempdir");
        let missing = dir.path().join("missing.gtpack");

        let err = materialize_pack_path(&missing, false).expect_err("missing input should fail");
        assert!(err.to_string().contains("unable to read input"));
    }

    #[test]
    fn build_pack_from_source_invokes_expected_arguments() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("pack-src");
        fs::create_dir_all(&source).expect("source dir");

        let args_log = dir.path().join("args.log");
        let script = dir.path().join("fake-packc.sh");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\nout=''\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = '--gtpack-out' ]; then\n    shift\n    out=\"$1\"\n  fi\n  shift\ndone\ntouch \"$out\"\n",
                args_log.display()
            ),
        )
        .expect("script");
        let mut perms = fs::metadata(&script).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).expect("chmod");

        let (_temp, gtpack) =
            build_pack_from_source_with_packc(&source, true, script.to_str().expect("script path"))
                .expect("build should succeed");

        let logged = fs::read_to_string(&args_log).expect("args log");
        assert!(gtpack.exists(), "gtpack should be created by fake builder");
        assert!(logged.contains("build"));
        assert!(logged.contains("--component-data"));
        assert!(logged.contains("--log"));
        assert!(logged.contains("info"));
    }

    #[test]
    fn build_pack_from_source_surfaces_command_failures() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("pack-src");
        fs::create_dir_all(&source).expect("source dir");

        let script = dir.path().join("fail-packc.sh");
        fs::write(&script, "#!/bin/sh\nexit 7\n").expect("script");
        let mut perms = fs::metadata(&script).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).expect("chmod");

        let err = build_pack_from_source_with_packc(
            &source,
            false,
            script.to_str().expect("script path"),
        )
        .expect_err("failing build should error");
        assert!(err.to_string().contains("packc build failed with status"), "ERR={err:?} / {err}");
    }
}
