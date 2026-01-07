use greentic_types::pack::extensions::component_manifests::{
    ComponentManifestIndexEntryV1, ComponentManifestIndexV1, EXT_COMPONENT_MANIFEST_INDEX_V1,
    ManifestEncoding,
};
use greentic_types::{ComponentManifest, decode_pack_manifest};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use zip::ZipArchive;

const COMPONENT_ID: &str = "demo.component";
const OPERATION: &str = "handle_message";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn write_stub_wasm(path: &Path) {
    const STUB: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    fs::create_dir_all(path.parent().unwrap()).expect("components dir");
    fs::write(path, STUB).expect("stub wasm");
}

fn write_pack_files(pack_dir: &Path) {
    fs::create_dir_all(pack_dir.join("flows")).expect("flows dir");
    write_stub_wasm(&pack_dir.join("components/demo.wasm"));

    let pack_yaml = format!(
        r#"pack_id: dev.local.manifest-demo
version: 0.0.1
kind: application
publisher: Test
components:
  - id: "{COMPONENT_ID}"
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
      - name: "{OPERATION}"
        input_schema: {{}}
        output_schema: {{}}
    wasm: "components/demo.wasm"

flows:
  - id: main
    file: flows/main.ygtc
    tags: [default]
    entrypoints: [main]
"#
    );
    fs::write(pack_dir.join("pack.yaml"), pack_yaml).expect("pack.yaml");

    let flow = format!(
        r#"id: main
type: messaging
start: call
nodes:
  call:
    operation: {OPERATION}
    component.exec:
      component: {COMPONENT_ID}
      input:
        text: "hi"
    routing:
      - out: true
"#
    );
    fs::write(pack_dir.join("flows/main.ygtc"), flow).expect("flow file");
}

fn read_zip_entry(path: &Path, entry: &str) -> Vec<u8> {
    let file = fs::File::open(path).expect("open gtpack");
    let mut archive = ZipArchive::new(file).expect("zip archive");
    let mut entry = archive.by_name(entry).expect("entry present");
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).expect("read entry");
    bytes
}

#[test]
fn component_manifests_are_embedded_and_indexed() {
    let temp = tempdir().expect("temp dir");
    write_pack_files(temp.path());

    let manifest_out = temp.path().join("dist/manifest.cbor");
    let gtpack_out = temp.path().join("dist/pack.gtpack");

    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("packc"))
        .current_dir(workspace_root())
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

    // Manifest index extension
    let manifest_bytes = fs::read(&manifest_out).expect("manifest bytes");
    let pack_manifest =
        decode_pack_manifest(&manifest_bytes).expect("decode manifest.cbor into PackManifest");
    let index_value = pack_manifest
        .extensions
        .as_ref()
        .and_then(|ext| ext.get(EXT_COMPONENT_MANIFEST_INDEX_V1))
        .and_then(|ext| ext.inline.as_ref())
        .and_then(|inline| match inline {
            greentic_types::ExtensionInline::Other(v) => Some(v.clone()),
            _ => None,
        })
        .expect("component manifest index extension present");
    let index =
        ComponentManifestIndexV1::from_extension_value(&index_value).expect("decode extension");
    assert_eq!(index.schema_version, 1);
    assert_eq!(index.entries.len(), 1);

    let ComponentManifestIndexEntryV1 {
        component_id,
        manifest_file,
        encoding,
        content_hash,
    } = &index.entries[0];
    assert_eq!(component_id, COMPONENT_ID);
    assert_eq!(encoding, &ManifestEncoding::Cbor);
    assert_eq!(
        manifest_file,
        &format!("components/{COMPONENT_ID}.manifest.cbor")
    );
    let manifest_hash = content_hash
        .as_ref()
        .expect("manifest hash present")
        .trim_start_matches("sha256:");

    // Embedded component manifest file
    let manifest_cbor = read_zip_entry(&gtpack_out, manifest_file);
    let decoded: ComponentManifest =
        serde_cbor::from_slice(&manifest_cbor).expect("decode component manifest");
    assert_eq!(decoded.id.as_str(), COMPONENT_ID);

    let mut sha = Sha256::new();
    sha.update(&manifest_cbor);
    let computed = format!("{:x}", sha.finalize());
    assert_eq!(
        manifest_hash, computed,
        "content hash should match file bytes"
    );
}
