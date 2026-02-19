use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use oci_distribution::Reference;
use oci_distribution::client::{Client, ClientConfig, ClientProtocol, ImageData};
use oci_distribution::errors::OciDistributionError;
use oci_distribution::manifest::{
    IMAGE_MANIFEST_LIST_MEDIA_TYPE, IMAGE_MANIFEST_MEDIA_TYPE, OCI_IMAGE_INDEX_MEDIA_TYPE,
    OCI_IMAGE_MEDIA_TYPE,
};
use oci_distribution::secrets::RegistryAuth;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

const OCI_ARTIFACT_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.artifact.manifest.v1+json";
const DOCKER_MANIFEST_MEDIA_TYPE: &str = "application/vnd.docker.distribution.manifest.v2+json";
const DOCKER_MANIFEST_LIST_MEDIA_TYPE: &str =
    "application/vnd.docker.distribution.manifest.list.v2+json";

/// Accepted manifest media types when pulling packs.
static DEFAULT_ACCEPTED_MANIFEST_TYPES: &[&str] = &[
    OCI_ARTIFACT_MANIFEST_MEDIA_TYPE,
    OCI_IMAGE_MEDIA_TYPE,
    OCI_IMAGE_INDEX_MEDIA_TYPE,
    IMAGE_MANIFEST_MEDIA_TYPE,
    IMAGE_MANIFEST_LIST_MEDIA_TYPE,
    DOCKER_MANIFEST_MEDIA_TYPE,
    DOCKER_MANIFEST_LIST_MEDIA_TYPE,
];

const PACK_LAYER_MEDIA_TYPE: &str = "application/vnd.greentic.pack+json";
const PACK_LAYER_MEDIA_TYPE_ZIP: &str = "application/vnd.greentic.gtpack.v1+zip";
const PACK_LAYER_MEDIA_TYPE_ZIP_LEGACY: &str = "application/vnd.greentic.gtpack+zip";
const PACK_LAYER_MEDIA_TYPE_PACK_ZIP: &str = "application/vnd.greentic.pack+zip";
const PACK_LAYER_MEDIA_TYPE_MARKDOWN: &str = "text/markdown";
const PACK_LAYER_MEDIA_TYPE_OCTET_STREAM: &str = "application/octet-stream";
const PACK_LAYER_MEDIA_TYPE_JSON: &str = "application/json";
const PACK_LAYER_MEDIA_TYPE_TAR: &str = "application/vnd.oci.image.layer.v1.tar";
const PACK_LAYER_MEDIA_TYPE_TAR_GZIP: &str = "application/vnd.oci.image.layer.v1.tar+gzip";
const PACK_LAYER_MEDIA_TYPE_TAR_ZSTD: &str = "application/vnd.oci.image.layer.v1.tar+zstd";
const PACK_FILENAME: &str = "pack.gtpack";

/// Configuration for fetching OCI packs.
#[derive(Clone, Debug)]
pub struct PackFetchOptions {
    pub allow_tags: bool,
    pub offline: bool,
    pub cache_dir: PathBuf,
    pub accepted_manifest_types: Vec<String>,
    /// Allowed layer media types when pulling from registry.
    pub accepted_layer_media_types: Vec<String>,
    pub preferred_layer_media_types: Vec<String>,
}

impl Default for PackFetchOptions {
    fn default() -> Self {
        Self {
            allow_tags: false,
            offline: false,
            cache_dir: default_cache_root(),
            accepted_manifest_types: DEFAULT_ACCEPTED_MANIFEST_TYPES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            accepted_layer_media_types: vec![
                PACK_LAYER_MEDIA_TYPE.to_string(),
                PACK_LAYER_MEDIA_TYPE_ZIP.to_string(),
                PACK_LAYER_MEDIA_TYPE_ZIP_LEGACY.to_string(),
                PACK_LAYER_MEDIA_TYPE_PACK_ZIP.to_string(),
                PACK_LAYER_MEDIA_TYPE_MARKDOWN.to_string(),
                PACK_LAYER_MEDIA_TYPE_OCTET_STREAM.to_string(),
                PACK_LAYER_MEDIA_TYPE_JSON.to_string(),
                PACK_LAYER_MEDIA_TYPE_TAR.to_string(),
                PACK_LAYER_MEDIA_TYPE_TAR_GZIP.to_string(),
                PACK_LAYER_MEDIA_TYPE_TAR_ZSTD.to_string(),
            ],
            preferred_layer_media_types: vec![
                PACK_LAYER_MEDIA_TYPE.to_string(),
                PACK_LAYER_MEDIA_TYPE_ZIP.to_string(),
                PACK_LAYER_MEDIA_TYPE_ZIP_LEGACY.to_string(),
                PACK_LAYER_MEDIA_TYPE_PACK_ZIP.to_string(),
                PACK_LAYER_MEDIA_TYPE_MARKDOWN.to_string(),
            ],
        }
    }
}

/// Result of fetching a single pack reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPack {
    pub original_reference: String,
    pub resolved_digest: String,
    pub media_type: String,
    pub path: PathBuf,
    pub fetched_from_network: bool,
    pub manifest_digest: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheMetadata {
    original_reference: String,
    resolved_digest: String,
    media_type: String,
    fetched_at_unix_seconds: u64,
    size_bytes: u64,
    #[serde(default)]
    manifest_digest: Option<String>,
}

/// Fetch OCI packs with caching and offline support.
pub struct OciPackFetcher<C: RegistryClient = DefaultRegistryClient> {
    client: C,
    opts: PackFetchOptions,
    cache: PackCache,
}

impl Default for OciPackFetcher<DefaultRegistryClient> {
    fn default() -> Self {
        Self::new(PackFetchOptions::default())
    }
}

impl<C: RegistryClient> OciPackFetcher<C> {
    pub fn new(opts: PackFetchOptions) -> Self {
        let cache = PackCache::new(opts.cache_dir.clone());
        Self {
            client: C::default_client(),
            opts,
            cache,
        }
    }

    pub fn with_client(client: C, opts: PackFetchOptions) -> Self {
        let cache = PackCache::new(opts.cache_dir.clone());
        Self {
            client,
            opts,
            cache,
        }
    }

    pub async fn fetch_pack(&self, reference: &str) -> Result<Vec<u8>, OciPackError> {
        let resolved = self.fetch_pack_to_cache(reference).await?;
        fs::read(&resolved.path).map_err(|source| OciPackError::Io {
            reference: reference.to_string(),
            source,
        })
    }

    pub async fn fetch_pack_to_cache(&self, reference: &str) -> Result<ResolvedPack, OciPackError> {
        let parsed =
            Reference::try_from(reference).map_err(|e| OciPackError::InvalidReference {
                reference: reference.to_string(),
                reason: e.to_string(),
            })?;

        if parsed.digest().is_none() && !self.opts.allow_tags {
            return Err(OciPackError::TagDisallowed {
                reference: reference.to_string(),
            });
        }

        let expected_digest = parsed.digest().map(normalize_digest);
        if let Some(expected_digest) = expected_digest.as_ref() {
            if let Some(hit) = self.cache.try_hit(expected_digest, reference) {
                return Ok(hit);
            }
            if self.opts.offline {
                return Err(OciPackError::OfflineMissing {
                    reference: reference.to_string(),
                    digest: expected_digest.clone(),
                });
            }
        } else if self.opts.offline {
            return Err(OciPackError::OfflineTaggedReference {
                reference: reference.to_string(),
            });
        }

        let accepted_layer_types = self
            .opts
            .accepted_layer_media_types
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>();
        let image = self
            .client
            .pull(&parsed, &accepted_layer_types)
            .await
            .map_err(|source| OciPackError::PullFailed {
                reference: reference.to_string(),
                source,
            })?;

        let chosen_layer = select_layer(
            &image.layers,
            &self.opts.preferred_layer_media_types,
            reference,
        )?;
        let resolved_digest = image
            .digest
            .clone()
            .or_else(|| chosen_layer.digest.clone())
            .unwrap_or_else(|| compute_digest(&chosen_layer.data));
        let manifest_digest = image.digest.clone();

        if let Some(expected) = expected_digest.as_ref()
            && expected != &resolved_digest
        {
            return Err(OciPackError::DigestMismatch {
                reference: reference.to_string(),
                expected: expected.clone(),
                actual: resolved_digest.clone(),
            });
        }

        let path = self.cache.write(
            &resolved_digest,
            &chosen_layer.media_type,
            &chosen_layer.data,
            reference,
            manifest_digest.clone(),
        )?;

        Ok(ResolvedPack {
            original_reference: reference.to_string(),
            resolved_digest,
            media_type: chosen_layer.media_type.clone(),
            path,
            fetched_from_network: true,
            manifest_digest,
        })
    }
}

pub async fn fetch_pack(oci_ref: &str) -> Result<Vec<u8>, OciPackError> {
    OciPackFetcher::default().fetch_pack(oci_ref).await
}

pub async fn fetch_pack_to_cache(oci_ref: &str) -> Result<ResolvedPack, OciPackError> {
    OciPackFetcher::default().fetch_pack_to_cache(oci_ref).await
}

fn select_layer<'a>(
    layers: &'a [PulledLayer],
    preferred_types: &[String],
    reference: &str,
) -> Result<&'a PulledLayer, OciPackError> {
    if layers.is_empty() {
        return Err(OciPackError::MissingLayers {
            reference: reference.to_string(),
        });
    }
    for ty in preferred_types {
        if let Some(layer) = layers.iter().find(|l| &l.media_type == ty) {
            return Ok(layer);
        }
    }
    Ok(&layers[0])
}

fn compute_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

fn normalize_digest(digest: &str) -> String {
    if digest.starts_with("sha256:") {
        digest.to_string()
    } else {
        format!("sha256:{digest}")
    }
}

pub(crate) fn default_cache_root() -> PathBuf {
    if let Ok(root) = std::env::var("GREENTIC_PACK_CACHE_DIR") {
        return PathBuf::from(root);
    }
    if let Some(cache) = dirs_next::cache_dir() {
        return cache.join("greentic").join("packs");
    }
    if let Ok(root) = std::env::var("GREENTIC_HOME") {
        return PathBuf::from(root).join("cache").join("packs");
    }
    PathBuf::from(".greentic").join("cache").join("packs")
}

#[derive(Clone, Debug)]
struct PackCache {
    root: PathBuf,
}

impl PackCache {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn write(
        &self,
        digest: &str,
        media_type: &str,
        data: &[u8],
        reference: &str,
        manifest_digest: Option<String>,
    ) -> Result<PathBuf, OciPackError> {
        let dir = self.artifact_dir(digest);
        fs::create_dir_all(&dir).map_err(|source| OciPackError::Io {
            reference: reference.to_string(),
            source,
        })?;
        let pack_path = dir.join(PACK_FILENAME);
        fs::write(&pack_path, data).map_err(|source| OciPackError::Io {
            reference: reference.to_string(),
            source,
        })?;

        let metadata = CacheMetadata {
            original_reference: reference.to_string(),
            resolved_digest: digest.to_string(),
            media_type: media_type.to_string(),
            fetched_at_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            size_bytes: data.len() as u64,
            manifest_digest,
        };
        let metadata_path = dir.join("metadata.json");
        let buf = serde_json::to_vec_pretty(&metadata).map_err(|source| OciPackError::Serde {
            reference: reference.to_string(),
            source,
        })?;
        fs::write(&metadata_path, buf).map_err(|source| OciPackError::Io {
            reference: reference.to_string(),
            source,
        })?;

        Ok(pack_path)
    }

    fn try_hit(&self, digest: &str, reference: &str) -> Option<ResolvedPack> {
        let metadata = self.read_metadata(digest).ok();
        let media_type = metadata
            .as_ref()
            .map(|m| m.media_type.clone())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let path = self.artifact_dir(digest).join(PACK_FILENAME);
        if !path.exists() {
            return None;
        }
        Some(ResolvedPack {
            original_reference: reference.to_string(),
            resolved_digest: digest.to_string(),
            media_type,
            path,
            fetched_from_network: false,
            manifest_digest: metadata.and_then(|m| m.manifest_digest),
        })
    }

    fn read_metadata(&self, digest: &str) -> anyhow::Result<CacheMetadata> {
        let metadata_path = self.artifact_dir(digest).join("metadata.json");
        let bytes = fs::read(metadata_path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn artifact_dir(&self, digest: &str) -> PathBuf {
        self.root.join(trim_digest_prefix(digest))
    }
}

fn trim_digest_prefix(digest: &str) -> &str {
    digest
        .strip_prefix("sha256:")
        .unwrap_or_else(|| digest.trim_start_matches('@'))
}

#[derive(Clone, Debug)]
pub struct PulledImage {
    pub digest: Option<String>,
    pub layers: Vec<PulledLayer>,
}

#[derive(Clone, Debug)]
pub struct PulledLayer {
    pub media_type: String,
    pub data: Vec<u8>,
    pub digest: Option<String>,
}

#[async_trait]
pub trait RegistryClient: Send + Sync {
    fn default_client() -> Self
    where
        Self: Sized;

    async fn pull(
        &self,
        reference: &Reference,
        accepted_manifest_types: &[&str],
    ) -> Result<PulledImage, OciDistributionError>;
}

/// Registry client backed by `oci-distribution` with HTTPS enforced and anonymous pulls.
#[derive(Clone)]
pub struct DefaultRegistryClient {
    inner: Client,
}

impl Default for DefaultRegistryClient {
    fn default() -> Self {
        Self::default_client()
    }
}

#[async_trait]
impl RegistryClient for DefaultRegistryClient {
    fn default_client() -> Self {
        let config = ClientConfig {
            protocol: ClientProtocol::Https,
            ..Default::default()
        };
        Self {
            inner: Client::new(config),
        }
    }

    async fn pull(
        &self,
        reference: &Reference,
        accepted_manifest_types: &[&str],
    ) -> Result<PulledImage, OciDistributionError> {
        let image = self
            .inner
            .pull(
                reference,
                &RegistryAuth::Anonymous,
                accepted_manifest_types.to_vec(),
            )
            .await?;
        Ok(convert_image(image))
    }
}

fn convert_image(image: ImageData) -> PulledImage {
    let layers = image
        .layers
        .into_iter()
        .map(|layer| {
            let digest = format!("sha256:{}", layer.sha256_digest());
            PulledLayer {
                media_type: layer.media_type,
                data: layer.data,
                digest: Some(digest),
            }
        })
        .collect();
    PulledImage {
        digest: image.digest,
        layers,
    }
}

#[derive(Debug, Error)]
pub enum OciPackError {
    #[error("invalid OCI reference `{reference}`: {reason}")]
    InvalidReference { reference: String, reason: String },
    #[error("tagged reference `{reference}` is disallowed (rerun with allow_tags)")]
    TagDisallowed { reference: String },
    #[error("offline mode prohibits tagged reference `{reference}`; pin by digest first")]
    OfflineTaggedReference { reference: String },
    #[error("offline mode could not find cached pack for `{reference}` (digest `{digest}`)")]
    OfflineMissing { reference: String, digest: String },
    #[error("no layers returned for `{reference}`")]
    MissingLayers { reference: String },
    #[error("digest mismatch for `{reference}`: expected {expected}, got {actual}")]
    DigestMismatch {
        reference: String,
        expected: String,
        actual: String,
    },
    #[error("failed to pull `{reference}`: {source}")]
    PullFailed {
        reference: String,
        #[source]
        source: oci_distribution::errors::OciDistributionError,
    },
    #[error("io error while caching `{reference}`: {source}")]
    Io {
        reference: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to serialize cache metadata for `{reference}`: {source}")]
    Serde {
        reference: String,
        #[source]
        source: serde_json::Error,
    },
}
