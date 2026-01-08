use greentic_types::{decode_pack_manifest, pack_manifest::PackManifest};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use zip::ZipArchive;

const DIGEST_REF: &str = "ghcr.io/demo/component@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const TAG_REF: &str = "ghcr.io/demo/component:latest";

fn write_stub_wasm(path: &Path) {
    const STUB: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    fs::create_dir_all(path.parent().unwrap()).expect("components dir");
    fs::write(path, STUB).expect("stub wasm");
}

fn write_pack_files(dir: &Path, oci_ref: &str, allow_tags: bool) -> PathBuf {
    fs::create_dir_all(dir.join("flows")).expect("flows dir");
    let wasm_path = dir.join("components/demo.wasm");
    write_stub_wasm(&wasm_path);
    let digest = format!("sha256:{:x}", Sha256::digest(fs::read(&wasm_path).unwrap()));

    let pack_yaml = format!(
        r#"pack_id: dev.local.oci-demo
version: 0.0.1
kind: application
publisher: Test
components:
  - id: "demo.component"
    version: "0.1.0"
    world: "greentic:component/component@0.5.0"
    supports: ["messaging"]
    profiles:
      default: "stateless"
      supported: ["stateless"]
    capabilities:
      wasi: {{}}
      host: {{}}
    operations:
      - name: "handle_message"
        input_schema: {{}}
        output_schema: {{}}
    wasm: "components/demo.wasm"

flows:
  - id: main
    file: flows/main.ygtc
    tags: [default]
    entrypoints: [main]

extensions:
  greentic.components:
    kind: greentic.components
    version: v1
    inline:
      refs:
        - "{oci_ref}"
      mode: eager
      allow_tags: {allow_tags}
"#
    );
    fs::write(dir.join("pack.yaml"), pack_yaml).expect("pack.yaml");

    let flow = r#"id: main
type: messaging
start: call
nodes:
  call:
    component.exec:
      component: demo.component
      operation: handle_message
      input:
        text: "hi"
    routing:
      - out: true
"#;
    fs::write(dir.join("flows/main.ygtc"), flow).expect("flow file");
    let sidecar = json!({
        "schema_version": 1,
        "flow": "flows/main.ygtc",
        "nodes": {
            "call": {
                "source": {
                    "kind": "local",
                    "path": "../components/demo.wasm",
                    "digest": digest
                }
            }
        }
    });
    fs::write(
        dir.join("flows/main.ygtc.resolve.json"),
        serde_json::to_vec_pretty(&sidecar).unwrap(),
    )
    .expect("write sidecar");

    dir.join("pack.yaml")
}

fn read_manifest_from_gtpack(gtpack: &Path) -> PackManifest {
    let file = fs::File::open(gtpack).expect("open gtpack");
    let mut archive = ZipArchive::new(file).expect("zip archive");
    let mut manifest_entry = archive.by_name("manifest.cbor").expect("manifest entry");
    let mut bytes = Vec::new();
    use std::io::Read;
    manifest_entry
        .read_to_end(&mut bytes)
        .expect("read manifest");
    decode_pack_manifest(&bytes).expect("decode manifest")
}

#[test]
fn digest_refs_are_preserved_in_gtpack() {
    let temp = tempdir().expect("temp dir");
    write_pack_files(temp.path(), DIGEST_REF, false);
    let manifest_out = temp.path().join("dist/manifest.cbor");
    let gtpack_out = temp.path().join("dist/pack.gtpack");

    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."))
        .args([
            "build",
            "--in",
            temp.path().to_str().unwrap(),
            "--manifest",
            manifest_out.to_str().unwrap(),
            "--gtpack-out",
            gtpack_out.to_str().unwrap(),
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

    let manifest = read_manifest_from_gtpack(&gtpack_out);
    let ext = manifest
        .extensions
        .as_ref()
        .and_then(|e| e.get("greentic.components"))
        .and_then(|e| e.inline.as_ref())
        .unwrap_or_else(|| panic!("components extension missing: {:?}", manifest.extensions));

    let inline = match ext {
        greentic_types::pack_manifest::ExtensionInline::Other(val) => val.clone(),
        other => json!(other),
    };
    assert_eq!(
        inline,
        json!({
            "refs": [DIGEST_REF],
            "mode": "eager",
            "allow_tags": false
        })
    );
}

#[test]
fn tag_refs_require_flag() {
    let temp = tempdir().expect("temp dir");
    write_pack_files(temp.path(), TAG_REF, true);
    let manifest_out = temp.path().join("dist/manifest.cbor");
    let gtpack_out = temp.path().join("dist/pack.gtpack");

    let run_build = |allow_tags: bool| {
        let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin!("packc"));
        cmd.current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."));
        cmd.args([
            "build",
            "--in",
            temp.path().to_str().unwrap(),
            "--manifest",
            manifest_out.to_str().unwrap(),
            "--gtpack-out",
            gtpack_out.to_str().unwrap(),
            "--log",
            "warn",
        ]);
        if allow_tags {
            cmd.arg("--allow-oci-tags");
        }
        cmd.output().expect("run packc build")
    };

    let denied = run_build(false);
    assert!(
        !denied.status.success(),
        "expected build to fail without --allow-oci-tags"
    );
    assert!(
        String::from_utf8_lossy(&denied.stderr).contains("digest-pinned"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&denied.stderr)
    );

    let allowed = run_build(true);
    assert!(
        allowed.status.success(),
        "packc build with --allow-oci-tags failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&allowed.stdout),
        String::from_utf8_lossy(&allowed.stderr)
    );
}
