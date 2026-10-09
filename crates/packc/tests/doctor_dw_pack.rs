//! End-to-end `doctor` coverage for the DW application pack shape.
//!
//! The archives here are written to match greentic-designer's
//! `orchestrate::dw_application_pack::write_gtpack` output: `manifest.json`
//! (pretty-printed `AnswerDocPackSpec`) + `metadata.json`, plus the optional
//! `flows/main.ygtc` and knowledge sidecars.

use std::io::Write;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::{Value, json};
use zip::write::SimpleFileOptions;

/// The message from the 2026-08-20 field report. It made a healthy partner pack
/// look corrupt; it must never come back.
const OLD_MESSAGE: &str = "manifest.cbor missing from archive";

struct DwPackBuilder {
    manifest: Option<String>,
    metadata: Option<String>,
    extra: Vec<(String, Vec<u8>)>,
}

impl DwPackBuilder {
    fn new() -> Self {
        Self {
            manifest: Some(
                serde_json::to_string_pretty(&json!({
                    "manifest_id": "support-triage",
                    "display_name": "Support Triage Worker",
                    "manifest": { "agents": {} },
                    "tenant": "acme",
                }))
                .expect("manifest json"),
            ),
            metadata: Some(
                serde_json::to_string_pretty(&json!({
                    "pack_id": "pack.dw.support-triage.9f2a1c",
                    "kind": "DwApplication",
                    "created_at": "2026-08-20T06:00:00Z",
                    "source": "designer/research/dw_application_pack",
                }))
                .expect("metadata json"),
            ),
            extra: Vec::new(),
        }
    }

    fn manifest(mut self, body: Option<&str>) -> Self {
        self.manifest = body.map(str::to_string);
        self
    }

    fn metadata(mut self, body: Option<&str>) -> Self {
        self.metadata = body.map(str::to_string);
        self
    }

    fn entry(mut self, name: &str, body: &str) -> Self {
        self.extra
            .push((name.to_string(), body.as_bytes().to_vec()));
        self
    }

    fn write(self, dir: &Path, name: &str) -> PathBuf {
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        if let Some(manifest) = self.manifest {
            entries.push(("manifest.json".to_string(), manifest.into_bytes()));
        }
        if let Some(metadata) = self.metadata {
            entries.push(("metadata.json".to_string(), metadata.into_bytes()));
        }
        entries.extend(self.extra);
        write_zip(dir, name, &entries)
    }
}

fn write_zip(dir: &Path, name: &str, entries: &[(String, Vec<u8>)]) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).expect("create archive");
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (entry, bytes) in entries {
        zip.start_file(entry.as_str(), options)
            .expect("start entry");
        zip.write_all(bytes).expect("write entry");
    }
    zip.finish().expect("finish archive");
    path
}

fn doctor(path: &Path, extra: &[&str]) -> std::process::Output {
    let mut cmd = Command::cargo_bin("greentic-pack").expect("greentic-pack binary");
    cmd.arg("doctor").arg(path);
    for arg in extra {
        cmd.arg(arg);
    }
    // greentic-flow may not be installed here; the DW path degrades to a
    // warning, but skipping keeps these tests independent of the environment.
    cmd.arg("--no-flow-doctor");
    cmd.output().expect("run doctor")
}

fn combined(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The field report, as a test: a valid designer export must pass, and must not
/// be described with the old missing-file wording.
#[test]
fn valid_dw_pack_passes_and_names_its_shape() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = DwPackBuilder::new().write(temp.path(), "support-triage.gtpack");

    let output = doctor(&path, &[]);
    let text = combined(&output);

    assert!(output.status.success(), "expected exit 0, got: {text}");
    assert!(
        text.contains("Pack shape: DW application pack"),
        "expected the shape line, got: {text}"
    );
    assert!(
        text.contains("Manifest id: support-triage"),
        "expected the manifest id, got: {text}"
    );
    assert!(
        text.contains("Not checked (not part of this pack shape)"),
        "the report must say what it did not check, got: {text}"
    );
    assert!(
        !text.contains(OLD_MESSAGE),
        "the old message must not resurface, got: {text}"
    );
    assert!(
        !text.to_lowercase().contains("corrupt"),
        "a valid pack must never be called corrupt, got: {text}"
    );
}

/// The DW report must not describe artefacts this shape never carries.
#[test]
fn dw_report_mentions_no_sbom_or_signature_findings() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = DwPackBuilder::new().write(temp.path(), "quiet.gtpack");

    let text = combined(&doctor(&path, &[]));
    let findings: Vec<&str> = text
        .lines()
        .filter(|line| line.trim_start().starts_with("error:") || line.contains("PACK_"))
        .collect();

    for line in findings {
        let lower = line.to_ascii_lowercase();
        assert!(
            !lower.contains("sbom") && !lower.contains("signature"),
            "DW report must not raise canonical-only findings: {line}"
        );
    }
}

#[test]
fn dangling_knowledge_asset_fails() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = DwPackBuilder::new()
        .entry(
            "knowledge_base.json",
            r#"{"version":1,"strategy":"static_injection","total_chars":4,"truncated":false,
                "files":[{"asset_path":"assets/knowledge/gone.txt","original_name":"gone.txt","chars":4}]}"#,
        )
        .write(temp.path(), "dangling.gtpack");

    let output = doctor(&path, &[]);
    let text = combined(&output);

    assert!(!output.status.success(), "expected exit 1, got: {text}");
    assert!(
        text.contains("PACK_DW_KNOWLEDGE_DANGLING_ASSET"),
        "expected the dangling-asset code, got: {text}"
    );
}

#[test]
fn declared_kind_mismatch_fails() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = DwPackBuilder::new()
        .metadata(Some(r#"{"pack_id":"p","kind":"Flow"}"#))
        .write(temp.path(), "mismatch.gtpack");

    let output = doctor(&path, &[]);
    let text = combined(&output);

    assert!(!output.status.success(), "expected exit 1, got: {text}");
    assert!(
        text.contains("PACK_DW_KIND_MISMATCH"),
        "expected the kind-mismatch code, got: {text}"
    );
}

#[test]
fn missing_metadata_is_a_broken_dw_pack_not_an_unknown_archive() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = DwPackBuilder::new()
        .metadata(None)
        .write(temp.path(), "no-metadata.gtpack");

    let output = doctor(&path, &[]);
    let text = combined(&output);

    assert!(!output.status.success(), "expected exit 1, got: {text}");
    assert!(
        text.contains("Pack shape: DW application pack"),
        "shape must still be named, got: {text}"
    );
    assert!(
        text.contains("PACK_DW_METADATA_MISSING"),
        "expected the metadata-missing code, got: {text}"
    );
    assert!(
        !text.contains("unrecognised"),
        "a DW pack missing metadata is broken, not unknown, got: {text}"
    );
}

/// Kind-awareness must not turn an unknown archive into a pass.
#[test]
fn unrecognised_archive_fails_loudly_and_names_both_shapes() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = write_zip(
        temp.path(),
        "mystery.gtpack",
        &[("README.txt".to_string(), b"hello".to_vec())],
    );

    let output = doctor(&path, &[]);
    let text = combined(&output);

    assert!(!output.status.success(), "expected exit 1, got: {text}");
    assert!(text.contains("unrecognised .gtpack shape"), "got: {text}");
    assert!(text.contains("manifest.cbor"), "got: {text}");
    assert!(text.contains("manifest.json"), "got: {text}");
    assert!(
        text.contains("README.txt"),
        "must list what it found: {text}"
    );
}

/// Nor a genuinely broken file.
#[test]
fn non_zip_still_fails_as_an_invalid_archive() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = temp.path().join("garbage.gtpack");
    std::fs::write(&path, b"not a zip at all").expect("write garbage");

    let output = doctor(&path, &[]);
    let text = combined(&output);

    assert!(!output.status.success(), "expected exit 1, got: {text}");
    assert!(
        text.contains("is not a valid gtpack archive"),
        "expected the invalid-archive message, got: {text}"
    );
}

#[test]
fn json_format_reports_the_shape_and_omits_canonical_only_keys() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = DwPackBuilder::new().write(temp.path(), "json.gtpack");

    let output = doctor(&path, &["--format", "json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let payload: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|err| panic!("doctor must emit JSON ({err}): {stdout}"));

    assert_eq!(payload["archive_shape"], json!("dw-application"));
    assert_eq!(payload["pack"]["manifest_id"], json!("support-triage"));
    assert_eq!(payload["validation"]["has_errors"], json!(false));
    for canonical_only in ["sbom", "manifest", "static_routes"] {
        assert!(
            payload.get(canonical_only).is_none(),
            "`{canonical_only}` describes a shape this archive does not have: {stdout}"
        );
    }
}

/// `inspect` is documented as a deprecated alias of `doctor` and shares the
/// same handler; it must behave identically here.
#[test]
fn inspect_alias_behaves_like_doctor() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = DwPackBuilder::new().write(temp.path(), "alias.gtpack");

    let mut cmd = Command::cargo_bin("greentic-pack").expect("greentic-pack binary");
    let output = cmd
        .arg("inspect")
        .arg(&path)
        .arg("--no-flow-doctor")
        .output()
        .expect("run inspect");
    let text = combined(&output);

    assert!(output.status.success(), "expected exit 0, got: {text}");
    assert!(
        text.contains("Pack shape: DW application pack"),
        "alias must produce the same report, got: {text}"
    );
}

/// A manifest that is not JSON is a real problem and must still fail.
#[test]
fn malformed_manifest_json_fails() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = DwPackBuilder::new()
        .manifest(Some("{not json"))
        .write(temp.path(), "malformed.gtpack");

    let output = doctor(&path, &[]);
    let text = combined(&output);

    assert!(!output.status.success(), "expected exit 1, got: {text}");
    assert!(
        text.contains("PACK_DW_MANIFEST_INVALID_JSON"),
        "expected the invalid-json code, got: {text}"
    );
}
