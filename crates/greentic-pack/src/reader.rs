use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use x509_parser::pem::parse_x509_pem;
use x509_parser::prelude::*;
use zip::ZipArchive;

use crate::builder::{
    PackManifest, SBOM_FORMAT, SIGNATURE_CHAIN_PATH, SIGNATURE_PATH, SbomEntry, SignatureEnvelope,
    hex_hash, signature_digest_from_entries,
};

#[cfg(test)]
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024;
#[cfg(not(test))]
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;

#[cfg(test)]
const MAX_FILE_BYTES: u64 = 64 * 1024;
#[cfg(not(test))]
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningPolicy {
    DevOk,
    Strict,
}

#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    pub signature_ok: bool,
    pub sbom_ok: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PackLoad {
    pub manifest: PackManifest,
    pub report: VerifyReport,
    pub sbom: Vec<SbomEntry>,
}

#[derive(Debug, Clone)]
pub struct PackVerifyResult {
    pub message: String,
}

impl PackVerifyResult {
    fn from_error(err: anyhow::Error) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

pub fn open_pack(path: &Path, policy: SigningPolicy) -> Result<PackLoad, PackVerifyResult> {
    match open_pack_inner(path, policy) {
        Ok(result) => Ok(result),
        Err(err) => Err(PackVerifyResult::from_error(err)),
    }
}

fn open_pack_inner(path: &Path, policy: SigningPolicy) -> Result<PackLoad> {
    let mut archive = ZipArchive::new(
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?,
    )
    .with_context(|| format!("{} is not a valid gtpack archive", path.display()))?;

    let (files, total) = read_archive_entries(&mut archive)?;
    if total > MAX_ARCHIVE_BYTES {
        bail!(
            "gtpack archive exceeds maximum allowed size ({} bytes)",
            MAX_ARCHIVE_BYTES
        );
    }

    let manifest_bytes = files
        .get("manifest.cbor")
        .cloned()
        .ok_or_else(|| anyhow!("manifest.cbor missing from archive"))?;
    let manifest: PackManifest =
        serde_cbor::from_slice(&manifest_bytes).context("manifest.cbor is invalid")?;

    let sbom_bytes = files
        .get("sbom.json")
        .cloned()
        .ok_or_else(|| anyhow!("sbom.json missing from archive"))?;
    let sbom_doc: SbomDocument =
        serde_json::from_slice(&sbom_bytes).context("sbom.json is not valid JSON")?;
    if sbom_doc.format != SBOM_FORMAT {
        bail!("unexpected SBOM format: {}", sbom_doc.format);
    }

    let mut warnings = Vec::new();
    verify_sbom(&files, &sbom_doc.files)?;
    verify_signature(
        &files,
        &manifest_bytes,
        &sbom_bytes,
        &sbom_doc.files,
        policy,
        &mut warnings,
    )?;

    Ok(PackLoad {
        manifest,
        report: VerifyReport {
            signature_ok: true,
            sbom_ok: true,
            warnings,
        },
        sbom: sbom_doc.files,
    })
}

#[derive(Deserialize)]
struct SbomDocument {
    format: String,
    files: Vec<SbomEntry>,
}

fn verify_sbom(files: &HashMap<String, Vec<u8>>, entries: &[SbomEntry]) -> Result<()> {
    let mut listed = HashSet::new();
    for entry in entries {
        let data = files
            .get(&entry.path)
            .ok_or_else(|| anyhow!("sbom references missing file `{}`", entry.path))?;
        let actual = hex_hash(data);
        if !actual.eq_ignore_ascii_case(&entry.hash_blake3) {
            bail!(
                "hash mismatch for {}: expected {}, found {}",
                entry.path,
                entry.hash_blake3,
                actual
            );
        }
        listed.insert(entry.path.clone());
    }

    for path in files.keys() {
        if path == SIGNATURE_PATH || path == SIGNATURE_CHAIN_PATH || path == "sbom.json" {
            continue;
        }
        if !listed.contains(path) {
            bail!("file `{}` missing from sbom.json", path);
        }
    }

    Ok(())
}

fn verify_signature(
    files: &HashMap<String, Vec<u8>>,
    manifest_bytes: &[u8],
    sbom_bytes: &[u8],
    entries: &[SbomEntry],
    policy: SigningPolicy,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let signature_bytes = files
        .get(SIGNATURE_PATH)
        .ok_or_else(|| anyhow!("signature file `{}` missing", SIGNATURE_PATH))?;
    let chain_bytes = files
        .get(SIGNATURE_CHAIN_PATH)
        .ok_or_else(|| anyhow!("certificate chain `{}` missing", SIGNATURE_CHAIN_PATH))?;

    let envelope: SignatureEnvelope =
        serde_json::from_slice(signature_bytes).context("signatures/pack.sig is not valid JSON")?;
    let digest = signature_digest_from_entries(entries, manifest_bytes, sbom_bytes);
    let digest_hex = digest.to_hex().to_string();
    if !digest_hex.eq_ignore_ascii_case(&envelope.digest) {
        bail!("signature digest mismatch");
    }

    match envelope.alg.to_ascii_lowercase().as_str() {
        "ed25519" => verify_ed25519_signature(&envelope, digest, chain_bytes, policy, warnings)?,
        other => bail!("unsupported signature algorithm: {}", other),
    }

    Ok(())
}

fn verify_ed25519_signature(
    envelope: &SignatureEnvelope,
    digest: blake3::Hash,
    chain_bytes: &[u8],
    policy: SigningPolicy,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let sig_raw = URL_SAFE_NO_PAD
        .decode(envelope.sig.as_bytes())
        .map_err(|err| anyhow!("invalid signature encoding: {err}"))?;
    let sig_array: [u8; 64] = sig_raw
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("signature must be 64 bytes"))?;
    let signature = Signature::from_bytes(&sig_array);

    let cert_der = parse_certificate_chain(chain_bytes)?;
    enforce_policy(&cert_der, policy, warnings)?;
    let first_cert = parse_certificate(&cert_der[0])?;
    let verifying_key = extract_ed25519_key(&first_cert)?;
    verifying_key
        .verify(digest.as_bytes(), &signature)
        .map_err(|err| anyhow!("signature verification failed: {err}"))?;
    Ok(())
}

fn extract_ed25519_key(cert: &X509Certificate<'_>) -> Result<VerifyingKey> {
    let spki = cert.public_key();
    let key_bytes = spki.subject_public_key.data.as_ref();
    if key_bytes.len() != 32 {
        bail!(
            "expected 32-byte Ed25519 public key, found {} bytes",
            key_bytes.len()
        );
    }
    let mut raw = [0u8; 32];
    raw.copy_from_slice(key_bytes);
    VerifyingKey::from_bytes(&raw).map_err(|err| anyhow!("invalid ed25519 key: {err}"))
}

fn parse_certificate(bytes: &[u8]) -> Result<X509Certificate<'_>> {
    let (_, cert) =
        X509Certificate::from_der(bytes).map_err(|err| anyhow!("invalid certificate: {err}"))?;
    Ok(cert)
}

fn parse_certificate_chain(mut data: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut certs = Vec::new();
    loop {
        data = trim_leading(data);
        if data.is_empty() {
            break;
        }
        let (rest, pem) = parse_x509_pem(data).map_err(|err| anyhow!("invalid PEM: {err}"))?;
        if pem.label != "CERTIFICATE" {
            bail!("unexpected PEM label {}; expected CERTIFICATE", pem.label);
        }
        certs.push(pem.contents.to_vec());
        data = rest;
    }

    if certs.is_empty() {
        bail!("certificate chain is empty");
    }

    Ok(certs)
}

fn enforce_policy(
    certs: &[Vec<u8>],
    policy: SigningPolicy,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let first = certs
        .first()
        .ok_or_else(|| anyhow!("certificate chain is empty"))?;
    let first_cert = parse_certificate(first)?;
    let is_dev = is_dev_certificate(&first_cert);

    match policy {
        SigningPolicy::DevOk => {
            if certs.len() != 1 {
                warnings.push(format!(
                    "chain contains {} certificates; dev mode expects exactly 1",
                    certs.len()
                ));
            }
        }
        SigningPolicy::Strict => {
            if is_dev {
                bail!("dev self-signed certificate is not allowed under strict policy");
            }
        }
    }

    Ok(())
}

fn is_dev_certificate(cert: &X509Certificate<'_>) -> bool {
    let cn_matches = cert
        .subject()
        .iter_common_name()
        .flat_map(|attr| attr.as_str())
        .any(|cn| cn == "greentic-dev-local");
    cn_matches && (cert.subject() == cert.issuer())
}

fn trim_leading(mut data: &[u8]) -> &[u8] {
    while let Some((&byte, rest)) = data.split_first() {
        if byte.is_ascii_whitespace() {
            data = rest;
        } else {
            break;
        }
    }
    data
}

fn read_archive_entries<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<(HashMap<String, Vec<u8>>, u64)> {
    let mut files = HashMap::new();
    let mut total = 0u64;

    for idx in 0..archive.len() {
        let mut entry = archive
            .by_index(idx)
            .with_context(|| format!("failed to read entry #{idx}"))?;

        if entry.is_dir() {
            continue;
        }
        if !entry.is_file() {
            bail!("archive entry {} is not a regular file", entry.name());
        }

        if let Some(mode) = entry.unix_mode() {
            let file_type = mode & 0o170000;
            if file_type != 0o100000 {
                bail!(
                    "unsupported file type for entry {}; only regular files are allowed",
                    entry.name()
                );
            }
        }

        let enclosed_path = entry
            .enclosed_name()
            .ok_or_else(|| anyhow!("archive entry contains unsafe path: {}", entry.name()))?
            .to_path_buf();
        let logical = normalize_entry_path(&enclosed_path)?;
        if files.contains_key(&logical) {
            bail!("duplicate entry detected: {}", logical);
        }

        let size = entry.size();
        if size > MAX_FILE_BYTES {
            bail!(
                "entry {} exceeds maximum allowed size of {} bytes",
                logical,
                MAX_FILE_BYTES
            );
        }

        total = total
            .checked_add(size)
            .ok_or_else(|| anyhow!("archive size overflow"))?;

        let mut buf = Vec::with_capacity(size as usize);
        entry
            .read_to_end(&mut buf)
            .with_context(|| format!("failed to read {}", logical))?;
        files.insert(logical, buf);
    }

    Ok((files, total))
}

fn normalize_entry_path(path: &Path) -> Result<String> {
    if path.is_absolute() {
        bail!("archive entry uses absolute path: {}", path.display());
    }

    if path.components().any(|comp| {
        matches!(
            comp,
            std::path::Component::ParentDir | std::path::Component::RootDir
        )
    }) {
        bail!(
            "archive entry contains invalid path segments: {}",
            path.display()
        );
    }

    let mut normalized = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Normal(seg) => {
                let segment = seg
                    .to_str()
                    .ok_or_else(|| anyhow!("entry contains non-utf8 segment"))?;
                if segment.is_empty() {
                    bail!("entry contains empty path segment");
                }
                normalized.push(segment.replace('\\', "/"));
            }
            std::path::Component::CurDir => continue,
            _ => bail!(
                "archive entry contains unsupported segment: {}",
                path.display()
            ),
        }
    }

    if normalized.is_empty() {
        bail!("archive entry lacks a valid filename");
    }

    Ok(normalized.join("/"))
}

#[cfg(test)]
mod tests {
    use super::{MAX_ARCHIVE_BYTES, MAX_FILE_BYTES, SigningPolicy, open_pack};
    use crate::builder::SIGNATURE_CHAIN_PATH;
    use crate::builder::{
        ComponentArtifact, FlowBundle, PackBuilder, PackMeta, Provenance, Signing,
    };
    use blake3;
    use semver::Version;
    use serde_json::{Map, json};
    use std::fs::{self, File};
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};
    use tempfile::{TempDir, tempdir};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    #[test]
    fn open_pack_succeeds_for_dev_signature() {
        let (_dir, path) = build_pack(true);
        let load = open_pack(&path, SigningPolicy::DevOk).expect("reader validates pack");
        assert_eq!(load.manifest.meta.pack_id, "ai.greentic.demo.reader");
        assert!(load.report.warnings.is_empty());
    }

    #[test]
    fn open_pack_rejects_missing_signature() {
        let (_dir, path) = build_pack(false);
        let err = open_pack(&path, SigningPolicy::DevOk).unwrap_err();
        assert!(err.message.contains("signature"));
    }

    #[test]
    fn strict_policy_rejects_dev_certificate() {
        let (_dir, path) = build_pack(true);
        let err = open_pack(&path, SigningPolicy::Strict).unwrap_err();
        assert!(err.message.contains("strict"));
    }

    #[test]
    fn dev_policy_warns_for_multi_certificate_chain() {
        let (_dir, original) = build_pack(true);
        let (_tmp, rewritten) = duplicate_chain(&original);
        let load = open_pack(&rewritten, SigningPolicy::DevOk).expect("dev policy accepts");
        assert!(load.report.warnings.iter().any(|msg| msg.contains("chain")));
    }

    #[test]
    fn path_traversal_entry_is_rejected() {
        let (_dir, path) = custom_zip(&[zip_entry("../evil", b"oops")]);
        let err = open_pack(&path, SigningPolicy::DevOk).unwrap_err();
        assert!(err.message.contains("unsafe path") || err.message.contains("invalid path"));
    }

    #[test]
    fn symlink_entry_is_rejected() {
        let (dir, path) = custom_zip(&[zip_entry("foo", b"bar")]);
        patch_external_attributes(&path, 0o120777 << 16);
        let err = open_pack(&path, SigningPolicy::DevOk).unwrap_err();
        assert!(
            err.message.contains("unsupported file type")
                || err.message.contains("not a regular file")
        );
        drop(dir);
    }

    #[test]
    fn oversized_entry_is_rejected() {
        let huge = vec![0u8; (MAX_FILE_BYTES + 1) as usize];
        let (_dir, path) = custom_zip(&[zip_entry("huge.bin", &huge)]);
        let err = open_pack(&path, SigningPolicy::DevOk).unwrap_err();
        assert!(err.message.contains("exceeds maximum"));
    }

    #[test]
    fn oversized_archive_is_rejected() {
        let chunk = vec![0u8; (MAX_FILE_BYTES / 2) as usize];
        let needed = (MAX_ARCHIVE_BYTES / chunk.len() as u64) + 1;
        let mut entries = Vec::new();
        for idx in 0..needed {
            let name = format!("chunk{idx}");
            entries.push((name, chunk.clone()));
        }
        let (_dir, path) = custom_zip(&entries);
        let err = open_pack(&path, SigningPolicy::DevOk).unwrap_err();
        assert!(err.message.contains("archive exceeds"));
    }

    fn temp_wasm(dir: &Path) -> PathBuf {
        let path = dir.join("component.wasm");
        std::fs::write(&path, [0x00u8, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]).unwrap();
        path
    }

    fn sample_meta() -> PackMeta {
        PackMeta {
            pack_version: crate::builder::PACK_VERSION,
            pack_id: "ai.greentic.demo.reader".into(),
            version: Version::parse("0.1.0").unwrap(),
            name: "Reader Demo".into(),
            kind: None,
            description: None,
            authors: vec!["Greentic".into()],
            license: None,
            homepage: None,
            support: None,
            vendor: None,
            imports: vec![],
            entry_flows: vec!["demo".into()],
            created_at_utc: "2025-01-01T00:00:00Z".into(),
            events: None,
            repo: None,
            messaging: None,
            interfaces: Vec::new(),
            annotations: Map::new(),
            distribution: None,
            components: Vec::new(),
        }
    }

    fn sample_flow() -> FlowBundle {
        let json = json!({
            "id": "demo",
            "kind": "flow/v1",
            "entry": "start",
            "nodes": []
        });
        FlowBundle {
            id: "demo".into(),
            kind: "flow/v1".into(),
            entry: "start".into(),
            yaml: "id: demo\nentry: start\n".into(),
            json: json.clone(),
            hash_blake3: blake3::hash(&serde_json::to_vec(&json).unwrap())
                .to_hex()
                .to_string(),
            nodes: Vec::new(),
        }
    }

    fn sample_provenance() -> Provenance {
        Provenance {
            builder: "greentic-pack@test".into(),
            git_commit: Some("abc123".into()),
            git_repo: None,
            toolchain: None,
            built_at_utc: "2025-01-01T00:00:00Z".into(),
            host: None,
            notes: None,
        }
    }

    fn build_pack(include_signature: bool) -> (TempDir, PathBuf) {
        let dir = tempdir().unwrap();
        let wasm = temp_wasm(dir.path());
        let out = dir.path().join("demo.gtpack");
        let mut builder = PackBuilder::new(sample_meta())
            .with_flow(sample_flow())
            .with_component(ComponentArtifact {
                name: "demo".into(),
                version: Version::parse("1.0.0").unwrap(),
                wasm_path: wasm,
                schema_json: None,
                manifest_json: None,
                capabilities: None,
                world: None,
                hash_blake3: None,
            })
            .with_provenance(sample_provenance());
        if !include_signature {
            builder = builder.with_signing(Signing::None);
        }
        builder.build(&out).unwrap();
        (dir, out)
    }

    fn custom_zip(entries: &[(String, Vec<u8>)]) -> (TempDir, PathBuf) {
        use zip::DateTime;

        let dir = tempdir().unwrap();
        let path = dir.path().join("custom.gtpack");
        let file = File::create(&path).unwrap();
        let mut writer = ZipWriter::new(file);
        let timestamp = DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).unwrap();
        for (name, data) in entries.iter() {
            let options = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .last_modified_time(timestamp)
                .unix_permissions(0o644);
            writer.start_file(name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
        (dir, path)
    }

    fn zip_entry(name: &str, data: &[u8]) -> (String, Vec<u8>) {
        (name.to_string(), data.to_vec())
    }

    fn patch_external_attributes(path: &Path, attr: u32) {
        let mut bytes = fs::read(path).unwrap();
        let signature = [0x50, 0x4b, 0x01, 0x02];
        let pos = bytes
            .windows(4)
            .rposition(|window| window == signature)
            .expect("central directory missing");
        let attr_pos = pos + 38;
        bytes[attr_pos..attr_pos + 4].copy_from_slice(&attr.to_le_bytes());
        fs::write(path, bytes).unwrap();
    }

    fn duplicate_chain(original: &Path) -> (TempDir, PathBuf) {
        use zip::DateTime;

        let mut archive = ZipArchive::new(File::open(original).unwrap()).unwrap();
        let dir = tempdir().unwrap();
        let new_path = dir.path().join("rewritten.gtpack");
        let file = File::create(&new_path).unwrap();
        let mut writer = ZipWriter::new(file);
        let timestamp = DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).unwrap();

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            if entry.name() == SIGNATURE_CHAIN_PATH {
                let original = data.clone();
                data.push(b'\n');
                data.extend_from_slice(&original);
            }
            let options = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .last_modified_time(timestamp)
                .unix_permissions(0o644);
            writer.start_file(entry.name(), options).unwrap();
            writer.write_all(&data).unwrap();
        }

        writer.finish().unwrap();
        (dir, new_path)
    }
}
