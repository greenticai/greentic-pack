use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use greentic_types::ComponentManifest;
use greentic_types::decode_pack_manifest;
use greentic_types::pack::extensions::component_manifests::{
    ComponentManifestIndexV1, EXT_COMPONENT_MANIFEST_INDEX_V1, ManifestEncoding,
};
use greentic_types::pack_manifest::{ExtensionInline, PackManifest as GpackManifest};
use serde::Deserialize;
use serde_cbor;
use serde_json;
use sha2::{Digest, Sha256};
use x509_parser::pem::parse_x509_pem;
use x509_parser::prelude::*;
use zip::ZipArchive;

use crate::builder::{
    ComponentEntry, FlowEntry, ImportRef, PackManifest, PackMeta, SBOM_FORMAT,
    SIGNATURE_CHAIN_PATH, SIGNATURE_PATH, SbomEntry, SignatureEnvelope, hex_hash,
    signature_digest_from_entries,
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
    pub files: HashMap<String, Vec<u8>>,
    pub gpack_manifest: Option<GpackManifest>,
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

#[derive(Debug, Clone)]
pub struct ComponentManifestIndexState {
    pub present: bool,
    pub index: Option<ComponentManifestIndexV1>,
    pub error: Option<String>,
}

impl ComponentManifestIndexState {
    pub fn ok(&self) -> bool {
        !self.present || self.error.is_none()
    }
}

#[derive(Debug, Clone)]
pub struct ComponentManifestFileStatus {
    pub component_id: String,
    pub manifest_file: String,
    pub encoding: ManifestEncoding,
    pub content_hash: Option<String>,
    pub file_present: bool,
    pub hash_ok: Option<bool>,
    pub decoded: bool,
    pub inline_match: Option<bool>,
    pub error: Option<String>,
}

impl ComponentManifestFileStatus {
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
            && self.file_present
            && self.decoded
            && self.hash_ok.unwrap_or(true)
            && self.inline_match.unwrap_or(true)
    }
}

#[derive(Debug, Clone)]
pub struct ManifestFileVerificationReport {
    pub extension_present: bool,
    pub extension_error: Option<String>,
    pub entries: Vec<ComponentManifestFileStatus>,
}

impl ManifestFileVerificationReport {
    pub fn ok(&self) -> bool {
        if !self.extension_present {
            return true;
        }
        self.extension_error.is_none()
            && self.entries.iter().all(ComponentManifestFileStatus::is_ok)
    }

    pub fn first_error(&self) -> Option<String> {
        if let Some(err) = &self.extension_error {
            return Some(err.clone());
        }
        self.entries.iter().find_map(|status| status.error.clone())
    }
}

pub fn open_pack(path: &Path, policy: SigningPolicy) -> Result<PackLoad, PackVerifyResult> {
    match open_pack_inner(path, policy) {
        Ok(result) => Ok(result),
        Err(err) => Err(PackVerifyResult::from_error(err)),
    }
}

impl PackLoad {
    pub fn component_manifest_index_v1(&self) -> ComponentManifestIndexState {
        let mut state = ComponentManifestIndexState {
            present: false,
            index: None,
            error: None,
        };

        let manifest = match self.gpack_manifest.as_ref() {
            Some(manifest) => manifest,
            None => return state,
        };

        let Some(extension) = manifest
            .extensions
            .as_ref()
            .and_then(|map| map.get(EXT_COMPONENT_MANIFEST_INDEX_V1))
        else {
            return state;
        };
        state.present = true;

        let inline = match extension.inline.as_ref() {
            Some(inline) => inline,
            None => {
                state.error = Some("component manifest index missing inline payload".into());
                return state;
            }
        };

        let payload = match inline {
            ExtensionInline::Other(value) => value,
            _ => {
                state.error =
                    Some("component manifest index inline payload has unexpected shape".into());
                return state;
            }
        };

        match ComponentManifestIndexV1::from_extension_value(payload) {
            Ok(index) => state.index = Some(index),
            Err(err) => state.error = Some(err.to_string()),
        }

        state
    }

    pub fn get_component_manifest_prefer_file(
        &self,
        component_id: &str,
    ) -> Result<Option<ComponentManifest>> {
        let state = self.component_manifest_index_v1();
        if let Some(err) = state.error {
            return Err(anyhow!(err));
        }

        if let Some(entry) = state.index.as_ref().and_then(|index| {
            index
                .entries
                .iter()
                .find(|entry| entry.component_id == component_id)
        }) {
            if entry.encoding != ManifestEncoding::Cbor {
                bail!("unsupported manifest encoding {:?}", entry.encoding);
            }

            if let Some(bytes) = self.files.get(&entry.manifest_file) {
                if let Some(expected) = entry.content_hash.as_deref() {
                    let actual = sha256_prefixed(bytes);
                    if !expected.eq_ignore_ascii_case(&actual) {
                        bail!(
                            "manifest hash mismatch for {}: expected {}, got {}",
                            entry.manifest_file,
                            expected,
                            actual
                        );
                    }
                }

                let decoded: ComponentManifest =
                    serde_cbor::from_slice(bytes).context("decode component manifest")?;
                if decoded.id.to_string() != entry.component_id {
                    bail!(
                        "manifest id {} does not match index component_id {}",
                        decoded.id,
                        entry.component_id
                    );
                }
                return Ok(Some(decoded));
            }
        }

        if let Some(component) = self.gpack_manifest.as_ref().and_then(|manifest| {
            manifest
                .components
                .iter()
                .find(|c| c.id.to_string() == component_id)
        }) {
            return Ok(Some(component.clone()));
        }

        Ok(None)
    }

    pub fn verify_component_manifest_files(&self) -> ManifestFileVerificationReport {
        let mut report = ManifestFileVerificationReport {
            extension_present: false,
            extension_error: None,
            entries: Vec::new(),
        };

        let state = self.component_manifest_index_v1();
        if !state.present {
            return report;
        }
        report.extension_present = true;

        let Some(index) = state.index else {
            report.extension_error = state.error;
            return report;
        };

        let inline_components = self
            .gpack_manifest
            .as_ref()
            .map(|manifest| &manifest.components);

        for entry in index.entries {
            let mut status = ComponentManifestFileStatus {
                component_id: entry.component_id.clone(),
                manifest_file: entry.manifest_file.clone(),
                encoding: entry.encoding.clone(),
                content_hash: entry.content_hash.clone(),
                file_present: false,
                hash_ok: None,
                decoded: false,
                inline_match: None,
                error: None,
            };

            if entry.encoding != ManifestEncoding::Cbor {
                status.error = Some("unsupported manifest encoding (expected cbor)".into());
                report.entries.push(status);
                continue;
            }

            let Some(bytes) = self.files.get(&entry.manifest_file) else {
                status.error = Some("manifest file missing from archive".into());
                report.entries.push(status);
                continue;
            };
            status.file_present = true;

            if let Some(expected) = entry.content_hash.as_deref() {
                if !expected.starts_with("sha256:") {
                    status.hash_ok = Some(false);
                    status.error = Some("content_hash must use sha256:<hex>".into());
                    report.entries.push(status);
                    continue;
                }
                let actual = sha256_prefixed(bytes);
                let matches = expected.eq_ignore_ascii_case(&actual);
                status.hash_ok = Some(matches);
                if !matches {
                    status.error = Some(format!(
                        "manifest hash mismatch: expected {}, got {}",
                        expected, actual
                    ));
                }
            }

            match serde_cbor::from_slice::<ComponentManifest>(bytes) {
                Ok(decoded) => {
                    status.decoded = true;
                    if decoded.id.to_string() != entry.component_id {
                        status.error.get_or_insert_with(|| {
                            format!(
                                "component id mismatch: index has {}, manifest has {}",
                                entry.component_id, decoded.id
                            )
                        });
                    }

                    if let Some(inline_components) = inline_components {
                        if let Some(inline) = inline_components.iter().find(|c| c.id == decoded.id)
                        {
                            let matches = inline == &decoded;
                            status.inline_match = Some(matches);
                            if !matches {
                                status.error.get_or_insert_with(|| {
                                    "external manifest differs from inline manifest".into()
                                });
                            }
                        } else {
                            status.inline_match = Some(false);
                            status.error.get_or_insert_with(|| {
                                "component missing from inline manifest".into()
                            });
                        }
                    }
                }
                Err(err) => {
                    status
                        .error
                        .get_or_insert_with(|| format!("failed to decode manifest: {err}"));
                }
            }

            report.entries.push(status);
        }

        report
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
    let decoded_gpack_manifest = decode_pack_manifest(&manifest_bytes).ok();
    match decode_manifest(&manifest_bytes).context("manifest.cbor is invalid")? {
        ManifestModel::Pack(manifest) => {
            let manifest = *manifest;
            let (sbom_doc, sbom_bytes, sbom_name) = read_sbom_required(&files)?;
            if sbom_doc.format != SBOM_FORMAT {
                bail!("unexpected SBOM format: {}", sbom_doc.format);
            }

            let mut warnings = Vec::new();
            verify_sbom(&files, &sbom_doc.files, sbom_name)?;
            let signature_ok = match (
                files.get(SIGNATURE_PATH),
                files.get(SIGNATURE_CHAIN_PATH),
                Some(&sbom_bytes),
            ) {
                (Some(_), Some(_), Some(sbom_bytes)) => match verify_signature(
                    &files,
                    &manifest_bytes,
                    sbom_bytes,
                    &sbom_doc.files,
                    policy,
                    &mut warnings,
                ) {
                    Ok(()) => true,
                    Err(err) => {
                        if matches!(policy, SigningPolicy::Strict) {
                            return Err(err);
                        }
                        warnings.push(format!("signature verification failed: {err}"));
                        false
                    }
                },
                (None, None, _) => match policy {
                    SigningPolicy::Strict => {
                        bail!("signature file `{}` missing", SIGNATURE_PATH)
                    }
                    SigningPolicy::DevOk => {
                        warnings.push("signature files missing; skipping verification".into());
                        false
                    }
                },
                _ => {
                    match policy {
                        SigningPolicy::Strict => bail!("signature files incomplete; missing chain"),
                        SigningPolicy::DevOk => warnings
                            .push("signature files incomplete; skipping verification".into()),
                    }
                    false
                }
            };

            Ok(PackLoad {
                manifest,
                report: VerifyReport {
                    signature_ok,
                    sbom_ok: true,
                    warnings,
                },
                sbom: sbom_doc.files,
                files,
                gpack_manifest: decoded_gpack_manifest,
            })
        }
        ManifestModel::Gpack(manifest) => {
            let manifest = *manifest;
            let mut warnings = Vec::new();
            if manifest.schema_version != "pack-v1" {
                warnings.push(format!(
                    "detected manifest schema {}; applying compatibility reader",
                    manifest.schema_version
                ));
            }

            let (sbom, sbom_ok, sbom_bytes, sbom_name) = read_sbom_optional(&files, &mut warnings);

            let signature_ok = match (
                files.get(SIGNATURE_PATH),
                files.get(SIGNATURE_CHAIN_PATH),
                sbom_bytes.as_deref(),
                sbom_ok,
            ) {
                (Some(_), Some(_), Some(sbom_bytes), true) => {
                    match verify_signature(
                        &files,
                        &manifest_bytes,
                        sbom_bytes,
                        &sbom,
                        policy,
                        &mut warnings,
                    ) {
                        Ok(()) => true,
                        Err(err) => {
                            warnings.push(format!("signature verification failed: {err}"));
                            false
                        }
                    }
                }
                (Some(_), Some(_), Some(_), false) => {
                    warnings.push(
                        "signature present but sbom validation failed; skipping verification"
                            .into(),
                    );
                    false
                }
                (Some(_), Some(_), None, _) => {
                    warnings.push(format!(
                        "signature present but {} missing; skipping verification",
                        sbom_name
                    ));
                    false
                }
                (None, None, _, _) => {
                    warnings.push("signature files missing; skipping verification".into());
                    false
                }
                _ => {
                    warnings.push("signature files incomplete; skipping verification".into());
                    false
                }
            };

            Ok(PackLoad {
                manifest: convert_gpack_manifest(&manifest, &files),
                report: VerifyReport {
                    signature_ok,
                    sbom_ok,
                    warnings,
                },
                sbom,
                files,
                gpack_manifest: Some(manifest),
            })
        }
    }
}

#[derive(Deserialize)]
struct SbomDocument {
    format: String,
    files: Vec<SbomEntry>,
}

fn verify_sbom(
    files: &HashMap<String, Vec<u8>>,
    entries: &[SbomEntry],
    sbom_name: &str,
) -> Result<()> {
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
        if path == SIGNATURE_PATH
            || path == SIGNATURE_CHAIN_PATH
            || path == "sbom.json"
            || path == "sbom.cbor"
        {
            continue;
        }
        if !listed.contains(path) {
            bail!("file `{}` missing from {}", path, sbom_name);
        }
    }

    Ok(())
}

fn read_sbom_required(
    files: &HashMap<String, Vec<u8>>,
) -> Result<(SbomDocument, Vec<u8>, &'static str)> {
    if let Some(sbom_bytes) = files.get("sbom.cbor") {
        let sbom_doc: SbomDocument =
            serde_cbor::from_slice(sbom_bytes).context("sbom.cbor is not valid CBOR")?;
        return Ok((sbom_doc, sbom_bytes.clone(), "sbom.cbor"));
    }
    if let Some(sbom_bytes) = files.get("sbom.json") {
        let sbom_doc: SbomDocument =
            serde_json::from_slice(sbom_bytes).context("sbom.json is not valid JSON")?;
        return Ok((sbom_doc, sbom_bytes.clone(), "sbom.json"));
    }
    Err(anyhow!("sbom.cbor missing from archive"))
}

fn read_sbom_optional(
    files: &HashMap<String, Vec<u8>>,
    warnings: &mut Vec<String>,
) -> (Vec<SbomEntry>, bool, Option<Vec<u8>>, &'static str) {
    if let Some(sbom_bytes) = files.get("sbom.cbor") {
        match serde_cbor::from_slice::<SbomDocument>(sbom_bytes) {
            Ok(sbom_doc) => {
                let mut ok = sbom_doc.format == SBOM_FORMAT;
                if !ok {
                    warnings.push(format!("unexpected SBOM format: {}", sbom_doc.format));
                }
                match verify_sbom(files, &sbom_doc.files, "sbom.cbor") {
                    Ok(()) => {}
                    Err(err) => {
                        warnings.push(err.to_string());
                        ok = false;
                    }
                }
                return (sbom_doc.files, ok, Some(sbom_bytes.clone()), "sbom.cbor");
            }
            Err(err) => {
                warnings.push(format!("sbom.cbor is not valid CBOR: {err}"));
                return (Vec::new(), false, Some(sbom_bytes.clone()), "sbom.cbor");
            }
        }
    }
    if let Some(sbom_bytes) = files.get("sbom.json") {
        match serde_json::from_slice::<SbomDocument>(sbom_bytes) {
            Ok(sbom_doc) => {
                let mut ok = sbom_doc.format == SBOM_FORMAT;
                if !ok {
                    warnings.push(format!("unexpected SBOM format: {}", sbom_doc.format));
                }
                match verify_sbom(files, &sbom_doc.files, "sbom.json") {
                    Ok(()) => {}
                    Err(err) => {
                        warnings.push(err.to_string());
                        ok = false;
                    }
                }
                return (sbom_doc.files, ok, Some(sbom_bytes.clone()), "sbom.json");
            }
            Err(err) => {
                warnings.push(format!("sbom.json is not valid JSON: {err}"));
                return (Vec::new(), false, Some(sbom_bytes.clone()), "sbom.json");
            }
        }
    }
    warnings.push("sbom.cbor missing; synthesized inventory for validation".into());
    (synthesize_sbom(files), false, None, "sbom.cbor")
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

#[derive(Debug)]
enum ManifestModel {
    Pack(Box<PackManifest>),
    Gpack(Box<GpackManifest>),
}

fn decode_manifest(bytes: &[u8]) -> Result<ManifestModel> {
    if let Ok(manifest) = serde_cbor::from_slice::<PackManifest>(bytes) {
        return Ok(ManifestModel::Pack(Box::new(manifest)));
    }

    let manifest = decode_pack_manifest(bytes)?;
    Ok(ManifestModel::Gpack(Box::new(manifest)))
}

fn synthesize_sbom(files: &HashMap<String, Vec<u8>>) -> Vec<SbomEntry> {
    let mut entries: Vec<_> = files
        .iter()
        .filter(|(path, _)| *path != SIGNATURE_PATH && *path != SIGNATURE_CHAIN_PATH)
        .map(|(path, data)| SbomEntry {
            path: path.clone(),
            size: data.len() as u64,
            hash_blake3: hex_hash(data),
            media_type: media_type_for(path).to_string(),
        })
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

fn media_type_for(path: &str) -> &'static str {
    if path.ends_with(".cbor") {
        "application/cbor"
    } else if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".wasm") {
        "application/wasm"
    } else if path.ends_with(".yaml") || path.ends_with(".yml") {
        "application/yaml"
    } else {
        "application/octet-stream"
    }
}

fn sha256_prefixed(bytes: &[u8]) -> String {
    let mut sha = Sha256::new();
    sha.update(bytes);
    format!("sha256:{:x}", sha.finalize())
}

fn convert_gpack_manifest(
    manifest: &GpackManifest,
    files: &HashMap<String, Vec<u8>>,
) -> PackManifest {
    let publisher = manifest.publisher.clone();
    let entry_flows = derive_entry_flows(manifest);
    let imports = manifest
        .dependencies
        .iter()
        .map(|dep| ImportRef {
            pack_id: dep.pack_id.to_string(),
            version_req: dep.version_req.to_string(),
        })
        .collect();
    let flows = manifest.flows.iter().map(convert_gpack_flow).collect();
    let components = manifest
        .components
        .iter()
        .map(|component| {
            let file_wasm = format!("components/{}.wasm", component.id);
            ComponentEntry {
                name: component.id.to_string(),
                version: component.version.clone(),
                file_wasm: file_wasm.clone(),
                hash_blake3: component_hash(&file_wasm, files),
                schema_file: None,
                manifest_file: None,
                world: Some(component.world.clone()),
                capabilities: serde_json::to_value(&component.capabilities).ok(),
            }
        })
        .collect();

    PackManifest {
        meta: PackMeta {
            pack_version: crate::builder::PACK_VERSION,
            pack_id: manifest.pack_id.to_string(),
            version: manifest.version.clone(),
            name: manifest.pack_id.to_string(),
            kind: None,
            description: None,
            authors: if publisher.is_empty() {
                Vec::new()
            } else {
                vec![publisher]
            },
            license: None,
            homepage: None,
            support: None,
            vendor: None,
            imports,
            entry_flows,
            created_at_utc: "1970-01-01T00:00:00Z".into(),
            events: None,
            repo: None,
            messaging: None,
            interfaces: Vec::new(),
            annotations: Default::default(),
            distribution: None,
            components: Vec::new(),
        },
        flows,
        components,
        distribution: None,
        component_descriptors: Vec::new(),
    }
}

fn convert_gpack_flow(entry: &greentic_types::pack_manifest::PackFlowEntry) -> FlowEntry {
    let flow_bytes = serde_json::to_vec(&entry.flow).unwrap_or_default();
    let entry_point = entry
        .entrypoints
        .first()
        .cloned()
        .or_else(|| entry.flow.entrypoints.keys().next().cloned())
        .unwrap_or_else(|| entry.id.to_string());

    FlowEntry {
        id: entry.id.to_string(),
        kind: entry.flow.schema_version.clone(),
        entry: entry_point,
        file_yaml: format!("flows/{}/flow.ygtc", entry.id),
        file_json: format!("flows/{}/flow.json", entry.id),
        hash_blake3: hex_hash(&flow_bytes),
    }
}

fn derive_entry_flows(manifest: &GpackManifest) -> Vec<String> {
    let mut entries = Vec::new();
    for flow in &manifest.flows {
        if flow.entrypoints.is_empty() && flow.flow.entrypoints.is_empty() {
            entries.push(flow.id.to_string());
            continue;
        }
        entries.extend(flow.entrypoints.iter().cloned());
        entries.extend(flow.flow.entrypoints.keys().cloned());
    }
    if entries.is_empty() {
        entries.push(manifest.pack_id.to_string());
    }
    entries.sort();
    entries.dedup();
    entries
}

fn component_hash(path: &str, files: &HashMap<String, Vec<u8>>) -> String {
    files
        .get(path)
        .map(|bytes| hex_hash(bytes))
        .unwrap_or_default()
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
    fn open_pack_warns_missing_signature_dev_policy() {
        let (_dir, path) = build_pack(false);
        let load = open_pack(&path, SigningPolicy::DevOk).expect("dev policy tolerates");
        assert!(
            load.report
                .warnings
                .iter()
                .any(|w| w.contains("signature files missing")),
            "expected warning about missing signatures"
        );
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
