use crate::cli::resolve::{self, ResolveArgs};
use crate::config::{
    AssetConfig, ComponentConfig, ComponentOperationConfig, FlowConfig, PackConfig,
};
use crate::extensions::validate_components_extension;
use crate::flow_resolve::load_flow_resolve_summary;
use crate::runtime::{NetworkPolicy, RuntimeContext};
use anyhow::{Context, Result, anyhow};
use greentic_distributor_client::{DistClient, DistOptions};
use greentic_flow::add_step::normalize::normalize_node_map;
use greentic_flow::compile_ygtc_file;
use greentic_flow::loader::load_ygtc_from_path;
use greentic_pack::builder::SbomEntry;
use greentic_pack::pack_lock::read_pack_lock;
use greentic_types::component_source::ComponentSourceRef;
use greentic_types::flow_resolve_summary::FlowResolveSummaryV1;
use greentic_types::pack::extensions::component_manifests::{
    ComponentManifestIndexEntryV1, ComponentManifestIndexV1, EXT_COMPONENT_MANIFEST_INDEX_V1,
    ManifestEncoding,
};
use greentic_types::pack::extensions::component_sources::{
    ArtifactLocationV1, ComponentSourceEntryV1, ComponentSourcesV1, EXT_COMPONENT_SOURCES_V1,
    ResolvedComponentV1,
};
use greentic_types::{
    BootstrapSpec, ComponentCapability, ComponentConfigurators, ComponentId, ComponentManifest,
    ComponentOperation, ExtensionInline, ExtensionRef, Flow, FlowId, PackDependency, PackFlowEntry,
    PackId, PackKind, PackManifest, PackSignatures, SecretRequirement, SecretScope, SemverReq,
    encode_pack_manifest,
};
use semver::Version;
use serde::Serialize;
use serde_cbor;
use serde_yaml_bw::Value as YamlValue;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tracing::info;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const SBOM_FORMAT: &str = "greentic-sbom-v1";

#[derive(Serialize)]
struct SbomDocument {
    format: String,
    files: Vec<SbomEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum BundleMode {
    Cache,
    None,
}

#[derive(Clone)]
pub struct BuildOptions {
    pub pack_dir: PathBuf,
    pub component_out: Option<PathBuf>,
    pub manifest_out: PathBuf,
    pub sbom_out: Option<PathBuf>,
    pub gtpack_out: Option<PathBuf>,
    pub lock_path: PathBuf,
    pub bundle: BundleMode,
    pub dry_run: bool,
    pub secrets_req: Option<PathBuf>,
    pub default_secret_scope: Option<String>,
    pub allow_oci_tags: bool,
    pub require_component_manifests: bool,
    pub runtime: RuntimeContext,
    pub skip_update: bool,
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
        let default_gtpack_name = pack_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("pack");
        let default_gtpack_out = pack_dir
            .join("dist")
            .join(format!("{default_gtpack_name}.gtpack"));
        let gtpack_out = Some(
            args.gtpack_out
                .map(|p| if p.is_absolute() { p } else { pack_dir.join(p) })
                .unwrap_or(default_gtpack_out),
        );
        let lock_path = args
            .lock
            .map(|p| if p.is_absolute() { p } else { pack_dir.join(p) })
            .unwrap_or_else(|| pack_dir.join("pack.lock.json"));

        Ok(Self {
            pack_dir,
            component_out,
            manifest_out,
            sbom_out,
            gtpack_out,
            lock_path,
            bundle: args.bundle,
            dry_run: args.dry_run,
            secrets_req: args.secrets_req,
            default_secret_scope: args.default_secret_scope,
            allow_oci_tags: args.allow_oci_tags,
            require_component_manifests: args.require_component_manifests,
            runtime: runtime.clone(),
            skip_update: args.no_update,
        })
    }
}

pub async fn run(opts: &BuildOptions) -> Result<()> {
    info!(
        pack_dir = %opts.pack_dir.display(),
        manifest_out = %opts.manifest_out.display(),
        gtpack_out = ?opts.gtpack_out,
        dry_run = opts.dry_run,
        "building greentic pack"
    );

    if !opts.skip_update {
        // Keep pack.yaml in sync before building.
        crate::cli::update::update_pack(&opts.pack_dir, false)?;
    }

    // Resolve component references into pack.lock.json before building to ensure
    // manifests/extensions can rely on the lockfile contents.
    resolve::handle(
        ResolveArgs {
            input: opts.pack_dir.clone(),
            lock: Some(opts.lock_path.clone()),
        },
        &opts.runtime,
        false,
    )
    .await?;

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
    validate_components_extension(&config.extensions, opts.allow_oci_tags)?;

    let secret_requirements = aggregate_secret_requirements(
        &config.components,
        opts.secrets_req.as_deref(),
        opts.default_secret_scope.as_deref(),
    )?;

    if !opts.lock_path.exists() {
        anyhow::bail!(
            "pack.lock.json is required (run `greentic-pack resolve`); missing: {}",
            opts.lock_path.display()
        );
    }
    let mut pack_lock = read_pack_lock(&opts.lock_path).with_context(|| {
        format!(
            "failed to read pack lock {} (try `greentic-pack resolve`)",
            opts.lock_path.display()
        )
    })?;

    let mut build = assemble_manifest(&config, &opts.pack_dir, &secret_requirements)?;
    build.lock_components =
        collect_lock_component_artifacts(&mut pack_lock, &opts.runtime, opts.bundle, opts.dry_run)
            .await?;

    let materialized = materialize_flow_components(
        &opts.pack_dir,
        &build.manifest.flows,
        &pack_lock,
        &build.components,
        &build.lock_components,
        opts.require_component_manifests,
    )?;
    build.manifest.components.extend(materialized.components);
    build.component_manifest_files = materialized.manifest_files;
    build.manifest.components.sort_by(|a, b| a.id.cmp(&b.id));

    let component_manifest_files =
        collect_component_manifest_files(&build.components, &build.component_manifest_files);
    build.manifest.extensions =
        merge_component_manifest_extension(build.manifest.extensions, &component_manifest_files)?;
    build.manifest.extensions = merge_component_sources_extension(
        build.manifest.extensions,
        &pack_lock,
        opts.bundle,
        materialized.manifest_paths.as_ref(),
    )?;
    if !opts.dry_run {
        greentic_pack::pack_lock::write_pack_lock(&opts.lock_path, &pack_lock)?;
    }

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
        package_gtpack(gtpack_out, &manifest_bytes, &build, opts.bundle)?;
        info!(gtpack_out = %gtpack_out.display(), "gtpack archive ready");
        eprintln!("wrote {}", gtpack_out.display());
    }

    Ok(())
}

struct BuildProducts {
    manifest: PackManifest,
    components: Vec<ComponentBinary>,
    lock_components: Vec<LockComponentBinary>,
    component_manifest_files: Vec<ComponentManifestFile>,
    flow_files: Vec<FlowFile>,
    assets: Vec<AssetFile>,
}

#[derive(Clone)]
struct ComponentBinary {
    id: String,
    source: PathBuf,
    manifest_bytes: Vec<u8>,
    manifest_path: String,
    manifest_hash_sha256: String,
}

#[derive(Clone)]
struct LockComponentBinary {
    logical_path: String,
    source: PathBuf,
}

#[derive(Clone)]
struct ComponentManifestFile {
    component_id: String,
    manifest_path: String,
    manifest_bytes: Vec<u8>,
    manifest_hash_sha256: String,
}

struct AssetFile {
    logical_path: String,
    source: PathBuf,
}

#[derive(Clone)]
struct FlowFile {
    logical_path: String,
    bytes: Vec<u8>,
    media_type: &'static str,
}

fn assemble_manifest(
    config: &PackConfig,
    pack_root: &Path,
    secret_requirements: &[SecretRequirement],
) -> Result<BuildProducts> {
    let components = build_components(&config.components)?;
    let (flows, flow_files) = build_flows(&config.flows, pack_root)?;
    let dependencies = build_dependencies(&config.dependencies)?;
    let assets = collect_assets(&config.assets, pack_root)?;
    let component_manifests: Vec<_> = components.iter().map(|c| c.0.clone()).collect();
    let bootstrap = build_bootstrap(config, &flows, &component_manifests)?;
    let extensions = normalize_extensions(&config.extensions);

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
        extensions,
    };

    Ok(BuildProducts {
        manifest,
        components: components.into_iter().map(|(_, bin)| bin).collect(),
        lock_components: Vec::new(),
        component_manifest_files: Vec::new(),
        flow_files,
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
        let (manifest, binary) = resolve_component_artifacts(cfg)?;

        result.push((manifest, binary));
    }

    Ok(result)
}

fn resolve_component_artifacts(
    cfg: &ComponentConfig,
) -> Result<(ComponentManifest, ComponentBinary)> {
    let resolved_wasm = resolve_component_wasm_path(&cfg.wasm)?;

    let mut manifest = if let Some(from_disk) = load_component_manifest_from_disk(&resolved_wasm)? {
        if from_disk.id.to_string() != cfg.id {
            anyhow::bail!(
                "component manifest id {} does not match pack.yaml id {}",
                from_disk.id,
                cfg.id
            );
        }
        if from_disk.version.to_string() != cfg.version {
            anyhow::bail!(
                "component manifest version {} does not match pack.yaml version {}",
                from_disk.version,
                cfg.version
            );
        }
        from_disk
    } else {
        manifest_from_config(cfg)?
    };

    // Ensure operations are populated from pack.yaml when missing in the on-disk manifest.
    if manifest.operations.is_empty() && !cfg.operations.is_empty() {
        manifest.operations = cfg
            .operations
            .iter()
            .map(operation_from_config)
            .collect::<Result<Vec<_>>>()?;
    }

    let manifest_bytes =
        serde_cbor::to_vec(&manifest).context("encode component manifest to cbor")?;
    let mut sha = Sha256::new();
    sha.update(&manifest_bytes);
    let manifest_hash_sha256 = format!("sha256:{:x}", sha.finalize());
    let manifest_path = format!("components/{}.manifest.cbor", cfg.id);

    let binary = ComponentBinary {
        id: cfg.id.clone(),
        source: resolved_wasm,
        manifest_bytes,
        manifest_path,
        manifest_hash_sha256,
    };

    Ok((manifest, binary))
}

fn manifest_from_config(cfg: &ComponentConfig) -> Result<ComponentManifest> {
    Ok(ComponentManifest {
        id: ComponentId::new(cfg.id.clone())
            .with_context(|| format!("invalid component id {}", cfg.id))?,
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
    })
}

fn resolve_component_wasm_path(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if !path.exists() {
        anyhow::bail!("component path {} does not exist", path.display());
    }
    if !path.is_dir() {
        anyhow::bail!(
            "component path {} must be a file or directory",
            path.display()
        );
    }

    let mut component_candidates = Vec::new();
    let mut wasm_candidates = Vec::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current)
            .with_context(|| format!("failed to list components in {}", current.display()))?
        {
            let entry = entry?;
            let entry_type = entry.file_type()?;
            let entry_path = entry.path();
            if entry_type.is_dir() {
                stack.push(entry_path);
                continue;
            }
            if entry_type.is_file() && entry_path.extension() == Some(std::ffi::OsStr::new("wasm"))
            {
                let file_name = entry_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if file_name.ends_with(".component.wasm") {
                    component_candidates.push(entry_path);
                } else {
                    wasm_candidates.push(entry_path);
                }
            }
        }
    }

    let choose = |mut list: Vec<PathBuf>| -> Result<PathBuf> {
        list.sort();
        if list.len() == 1 {
            Ok(list.remove(0))
        } else {
            let options = list
                .iter()
                .map(|p| p.strip_prefix(path).unwrap_or(p).display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "multiple wasm artifacts found under {}: {} (pick a single *.component.wasm or *.wasm)",
                path.display(),
                options
            );
        }
    };

    if !component_candidates.is_empty() {
        return choose(component_candidates);
    }
    if !wasm_candidates.is_empty() {
        return choose(wasm_candidates);
    }

    anyhow::bail!(
        "no wasm artifact found under {}; expected *.component.wasm or *.wasm",
        path.display()
    );
}

fn load_component_manifest_from_disk(path: &Path) -> Result<Option<ComponentManifest>> {
    let manifest_dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow!("component path {} has no parent directory", path.display()))?
    };
    let manifest_path = manifest_dir.join("component.json");
    if !manifest_path.exists() {
        return Ok(None);
    }

    let manifest: ComponentManifest = serde_json::from_slice(
        &fs::read(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?,
    )
    .with_context(|| format!("{} is not a valid component.json", manifest_path.display()))?;

    Ok(Some(manifest))
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

fn build_flows(
    configs: &[FlowConfig],
    pack_root: &Path,
) -> Result<(Vec<PackFlowEntry>, Vec<FlowFile>)> {
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    let mut flow_files = Vec::new();

    for cfg in configs {
        info!(id = %cfg.id, path = %cfg.file.display(), "compiling flow");
        let yaml_bytes = fs::read(&cfg.file)
            .with_context(|| format!("failed to read flow {}", cfg.file.display()))?;
        let mut flow: Flow = compile_ygtc_file(&cfg.file)
            .with_context(|| format!("failed to compile {}", cfg.file.display()))?;
        populate_component_exec_operations(&mut flow, &cfg.file).with_context(|| {
            format!(
                "failed to resolve component.exec operations in {}",
                cfg.file.display()
            )
        })?;
        normalize_legacy_component_exec_ids(&mut flow)?;
        let summary = load_flow_resolve_summary(pack_root, cfg, &flow)?;
        apply_summary_component_ids(&mut flow, &summary).with_context(|| {
            format!("failed to resolve component ids in {}", cfg.file.display())
        })?;

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

        let flow_id = flow_entry.id.to_string();
        flow_files.push(FlowFile {
            logical_path: format!("flows/{flow_id}/flow.ygtc"),
            bytes: yaml_bytes,
            media_type: "application/yaml",
        });
        flow_files.push(FlowFile {
            logical_path: format!("flows/{flow_id}/flow.json"),
            bytes: serde_json::to_vec(&flow_entry.flow).context("encode flow json")?,
            media_type: "application/json",
        });
        entries.push(flow_entry);
    }

    Ok((entries, flow_files))
}

fn apply_summary_component_ids(flow: &mut Flow, summary: &FlowResolveSummaryV1) -> Result<()> {
    for (node_id, node) in flow.nodes.iter_mut() {
        let resolved = summary.nodes.get(node_id.as_str()).ok_or_else(|| {
            anyhow!(
                "flow resolve summary missing node {} (expected component id for node)",
                node_id
            )
        })?;
        let summary_id = resolved.component_id.as_str();
        if node.component.id.as_str().is_empty() || node.component.id.as_str() == "component.exec" {
            node.component.id = resolved.component_id.clone();
            continue;
        }
        if node.component.id.as_str() != summary_id {
            anyhow::bail!(
                "node {} component id {} does not match resolve summary {}",
                node_id,
                node.component.id.as_str(),
                summary_id
            );
        }
    }
    Ok(())
}

fn populate_component_exec_operations(flow: &mut Flow, path: &Path) -> Result<()> {
    let needs_op = flow.nodes.values().any(|node| {
        node.component.id.as_str() == "component.exec" && node.component.operation.is_none()
    });
    if !needs_op {
        return Ok(());
    }

    let flow_doc = load_ygtc_from_path(path)?;
    let mut operations = BTreeMap::new();

    for (node_id, node_doc) in flow_doc.nodes {
        let value = serde_json::to_value(&node_doc)
            .with_context(|| format!("failed to normalize component.exec node {}", node_id))?;
        let normalized = normalize_node_map(value)?;
        if !normalized.operation.trim().is_empty() {
            operations.insert(node_id, normalized.operation);
        }
    }

    for (node_id, node) in flow.nodes.iter_mut() {
        if node.component.id.as_str() != "component.exec" || node.component.operation.is_some() {
            continue;
        }
        if let Some(op) = operations.get(node_id.as_str()) {
            node.component.operation = Some(op.clone());
        }
    }

    Ok(())
}

fn normalize_legacy_component_exec_ids(flow: &mut Flow) -> Result<()> {
    for (node_id, node) in flow.nodes.iter_mut() {
        if node.component.id.as_str() != "component.exec" {
            continue;
        }
        let Some(op) = node.component.operation.as_deref() else {
            continue;
        };
        if !op.contains('.') && !op.contains(':') {
            continue;
        }
        node.component.id = ComponentId::new(op).with_context(|| {
            format!("invalid component id {} resolved for node {}", op, node_id)
        })?;
        node.component.operation = None;
    }
    Ok(())
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

fn merge_component_manifest_extension(
    extensions: Option<BTreeMap<String, ExtensionRef>>,
    manifest_files: &[ComponentManifestFile],
) -> Result<Option<BTreeMap<String, ExtensionRef>>> {
    if manifest_files.is_empty() {
        return Ok(extensions);
    }

    let entries: Vec<_> = manifest_files
        .iter()
        .map(|entry| ComponentManifestIndexEntryV1 {
            component_id: entry.component_id.clone(),
            manifest_file: entry.manifest_path.clone(),
            encoding: ManifestEncoding::Cbor,
            content_hash: Some(entry.manifest_hash_sha256.clone()),
        })
        .collect();

    let index = ComponentManifestIndexV1::new(entries);
    let value = index
        .to_extension_value()
        .context("serialize component manifest index extension")?;

    let ext = ExtensionRef {
        kind: EXT_COMPONENT_MANIFEST_INDEX_V1.to_string(),
        version: "v1".to_string(),
        digest: None,
        location: None,
        inline: Some(ExtensionInline::Other(value)),
    };

    let mut map = extensions.unwrap_or_default();
    map.insert(EXT_COMPONENT_MANIFEST_INDEX_V1.to_string(), ext);
    if map.is_empty() {
        Ok(None)
    } else {
        Ok(Some(map))
    }
}

fn merge_component_sources_extension(
    extensions: Option<BTreeMap<String, ExtensionRef>>,
    lock: &greentic_pack::pack_lock::PackLockV1,
    _bundle: BundleMode,
    manifest_paths: Option<&std::collections::BTreeMap<String, String>>,
) -> Result<Option<BTreeMap<String, ExtensionRef>>> {
    let mut entries = Vec::new();
    for comp in &lock.components {
        if comp.r#ref.starts_with("file://") {
            continue;
        }
        let source = match ComponentSourceRef::from_str(&comp.r#ref) {
            Ok(parsed) => parsed,
            Err(_) => {
                eprintln!(
                    "warning: skipping pack.lock entry `{}` with unsupported ref {}",
                    comp.name, comp.r#ref
                );
                continue;
            }
        };
        let manifest_path = manifest_paths.and_then(|paths| {
            comp.component_id
                .as_ref()
                .map(|id| id.as_str())
                .and_then(|key| paths.get(key))
                .or_else(|| paths.get(&comp.name))
                .cloned()
        });
        let artifact = if comp.bundled {
            let wasm_path = comp.bundled_path.clone().ok_or_else(|| {
                anyhow!(
                    "pack.lock entry {} marked bundled but missing bundled_path",
                    comp.name
                )
            })?;
            ArtifactLocationV1::Inline {
                wasm_path,
                manifest_path,
            }
        } else {
            ArtifactLocationV1::Remote
        };
        entries.push(ComponentSourceEntryV1 {
            name: comp.name.clone(),
            component_id: comp.component_id.clone(),
            source,
            resolved: ResolvedComponentV1 {
                digest: comp.digest.clone(),
                signature: None,
                signed_by: None,
            },
            artifact,
            licensing_hint: None,
            metering_hint: None,
        });
    }

    if entries.is_empty() {
        return Ok(extensions);
    }

    let payload = ComponentSourcesV1::new(entries)
        .to_extension_value()
        .context("serialize component_sources extension")?;

    let ext = ExtensionRef {
        kind: EXT_COMPONENT_SOURCES_V1.to_string(),
        version: "v1".to_string(),
        digest: None,
        location: None,
        inline: Some(ExtensionInline::Other(payload)),
    };

    let mut map = extensions.unwrap_or_default();
    map.insert(EXT_COMPONENT_SOURCES_V1.to_string(), ext);
    if map.is_empty() {
        Ok(None)
    } else {
        Ok(Some(map))
    }
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

fn package_gtpack(
    out_path: &Path,
    manifest_bytes: &[u8],
    build: &BuildProducts,
    bundle: BundleMode,
) -> Result<()> {
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

    let mut sbom_entries = Vec::new();
    record_sbom_entry(
        &mut sbom_entries,
        "manifest.cbor",
        manifest_bytes,
        "application/cbor",
    );
    write_zip_entry(&mut writer, "manifest.cbor", manifest_bytes, options)?;

    let mut flow_files = build.flow_files.clone();
    flow_files.sort_by(|a, b| a.logical_path.cmp(&b.logical_path));
    for flow_file in flow_files {
        record_sbom_entry(
            &mut sbom_entries,
            &flow_file.logical_path,
            &flow_file.bytes,
            flow_file.media_type,
        );
        write_zip_entry(
            &mut writer,
            &flow_file.logical_path,
            &flow_file.bytes,
            options,
        )?;
    }

    let mut lock_components = build.lock_components.clone();
    lock_components.sort_by(|a, b| a.logical_path.cmp(&b.logical_path));
    for comp in lock_components {
        let bytes = fs::read(&comp.source).with_context(|| {
            format!("failed to read cached component {}", comp.source.display())
        })?;
        record_sbom_entry(
            &mut sbom_entries,
            &comp.logical_path,
            &bytes,
            "application/wasm",
        );
        write_zip_entry(&mut writer, &comp.logical_path, &bytes, options)?;
    }

    let mut lock_manifests = build.component_manifest_files.clone();
    lock_manifests.sort_by(|a, b| a.manifest_path.cmp(&b.manifest_path));
    for manifest in lock_manifests {
        record_sbom_entry(
            &mut sbom_entries,
            &manifest.manifest_path,
            &manifest.manifest_bytes,
            "application/cbor",
        );
        write_zip_entry(
            &mut writer,
            &manifest.manifest_path,
            &manifest.manifest_bytes,
            options,
        )?;
    }

    if bundle != BundleMode::None {
        let mut components = build.components.clone();
        components.sort_by(|a, b| a.id.cmp(&b.id));
        for comp in components {
            let logical_wasm = format!("components/{}.wasm", comp.id);
            let wasm_bytes = fs::read(&comp.source)
                .with_context(|| format!("failed to read component {}", comp.source.display()))?;
            record_sbom_entry(
                &mut sbom_entries,
                &logical_wasm,
                &wasm_bytes,
                "application/wasm",
            );
            write_zip_entry(&mut writer, &logical_wasm, &wasm_bytes, options)?;

            record_sbom_entry(
                &mut sbom_entries,
                &comp.manifest_path,
                &comp.manifest_bytes,
                "application/cbor",
            );
            write_zip_entry(
                &mut writer,
                &comp.manifest_path,
                &comp.manifest_bytes,
                options,
            )?;
        }
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
        record_sbom_entry(
            &mut sbom_entries,
            &logical,
            &bytes,
            "application/octet-stream",
        );
        write_zip_entry(&mut writer, &logical, &bytes, options)?;
    }

    sbom_entries.sort_by(|a, b| a.path.cmp(&b.path));
    let sbom_doc = SbomDocument {
        format: SBOM_FORMAT.to_string(),
        files: sbom_entries,
    };
    let sbom_bytes = serde_cbor::to_vec(&sbom_doc).context("failed to encode sbom.cbor")?;
    write_zip_entry(&mut writer, "sbom.cbor", &sbom_bytes, options)?;

    writer
        .finish()
        .context("failed to finalise gtpack archive")?;
    Ok(())
}

async fn collect_lock_component_artifacts(
    lock: &mut greentic_pack::pack_lock::PackLockV1,
    runtime: &RuntimeContext,
    bundle: BundleMode,
    allow_missing: bool,
) -> Result<Vec<LockComponentBinary>> {
    let dist = DistClient::new(DistOptions {
        cache_dir: runtime.cache_dir(),
        allow_tags: true,
        offline: runtime.network_policy() == NetworkPolicy::Offline,
        allow_insecure_local_http: false,
    });

    let mut artifacts = Vec::new();
    let mut seen_paths = BTreeSet::new();
    for comp in &mut lock.components {
        if comp.r#ref.starts_with("file://") {
            comp.bundled = false;
            comp.bundled_path = None;
            comp.wasm_sha256 = None;
            comp.resolved_digest = None;
            continue;
        }
        let parsed = ComponentSourceRef::from_str(&comp.r#ref).ok();
        let is_tag = parsed.as_ref().map(|r| r.is_tag()).unwrap_or(false);
        let should_bundle = is_tag || bundle == BundleMode::Cache;
        if !should_bundle {
            comp.bundled = false;
            comp.bundled_path = None;
            comp.wasm_sha256 = None;
            comp.resolved_digest = None;
            continue;
        }

        let resolved = if is_tag {
            let item = if runtime.network_policy() == NetworkPolicy::Offline {
                dist.ensure_cached(&comp.digest).await.map_err(|err| {
                    anyhow!(
                        "tag ref {} must be bundled but cache is missing ({})",
                        comp.r#ref,
                        err
                    )
                })?
            } else {
                dist.resolve_ref(&comp.r#ref)
                    .await
                    .map_err(|err| anyhow!("failed to resolve {}: {}", comp.r#ref, err))?
            };
            let cache_path = item.cache_path.clone().ok_or_else(|| {
                anyhow!("tag ref {} resolved but cache path is missing", comp.r#ref)
            })?;
            ResolvedLockItem { item, cache_path }
        } else {
            let mut resolved = dist
                .ensure_cached(&comp.digest)
                .await
                .ok()
                .and_then(|item| item.cache_path.clone().map(|path| (item, path)));
            if resolved.is_none()
                && runtime.network_policy() != NetworkPolicy::Offline
                && !allow_missing
                && comp.r#ref.starts_with("oci://")
            {
                let item = dist
                    .resolve_ref(&comp.r#ref)
                    .await
                    .map_err(|err| anyhow!("failed to resolve {}: {}", comp.r#ref, err))?;
                if let Some(path) = item.cache_path.clone() {
                    resolved = Some((item, path));
                }
            }
            let Some((item, path)) = resolved else {
                eprintln!(
                    "warning: component {} is not cached; skipping embed",
                    comp.name
                );
                comp.bundled = false;
                comp.bundled_path = None;
                comp.wasm_sha256 = None;
                comp.resolved_digest = None;
                continue;
            };
            ResolvedLockItem {
                item,
                cache_path: path,
            }
        };

        let cache_path = resolved.cache_path;
        let bytes = fs::read(&cache_path)
            .with_context(|| format!("failed to read cached component {}", cache_path.display()))?;
        let wasm_sha256 = format!("{:x}", Sha256::digest(&bytes));
        let logical_path = if is_tag {
            format!("blobs/sha256/{}.wasm", wasm_sha256)
        } else {
            format!("components/{}.wasm", comp.name)
        };

        comp.bundled = true;
        comp.bundled_path = Some(logical_path.clone());
        comp.wasm_sha256 = Some(wasm_sha256.clone());
        if is_tag {
            comp.digest = format!("sha256:{wasm_sha256}");
            comp.resolved_digest = Some(resolved.item.digest.clone());
        } else {
            comp.resolved_digest = None;
        }

        if seen_paths.insert(logical_path.clone()) {
            artifacts.push(LockComponentBinary {
                logical_path: logical_path.clone(),
                source: cache_path.clone(),
            });
        }
        if let Some(component_id) = comp.component_id.as_ref()
            && comp.bundled
        {
            let alias_path = format!("components/{}.wasm", component_id.as_str());
            if alias_path != logical_path && seen_paths.insert(alias_path.clone()) {
                artifacts.push(LockComponentBinary {
                    logical_path: alias_path,
                    source: cache_path.clone(),
                });
            }
        }
    }

    Ok(artifacts)
}

struct ResolvedLockItem {
    item: greentic_distributor_client::ResolvedArtifact,
    cache_path: PathBuf,
}

struct MaterializedComponents {
    components: Vec<ComponentManifest>,
    manifest_files: Vec<ComponentManifestFile>,
    manifest_paths: Option<BTreeMap<String, String>>,
}

fn record_sbom_entry(entries: &mut Vec<SbomEntry>, path: &str, bytes: &[u8], media_type: &str) {
    entries.push(SbomEntry {
        path: path.to_string(),
        size: bytes.len() as u64,
        hash_blake3: blake3::hash(bytes).to_hex().to_string(),
        media_type: media_type.to_string(),
    });
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

fn collect_component_manifest_files(
    components: &[ComponentBinary],
    extra: &[ComponentManifestFile],
) -> Vec<ComponentManifestFile> {
    let mut files: Vec<ComponentManifestFile> = components
        .iter()
        .map(|binary| ComponentManifestFile {
            component_id: binary.id.clone(),
            manifest_path: binary.manifest_path.clone(),
            manifest_bytes: binary.manifest_bytes.clone(),
            manifest_hash_sha256: binary.manifest_hash_sha256.clone(),
        })
        .collect();
    files.extend(extra.iter().cloned());
    files.sort_by(|a, b| a.component_id.cmp(&b.component_id));
    files.dedup_by(|a, b| a.component_id == b.component_id);
    files
}

fn materialize_flow_components(
    pack_dir: &Path,
    flows: &[PackFlowEntry],
    pack_lock: &greentic_pack::pack_lock::PackLockV1,
    components: &[ComponentBinary],
    lock_components: &[LockComponentBinary],
    require_component_manifests: bool,
) -> Result<MaterializedComponents> {
    let referenced = collect_flow_component_ids(flows);
    if referenced.is_empty() {
        return Ok(MaterializedComponents {
            components: Vec::new(),
            manifest_files: Vec::new(),
            manifest_paths: None,
        });
    }

    let mut existing = BTreeSet::new();
    for component in components {
        existing.insert(component.id.clone());
    }

    let mut lock_by_id = BTreeMap::new();
    let mut lock_by_name = BTreeMap::new();
    for entry in &pack_lock.components {
        if let Some(component_id) = entry.component_id.as_ref() {
            lock_by_id.insert(component_id.as_str().to_string(), entry);
        }
        lock_by_name.insert(entry.name.clone(), entry);
    }

    let mut bundle_sources = BTreeMap::new();
    for entry in lock_components {
        bundle_sources.insert(entry.logical_path.clone(), entry.source.clone());
    }

    let mut materialized_components = Vec::new();
    let mut manifest_files = Vec::new();
    let mut manifest_paths: BTreeMap<String, String> = BTreeMap::new();

    for component_id in referenced {
        if existing.contains(&component_id) {
            continue;
        }

        let lock_entry = lock_by_id
            .get(&component_id)
            .copied()
            .or_else(|| lock_by_name.get(&component_id).copied());
        let Some(lock_entry) = lock_entry else {
            handle_missing_component_manifest(&component_id, None, require_component_manifests)?;
            continue;
        };
        if !lock_entry.bundled {
            if require_component_manifests {
                anyhow::bail!(
                    "component {} is not bundled; cannot materialize manifest without local artifacts",
                    lock_entry.name
                );
            }
            eprintln!(
                "warning: component {} is not bundled; pack will emit PACK_COMPONENT_NOT_EXPLICIT",
                lock_entry.name
            );
            continue;
        }

        let bundled_source = lock_entry
            .bundled_path
            .as_deref()
            .and_then(|path| bundle_sources.get(path));
        let manifest = load_component_manifest_for_lock(pack_dir, lock_entry, bundled_source)?;

        let Some(manifest) = manifest else {
            handle_missing_component_manifest(
                &component_id,
                Some(&lock_entry.name),
                require_component_manifests,
            )?;
            continue;
        };

        if let Some(lock_id) = lock_entry.component_id.as_ref()
            && manifest.id.as_str() != lock_id.as_str()
        {
            anyhow::bail!(
                "component manifest id {} does not match pack.lock component_id {}",
                manifest.id.as_str(),
                lock_id.as_str()
            );
        }

        let manifest_file = component_manifest_file_from_manifest(&manifest)?;
        manifest_paths.insert(
            manifest.id.as_str().to_string(),
            manifest_file.manifest_path.clone(),
        );
        manifest_paths.insert(lock_entry.name.clone(), manifest_file.manifest_path.clone());

        materialized_components.push(manifest);
        manifest_files.push(manifest_file);
    }

    let manifest_paths = if manifest_paths.is_empty() {
        None
    } else {
        Some(manifest_paths)
    };

    Ok(MaterializedComponents {
        components: materialized_components,
        manifest_files,
        manifest_paths,
    })
}

fn collect_flow_component_ids(flows: &[PackFlowEntry]) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for flow in flows {
        for node in flow.flow.nodes.values() {
            if node.component.pack_alias.is_some() {
                continue;
            }
            let id = node.component.id.as_str();
            if !id.is_empty() {
                ids.insert(id.to_string());
            }
        }
    }
    ids
}

fn load_component_manifest_for_lock(
    pack_dir: &Path,
    lock_entry: &greentic_pack::pack_lock::LockedComponent,
    bundled_source: Option<&PathBuf>,
) -> Result<Option<ComponentManifest>> {
    let mut search_paths = Vec::new();
    if let Some(component_id) = lock_entry.component_id.as_ref() {
        let id = component_id.as_str();
        search_paths.extend(component_manifest_search_paths(pack_dir, id));
    }
    search_paths.extend(component_manifest_search_paths(pack_dir, &lock_entry.name));
    if let Some(source) = bundled_source {
        if let Some(parent) = source.parent() {
            search_paths.push(parent.join("component.manifest.cbor"));
            search_paths.push(parent.join("component.manifest.json"));
        }
    }

    for path in search_paths {
        if path.exists() {
            return Ok(Some(load_component_manifest_from_file(&path)?));
        }
    }

    Ok(None)
}

fn component_manifest_search_paths(pack_dir: &Path, name: &str) -> Vec<PathBuf> {
    vec![
        pack_dir
            .join("components")
            .join(format!("{name}.manifest.cbor")),
        pack_dir
            .join("components")
            .join(format!("{name}.manifest.json")),
        pack_dir
            .join("components")
            .join(name)
            .join("component.manifest.cbor"),
        pack_dir
            .join("components")
            .join(name)
            .join("component.manifest.json"),
    ]
}

fn load_component_manifest_from_file(path: &Path) -> Result<ComponentManifest> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cbor"))
    {
        let manifest = serde_cbor::from_slice(&bytes)
            .with_context(|| format!("{} is not valid CBOR", path.display()))?;
        return Ok(manifest);
    }

    let manifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    Ok(manifest)
}

fn component_manifest_file_from_manifest(
    manifest: &ComponentManifest,
) -> Result<ComponentManifestFile> {
    let manifest_bytes =
        serde_cbor::to_vec(manifest).context("encode component manifest to cbor")?;
    let mut sha = Sha256::new();
    sha.update(&manifest_bytes);
    let manifest_hash_sha256 = format!("sha256:{:x}", sha.finalize());
    let manifest_path = format!("components/{}.manifest.cbor", manifest.id.as_str());

    Ok(ComponentManifestFile {
        component_id: manifest.id.as_str().to_string(),
        manifest_path,
        manifest_bytes,
        manifest_hash_sha256,
    })
}

fn handle_missing_component_manifest(
    component_id: &str,
    component_name: Option<&str>,
    require_component_manifests: bool,
) -> Result<()> {
    let label = component_name.unwrap_or(component_id);
    if require_component_manifests {
        anyhow::bail!(
            "component manifest metadata missing for {} (supply component.manifest.json or use --require-component-manifests=false)",
            label
        );
    }
    eprintln!(
        "warning: component manifest metadata missing for {}; pack will emit PACK_COMPONENT_NOT_EXPLICIT",
        label
    );
    Ok(())
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
    use greentic_pack::pack_lock::{LockedComponent, PackLockV1};
    use greentic_types::flow::FlowKind;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::fs::File;
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
                manifest_bytes: serde_cbor::to_vec(&component).expect("component cbor"),
                manifest_path: format!("components/{}.manifest.cbor", component.id),
                manifest_hash_sha256: {
                    let mut sha = Sha256::new();
                    sha.update(serde_cbor::to_vec(&component).expect("component cbor"));
                    format!("sha256:{:x}", sha.finalize())
                },
            }],
            lock_components: Vec::new(),
            flow_files: Vec::new(),
            assets: Vec::new(),
        };

        let out = temp.path().join("demo.gtpack");
        package_gtpack(&out, &manifest_bytes, &build, BundleMode::Cache).expect("package gtpack");

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
    fn component_sources_extension_respects_bundle() {
        let lock_tag = PackLockV1::new(vec![LockedComponent {
            name: "demo.tagged".into(),
            r#ref: "oci://ghcr.io/demo/component:1.0.0".into(),
            digest: "sha256:deadbeef".into(),
            component_id: None,
            bundled: true,
            bundled_path: Some("blobs/sha256/deadbeef.wasm".into()),
            wasm_sha256: Some("deadbeef".repeat(8)),
            resolved_digest: Some("sha256:deadbeef".into()),
        }]);

        let ext_none = merge_component_sources_extension(None, &lock_tag, BundleMode::None, None)
            .expect("ext");
        let value = match ext_none
            .unwrap()
            .get(EXT_COMPONENT_SOURCES_V1)
            .and_then(|e| e.inline.as_ref())
        {
            Some(ExtensionInline::Other(v)) => v.clone(),
            _ => panic!("missing inline"),
        };
        let decoded = ComponentSourcesV1::from_extension_value(&value).expect("decode");
        assert!(matches!(
            decoded.components[0].artifact,
            ArtifactLocationV1::Inline { .. }
        ));

        let lock_digest = PackLockV1::new(vec![LockedComponent {
            name: "demo.component".into(),
            r#ref: "oci://ghcr.io/demo/component@sha256:deadbeef".into(),
            digest: "sha256:deadbeef".into(),
            component_id: None,
            bundled: false,
            bundled_path: None,
            wasm_sha256: None,
            resolved_digest: None,
        }]);

        let ext_none =
            merge_component_sources_extension(None, &lock_digest, BundleMode::None, None)
                .expect("ext");
        let value = match ext_none
            .unwrap()
            .get(EXT_COMPONENT_SOURCES_V1)
            .and_then(|e| e.inline.as_ref())
        {
            Some(ExtensionInline::Other(v)) => v.clone(),
            _ => panic!("missing inline"),
        };
        let decoded = ComponentSourcesV1::from_extension_value(&value).expect("decode");
        assert!(matches!(
            decoded.components[0].artifact,
            ArtifactLocationV1::Remote
        ));

        let lock_digest_bundled = PackLockV1::new(vec![LockedComponent {
            name: "demo.component".into(),
            r#ref: "oci://ghcr.io/demo/component@sha256:deadbeef".into(),
            digest: "sha256:deadbeef".into(),
            component_id: None,
            bundled: true,
            bundled_path: Some("components/demo.component.wasm".into()),
            wasm_sha256: Some("deadbeef".repeat(8)),
            resolved_digest: None,
        }]);

        let ext_cache =
            merge_component_sources_extension(None, &lock_digest_bundled, BundleMode::Cache, None)
                .expect("ext");
        let value = match ext_cache
            .unwrap()
            .get(EXT_COMPONENT_SOURCES_V1)
            .and_then(|e| e.inline.as_ref())
        {
            Some(ExtensionInline::Other(v)) => v.clone(),
            _ => panic!("missing inline"),
        };
        let decoded = ComponentSourcesV1::from_extension_value(&value).expect("decode");
        assert!(matches!(
            decoded.components[0].artifact,
            ArtifactLocationV1::Inline { .. }
        ));
    }

    #[test]
    fn component_sources_extension_skips_file_refs() {
        let lock = PackLockV1::new(vec![LockedComponent {
            name: "local.component".into(),
            r#ref: "file:///tmp/component.wasm".into(),
            digest: "sha256:deadbeef".into(),
            component_id: None,
            bundled: false,
            bundled_path: None,
            wasm_sha256: None,
            resolved_digest: None,
        }]);

        let ext_none =
            merge_component_sources_extension(None, &lock, BundleMode::Cache, None).expect("ext");
        assert!(ext_none.is_none(), "file refs should be omitted");

        let lock = PackLockV1::new(vec![
            LockedComponent {
                name: "local.component".into(),
                r#ref: "file:///tmp/component.wasm".into(),
                digest: "sha256:deadbeef".into(),
                component_id: None,
                bundled: false,
                bundled_path: None,
                wasm_sha256: None,
                resolved_digest: None,
            },
            LockedComponent {
                name: "remote.component".into(),
                r#ref: "oci://ghcr.io/demo/component:2.0.0".into(),
                digest: "sha256:cafebabe".into(),
                component_id: None,
                bundled: false,
                bundled_path: None,
                wasm_sha256: None,
                resolved_digest: None,
            },
        ]);

        let ext_some =
            merge_component_sources_extension(None, &lock, BundleMode::None, None).expect("ext");
        let value = match ext_some
            .unwrap()
            .get(EXT_COMPONENT_SOURCES_V1)
            .and_then(|e| e.inline.as_ref())
        {
            Some(ExtensionInline::Other(v)) => v.clone(),
            _ => panic!("missing inline"),
        };
        let decoded = ComponentSourcesV1::from_extension_value(&value).expect("decode");
        assert_eq!(decoded.components.len(), 1);
        assert!(matches!(
            decoded.components[0].source,
            ComponentSourceRef::Oci(_)
        ));
    }

    #[test]
    fn build_embeds_lock_components_from_cache() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let temp = tempdir().expect("temp dir");
            let pack_dir = temp.path().join("pack");
            fs::create_dir_all(pack_dir.join("flows")).expect("flows dir");
            fs::create_dir_all(pack_dir.join("components")).expect("components dir");

            let wasm_path = pack_dir.join("components/dummy.wasm");
            fs::write(&wasm_path, [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])
                .expect("write wasm");

            let flow_path = pack_dir.join("flows/main.ygtc");
            fs::write(
                &flow_path,
                r#"id: main
type: messaging
start: call
nodes:
  call:
    handle_message:
      text: "hi"
    routing: out
"#,
            )
            .expect("write flow");

            let cache_dir = temp.path().join("cache");
            let cached_bytes = b"cached-component";
            let digest = format!("sha256:{:x}", Sha256::digest(cached_bytes));
            let cache_path = cache_dir
                .join(digest.trim_start_matches("sha256:"))
                .join("component.wasm");
            fs::create_dir_all(cache_path.parent().expect("cache parent")).expect("cache dir");
            fs::write(&cache_path, cached_bytes).expect("write cached");

            let summary = serde_json::json!({
                "schema_version": 1,
                "flow": "main.ygtc",
                "nodes": {
                    "call": {
                        "component_id": "dummy.component",
                        "source": {
                            "kind": "oci",
                            "ref": format!("oci://ghcr.io/demo/component@{digest}")
                        },
                        "digest": digest
                    }
                }
            });
            fs::write(
                flow_path.with_extension("ygtc.resolve.summary.json"),
                serde_json::to_vec_pretty(&summary).expect("summary json"),
            )
            .expect("write summary");

            let pack_yaml = r#"pack_id: demo.lock-bundle
version: 0.1.0
kind: application
publisher: Test
components:
  - id: dummy.component
    version: "0.1.0"
    world: "greentic:component/component@0.5.0"
    supports: ["messaging"]
    profiles:
      default: "stateless"
      supported: ["stateless"]
    capabilities:
      wasi: {}
      host: {}
    operations:
      - name: "handle_message"
        input_schema: {}
        output_schema: {}
    wasm: "components/dummy.wasm"
flows:
  - id: main
    file: flows/main.ygtc
    tags: [default]
    entrypoints: [main]
"#;
            fs::write(pack_dir.join("pack.yaml"), pack_yaml).expect("pack.yaml");

            let runtime = crate::runtime::resolve_runtime(
                Some(pack_dir.as_path()),
                Some(cache_dir.as_path()),
                true,
                None,
            )
            .expect("runtime");

            let opts = BuildOptions {
                pack_dir: pack_dir.clone(),
                component_out: None,
                manifest_out: pack_dir.join("dist/manifest.cbor"),
                sbom_out: None,
                gtpack_out: Some(pack_dir.join("dist/pack.gtpack")),
                lock_path: pack_dir.join("pack.lock.json"),
                bundle: BundleMode::Cache,
                dry_run: false,
                secrets_req: None,
                default_secret_scope: None,
                allow_oci_tags: false,
                require_component_manifests: false,
                runtime,
                skip_update: false,
            };

            run(&opts).await.expect("build");

            let gtpack_path = opts.gtpack_out.expect("gtpack path");
            let mut archive = ZipArchive::new(File::open(&gtpack_path).expect("open gtpack"))
                .expect("read gtpack");
            assert!(
                archive.by_name("components/main___call.wasm").is_ok(),
                "missing lock component artifact in gtpack"
            );
        });
    }

    #[test]
    #[ignore = "requires network access to fetch OCI component"]
    fn build_fetches_and_embeds_lock_components_online() {
        if std::env::var("GREENTIC_PACK_ONLINE").is_err() {
            return;
        }
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let temp = tempdir().expect("temp dir");
            let pack_dir = temp.path().join("pack");
            fs::create_dir_all(pack_dir.join("flows")).expect("flows dir");
            fs::create_dir_all(pack_dir.join("components")).expect("components dir");

            let wasm_path = pack_dir.join("components/dummy.wasm");
            fs::write(&wasm_path, [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])
                .expect("write wasm");

            let flow_path = pack_dir.join("flows/main.ygtc");
            fs::write(
                &flow_path,
                r#"id: main
type: messaging
start: call
nodes:
  call:
    handle_message:
      text: "hi"
    routing: out
"#,
            )
            .expect("write flow");

            let digest =
                "sha256:0904bee6ecd737506265e3f38f3e4fe6b185c20fd1b0e7c06ce03cdeedc00340";
            let summary = serde_json::json!({
                "schema_version": 1,
                "flow": "main.ygtc",
                "nodes": {
                    "call": {
                        "component_id": "dummy.component",
                        "source": {
                            "kind": "oci",
                            "ref": format!("oci://ghcr.io/greentic-ai/components/templates@{digest}")
                        },
                        "digest": digest
                    }
                }
            });
            fs::write(
                flow_path.with_extension("ygtc.resolve.summary.json"),
                serde_json::to_vec_pretty(&summary).expect("summary json"),
            )
            .expect("write summary");

            let pack_yaml = r#"pack_id: demo.lock-online
version: 0.1.0
kind: application
publisher: Test
components:
  - id: dummy.component
    version: "0.1.0"
    world: "greentic:component/component@0.5.0"
    supports: ["messaging"]
    profiles:
      default: "stateless"
      supported: ["stateless"]
    capabilities:
      wasi: {}
      host: {}
    operations:
      - name: "handle_message"
        input_schema: {}
        output_schema: {}
    wasm: "components/dummy.wasm"
flows:
  - id: main
    file: flows/main.ygtc
    tags: [default]
    entrypoints: [main]
"#;
            fs::write(pack_dir.join("pack.yaml"), pack_yaml).expect("pack.yaml");

            let cache_dir = temp.path().join("cache");
            let runtime = crate::runtime::resolve_runtime(
                Some(pack_dir.as_path()),
                Some(cache_dir.as_path()),
                false,
                None,
            )
            .expect("runtime");

            let opts = BuildOptions {
                pack_dir: pack_dir.clone(),
                component_out: None,
                manifest_out: pack_dir.join("dist/manifest.cbor"),
                sbom_out: None,
                gtpack_out: Some(pack_dir.join("dist/pack.gtpack")),
                lock_path: pack_dir.join("pack.lock.json"),
                bundle: BundleMode::Cache,
                dry_run: false,
                secrets_req: None,
                default_secret_scope: None,
                allow_oci_tags: false,
                require_component_manifests: false,
                runtime,
                skip_update: false,
            };

            run(&opts).await.expect("build");

            let gtpack_path = opts.gtpack_out.expect("gtpack path");
            let mut archive =
                ZipArchive::new(File::open(&gtpack_path).expect("open gtpack"))
                    .expect("read gtpack");
            assert!(
                archive.by_name("components/main___call.wasm").is_ok(),
                "missing lock component artifact in gtpack"
            );
        });
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
