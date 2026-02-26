use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use blake3::Hasher;
use ed25519_dalek::Signer as _;
use ed25519_dalek::SigningKey;
use getrandom::fill as fill_random;
use greentic_types::cbor::canonical;
use pkcs8::EncodePrivateKey;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ED25519};
use rustls_pki_types::PrivatePkcs8KeyDer;
use schemars::JsonSchema;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime as ZipDateTime, ZipWriter};

use crate::events::EventsSection;
use crate::kind::PackKind;
use crate::messaging::MessagingSection;
use crate::repo::{InterfaceBinding, RepoPackSection};

pub(crate) const SBOM_FORMAT: &str = "greentic-sbom-v1";
pub(crate) const SIGNATURE_PATH: &str = "signatures/pack.sig";
pub(crate) const SIGNATURE_CHAIN_PATH: &str = "signatures/chain.pem";
pub const PACK_VERSION: u32 = 1;

fn default_pack_version() -> u32 {
    PACK_VERSION
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackMeta {
    #[serde(rename = "packVersion", default = "default_pack_version")]
    pub pack_version: u32,
    pub pack_id: String,
    pub version: Version,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<PackKind>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub support: Option<String>,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub imports: Vec<ImportRef>,
    pub entry_flows: Vec<String>,
    pub created_at_utc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<EventsSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<RepoPackSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messaging: Option<MessagingSection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<InterfaceBinding>,
    #[serde(default)]
    pub annotations: JsonMap<String, JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribution: Option<DistributionSection>,
    #[serde(default)]
    pub components: Vec<ComponentDescriptor>,
}

impl PackMeta {
    fn validate(&self) -> Result<()> {
        if self.pack_version != PACK_VERSION {
            bail!(
                "unsupported packVersion {}; expected {}",
                self.pack_version,
                PACK_VERSION
            );
        }
        if self.pack_id.trim().is_empty() {
            bail!("pack_id is required");
        }
        if self.name.trim().is_empty() {
            bail!("name is required");
        }
        if self.entry_flows.is_empty() {
            bail!("at least one entry flow is required");
        }
        if self.created_at_utc.trim().is_empty() {
            bail!("created_at_utc is required");
        }
        if let Some(kind) = &self.kind {
            kind.validate_allowed()?;
        }
        if let Some(events) = &self.events {
            events.validate()?;
        }
        if let Some(repo) = &self.repo {
            repo.validate()?;
        }
        if let Some(messaging) = &self.messaging {
            messaging.validate()?;
        }
        for binding in &self.interfaces {
            binding.validate("interfaces")?;
        }
        validate_distribution(self.kind.as_ref(), self.distribution.as_ref())?;
        validate_components(&self.components)?;
        Ok(())
    }
}

pub use greentic_flow::flow_bundle::{ComponentPin, FlowBundle, NodeRef};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportRef {
    pub pack_id: String,
    pub version_req: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ComponentDescriptor {
    pub component_id: String,
    pub version: String,
    pub digest: String,
    pub artifact_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DistributionSection {
    #[serde(default)]
    pub bundle_id: Option<String>,
    #[serde(default)]
    pub tenant: JsonMap<String, JsonValue>,
    pub environment_ref: String,
    pub desired_state_version: String,
    #[serde(default)]
    pub components: Vec<ComponentDescriptor>,
    #[serde(default)]
    pub platform_components: Vec<ComponentDescriptor>,
}

#[derive(Clone, Debug)]
pub struct ComponentArtifact {
    pub name: String,
    pub version: Version,
    pub wasm_path: PathBuf,
    pub schema_json: Option<String>,
    pub manifest_json: Option<String>,
    pub capabilities: Option<JsonValue>,
    pub world: Option<String>,
    pub hash_blake3: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provenance {
    pub builder: String,
    #[serde(default)]
    pub git_commit: Option<String>,
    #[serde(default)]
    pub git_repo: Option<String>,
    #[serde(default)]
    pub toolchain: Option<String>,
    pub built_at_utc: String,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExternalSignature {
    pub alg: String,
    pub sig: Vec<u8>,
}

pub trait Signer: Send + Sync {
    fn sign(&self, message: &[u8]) -> Result<ExternalSignature>;
    fn chain_pem(&self) -> Result<Vec<u8>>;
}

type DynSigner = dyn Signer + Send + Sync + 'static;

#[derive(Clone, Default)]
pub enum Signing {
    #[default]
    Dev,
    None,
    External(Arc<DynSigner>),
}

pub struct PackBuilder {
    meta: PackMeta,
    flows: Vec<FlowBundle>,
    components: Vec<ComponentArtifact>,
    assets: Vec<Asset>,
    signing: Signing,
    provenance: Option<Provenance>,
    component_descriptors: Vec<ComponentDescriptor>,
    distribution: Option<DistributionSection>,
}

struct Asset {
    path: String,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct BuildResult {
    pub out_path: PathBuf,
    pub manifest_hash_blake3: String,
    pub files: Vec<SbomEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SbomEntry {
    pub path: String,
    pub size: u64,
    pub hash_blake3: String,
    pub media_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackManifest {
    pub meta: PackMeta,
    pub flows: Vec<FlowEntry>,
    pub components: Vec<ComponentEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribution: Option<DistributionSection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub component_descriptors: Vec<ComponentDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowEntry {
    pub id: String,
    pub kind: String,
    pub entry: String,
    pub file_yaml: String,
    pub file_json: String,
    pub hash_blake3: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentEntry {
    pub name: String,
    pub version: Version,
    pub file_wasm: String,
    pub hash_blake3: String,
    pub schema_file: Option<String>,
    pub manifest_file: Option<String>,
    pub world: Option<String>,
    pub capabilities: Option<JsonValue>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SignatureEnvelope {
    pub alg: String,
    pub sig: String,
    pub digest: String,
    pub signed_at_utc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_fingerprint: Option<String>,
}

impl SignatureEnvelope {
    fn new(
        alg: impl Into<String>,
        sig_bytes: &[u8],
        digest: &blake3::Hash,
        key_fingerprint: Option<String>,
    ) -> Self {
        let signed_at = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
        Self {
            alg: alg.into(),
            sig: URL_SAFE_NO_PAD.encode(sig_bytes),
            digest: digest.to_hex().to_string(),
            signed_at_utc: signed_at,
            key_fingerprint,
        }
    }
}

struct PendingFile {
    path: String,
    media_type: String,
    bytes: Vec<u8>,
}

impl PendingFile {
    fn new(path: String, media_type: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            path,
            media_type: media_type.into(),
            bytes,
        }
    }

    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn hash(&self) -> String {
        hex_hash(&self.bytes)
    }
}

impl PackBuilder {
    pub fn new(meta: PackMeta) -> Self {
        Self {
            component_descriptors: meta.components.clone(),
            distribution: meta.distribution.clone(),
            meta,
            flows: Vec::new(),
            components: Vec::new(),
            assets: Vec::new(),
            signing: Signing::Dev,
            provenance: None,
        }
    }

    pub fn with_flow(mut self, flow: FlowBundle) -> Self {
        self.flows.push(flow);
        self
    }

    pub fn with_component(mut self, component: ComponentArtifact) -> Self {
        self.components.push(component);
        self
    }

    pub fn with_component_wasm(
        self,
        name: impl Into<String>,
        version: Version,
        wasm_path: impl Into<PathBuf>,
    ) -> Self {
        self.with_component(ComponentArtifact {
            name: name.into(),
            version,
            wasm_path: wasm_path.into(),
            schema_json: None,
            manifest_json: None,
            capabilities: None,
            world: None,
            hash_blake3: None,
        })
    }

    pub fn with_asset_bytes(mut self, path_in_pack: impl Into<String>, bytes: Vec<u8>) -> Self {
        self.assets.push(Asset {
            path: path_in_pack.into(),
            bytes,
        });
        self
    }

    pub fn with_signing(mut self, signing: Signing) -> Self {
        self.signing = signing;
        self
    }

    pub fn with_provenance(mut self, provenance: Provenance) -> Self {
        self.provenance = Some(provenance);
        self
    }

    pub fn with_component_descriptors(
        mut self,
        descriptors: impl IntoIterator<Item = ComponentDescriptor>,
    ) -> Self {
        self.component_descriptors.extend(descriptors);
        self
    }

    pub fn with_distribution(mut self, distribution: DistributionSection) -> Self {
        self.distribution = Some(distribution);
        self
    }

    pub fn build(self, out_path: impl AsRef<Path>) -> Result<BuildResult> {
        let meta = self.meta;
        meta.validate()?;
        let distribution = self.distribution.or_else(|| meta.distribution.clone());
        let component_descriptors = if self.component_descriptors.is_empty() {
            meta.components.clone()
        } else {
            self.component_descriptors.clone()
        };

        if self.flows.is_empty() {
            bail!("at least one flow must be provided");
        }

        let mut flow_entries = Vec::new();
        let mut pending_files: Vec<PendingFile> = Vec::new();
        let mut seen_flow_ids = BTreeSet::new();

        for flow in self.flows {
            validate_identifier(&flow.id, "flow id")?;
            if flow.entry.trim().is_empty() {
                bail!("flow {} is missing an entry node", flow.id);
            }
            if !seen_flow_ids.insert(flow.id.clone()) {
                bail!("duplicate flow id detected: {}", flow.id);
            }

            let yaml_path = normalize_relative_path(&["flows", &flow.id, "flow.ygtc"])?;
            let yaml_bytes = normalize_newlines(&flow.yaml).into_bytes();
            pending_files.push(PendingFile::new(
                yaml_path.clone(),
                "application/yaml",
                yaml_bytes,
            ));

            let json_path = normalize_relative_path(&["flows", &flow.id, "flow.json"])?;
            let json_bytes = serde_json::to_vec(&flow.json)?;
            pending_files.push(PendingFile::new(
                json_path.clone(),
                "application/json",
                json_bytes,
            ));

            flow_entries.push(FlowEntry {
                id: flow.id,
                kind: flow.kind,
                entry: flow.entry,
                file_yaml: yaml_path,
                file_json: json_path,
                hash_blake3: flow.hash_blake3,
            });
        }

        for entry in &meta.entry_flows {
            if !seen_flow_ids.contains(entry) {
                bail!("entry flow `{}` not present in provided flows", entry);
            }
        }

        flow_entries.sort_by(|a, b| a.id.cmp(&b.id));

        let mut component_entries = Vec::new();
        let mut seen_components = BTreeSet::new();

        for component in self.components {
            validate_identifier(&component.name, "component name")?;
            let key = format!("{}@{}", component.name, component.version);
            if !seen_components.insert(key.clone()) {
                bail!("duplicate component artifact detected: {}", key);
            }

            let wasm_bytes = fs::read(&component.wasm_path).with_context(|| {
                format!(
                    "failed to read component wasm at {}",
                    component.wasm_path.display()
                )
            })?;
            let wasm_hash = hex_hash(&wasm_bytes);
            if let Some(expected) = component.hash_blake3.as_deref()
                && !equals_ignore_case(expected, &wasm_hash)
            {
                bail!(
                    "component {} hash mismatch: expected {}, got {}",
                    key,
                    expected,
                    wasm_hash
                );
            }

            let wasm_path = normalize_relative_path(&["components", &key, "component.wasm"])?;
            pending_files.push(PendingFile::new(
                wasm_path.clone(),
                "application/wasm",
                wasm_bytes,
            ));

            let mut schema_file = None;
            if let Some(schema) = component.schema_json.as_ref() {
                let schema_path = normalize_relative_path(&["schemas", &key, "node.schema.json"])?;
                pending_files.push(PendingFile::new(
                    schema_path.clone(),
                    "application/schema+json",
                    normalize_newlines(schema).into_bytes(),
                ));
                schema_file = Some(schema_path);
            }

            let mut manifest_file = None;
            if let Some(manifest_json) = component.manifest_json.as_ref() {
                let manifest_path =
                    normalize_relative_path(&["components", &key, "manifest.json"])?;
                pending_files.push(PendingFile::new(
                    manifest_path.clone(),
                    "application/json",
                    normalize_newlines(manifest_json).into_bytes(),
                ));
                manifest_file = Some(manifest_path);
            }

            component_entries.push(ComponentEntry {
                name: component.name,
                version: component.version,
                file_wasm: wasm_path,
                hash_blake3: wasm_hash,
                schema_file,
                manifest_file,
                world: component.world,
                capabilities: component.capabilities,
            });
        }

        component_entries
            .sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.version.cmp(&b.version)));

        for asset in self.assets {
            let path = normalize_relative_path(&["assets", &asset.path])?;
            pending_files.push(PendingFile::new(
                path,
                "application/octet-stream",
                asset.bytes,
            ));
        }

        let manifest_model = PackManifest {
            meta: meta.clone(),
            flows: flow_entries,
            components: component_entries,
            distribution,
            component_descriptors,
        };

        let manifest_cbor = encode_manifest_cbor(&manifest_model)?;
        let manifest_json = serde_json::to_vec_pretty(&manifest_model)?;

        pending_files.push(PendingFile::new(
            "manifest.cbor".to_string(),
            "application/cbor",
            manifest_cbor.clone(),
        ));
        pending_files.push(PendingFile::new(
            "manifest.json".to_string(),
            "application/json",
            manifest_json,
        ));

        let provenance = finalize_provenance(self.provenance);
        let provenance_json = serde_json::to_vec_pretty(&provenance)?;
        pending_files.push(PendingFile::new(
            "provenance.json".to_string(),
            "application/json",
            provenance_json,
        ));

        let mut sbom_entries = Vec::new();
        for file in pending_files.iter() {
            sbom_entries.push(SbomEntry {
                path: file.path.clone(),
                size: file.size(),
                hash_blake3: file.hash(),
                media_type: file.media_type.clone(),
            });
        }
        let build_files = sbom_entries.clone();

        let sbom_document = serde_json::json!({
            "format": SBOM_FORMAT,
            "files": sbom_entries,
        });
        let sbom_bytes = serde_json::to_vec_pretty(&sbom_document)?;
        pending_files.push(PendingFile::new(
            "sbom.json".to_string(),
            "application/json",
            sbom_bytes.clone(),
        ));

        let manifest_hash = hex_hash(&manifest_cbor);

        let mut signature_files = Vec::new();
        if !matches!(self.signing, Signing::None) {
            let digest = signature_digest_from_entries(&build_files, &manifest_cbor, &sbom_bytes);
            let (signature_doc, chain_bytes) = match &self.signing {
                Signing::Dev => dev_signature(&digest)?,
                Signing::None => unreachable!(),
                Signing::External(signer) => external_signature(&**signer, &digest)?,
            };

            let sig_bytes = serde_json::to_vec_pretty(&signature_doc)?;
            signature_files.push(PendingFile::new(
                SIGNATURE_PATH.to_string(),
                "application/json",
                sig_bytes,
            ));

            if let Some(chain) = chain_bytes {
                signature_files.push(PendingFile::new(
                    SIGNATURE_CHAIN_PATH.to_string(),
                    "application/x-pem-file",
                    chain,
                ));
            }
        }

        let mut all_files = pending_files;
        all_files.extend(signature_files);
        all_files.sort_by(|a, b| a.path.cmp(&b.path));

        let out_path = out_path.as_ref().to_path_buf();
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }

        write_zip(&out_path, &all_files)?;

        Ok(BuildResult {
            out_path,
            manifest_hash_blake3: manifest_hash,
            files: build_files,
        })
    }
}

fn equals_ignore_case(expected: &str, actual: &str) -> bool {
    expected.trim().eq_ignore_ascii_case(actual.trim())
}

fn normalize_newlines(input: &str) -> String {
    input.replace("\r\n", "\n")
}

fn normalize_relative_path(parts: &[&str]) -> Result<String> {
    let mut segments = Vec::new();
    for part in parts {
        let normalized = part.replace('\\', "/");
        for piece in normalized.split('/') {
            if piece.is_empty() {
                bail!("invalid path segment");
            }
            if piece == "." || piece == ".." {
                bail!("path traversal is not permitted");
            }
            segments.push(piece.to_string());
        }
    }
    Ok(segments.join("/"))
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{} must not be empty", label);
    }
    if value.contains("..") {
        bail!("{} must not contain '..'", label);
    }
    Ok(())
}

fn encode_manifest_cbor(manifest: &PackManifest) -> Result<Vec<u8>> {
    canonical::to_canonical_cbor_allow_floats(manifest).map_err(Into::into)
}

fn validate_digest(digest: &str) -> Result<()> {
    if digest.trim().is_empty() {
        bail!("component digest must not be empty");
    }
    if !digest.starts_with("sha256:") {
        bail!("component digest must start with sha256:");
    }
    Ok(())
}

fn validate_component_descriptor(component: &ComponentDescriptor) -> Result<()> {
    if component.component_id.trim().is_empty() {
        bail!("component_id must not be empty");
    }
    if component.version.trim().is_empty() {
        bail!("component version must not be empty");
    }
    if component.artifact_path.trim().is_empty() {
        bail!("component artifact_path must not be empty");
    }
    validate_digest(&component.digest)?;
    if let Some(kind) = &component.kind
        && kind.trim().is_empty()
    {
        bail!("component kind must not be empty when provided");
    }
    if let Some(artifact_type) = &component.artifact_type
        && artifact_type.trim().is_empty()
    {
        bail!("component artifact_type must not be empty when provided");
    }
    if let Some(platform) = &component.platform
        && platform.trim().is_empty()
    {
        bail!("component platform must not be empty when provided");
    }
    if let Some(entrypoint) = &component.entrypoint
        && entrypoint.trim().is_empty()
    {
        bail!("component entrypoint must not be empty when provided");
    }
    for tag in &component.tags {
        if tag.trim().is_empty() {
            bail!("component tags must not contain empty entries");
        }
    }
    Ok(())
}

pub fn validate_components(components: &[ComponentDescriptor]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for component in components {
        validate_component_descriptor(component)?;
        let key = (component.component_id.clone(), component.version.clone());
        if !seen.insert(key) {
            bail!("duplicate component entry detected");
        }
    }
    Ok(())
}

pub fn validate_distribution(
    kind: Option<&PackKind>,
    distribution: Option<&DistributionSection>,
) -> Result<()> {
    match (kind, distribution) {
        (Some(PackKind::DistributionBundle), Some(section)) => {
            if let Some(bundle_id) = &section.bundle_id
                && bundle_id.trim().is_empty()
            {
                bail!("distribution.bundle_id must not be empty when provided");
            }
            if section.environment_ref.trim().is_empty() {
                bail!("distribution.environment_ref must not be empty");
            }
            if section.desired_state_version.trim().is_empty() {
                bail!("distribution.desired_state_version must not be empty");
            }
            validate_components(&section.components)?;
            validate_components(&section.platform_components)?;
        }
        (Some(PackKind::DistributionBundle), None) => {
            bail!("distribution section is required for kind distribution-bundle");
        }
        (_, Some(_)) => {
            bail!("distribution section is only allowed when kind is distribution-bundle");
        }
        _ => {}
    }
    Ok(())
}

fn finalize_provenance(provenance: Option<Provenance>) -> Provenance {
    let builder_default = format!("greentic-pack@{}", env!("CARGO_PKG_VERSION"));
    let now = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    match provenance {
        Some(mut prov) => {
            if prov.builder.trim().is_empty() {
                prov.builder = builder_default;
            }
            if prov.built_at_utc.trim().is_empty() {
                prov.built_at_utc = now;
            }
            prov
        }
        None => Provenance {
            builder: builder_default,
            git_commit: None,
            git_repo: None,
            toolchain: None,
            built_at_utc: now,
            host: None,
            notes: None,
        },
    }
}

pub(crate) fn signature_digest_from_entries(
    entries: &[SbomEntry],
    manifest_cbor: &[u8],
    sbom_bytes: &[u8],
) -> blake3::Hash {
    let mut hasher = Hasher::new();
    hasher.update(manifest_cbor);
    hasher.update(sbom_bytes);

    let mut records: Vec<(String, String)> = entries
        .iter()
        .map(|entry| (entry.path.clone(), entry.hash_blake3.clone()))
        .collect();
    records.sort_by(|a, b| a.0.cmp(&b.0));

    for (path, hash) in records {
        hasher.update(path.as_bytes());
        hasher.update(b"\n");
        hasher.update(hash.as_bytes());
    }

    hasher.finalize()
}

fn dev_signature(digest: &blake3::Hash) -> Result<(SignatureEnvelope, Option<Vec<u8>>)> {
    let mut secret = [0u8; 32];
    fill_random(&mut secret).map_err(|err| anyhow!("failed to generate dev signing key: {err}"))?;
    let signing_key = SigningKey::from_bytes(&secret);
    let signature = signing_key.sign(digest.as_bytes());
    let signature_bytes = signature.to_bytes();

    let pkcs8_doc = signing_key
        .to_pkcs8_der()
        .map_err(|err| anyhow!("failed to encode dev keypair: {err}"))?;
    let pkcs8_der = PrivatePkcs8KeyDer::from(pkcs8_doc.as_bytes().to_vec());
    let key_pair = KeyPair::from_pkcs8_der_and_sign_algo(&pkcs8_der, &PKCS_ED25519)
        .map_err(|err| anyhow!("failed to load dev keypair for certificate: {err}"))?;

    let mut params = CertificateParams::new(Vec::<String>::new())?;
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, "greentic-dev-local");
    let cert = params.self_signed(&key_pair)?;
    let chain = normalize_newlines(&cert.pem()).into_bytes();
    let fingerprint = hex_hash(signing_key.verifying_key().as_bytes());

    let envelope = SignatureEnvelope::new("ed25519", &signature_bytes, digest, Some(fingerprint));
    Ok((envelope, Some(chain)))
}

fn external_signature(
    signer: &DynSigner,
    digest: &blake3::Hash,
) -> Result<(SignatureEnvelope, Option<Vec<u8>>)> {
    let ExternalSignature { alg, sig } = signer.sign(digest.as_bytes())?;
    let chain = signer.chain_pem()?;
    let chain_bytes = if chain.is_empty() {
        None
    } else {
        let chain_str = String::from_utf8(chain)?;
        Some(normalize_newlines(&chain_str).into_bytes())
    };
    let envelope = SignatureEnvelope::new(alg, &sig, digest, None);
    Ok((envelope, chain_bytes))
}

pub(crate) fn hex_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn write_zip(out_path: &Path, files: &[PendingFile]) -> Result<()> {
    let file = fs::File::create(out_path)
        .with_context(|| format!("failed to create {}", out_path.display()))?;
    let mut writer = ZipWriter::new(file);
    let timestamp = zip_timestamp();

    for entry in files {
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .last_modified_time(timestamp)
            .unix_permissions(0o644)
            .large_file(false);
        writer
            .start_file(&entry.path, options)
            .with_context(|| format!("failed to add {} to archive", entry.path))?;
        writer
            .write_all(&entry.bytes)
            .with_context(|| format!("failed to write {}", entry.path))?;
    }

    writer.finish().context("failed to finish gtpack archive")?;
    Ok(())
}

fn zip_timestamp() -> ZipDateTime {
    ZipDateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).unwrap_or_else(|_| ZipDateTime::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;
    use zip::ZipArchive;

    use std::fs::{self, File};
    use std::io::Read;

    #[test]
    fn deterministic_build_without_signing() {
        let temp = tempdir().unwrap();
        let wasm_path = temp.path().join("component.wasm");
        fs::write(&wasm_path, test_wasm_bytes()).unwrap();

        let builder = || {
            PackBuilder::new(sample_meta())
                .with_flow(sample_flow())
                .with_component(sample_component(&wasm_path))
                .with_signing(Signing::None)
                .with_provenance(sample_provenance())
        };

        let out_a = temp.path().join("a.gtpack");
        let out_b = temp.path().join("b.gtpack");

        builder().build(&out_a).unwrap();
        builder().build(&out_b).unwrap();

        let bytes_a = fs::read(&out_a).unwrap();
        let bytes_b = fs::read(&out_b).unwrap();
        assert_eq!(bytes_a, bytes_b, "gtpack output should be deterministic");

        let result = PackBuilder::new(sample_meta())
            .with_flow(sample_flow())
            .with_component(sample_component(&wasm_path))
            .with_signing(Signing::None)
            .with_provenance(sample_provenance())
            .build(temp.path().join("result.gtpack"))
            .unwrap();

        assert!(
            result
                .files
                .iter()
                .any(|entry| entry.path == "components/oauth@1.0.0/component.wasm")
        );
    }

    #[test]
    fn dev_signing_writes_signature_files() {
        let temp = tempdir().unwrap();
        let wasm_path = temp.path().join("component.wasm");
        fs::write(&wasm_path, test_wasm_bytes()).unwrap();

        let out_path = temp.path().join("signed.gtpack");
        PackBuilder::new(sample_meta())
            .with_flow(sample_flow())
            .with_component(sample_component(&wasm_path))
            .with_signing(Signing::Dev)
            .with_provenance(sample_provenance())
            .build(&out_path)
            .unwrap();

        let reader = File::open(&out_path).unwrap();
        let mut archive = ZipArchive::new(reader).unwrap();
        let mut signature_found = false;
        let mut chain_found = false;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).unwrap();
            match file.name() {
                SIGNATURE_PATH => {
                    signature_found = true;
                    let mut contents = String::new();
                    file.read_to_string(&mut contents).unwrap();
                    assert!(contents.contains("\"alg\": \"ed25519\""));
                }
                SIGNATURE_CHAIN_PATH => {
                    chain_found = true;
                }
                _ => {}
            }
        }

        assert!(signature_found, "signature should be present");
        assert!(chain_found, "certificate chain should be present");
    }

    fn sample_meta() -> PackMeta {
        PackMeta {
            pack_version: PACK_VERSION,
            pack_id: "ai.greentic.demo.test".to_string(),
            version: Version::parse("0.1.0").unwrap(),
            name: "Test Pack".to_string(),
            kind: None,
            description: Some("integration test".to_string()),
            authors: vec!["Greentic".to_string()],
            license: Some("MIT".to_string()),
            homepage: None,
            support: None,
            vendor: None,
            imports: Vec::new(),
            entry_flows: vec!["main".to_string()],
            created_at_utc: "2025-01-01T00:00:00Z".to_string(),
            events: None,
            repo: None,
            messaging: None,
            interfaces: Vec::new(),
            annotations: JsonMap::new(),
            distribution: None,
            components: Vec::new(),
        }
    }

    fn sample_flow() -> FlowBundle {
        let flow_json = json!({
            "id": "main",
            "kind": "flow/v1",
            "entry": "start",
            "nodes": []
        });
        let hash = blake3::hash(&serde_json::to_vec(&flow_json).unwrap())
            .to_hex()
            .to_string();
        FlowBundle {
            id: "main".to_string(),
            kind: "flow/v1".to_string(),
            entry: "start".to_string(),
            yaml: "id: main\nentry: start\n".to_string(),
            json: flow_json,
            hash_blake3: hash,
            nodes: Vec::new(),
        }
    }

    fn sample_component(wasm_path: &Path) -> ComponentArtifact {
        ComponentArtifact {
            name: "oauth".to_string(),
            version: Version::parse("1.0.0").unwrap(),
            wasm_path: wasm_path.to_path_buf(),
            schema_json: None,
            manifest_json: None,
            capabilities: None,
            world: Some("component:tool".to_string()),
            hash_blake3: None,
        }
    }

    fn test_wasm_bytes() -> Vec<u8> {
        vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]
    }

    fn sample_provenance() -> Provenance {
        Provenance {
            builder: "greentic-pack@test".to_string(),
            git_commit: Some("abc123".to_string()),
            git_repo: Some("https://example.com/repo.git".to_string()),
            toolchain: Some("rustc 1.85.0".to_string()),
            built_at_utc: "2025-01-01T00:00:00Z".to_string(),
            host: Some("ci".to_string()),
            notes: None,
        }
    }
}
