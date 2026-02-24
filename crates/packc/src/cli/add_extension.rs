use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use greentic_types::provider::{ProviderDecl, ProviderRuntimeRef};
use serde_yaml_bw::{self, Mapping, Sequence, Value as YamlValue};
use walkdir::WalkDir;

use crate::config::PackConfig;

pub const PROVIDER_RUNTIME_WORLD: &str = "greentic:provider/schema-core@1.0.0";
const PROVIDER_EXTENSION_KEY: &str = "greentic.provider-extension.v1";
const PROVIDER_EXTENSION_PATH: [&str; 3] = ["greentic", "provider-extension", "v1"];
const CAPABILITIES_EXTENSION_KEY: &str = "greentic.ext.capabilities.v1";

#[derive(Debug, Subcommand)]
pub enum AddExtensionCommand {
    /// Add or update the provider extension entry.
    Provider(ProviderArgs),
    /// Add or update a capability offer entry.
    Capability(CapabilityArgs),
}

#[derive(Debug, Args)]
pub struct ProviderArgs {
    /// Path to a pack source directory containing pack.yaml.
    #[arg(long = "pack-dir", value_name = "DIR")]
    pub pack_dir: PathBuf,

    /// Print what would change without writing files.
    #[arg(long)]
    pub dry_run: bool,

    /// Provider identifier to add or update.
    #[arg(long = "id", value_name = "PROVIDER_ID")]
    pub provider_id: String,

    /// Provider kind (e.g. messaging, events).
    #[arg(long = "kind", value_name = "KIND")]
    pub kind: String,

    /// Optional provider title to store in metadata.
    #[arg(long, value_name = "TITLE")]
    pub title: Option<String>,

    /// Optional description to store in metadata.
    #[arg(long, value_name = "DESCRIPTION")]
    pub description: Option<String>,
    /// Optional validator reference for generated provider metadata.
    #[arg(long = "validator-ref", value_name = "VALIDATOR_REF")]
    pub validator_ref: Option<String>,
    /// Optional validator digest for strict pinning.
    #[arg(long = "validator-digest", value_name = "DIGEST")]
    pub validator_digest: Option<String>,

    /// Convenience route hint (if schema supports route<->flow binding).
    #[arg(long = "route", value_name = "ROUTE")]
    pub route: Option<String>,

    /// Convenience flow hint (if schema supports route<->flow binding).
    #[arg(long = "flow", value_name = "FLOW")]
    pub flow: Option<String>,
}

#[derive(Debug, Args)]
pub struct CapabilityArgs {
    /// Path to a pack source directory containing pack.yaml.
    #[arg(long = "pack-dir", value_name = "DIR")]
    pub pack_dir: PathBuf,

    /// Print what would change without writing files.
    #[arg(long)]
    pub dry_run: bool,

    /// Stable offer id.
    #[arg(long = "offer-id", value_name = "ID")]
    pub offer_id: String,

    /// Capability identifier.
    #[arg(long = "cap-id", value_name = "CAP_ID")]
    pub cap_id: String,

    /// Capability contract version.
    #[arg(long, default_value = "v1")]
    pub version: String,

    /// Provider component reference.
    #[arg(long = "component-ref", value_name = "COMPONENT")]
    pub component_ref: String,

    /// Provider operation.
    #[arg(long = "op", value_name = "OP")]
    pub op: String,

    /// Selection priority (ascending).
    #[arg(long, default_value_t = 0)]
    pub priority: i32,

    /// Mark offer as requiring setup.
    #[arg(long = "requires-setup", default_value_t = false)]
    pub requires_setup: bool,

    /// Pack-relative QA spec ref (required when --requires-setup).
    #[arg(long = "qa-ref", value_name = "REF")]
    pub qa_ref: Option<String>,

    /// Exact operation names for hook applicability (repeatable).
    #[arg(long = "hook-op-name", value_name = "OP_NAME")]
    pub hook_op_names: Vec<String>,
}

pub fn handle(command: AddExtensionCommand) -> Result<()> {
    match command {
        AddExtensionCommand::Provider(args) => handle_provider(args),
        AddExtensionCommand::Capability(args) => handle_capability(args),
    }
}

fn handle_provider(args: ProviderArgs) -> Result<()> {
    eprintln!(
        "note: provider extension updates use the legacy schema-core path (`greentic:provider/schema-core@1.0.0`)"
    );
    edit_pack_dir(&args.pack_dir, &args)?;
    Ok(())
}

fn handle_capability(args: CapabilityArgs) -> Result<()> {
    let root = normalize_root(&args.pack_dir)?;
    let pack_yaml = root.join("pack.yaml");
    let (_, contents) = read_pack_yaml(&pack_yaml)?;
    let updated_yaml = inject_capability_offer(&contents, &args)?;

    if args.dry_run {
        println!("--- dry-run: updated pack.yaml ---");
        println!("{updated_yaml}");
        return Ok(());
    }

    fs::write(&pack_yaml, updated_yaml)
        .with_context(|| format!("write {}", pack_yaml.display()))?;
    println!("capabilities extension updated in {}", pack_yaml.display());
    Ok(())
}

fn edit_pack_dir(pack_dir: &Path, args: &ProviderArgs) -> Result<()> {
    let root = normalize_root(pack_dir)?;
    let pack_yaml = root.join("pack.yaml");
    let (pack_config, contents) = read_pack_yaml(&pack_yaml)?;
    let metadata = ProviderMetadata::from_args(args);
    let updated_yaml = inject_provider_entry(
        &contents,
        &build_provider_decl(args, &root)?,
        metadata,
        &pack_config.version,
    )?;

    if args.dry_run {
        println!("--- dry-run: updated pack.yaml ---");
        println!("{updated_yaml}");
        return Ok(());
    }

    fs::write(&pack_yaml, updated_yaml)
        .with_context(|| format!("write {}", pack_yaml.display()))?;
    println!("provider extension updated in {}", pack_yaml.display());
    Ok(())
}

fn normalize_root(path: &Path) -> Result<PathBuf> {
    let canonical = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(canonical)
}

fn read_pack_yaml(path: &Path) -> Result<(PackConfig, String)> {
    let contents = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let config: PackConfig = serde_yaml_bw::from_str(&contents)
        .with_context(|| format!("{} is not a valid pack.yaml", path.display()))?;
    Ok((config, contents))
}

#[derive(Default)]
struct ProviderMetadata {
    title: Option<String>,
    description: Option<String>,
    route: Option<String>,
    flow: Option<String>,
    validator_ref: Option<String>,
    validator_digest: Option<String>,
}

impl ProviderMetadata {
    fn from_args(args: &ProviderArgs) -> Self {
        Self {
            title: args.title.clone(),
            description: args.description.clone(),
            route: args.route.clone(),
            flow: args.flow.clone(),
            validator_ref: args.validator_ref.clone(),
            validator_digest: args.validator_digest.clone(),
        }
    }
}

fn build_provider_decl(args: &ProviderArgs, root: &Path) -> Result<ProviderDecl> {
    let config_ref = find_config_schema_ref(root, &args.kind, &args.provider_id);
    let capabilities = vec![args.kind.clone()];
    let ops = match args.kind.as_str() {
        "messaging" => vec!["send".to_string(), "receive".to_string()],
        "events" => vec!["emit".to_string(), "subscribe".to_string()],
        _ => vec!["run".to_string()],
    };

    Ok(ProviderDecl {
        provider_type: args.provider_id.clone(),
        capabilities,
        ops,
        config_schema_ref: config_ref,
        state_schema_ref: None,
        runtime: ProviderRuntimeRef {
            component_ref: args.provider_id.clone(),
            export: "provider".to_string(),
            world: PROVIDER_RUNTIME_WORLD.to_string(),
        },
        docs_ref: None,
    })
}

fn find_config_schema_ref(root: &Path, kind: &str, provider_id: &str) -> String {
    let schemas = root.join("schemas");
    if schemas.exists() {
        let provider_kw = provider_id.to_ascii_lowercase();
        for entry in WalkDir::new(&schemas)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.contains(&provider_kw)
                && name.contains("config.schema")
                && let Ok(rel) = entry.path().strip_prefix(root)
            {
                return rel
                    .components()
                    .map(|comp| comp.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
            }
        }
    }

    format!("schemas/{}/{}/config.schema.json", kind, provider_id)
}

fn inject_provider_entry(
    contents: &str,
    provider: &ProviderDecl,
    metadata: ProviderMetadata,
    version: &str,
) -> Result<String> {
    let mut document: YamlValue =
        serde_yaml_bw::from_str(contents).context("parse pack.yaml for extension merge")?;
    let mapping = document
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("pack.yaml root must be a mapping"))?;
    let extensions = mapping
        .entry(yaml_key("extensions"))
        .or_insert_with(|| YamlValue::Mapping(Mapping::new()));
    let extensions_map = extensions
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("extensions must be a mapping"))?;

    let location = detect_extension_location(extensions_map);
    let extension_map = resolve_extension_map(extensions_map, &location)
        .context("locate provider extension slot")?;
    extension_map
        .entry(yaml_key("kind"))
        .or_insert_with(|| YamlValue::String(PROVIDER_EXTENSION_KEY.to_string(), None));
    extension_map
        .entry(yaml_key("version"))
        .or_insert_with(|| YamlValue::String(version.to_string(), None));

    let inline = extension_map
        .entry(yaml_key("inline"))
        .or_insert_with(|| YamlValue::Mapping(Mapping::new()));
    let inline_map = match inline {
        YamlValue::Mapping(map) => map,
        _ => {
            *inline = YamlValue::Mapping(Mapping::new());
            inline.as_mapping_mut().unwrap()
        }
    };

    let providers_key = yaml_key("providers");
    let providers_entry = inline_map
        .entry(providers_key.clone())
        .or_insert_with(|| YamlValue::Sequence(Sequence::default()));
    let providers = match providers_entry {
        YamlValue::Sequence(seq) => seq,
        _ => {
            *providers_entry = YamlValue::Sequence(Sequence::default());
            providers_entry.as_sequence_mut().unwrap()
        }
    };

    let mut provider_value =
        serde_yaml_bw::to_value(provider).context("serialize provider declaration")?;
    if let Some(map) = provider_value.as_mapping_mut() {
        if let Some(title) = metadata.title {
            map.insert(yaml_key("title"), YamlValue::String(title, None));
        }
        if let Some(desc) = metadata.description {
            map.insert(yaml_key("description"), YamlValue::String(desc, None));
        }
        if let Some(route) = metadata.route {
            map.insert(yaml_key("route"), YamlValue::String(route, None));
        }
        if let Some(flow) = metadata.flow {
            map.insert(yaml_key("flow"), YamlValue::String(flow, None));
        }
        if let Some(validator_ref) = metadata.validator_ref {
            map.insert(
                yaml_key("validator_ref"),
                YamlValue::String(validator_ref, None),
            );
        }
        if let Some(validator_digest) = metadata.validator_digest {
            map.insert(
                yaml_key("validator_digest"),
                YamlValue::String(validator_digest, None),
            );
        }
    }
    upsert_provider(providers, provider_value, &provider.provider_type);

    serde_yaml_bw::to_string(&document).context("serialize updated pack.yaml")
}

fn inject_capability_offer(contents: &str, args: &CapabilityArgs) -> Result<String> {
    if args.requires_setup && args.qa_ref.is_none() {
        anyhow::bail!("--qa-ref is required when --requires-setup is set");
    }
    if let Some(qa_ref) = args.qa_ref.as_ref()
        && qa_ref.trim().is_empty()
    {
        anyhow::bail!("--qa-ref must not be empty");
    }

    let mut document: YamlValue =
        serde_yaml_bw::from_str(contents).context("parse pack.yaml for extension merge")?;
    let mapping = document
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("pack.yaml root must be a mapping"))?;
    let extensions = mapping
        .entry(yaml_key("extensions"))
        .or_insert_with(|| YamlValue::Mapping(Mapping::new()));
    let extensions_map = extensions
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("extensions must be a mapping"))?;
    let extension_slot = extensions_map
        .entry(yaml_key(CAPABILITIES_EXTENSION_KEY))
        .or_insert_with(|| YamlValue::Mapping(Mapping::new()));
    let extension_map = extension_slot
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("capabilities extension slot must be a mapping"))?;
    extension_map
        .entry(yaml_key("kind"))
        .or_insert_with(|| YamlValue::String(CAPABILITIES_EXTENSION_KEY.to_string(), None));
    extension_map
        .entry(yaml_key("version"))
        .or_insert_with(|| YamlValue::String("1.0.0".to_string(), None));

    let inline = extension_map
        .entry(yaml_key("inline"))
        .or_insert_with(|| YamlValue::Mapping(Mapping::new()));
    let inline_map = match inline {
        YamlValue::Mapping(map) => map,
        _ => {
            *inline = YamlValue::Mapping(Mapping::new());
            inline.as_mapping_mut().expect("inline map")
        }
    };
    inline_map
        .entry(yaml_key("schema_version"))
        .or_insert_with(|| YamlValue::Number(1u64.into(), None));

    let offers_entry = inline_map
        .entry(yaml_key("offers"))
        .or_insert_with(|| YamlValue::Sequence(Sequence::default()));
    let offers = match offers_entry {
        YamlValue::Sequence(seq) => seq,
        _ => {
            *offers_entry = YamlValue::Sequence(Sequence::default());
            offers_entry.as_sequence_mut().expect("offers seq")
        }
    };

    let mut offer = Mapping::new();
    offer.insert(
        yaml_key("offer_id"),
        YamlValue::String(args.offer_id.clone(), None),
    );
    offer.insert(
        yaml_key("cap_id"),
        YamlValue::String(args.cap_id.clone(), None),
    );
    offer.insert(
        yaml_key("version"),
        YamlValue::String(args.version.clone(), None),
    );
    let mut provider = Mapping::new();
    provider.insert(
        yaml_key("component_ref"),
        YamlValue::String(args.component_ref.clone(), None),
    );
    provider.insert(yaml_key("op"), YamlValue::String(args.op.clone(), None));
    offer.insert(yaml_key("provider"), YamlValue::Mapping(provider));
    offer.insert(
        yaml_key("priority"),
        YamlValue::Number(i64::from(args.priority).into(), None),
    );
    offer.insert(
        yaml_key("requires_setup"),
        YamlValue::Bool(args.requires_setup, None),
    );
    if let Some(qa_ref) = args.qa_ref.as_ref() {
        let mut setup = Mapping::new();
        setup.insert(yaml_key("qa_ref"), YamlValue::String(qa_ref.clone(), None));
        offer.insert(yaml_key("setup"), YamlValue::Mapping(setup));
    }
    if !args.hook_op_names.is_empty() {
        let mut applies_to = Mapping::new();
        let mut op_names = Sequence::default();
        for name in &args.hook_op_names {
            op_names.push(YamlValue::String(name.clone(), None));
        }
        applies_to.insert(yaml_key("op_names"), YamlValue::Sequence(op_names));
        offer.insert(yaml_key("applies_to"), YamlValue::Mapping(applies_to));
    }
    upsert_capability_offer(offers, YamlValue::Mapping(offer), &args.offer_id);

    serde_yaml_bw::to_string(&document).context("serialize updated pack.yaml")
}

fn upsert_capability_offer(offers: &mut Vec<YamlValue>, offer: YamlValue, offer_id: &str) {
    for entry in offers.iter_mut() {
        if entry_matches_capability_offer(entry, offer_id) {
            *entry = offer;
            return;
        }
    }
    offers.push(offer);
}

fn entry_matches_capability_offer(entry: &YamlValue, offer_id: &str) -> bool {
    let key = yaml_key("offer_id");
    if let YamlValue::Mapping(map) = entry
        && let Some(YamlValue::String(value, _)) = map.get(&key)
    {
        return value == offer_id;
    }
    false
}

fn upsert_provider(providers: &mut Vec<YamlValue>, provider: YamlValue, provider_id: &str) {
    for entry in providers.iter_mut() {
        if entry_matches_provider(entry, provider_id) {
            *entry = provider;
            return;
        }
    }
    providers.push(provider);
}

fn entry_matches_provider(entry: &YamlValue, provider_id: &str) -> bool {
    let provider_key = yaml_key("provider_type");
    if let YamlValue::Mapping(map) = entry
        && let Some(YamlValue::String(value, _)) = map.get(&provider_key)
    {
        return value == provider_id;
    }
    false
}

enum ExtensionLocation {
    Flat,
    Nested,
}

fn detect_extension_location(extensions: &Mapping) -> ExtensionLocation {
    let provider_key = yaml_key(PROVIDER_EXTENSION_KEY);
    if extensions.contains_key(&provider_key) {
        return ExtensionLocation::Flat;
    }
    let mut current = extensions;
    for segment in PROVIDER_EXTENSION_PATH
        .iter()
        .take(PROVIDER_EXTENSION_PATH.len() - 1)
    {
        let key = yaml_key(*segment);
        if let Some(next) = current.get(&key).and_then(YamlValue::as_mapping) {
            current = next;
        } else {
            return ExtensionLocation::Flat;
        }
    }
    ExtensionLocation::Nested
}

fn resolve_extension_map<'a>(
    extensions: &'a mut Mapping,
    location: &ExtensionLocation,
) -> Result<&'a mut Mapping> {
    match location {
        ExtensionLocation::Flat => {
            let key = yaml_key(PROVIDER_EXTENSION_KEY);
            let slot = extensions
                .entry(key)
                .or_insert_with(|| YamlValue::Mapping(Mapping::new()));
            slot.as_mapping_mut()
                .ok_or_else(|| anyhow::anyhow!("extension slot must be a mapping"))
        }
        ExtensionLocation::Nested => {
            let mut current_map = extensions;
            for segment in PROVIDER_EXTENSION_PATH.iter() {
                let key = yaml_key(*segment);
                let entry = current_map
                    .entry(key)
                    .or_insert_with(|| YamlValue::Mapping(Mapping::new()));
                current_map = entry
                    .as_mapping_mut()
                    .ok_or_else(|| anyhow::anyhow!("nested extension value must be a mapping"))?;
            }
            Ok(current_map)
        }
    }
}

fn yaml_key(value: impl Into<String>) -> YamlValue {
    YamlValue::String(value.into(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml_bw;

    fn sample_flat_yaml() -> String {
        r#"pack_id: demo
version: 0.1.0
extensions:
  greentic.provider-extension.v1:
    kind: greentic.provider-extension.v1
    version: 0.1.0
    inline:
      providers:
        - provider_type: existing
          capabilities: [messaging]
          ops: [send]
          config_schema_ref: schemas/messaging/existing/config.schema.json
          runtime:
            component_ref: existing
            export: provider
            world: greentic:provider/schema-core@1.0.0
"#
        .to_string()
    }

    fn sample_nested_yaml() -> String {
        r#"pack_id: demo
version: 0.1.0
extensions:
  greentic:
    provider-extension:
      v1:
        inline:
          providers: []
"#
        .to_string()
    }

    fn provider_decl() -> ProviderDecl {
        ProviderDecl {
            provider_type: "demo.provider".to_string(),
            capabilities: vec!["messaging".to_string()],
            ops: vec!["send".to_string()],
            config_schema_ref: "schemas/messaging/demo/config.schema.json".to_string(),
            state_schema_ref: None,
            runtime: ProviderRuntimeRef {
                component_ref: "demo.provider".to_string(),
                export: "provider".to_string(),
                world: PROVIDER_RUNTIME_WORLD.to_string(),
            },
            docs_ref: None,
        }
    }

    #[test]
    fn inject_flat_extension() {
        let contents = sample_flat_yaml();
        let updated = inject_provider_entry(
            &contents,
            &provider_decl(),
            ProviderMetadata::default(),
            "0.1.0",
        )
        .unwrap();
        let doc: YamlValue = serde_yaml_bw::from_str(&updated).unwrap();

        let providers = doc["extensions"]["greentic.provider-extension.v1"]["inline"]["providers"]
            .as_sequence()
            .expect("providers list");
        assert!(
            providers
                .iter()
                .any(|entry| entry_matches_provider(entry, "demo.provider"))
        );
    }

    #[test]
    fn inject_nested_extension() {
        let contents = sample_nested_yaml();
        let updated = inject_provider_entry(
            &contents,
            &provider_decl(),
            ProviderMetadata::default(),
            "0.1.0",
        )
        .unwrap();
        let doc: YamlValue = serde_yaml_bw::from_str(&updated).unwrap();

        assert!(
            doc["extensions"]["greentic"]["provider-extension"]["v1"]["inline"]["providers"]
                .as_sequence()
                .unwrap()
                .iter()
                .any(|entry| entry_matches_provider(entry, "demo.provider"))
        );
    }
}
