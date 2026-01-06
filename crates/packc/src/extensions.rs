use anyhow::{Context, Result, bail};
use greentic_types::pack_manifest::{ExtensionInline, ExtensionRef};
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::collections::BTreeMap;

pub const COMPONENTS_EXTENSION_KEY: &str = "greentic.components";

#[derive(Debug, Clone)]
pub struct ComponentsExtension {
    pub refs: Vec<String>,
    pub mode: Option<String>,
    pub allow_tags: Option<bool>,
}

pub fn validate_components_extension(
    extensions: &Option<BTreeMap<String, ExtensionRef>>,
    allow_tags: bool,
) -> Result<Option<ComponentsExtension>> {
    let Some(ext) = extensions
        .as_ref()
        .and_then(|all| all.get(COMPONENTS_EXTENSION_KEY))
    else {
        return Ok(None);
    };

    let payload = ext.inline.as_ref().ok_or_else(|| {
        anyhow::anyhow!("extensions[{COMPONENTS_EXTENSION_KEY}] inline is required")
    })?;

    let payload = match payload {
        ExtensionInline::Other(value) => value.clone(),
        other => serde_json::to_value(other).context("serialize inline extension")?,
    };

    let map = payload.as_object().cloned().ok_or_else(|| {
        anyhow::anyhow!("extensions[{COMPONENTS_EXTENSION_KEY}] inline must be an object")
    })?;

    let refs = extract_refs(&map, allow_tags)?;
    let mode = extract_mode(&map)?;
    let allow_tags_inline = map.get("allow_tags").and_then(JsonValue::as_bool);

    Ok(Some(ComponentsExtension {
        refs,
        mode,
        allow_tags: allow_tags_inline,
    }))
}

fn extract_refs(map: &JsonMap<String, JsonValue>, allow_tags: bool) -> Result<Vec<String>> {
    let refs = map.get("refs").ok_or_else(|| {
        anyhow::anyhow!("extensions[{COMPONENTS_EXTENSION_KEY}] inline.refs is required")
    })?;
    let arr = refs.as_array().ok_or_else(|| {
        anyhow::anyhow!("extensions[{COMPONENTS_EXTENSION_KEY}] inline.refs must be an array")
    })?;

    let mut result = Vec::new();
    for value in arr {
        let reference = value.as_str().ok_or_else(|| {
            anyhow::anyhow!(
                "extensions[{COMPONENTS_EXTENSION_KEY}] inline.refs entries must be strings"
            )
        })?;
        validate_oci_ref(reference, allow_tags)?;
        result.push(reference.to_string());
    }
    Ok(result)
}

fn extract_mode(map: &JsonMap<String, JsonValue>) -> Result<Option<String>> {
    let Some(mode) = map.get("mode") else {
        return Ok(None);
    };
    let Some(mode_str) = mode.as_str() else {
        bail!("extensions[{COMPONENTS_EXTENSION_KEY}] inline.mode must be a string when present");
    };
    match mode_str {
        "eager" | "lazy" => Ok(Some(mode_str.to_string())),
        other => bail!(
            "extensions[{COMPONENTS_EXTENSION_KEY}] inline.mode must be one of [eager, lazy]; found `{other}`"
        ),
    }
}

fn validate_oci_ref(reference: &str, allow_tags: bool) -> Result<()> {
    if let Some((repo, digest)) = reference.rsplit_once('@') {
        if repo.trim().is_empty() {
            bail!("OCI component ref is missing a repository before the digest: `{reference}`");
        }
        if !digest.starts_with("sha256:") {
            bail!("OCI component ref digest must start with sha256: `{reference}`");
        }
        let hex = &digest["sha256:".len()..];
        if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("OCI component ref must include a 64-character hex sha256 digest: `{reference}`");
        }
        if !repo.contains('/') {
            bail!("OCI component ref must include a registry/repository path: `{reference}`");
        }
        return Ok(());
    }

    let last_slash = reference.rfind('/').ok_or_else(|| {
        anyhow::anyhow!("OCI component ref must include a registry/repository path: `{reference}`")
    })?;
    let last_colon = reference.rfind(':').ok_or_else(|| {
        anyhow::anyhow!(
            "OCI component ref must be digest-pinned (...@sha256:...){}",
            if allow_tags {
                " or include a tag (:tag)"
            } else {
                ""
            }
        )
    })?;

    if last_colon <= last_slash {
        bail!("OCI component ref must include a tag or digest: `{reference}`");
    }

    let tag = &reference[last_colon + 1..];
    if tag.is_empty() {
        bail!("OCI component ref tag must not be empty: `{reference}`");
    }
    if !allow_tags {
        bail!(
            "OCI component ref must be digest-pinned (...@sha256:...). Re-run with --allow-oci-tags to permit tags."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ext_with_payload(payload: JsonValue) -> BTreeMap<String, ExtensionRef> {
        let mut map = BTreeMap::new();
        map.insert(
            COMPONENTS_EXTENSION_KEY.to_string(),
            ExtensionRef {
                kind: COMPONENTS_EXTENSION_KEY.to_string(),
                version: "v1".to_string(),
                digest: None,
                location: None,
                inline: Some(ExtensionInline::Other(payload)),
            },
        );
        map
    }

    #[test]
    fn digest_refs_are_allowed_by_default() {
        let extensions = ext_with_payload(json!({
            "refs": ["ghcr.io/org/demo@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]
        }));
        validate_components_extension(&Some(extensions), false).expect("digest ok");
    }

    #[test]
    fn tag_refs_are_rejected_by_default() {
        let extensions = ext_with_payload(json!({
            "refs": ["ghcr.io/org/demo:latest"]
        }));
        let err = validate_components_extension(&Some(extensions), false).unwrap_err();
        assert!(
            err.to_string().contains("digest-pinned"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn tag_refs_are_allowed_with_flag() {
        let extensions = ext_with_payload(json!({
            "refs": ["ghcr.io/org/demo:latest"]
        }));
        validate_components_extension(&Some(extensions), true).expect("tag allowed");
    }

    #[test]
    fn invalid_refs_are_rejected() {
        let extensions = ext_with_payload(json!({
            "refs": ["not-an-oci-ref"]
        }));
        assert!(validate_components_extension(&Some(extensions), true).is_err());
    }
}
