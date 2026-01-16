use greentic_types::pack::extensions::component_sources::EXT_COMPONENT_SOURCES_V1;
use greentic_types::{PackManifest, decode_pack_manifest, encode_pack_manifest};
use serde_json::json;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::tempdir;
use zip::CompressionMethod;
use zip::write::FileOptions;
use zip::{ZipArchive, ZipWriter};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn write_pack(dir: &Path) {
    let flows_dir = dir.join("flows");
    fs::create_dir_all(&flows_dir).expect("flows dir");
    let flow_path = flows_dir.join("main.ygtc");
    fs::write(
        &flow_path,
        r#"id: main
type: messaging
start: templates
nodes:
  templates:
    component.exec:
      component: ai.greentic.component-templates
      operation: handle_message
      input:
        text: "hi"
    routing:
      - out: true
"#,
    )
    .expect("flow file");

    let digest = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let summary = json!({
        "schema_version": 1,
        "flow": "main.ygtc",
        "nodes": {
            "templates": {
                "component_id": "ai.greentic.component-templates",
                "source": {
                    "kind": "oci",
                    "ref": "oci://ghcr.io/greentic-ai/components/templates@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                },
                "digest": digest
            }
        }
    });
    fs::write(
        flow_path.with_extension("ygtc.resolve.summary.json"),
        serde_json::to_vec_pretty(&summary).expect("encode summary"),
    )
    .expect("summary file");

    let pack_yaml = r#"pack_id: dev.local.hello-pack
version: 0.1.0
kind: application
publisher: Greentic
components: []
flows:
  - id: main
    file: flows/main.ygtc
    tags: [default]
    entrypoints: [default]
"#;
    fs::write(dir.join("pack.yaml"), pack_yaml).expect("pack.yaml");
}

fn rewrite_manifest_without_component_sources(src: &Path, dest: &Path) {
    let src_file = File::open(src).expect("open source gtpack");
    let mut archive = ZipArchive::new(src_file).expect("open zip");
    let dest_file = File::create(dest).expect("create dest gtpack");
    let mut writer = ZipWriter::new(dest_file);

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).expect("zip entry");
        let name = entry.name().to_string();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).expect("read zip entry");
        if name == "manifest.cbor" {
            let mut manifest: PackManifest = decode_pack_manifest(&buf).expect("decode manifest");
            if let Some(ext) = manifest.extensions.as_mut() {
                ext.remove(EXT_COMPONENT_SOURCES_V1);
            }
            buf = encode_pack_manifest(&manifest).expect("encode manifest");
        }

        let mut options = FileOptions::<()>::default();
        options = options.compression_method(match entry.compression() {
            CompressionMethod::Stored => CompressionMethod::Stored,
            _ => CompressionMethod::Deflated,
        });
        writer.start_file(name, options).expect("start file");
        writer.write_all(&buf).expect("write file");
    }

    writer.finish().expect("finish zip");
}

#[test]
fn doctor_accepts_remote_component_sources() {
    let temp = tempdir().expect("temp dir");
    write_pack(temp.path());
    let gtpack_out = temp.path().join("dist/pack.gtpack");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .args([
            "build",
            "--in",
            temp.path().to_str().unwrap(),
            "--gtpack-out",
            gtpack_out.to_str().unwrap(),
            "--bundle",
            "none",
            "--no-update",
            "--log",
            "warn",
        ])
        .output()
        .expect("run packc build");
    assert!(
        output.status.success(),
        "packc build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let doctor = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .args(["doctor", "--pack", gtpack_out.to_str().unwrap(), "--json"])
        .output()
        .expect("run doctor");
    assert!(
        doctor.status.success(),
        "doctor failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&doctor.stdout),
        String::from_utf8_lossy(&doctor.stderr)
    );
}

#[test]
fn doctor_rejects_missing_component_sources() {
    let temp = tempdir().expect("temp dir");
    write_pack(temp.path());
    let gtpack_out = temp.path().join("dist/pack.gtpack");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .args([
            "build",
            "--in",
            temp.path().to_str().unwrap(),
            "--gtpack-out",
            gtpack_out.to_str().unwrap(),
            "--bundle",
            "none",
            "--no-update",
            "--log",
            "warn",
        ])
        .output()
        .expect("run packc build");
    assert!(
        output.status.success(),
        "packc build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let broken = temp.path().join("dist/broken.gtpack");
    rewrite_manifest_without_component_sources(&gtpack_out, &broken);

    let doctor = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(workspace_root())
        .args(["doctor", "--pack", broken.to_str().unwrap(), "--json"])
        .output()
        .expect("run doctor");
    assert!(
        !doctor.status.success(),
        "expected doctor to fail for missing component sources"
    );
    let stderr = String::from_utf8_lossy(&doctor.stderr);
    assert!(
        stderr.contains("missing from manifest/component sources"),
        "unexpected stderr: {stderr}"
    );
}
