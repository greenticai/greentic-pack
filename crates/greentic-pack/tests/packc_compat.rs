use std::path::PathBuf;
use std::process::Command;

use assert_cmd::prelude::*;
use greentic_pack::reader::{SigningPolicy, open_pack};
use tempfile::TempDir;

#[test]
fn open_pack_accepts_packc_gtpack() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path().join("demo-pack");
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    Command::new("cargo")
        .current_dir(&workspace_root)
        .args([
            "run",
            "-p",
            "packc",
            "--quiet",
            "--",
            "new",
            "--dir",
            pack_dir.to_str().unwrap(),
            "demo.test",
        ])
        .assert()
        .success();

    let pack_path = temp.path().join("pack.gtpack");
    Command::new("cargo")
        .current_dir(&workspace_root)
        .args([
            "run",
            "-p",
            "packc",
            "--quiet",
            "--",
            "build",
            "--in",
            pack_dir.to_str().unwrap(),
            "--gtpack-out",
            pack_path.to_str().unwrap(),
            "--offline",
        ])
        .assert()
        .success();

    let load = open_pack(&pack_path, SigningPolicy::DevOk)
        .expect("packc-generated gtpack should be accepted");

    assert_eq!(load.manifest.meta.pack_id, "demo.test");
    assert!(!load.report.signature_ok);
    assert!(!load.report.sbom_ok);
    assert!(load.report.warnings.iter().any(|msg| msg.contains("sbom")));
    assert!(!load.sbom.is_empty());
}
