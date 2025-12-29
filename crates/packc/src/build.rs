use crate::config::{
    AssetConfig, ComponentConfig, ComponentOperationConfig, FlowConfig, PackConfig,
};
use crate::runtime::RuntimeContext;
use anyhow::{Context, Result, anyhow};
use greentic_flow::compile_ygtc_str;
use greentic_types::{
    BootstrapSpec, ComponentCapability, ComponentConfigurators, ComponentId, ComponentManifest,
    ComponentOperation, Flow, FlowId, PackDependency, PackFlowEntry, PackId, PackKind,
    PackManifest, PackSignatures, SecretRequirement, SecretScope, SemverReq, encode_pack_manifest,
};
use semver::Version;
use serde_yaml_bw::Value as YamlValue;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::info;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

#[derive(Clone)]
pub struct BuildOptions {
    pub pack_dir: PathBuf,
    pub component_out: Option<PathBuf>,
    pub manifest_out: PathBuf,
    pub sbom_out: Option<PathBuf>,
    pub gtpack_out: Option<PathBuf>,
    pub dry_run: bool,
    pub secrets_req: Option<PathBuf>,
    pub default_secret_scope: Option<String>,
    pub runtime: RuntimeContext,
}

impl BuildOptions {
    pub fn from_args(args: crate::BuildArgs, runtime: &RuntimeContext) -> Result<Self> {
        let pack_dir = args
            .input
            .canonicalize()
            .with_context(|| format!("failed to canonicalize pack dir {}", args.input.display()))?;

        let component_out = args
            .component_out
            .map(|p| if p.is_absolute() { p } else { pack_dir.join(p) });
        let manifest_out = args
            .manifest
            .map(|p| if p.is_relative() { pack_dir.join(p) } else { p })
            .unwrap_or_else(|| pack_dir.join("dist").join("manifest.cbor"));
        let sbom_out = args
            .sbom
            .map(|p| if p.is_absolute() { p } else { pack_dir.join(p) });
        let gtpack_out = args
            .gtpack_out
            .map(|p| if p.is_absolute() { p } else { pack_dir.join(p) });

        Ok(Self {
            pack_dir,
            component_out,
            manifest_out,
            sbom_out,
            gtpack_out,
            dry_run: args.dry_run,
            secrets_req: args.secrets_req,
            default_secret_scope: args.default_secret_scope,
            runtime: runtime.clone(),
        })
    }
}

pub fn run(opts: &BuildOptions) -> Result<()> {
    info!(
        pack_dir = %opts.pack_dir.display(),
        manifest_out = %opts.manifest_out.display(),
        gtpack_out = ?opts.gtpack_out,
        dry_run = opts.dry_run,
        "building greentic pack"
    );

    let config = crate::config::load_pack_config(&opts.pack_dir)?;
    info!(
        id = %config.pack_id,
        version = %config.version,
        kind = %config.kind,
        components = config.components.len(),
        flows = config.flows.len(),
        dependencies = config.dependencies.len(),
        "loaded pack.yaml"
    );

    let secret_requirements = aggregate_secret_requirements(
        &config.components,
        opts.secrets_req.as_deref(),
        opts.default_secret_scope.as_deref(),
    )?;

    let build = assemble_manifest(&config, &opts.pack_dir, &secret_requirements)?;
    let manifest_bytes = encode_pack_manifest(&build.manifest)?;
    info!(len = manifest_bytes.len(), "encoded manifest.cbor");

    if opts.dry_run {
        info!("dry-run complete; no files written");
        return Ok(());
    }

    if let Some(component_out) = opts.component_out.as_ref() {
        write_stub_wasm(component_out)?;
    }

    write_bytes(&opts.manifest_out, &manifest_bytes)?;

    if let Some(sbom_out) = opts.sbom_out.as_ref() {
        write_bytes(sbom_out, br#"{"files":[]} "#)?;
    }

    if let Some(gtpack_out) = opts.gtpack_out.as_ref() {
        let mut build = build;
        if !secret_requirements.is_empty() {
            let logical = "secret-requirements.json".to_string();
            let req_path =
                write_secret_requirements_file(&opts.pack_dir, &secret_requirements, &logical)?;
            build.assets.push(AssetFile {
                logical_path: logical,
                source: req_path,
            });
        }
        package_gtpack(gtpack_out, &manifest_bytes, &build)?;
        info!(gtpack_out = %gtpack_out.display(), "gtpack archive ready");
    }

    Ok(())
}

struct BuildProducts {
    manifest: PackManifest,
    components: Vec<ComponentBinary>,
    assets: Vec<AssetFile>,
}

struct ComponentBinary {
    id: String,
    source: PathBuf,
}

struct AssetFile {
    logical_path: String,
    source: PathBuf,
}

fn assemble_manifest(
    config: &PackConfig,
    pack_root: &Path,
    secret_requirements: &[SecretRequirement],
) -> Result<BuildProducts> {
    let components = build_components(&config.components)?;
    let flows = build_flows(&config.flows)?;
    let dependencies = build_dependencies(&config.dependencies)?;
    let assets = collect_assets(&config.assets, pack_root)?;
    let component_manifests: Vec<_> = components.iter().map(|c| c.0.clone()).collect();
    let bootstrap = build_bootstrap(config, &flows, &component_manifests)?;

    let manifest = PackManifest {
        schema_version: "pack-v1".to_string(),
        pack_id: PackId::new(config.pack_id.clone()).context("invalid pack_id")?,
        version: Version::parse(&config.version)
            .context("invalid pack version (expected semver)")?,
        kind: map_kind(&config.kind)?,
        publisher: config.publisher.clone(),
        components: component_manifests,
        flows,
        dependencies,
        capabilities: derive_pack_capabilities(&components),
        secret_requirements: secret_requirements.to_vec(),
        signatures: PackSignatures::default(),
        bootstrap,
        extensions: normalize_extensions(&config.extensions),
    };

    Ok(BuildProducts {
        manifest,
        components: components.into_iter().map(|(_, bin)| bin).collect(),
        assets,
    })
}

fn build_components(
    configs: &[ComponentConfig],
) -> Result<Vec<(ComponentManifest, ComponentBinary)>> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();

    for cfg in configs {
        if !seen.insert(cfg.id.clone()) {
            anyhow::bail!("duplicate component id {}", cfg.id);
        }

        info!(id = %cfg.id, wasm = %cfg.wasm.display(), "adding component");
        let manifest = ComponentManifest {
            id: ComponentId::new(cfg.id.clone()).context("invalid component id")?,
            version: Version::parse(&cfg.version)
                .context("invalid component version (expected semver)")?,
            supports: cfg.supports.iter().map(|k| k.to_kind()).collect(),
            world: cfg.world.clone(),
            profiles: cfg.profiles.clone(),
            capabilities: cfg.capabilities.clone(),
            configurators: convert_configurators(cfg)?,
            operations: cfg
                .operations
                .iter()
                .map(operation_from_config)
                .collect::<Result<Vec<_>>>()?,
            config_schema: cfg.config_schema.clone(),
            resources: cfg.resources.clone().unwrap_or_default(),
            dev_flows: BTreeMap::new(),
        };

        let binary = ComponentBinary {
            id: cfg.id.clone(),
            source: cfg.wasm.clone(),
        };

        result.push((manifest, binary));
    }

    Ok(result)
}

fn operation_from_config(cfg: &ComponentOperationConfig) -> Result<ComponentOperation> {
    Ok(ComponentOperation {
        name: cfg.name.clone(),
        input_schema: cfg.input_schema.clone(),
        output_schema: cfg.output_schema.clone(),
    })
}

fn convert_configurators(cfg: &ComponentConfig) -> Result<Option<ComponentConfigurators>> {
    let Some(configurators) = cfg.configurators.as_ref() else {
        return Ok(None);
    };

    let basic = match &configurators.basic {
        Some(id) => Some(FlowId::new(id).context("invalid configurator flow id")?),
        None => None,
    };
    let full = match &configurators.full {
        Some(id) => Some(FlowId::new(id).context("invalid configurator flow id")?),
        None => None,
    };

    Ok(Some(ComponentConfigurators { basic, full }))
}

fn build_bootstrap(
    config: &PackConfig,
    flows: &[PackFlowEntry],
    components: &[ComponentManifest],
) -> Result<Option<BootstrapSpec>> {
    let Some(raw) = config.bootstrap.as_ref() else {
        return Ok(None);
    };

    let flow_ids: BTreeSet<_> = flows.iter().map(|flow| flow.id.to_string()).collect();
    let component_ids: BTreeSet<_> = components.iter().map(|c| c.id.to_string()).collect();

    let mut spec = BootstrapSpec::default();

    if let Some(install_flow) = &raw.install_flow {
        if !flow_ids.contains(install_flow) {
            anyhow::bail!(
                "bootstrap.install_flow references unknown flow {}",
                install_flow
            );
        }
        spec.install_flow = Some(install_flow.clone());
    }

    if let Some(upgrade_flow) = &raw.upgrade_flow {
        if !flow_ids.contains(upgrade_flow) {
            anyhow::bail!(
                "bootstrap.upgrade_flow references unknown flow {}",
                upgrade_flow
            );
        }
        spec.upgrade_flow = Some(upgrade_flow.clone());
    }

    if let Some(component) = &raw.installer_component {
        if !component_ids.contains(component) {
            anyhow::bail!(
                "bootstrap.installer_component references unknown component {}",
                component
            );
        }
        spec.installer_component = Some(component.clone());
    }

    if spec.install_flow.is_none()
        && spec.upgrade_flow.is_none()
        && spec.installer_component.is_none()
    {
        return Ok(None);
    }

    Ok(Some(spec))
}

fn build_flows(configs: &[FlowConfig]) -> Result<Vec<PackFlowEntry>> {
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();

    for cfg in configs {
        info!(id = %cfg.id, path = %cfg.file.display(), "compiling flow");
        let yaml_src = fs::read_to_string(&cfg.file)
            .with_context(|| format!("failed to read flow {}", cfg.file.display()))?;

        let flow: Flow = compile_ygtc_str(&yaml_src)
            .with_context(|| format!("failed to compile {}", cfg.file.display()))?;

        let flow_id = flow.id.to_string();
        if !seen.insert(flow_id.clone()) {
            anyhow::bail!("duplicate flow id {}", flow_id);
        }

        let entrypoints = if cfg.entrypoints.is_empty() {
            flow.entrypoints.keys().cloned().collect()
        } else {
            cfg.entrypoints.clone()
        };

        let flow_entry = PackFlowEntry {
            id: flow.id.clone(),
            kind: flow.kind,
            flow,
            tags: cfg.tags.clone(),
            entrypoints,
        };
        entries.push(flow_entry);
    }

    Ok(entries)
}

fn build_dependencies(configs: &[crate::config::DependencyConfig]) -> Result<Vec<PackDependency>> {
    let mut deps = Vec::new();
    let mut seen = BTreeSet::new();
    for cfg in configs {
        if !seen.insert(cfg.alias.clone()) {
            anyhow::bail!("duplicate dependency alias {}", cfg.alias);
        }
        deps.push(PackDependency {
            alias: cfg.alias.clone(),
            pack_id: PackId::new(cfg.pack_id.clone()).context("invalid dependency pack_id")?,
            version_req: SemverReq::parse(&cfg.version_req)
                .context("invalid dependency version requirement")?,
            required_capabilities: cfg.required_capabilities.clone(),
        });
    }
    Ok(deps)
}

fn collect_assets(configs: &[AssetConfig], pack_root: &Path) -> Result<Vec<AssetFile>> {
    let mut assets = Vec::new();
    for cfg in configs {
        let logical = cfg
            .path
            .strip_prefix(pack_root)
            .unwrap_or(&cfg.path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if logical.is_empty() {
            anyhow::bail!("invalid asset path {}", cfg.path.display());
        }
        assets.push(AssetFile {
            logical_path: logical,
            source: cfg.path.clone(),
        });
    }
    Ok(assets)
}

fn normalize_extensions(
    extensions: &Option<BTreeMap<String, greentic_types::ExtensionRef>>,
) -> Option<BTreeMap<String, greentic_types::ExtensionRef>> {
    extensions.as_ref().filter(|map| !map.is_empty()).cloned()
}

fn derive_pack_capabilities(
    components: &[(ComponentManifest, ComponentBinary)],
) -> Vec<ComponentCapability> {
    let mut seen = BTreeSet::new();
    let mut caps = Vec::new();

    for (component, _) in components {
        let mut add = |name: &str| {
            if seen.insert(name.to_string()) {
                caps.push(ComponentCapability {
                    name: name.to_string(),
                    description: None,
                });
            }
        };

        if component.capabilities.host.secrets.is_some() {
            add("host:secrets");
        }
        if let Some(state) = &component.capabilities.host.state {
            if state.read {
                add("host:state:read");
            }
            if state.write {
                add("host:state:write");
            }
        }
        if component.capabilities.host.messaging.is_some() {
            add("host:messaging");
        }
        if component.capabilities.host.events.is_some() {
            add("host:events");
        }
        if component.capabilities.host.http.is_some() {
            add("host:http");
        }
        if component.capabilities.host.telemetry.is_some() {
            add("host:telemetry");
        }
        if component.capabilities.host.iac.is_some() {
            add("host:iac");
        }
        if let Some(fs) = component.capabilities.wasi.filesystem.as_ref() {
            add(&format!(
                "wasi:fs:{}",
                format!("{:?}", fs.mode).to_lowercase()
            ));
            if !fs.mounts.is_empty() {
                add("wasi:fs:mounts");
            }
        }
        if component.capabilities.wasi.random {
            add("wasi:random");
        }
        if component.capabilities.wasi.clocks {
            add("wasi:clocks");
        }
    }

    caps
}

fn map_kind(raw: &str) -> Result<PackKind> {
    match raw.to_ascii_lowercase().as_str() {
        "application" => Ok(PackKind::Application),
        "provider" => Ok(PackKind::Provider),
        "infrastructure" => Ok(PackKind::Infrastructure),
        "library" => Ok(PackKind::Library),
        other => Err(anyhow!("unknown pack kind {}", other)),
    }
}

fn package_gtpack(out_path: &Path, manifest_bytes: &[u8], build: &BuildProducts) -> Result<()> {
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let file = fs::File::create(out_path)
        .with_context(|| format!("failed to create {}", out_path.display()))?;
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o644);

    write_zip_entry(&mut writer, "manifest.cbor", manifest_bytes, options)?;

    let mut component_entries: Vec<_> = build
        .components
        .iter()
        .map(|c| (format!("components/{}.wasm", c.id), c.source.clone()))
        .collect();
    component_entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (logical, source) in component_entries {
        let bytes = fs::read(&source)
            .with_context(|| format!("failed to read component {}", source.display()))?;
        write_zip_entry(&mut writer, &logical, &bytes, options)?;
    }

    let mut asset_entries: Vec<_> = build
        .assets
        .iter()
        .map(|a| (format!("assets/{}", &a.logical_path), a.source.clone()))
        .collect();
    asset_entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (logical, source) in asset_entries {
        let bytes = fs::read(&source)
            .with_context(|| format!("failed to read asset {}", source.display()))?;
        write_zip_entry(&mut writer, &logical, &bytes, options)?;
    }

    writer
        .finish()
        .context("failed to finalise gtpack archive")?;
    Ok(())
}

fn write_zip_entry(
    writer: &mut ZipWriter<std::fs::File>,
    logical_path: &str,
    bytes: &[u8],
    options: SimpleFileOptions,
) -> Result<()> {
    writer
        .start_file(logical_path, options)
        .with_context(|| format!("failed to start {}", logical_path))?;
    writer
        .write_all(bytes)
        .with_context(|| format!("failed to write {}", logical_path))?;
    Ok(())
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    fs::write(path, bytes).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn write_stub_wasm(path: &Path) -> Result<()> {
    const STUB: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    write_bytes(path, STUB)
}

fn aggregate_secret_requirements(
    components: &[ComponentConfig],
    override_path: Option<&Path>,
    default_scope: Option<&str>,
) -> Result<Vec<SecretRequirement>> {
    let default_scope = default_scope.map(parse_default_scope).transpose()?;
    let mut merged: BTreeMap<(String, String, String), SecretRequirement> = BTreeMap::new();

    let mut process_req = |req: &SecretRequirement, source: &str| -> Result<()> {
        let mut req = req.clone();
        if req.scope.is_none() {
            if let Some(scope) = default_scope.clone() {
                req.scope = Some(scope);
                tracing::warn!(
                    key = %secret_key_string(&req),
                    source,
                    "secret requirement missing scope; applying default scope"
                );
            } else {
                anyhow::bail!(
                    "secret requirement {} from {} is missing scope (provide --default-secret-scope or fix the component manifest)",
                    secret_key_string(&req),
                    source
                );
            }
        }
        let scope = req.scope.as_ref().expect("scope present");
        let fmt = fmt_key(&req);
        let key_tuple = (req.key.clone().into(), scope_key(scope), fmt.clone());
        if let Some(existing) = merged.get_mut(&key_tuple) {
            merge_requirement(existing, &req);
        } else {
            merged.insert(key_tuple, req);
        }
        Ok(())
    };

    for component in components {
        if let Some(secret_caps) = component.capabilities.host.secrets.as_ref() {
            for req in &secret_caps.required {
                process_req(req, &component.id)?;
            }
        }
    }

    if let Some(path) = override_path {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read secrets override {}", path.display()))?;
        let value: serde_json::Value = if path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"))
            .unwrap_or(false)
        {
            let yaml: YamlValue = serde_yaml_bw::from_str(&contents)
                .with_context(|| format!("{} is not valid YAML", path.display()))?;
            serde_json::to_value(yaml).context("failed to normalise YAML secrets override")?
        } else {
            serde_json::from_str(&contents)
                .with_context(|| format!("{} is not valid JSON", path.display()))?
        };

        let overrides: Vec<SecretRequirement> =
            serde_json::from_value(value).with_context(|| {
                format!(
                    "{} must be an array of secret requirements (migration bridge)",
                    path.display()
                )
            })?;
        for req in &overrides {
            process_req(req, &format!("override:{}", path.display()))?;
        }
    }

    let mut out: Vec<SecretRequirement> = merged.into_values().collect();
    out.sort_by(|a, b| {
        let a_scope = a.scope.as_ref().map(scope_key).unwrap_or_default();
        let b_scope = b.scope.as_ref().map(scope_key).unwrap_or_default();
        (a_scope, secret_key_string(a), fmt_key(a)).cmp(&(
            b_scope,
            secret_key_string(b),
            fmt_key(b),
        ))
    });
    Ok(out)
}

fn fmt_key(req: &SecretRequirement) -> String {
    req.format
        .as_ref()
        .map(|f| format!("{:?}", f))
        .unwrap_or_else(|| "unspecified".to_string())
}

fn scope_key(scope: &SecretScope) -> String {
    format!(
        "{}/{}/{}",
        &scope.env,
        &scope.tenant,
        scope
            .team
            .as_deref()
            .map(|t| t.to_string())
            .unwrap_or_else(|| "_".to_string())
    )
}

fn secret_key_string(req: &SecretRequirement) -> String {
    let key: String = req.key.clone().into();
    key
}

fn merge_requirement(base: &mut SecretRequirement, incoming: &SecretRequirement) {
    if base.description.is_none() {
        base.description = incoming.description.clone();
    }
    if let Some(schema) = &incoming.schema {
        if base.schema.is_none() {
            base.schema = Some(schema.clone());
        } else if base.schema.as_ref() != Some(schema) {
            tracing::warn!(
                key = %secret_key_string(base),
                "conflicting secret schema encountered; keeping first"
            );
        }
    }

    if !incoming.examples.is_empty() {
        for example in &incoming.examples {
            if !base.examples.contains(example) {
                base.examples.push(example.clone());
            }
        }
    }

    base.required = base.required || incoming.required;
}

fn parse_default_scope(raw: &str) -> Result<SecretScope> {
    let parts: Vec<_> = raw.split('/').collect();
    if parts.len() < 2 || parts.len() > 3 {
        anyhow::bail!(
            "default secret scope must be ENV/TENANT or ENV/TENANT/TEAM (got {})",
            raw
        );
    }
    Ok(SecretScope {
        env: parts[0].to_string(),
        tenant: parts[1].to_string(),
        team: parts.get(2).map(|s| s.to_string()),
    })
}

fn write_secret_requirements_file(
    pack_root: &Path,
    requirements: &[SecretRequirement],
    logical_name: &str,
) -> Result<PathBuf> {
    let path = pack_root.join(".packc").join(logical_name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let data = serde_json::to_vec_pretty(&requirements)
        .context("failed to serialise secret requirements")?;
    fs::write(&path, data).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BootstrapConfig;
    use greentic_types::flow::FlowKind;
    use serde_json::json;
    use std::io::Read;
    use std::{fs, path::PathBuf};
    use tempfile::tempdir;
    use zip::ZipArchive;

    #[test]
    fn map_kind_accepts_known_values() {
        assert!(matches!(
            map_kind("application").unwrap(),
            PackKind::Application
        ));
        assert!(matches!(map_kind("provider").unwrap(), PackKind::Provider));
        assert!(matches!(
            map_kind("infrastructure").unwrap(),
            PackKind::Infrastructure
        ));
        assert!(matches!(map_kind("library").unwrap(), PackKind::Library));
        assert!(map_kind("unknown").is_err());
    }

    #[test]
    fn collect_assets_preserves_relative_paths() {
        let root = PathBuf::from("/packs/demo");
        let assets = vec![AssetConfig {
            path: root.join("assets").join("foo.txt"),
        }];
        let collected = collect_assets(&assets, &root).expect("collect assets");
        assert_eq!(collected[0].logical_path, "assets/foo.txt");
    }

    #[test]
    fn build_bootstrap_requires_known_references() {
        let config = pack_config_with_bootstrap(BootstrapConfig {
            install_flow: Some("flow.a".to_string()),
            upgrade_flow: None,
            installer_component: Some("component.a".to_string()),
        });
        let flows = vec![flow_entry("flow.a")];
        let components = vec![minimal_component_manifest("component.a")];

        let bootstrap = build_bootstrap(&config, &flows, &components)
            .expect("bootstrap populated")
            .expect("bootstrap present");

        assert_eq!(bootstrap.install_flow.as_deref(), Some("flow.a"));
        assert_eq!(bootstrap.upgrade_flow, None);
        assert_eq!(
            bootstrap.installer_component.as_deref(),
            Some("component.a")
        );
    }

    #[test]
    fn build_bootstrap_rejects_unknown_flow() {
        let config = pack_config_with_bootstrap(BootstrapConfig {
            install_flow: Some("missing".to_string()),
            upgrade_flow: None,
            installer_component: Some("component.a".to_string()),
        });
        let flows = vec![flow_entry("flow.a")];
        let components = vec![minimal_component_manifest("component.a")];

        let err = build_bootstrap(&config, &flows, &components).unwrap_err();
        assert!(
            err.to_string()
                .contains("bootstrap.install_flow references unknown flow"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn component_manifest_without_dev_flows_defaults_to_empty() {
        let manifest: ComponentManifest = serde_json::from_value(json!({
            "id": "component.dev",
            "version": "1.0.0",
            "supports": ["messaging"],
            "world": "greentic:demo@1.0.0",
            "profiles": { "default": "default", "supported": ["default"] },
            "capabilities": { "wasi": {}, "host": {} },
            "operations": [],
            "resources": {}
        }))
        .expect("manifest without dev_flows");

        assert!(manifest.dev_flows.is_empty());

        let pack_manifest = pack_manifest_with_component(manifest.clone());
        let encoded = encode_pack_manifest(&pack_manifest).expect("encode manifest");
        let decoded: PackManifest =
            greentic_types::decode_pack_manifest(&encoded).expect("decode manifest");
        let stored = decoded
            .components
            .iter()
            .find(|item| item.id == manifest.id)
            .expect("component present");
        assert!(stored.dev_flows.is_empty());
    }

    #[test]
    fn dev_flows_round_trip_in_manifest_and_gtpack() {
        let component = manifest_with_dev_flow();
        let pack_manifest = pack_manifest_with_component(component.clone());
        let manifest_bytes = encode_pack_manifest(&pack_manifest).expect("encode manifest");

        let decoded: PackManifest =
            greentic_types::decode_pack_manifest(&manifest_bytes).expect("decode manifest");
        let decoded_component = decoded
            .components
            .iter()
            .find(|item| item.id == component.id)
            .expect("component present");
        assert_eq!(decoded_component.dev_flows, component.dev_flows);

        let temp = tempdir().expect("temp dir");
        let wasm_path = temp.path().join("component.wasm");
        write_stub_wasm(&wasm_path).expect("write stub wasm");

        let build = BuildProducts {
            manifest: pack_manifest,
            components: vec![ComponentBinary {
                id: component.id.to_string(),
                source: wasm_path,
            }],
            assets: Vec::new(),
        };

        let out = temp.path().join("demo.gtpack");
        package_gtpack(&out, &manifest_bytes, &build).expect("package gtpack");

        let mut archive = ZipArchive::new(fs::File::open(&out).expect("open gtpack"))
            .expect("read gtpack archive");
        let mut manifest_entry = archive.by_name("manifest.cbor").expect("manifest.cbor");
        let mut stored = Vec::new();
        manifest_entry
            .read_to_end(&mut stored)
            .expect("read manifest");
        let decoded: PackManifest =
            greentic_types::decode_pack_manifest(&stored).expect("decode packaged manifest");

        let stored_component = decoded
            .components
            .iter()
            .find(|item| item.id == component.id)
            .expect("component preserved");
        assert_eq!(stored_component.dev_flows, component.dev_flows);
    }

    #[test]
    fn aggregate_secret_requirements_dedupes_and_sorts() {
        let component: ComponentConfig = serde_json::from_value(json!({
            "id": "component.a",
            "version": "1.0.0",
            "world": "greentic:demo@1.0.0",
            "supports": [],
            "profiles": { "default": "default", "supported": ["default"] },
            "capabilities": {
                "wasi": {},
                "host": {
                    "secrets": {
                        "required": [
                    {
                        "key": "db/password",
                        "required": true,
                        "scope": { "env": "dev", "tenant": "t1" },
                        "format": "text",
                        "description": "primary"
                    }
                ]
            }
        }
            },
            "wasm": "component.wasm",
            "operations": [],
            "resources": {}
        }))
        .expect("component config");

        let dupe: ComponentConfig = serde_json::from_value(json!({
            "id": "component.b",
            "version": "1.0.0",
            "world": "greentic:demo@1.0.0",
            "supports": [],
            "profiles": { "default": "default", "supported": ["default"] },
            "capabilities": {
                "wasi": {},
                "host": {
                    "secrets": {
                        "required": [
                            {
                        "key": "db/password",
                        "required": true,
                        "scope": { "env": "dev", "tenant": "t1" },
                        "format": "text",
                        "description": "secondary",
                        "examples": ["example"]
                    }
                ]
            }
                }
            },
            "wasm": "component.wasm",
            "operations": [],
            "resources": {}
        }))
        .expect("component config");

        let reqs = aggregate_secret_requirements(&[component, dupe], None, None)
            .expect("aggregate secrets");
        assert_eq!(reqs.len(), 1);
        let req = &reqs[0];
        assert_eq!(req.description.as_deref(), Some("primary"));
        assert!(req.examples.contains(&"example".to_string()));
    }

    fn pack_config_with_bootstrap(bootstrap: BootstrapConfig) -> PackConfig {
        PackConfig {
            pack_id: "demo.pack".to_string(),
            version: "1.0.0".to_string(),
            kind: "application".to_string(),
            publisher: "demo".to_string(),
            bootstrap: Some(bootstrap),
            components: Vec::new(),
            dependencies: Vec::new(),
            flows: Vec::new(),
            assets: Vec::new(),
            extensions: None,
        }
    }

    fn flow_entry(id: &str) -> PackFlowEntry {
        let flow: Flow = serde_json::from_value(json!({
            "schema_version": "flow/v1",
            "id": id,
            "kind": "messaging"
        }))
        .expect("flow json");

        PackFlowEntry {
            id: FlowId::new(id).expect("flow id"),
            kind: FlowKind::Messaging,
            flow,
            tags: Vec::new(),
            entrypoints: Vec::new(),
        }
    }

    fn minimal_component_manifest(id: &str) -> ComponentManifest {
        serde_json::from_value(json!({
            "id": id,
            "version": "1.0.0",
            "supports": [],
            "world": "greentic:demo@1.0.0",
            "profiles": { "default": "default", "supported": ["default"] },
            "capabilities": { "wasi": {}, "host": {} },
            "operations": [],
            "resources": {}
        }))
        .expect("component manifest")
    }

    fn manifest_with_dev_flow() -> ComponentManifest {
        serde_json::from_str(include_str!(
            "../tests/fixtures/component_manifest_with_dev_flows.json"
        ))
        .expect("fixture manifest")
    }

    fn pack_manifest_with_component(component: ComponentManifest) -> PackManifest {
        let flow = serde_json::from_value(json!({
            "schema_version": "flow/v1",
            "id": "flow.dev",
            "kind": "messaging"
        }))
        .expect("flow json");

        PackManifest {
            schema_version: "pack-v1".to_string(),
            pack_id: PackId::new("demo.pack").expect("pack id"),
            version: Version::parse("1.0.0").expect("version"),
            kind: PackKind::Application,
            publisher: "demo".to_string(),
            components: vec![component],
            flows: vec![PackFlowEntry {
                id: FlowId::new("flow.dev").expect("flow id"),
                kind: FlowKind::Messaging,
                flow,
                tags: Vec::new(),
                entrypoints: Vec::new(),
            }],
            dependencies: Vec::new(),
            capabilities: Vec::new(),
            secret_requirements: Vec::new(),
            signatures: PackSignatures::default(),
            bootstrap: None,
            extensions: None,
        }
    }
}
