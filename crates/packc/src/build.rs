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
use greentic_types::cbor::canonical;
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
use greentic_types::pack_manifest::{ExtensionInline as PackManifestExtensionInline, ExtensionRef};
use greentic_types::{
    BootstrapSpec, ComponentCapability, ComponentConfigurators, ComponentId, ComponentManifest,
    ComponentOperation, ExtensionInline, Flow, FlowId, PackDependency, PackFlowEntry, PackId,
    PackKind, PackManifest, PackSignatures, SecretRequirement, SecretScope, SemverReq,
    encode_pack_manifest,
};
use semver::Version;
use serde::Serialize;
use serde_cbor;
use serde_json::json;
use serde_yaml_bw::Value as YamlValue;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tracing::{info, warn};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const SBOM_FORMAT: &str = "greentic-sbom-v1";
const EXT_BUILD_MODE_ID: &str = "greentic.pack-mode.v1";

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
    pub no_extra_dirs: bool,
    pub dev: bool,
    pub runtime: RuntimeContext,
    pub skip_update: bool,
    pub allow_pack_schema: bool,
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
            .unwrap_or_else(|| pack_dir.join("pack.lock.cbor"));

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
            no_extra_dirs: args.no_extra_dirs,
            dev: args.dev,
            runtime: runtime.clone(),
            skip_update: args.no_update,
            allow_pack_schema: args.allow_pack_schema,
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

    // Resolve component references into pack.lock.cbor before building to ensure
    // manifests/extensions can rely on the lockfile contents.
    if !(opts.dry_run && opts.lock_path.exists()) {
        resolve::handle(
            ResolveArgs {
                input: opts.pack_dir.clone(),
                lock: Some(opts.lock_path.clone()),
            },
            &opts.runtime,
            false,
        )
        .await?;
    }

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

    let secret_requirements_override =
        resolve_secret_requirements_override(&opts.pack_dir, opts.secrets_req.as_ref());
    let secret_requirements = aggregate_secret_requirements(
        &config.components,
        secret_requirements_override.as_deref(),
        opts.default_secret_scope.as_deref(),
    )?;

    if !opts.lock_path.exists() {
        anyhow::bail!(
            "pack.lock.cbor is required (run `greentic-pack resolve`); missing: {}",
            opts.lock_path.display()
        );
    }
    let pack_lock = read_pack_lock(&opts.lock_path).with_context(|| {
        format!(
            "failed to read pack lock {} (try `greentic-pack resolve`)",
            opts.lock_path.display()
        )
    })?;

    let mut build = assemble_manifest(
        &config,
        &opts.pack_dir,
        &secret_requirements,
        !opts.no_extra_dirs,
        opts.dev,
        opts.allow_pack_schema,
    )?;
    build.lock_components =
        collect_lock_component_artifacts(&pack_lock, &opts.runtime, opts.bundle, opts.dry_run)
            .await?;

    let mut bundled_paths = BTreeMap::new();
    for entry in &build.lock_components {
        bundled_paths.insert(entry.component_id.clone(), entry.logical_path.clone());
    }

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
        &bundled_paths,
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
        if opts.dev && !secret_requirements.is_empty() {
            let logical = "secret-requirements.json".to_string();
            let req_path =
                write_secret_requirements_file(&opts.pack_dir, &secret_requirements, &logical)?;
            build.assets.push(AssetFile {
                logical_path: logical,
                source: req_path,
            });
        }
        let warnings = package_gtpack(gtpack_out, &manifest_bytes, &build, opts.bundle, opts.dev)?;
        for warning in warnings {
            warn!(warning);
        }
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
    extra_files: Vec<ExtraFile>,
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
    component_id: String,
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

struct ExtraFile {
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
    include_extra_dirs: bool,
    dev_mode: bool,
    allow_pack_schema: bool,
) -> Result<BuildProducts> {
    let components = build_components(&config.components, allow_pack_schema)?;
    let (flows, flow_files) = build_flows(&config.flows, pack_root)?;
    let dependencies = build_dependencies(&config.dependencies)?;
    let assets = collect_assets(&config.assets, pack_root)?;
    let extra_files = if include_extra_dirs {
        collect_extra_dir_files(pack_root)?
    } else {
        Vec::new()
    };
    let component_manifests: Vec<_> = components.iter().map(|c| c.0.clone()).collect();
    let bootstrap = build_bootstrap(config, &flows, &component_manifests)?;
    let extensions = normalize_extensions(&config.extensions);

    let mut manifest = PackManifest {
        schema_version: "pack-v1".to_string(),
        pack_id: PackId::new(config.pack_id.clone()).context("invalid pack_id")?,
        name: config.name.clone(),
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

    annotate_manifest_build_mode(&mut manifest, dev_mode);

    Ok(BuildProducts {
        manifest,
        components: components.into_iter().map(|(_, bin)| bin).collect(),
        lock_components: Vec::new(),
        component_manifest_files: Vec::new(),
        flow_files,
        assets,
        extra_files,
    })
}

fn annotate_manifest_build_mode(manifest: &mut PackManifest, dev_mode: bool) {
    let extensions = manifest.extensions.get_or_insert_with(BTreeMap::new);
    extensions.insert(
        EXT_BUILD_MODE_ID.to_string(),
        ExtensionRef {
            kind: EXT_BUILD_MODE_ID.to_string(),
            version: "1".to_string(),
            digest: None,
            location: None,
            inline: Some(PackManifestExtensionInline::Other(json!({
                "mode": if dev_mode { "dev" } else { "prod" }
            }))),
        },
    );
}

fn build_components(
    configs: &[ComponentConfig],
    allow_pack_schema: bool,
) -> Result<Vec<(ComponentManifest, ComponentBinary)>> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();

    for cfg in configs {
        if !seen.insert(cfg.id.clone()) {
            anyhow::bail!("duplicate component id {}", cfg.id);
        }

        info!(id = %cfg.id, wasm = %cfg.wasm.display(), "adding component");
        let (manifest, binary) = resolve_component_artifacts(cfg, allow_pack_schema)?;

        result.push((manifest, binary));
    }

    Ok(result)
}

fn resolve_component_artifacts(
    cfg: &ComponentConfig,
    allow_pack_schema: bool,
) -> Result<(ComponentManifest, ComponentBinary)> {
    let resolved_wasm = resolve_component_wasm_path(&cfg.wasm)?;

    let mut manifest = if let Some(from_disk) =
        load_component_manifest_from_disk(&resolved_wasm, &cfg.id)?
    {
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
    } else if allow_pack_schema {
        warn!(
            id = %cfg.id,
            "migration-only path enabled: deriving component manifest/schema from pack.yaml (--allow-pack-schema)"
        );
        manifest_from_config(cfg)?
    } else {
        anyhow::bail!(
            "component {} is missing component.manifest.json; refusing to derive schema from pack.yaml on 0.6 path (migration-only override: --allow-pack-schema)",
            cfg.id
        );
    };

    // Ensure operations are populated from pack.yaml when missing in the on-disk manifest.
    if manifest.operations.is_empty() && !cfg.operations.is_empty() {
        manifest.operations = cfg
            .operations
            .iter()
            .map(operation_from_config)
            .collect::<Result<Vec<_>>>()?;
    }

    let manifest_bytes = canonical::to_canonical_cbor_allow_floats(&manifest)
        .context("encode component manifest to canonical cbor")?;
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

fn load_component_manifest_from_disk(
    path: &Path,
    component_id: &str,
) -> Result<Option<ComponentManifest>> {
    let manifest_dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow!("component path {} has no parent directory", path.display()))?
    };
    let id_manifest_suffix = format!("{component_id}.manifest");

    // Search near the wasm artifact first, then walk up parent directories.
    // This supports components built into nested target/*/release paths where
    // component.manifest.json lives at the component root.
    for dir in std::iter::successors(Some(manifest_dir.as_path()), |d| d.parent()) {
        let candidates = [
            dir.join("component.manifest.cbor"),
            dir.join("component.manifest.json"),
            dir.join("component.json"),
            dir.join(format!("{id_manifest_suffix}.cbor")),
            dir.join(format!("{id_manifest_suffix}.json")),
            dir.join(format!("{component_id}.json")),
        ];
        for manifest_path in candidates {
            if !manifest_path.exists() {
                continue;
            }
            let manifest = load_component_manifest_from_file(&manifest_path)?;
            return Ok(Some(manifest));
        }
    }

    Ok(None)
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

fn is_reserved_extra_file(logical_path: &str) -> bool {
    matches!(logical_path, "sbom.cbor" | "sbom.json")
}

fn collect_extra_dir_files(pack_root: &Path) -> Result<Vec<ExtraFile>> {
    let excluded = [
        "components",
        "flows",
        "dist",
        "target",
        ".git",
        ".github",
        ".idea",
        ".vscode",
        "node_modules",
    ];
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in fs::read_dir(pack_root)
        .with_context(|| format!("failed to list pack root {}", pack_root.display()))?
    {
        let entry = entry?;
        let entry_type = entry.file_type()?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry_type.is_file() {
            let logical = name.to_string();
            if is_reserved_extra_file(&logical) {
                continue;
            }
            if !logical.is_empty() && seen.insert(logical.clone()) {
                entries.push(ExtraFile {
                    logical_path: logical,
                    source: entry.path(),
                });
            }
            continue;
        }
        if !entry_type.is_dir() {
            continue;
        }
        if name.starts_with('.') || excluded.contains(&name.as_ref()) {
            continue;
        }
        let root = entry.path();
        for sub in WalkDir::new(&root)
            .into_iter()
            .filter_entry(|walk| !walk.file_name().to_string_lossy().starts_with('.'))
            .filter_map(Result::ok)
        {
            if !sub.file_type().is_file() {
                continue;
            }
            let logical = sub
                .path()
                .strip_prefix(pack_root)
                .unwrap_or(sub.path())
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if logical.is_empty() || !seen.insert(logical.clone()) {
                continue;
            }
            if is_reserved_extra_file(&logical) {
                continue;
            }
            entries.push(ExtraFile {
                logical_path: logical,
                source: sub.path().to_path_buf(),
            });
        }
    }
    Ok(entries)
}

fn map_extra_files(
    extras: &[ExtraFile],
    asset_paths: &mut BTreeSet<String>,
    dev_mode: bool,
    warnings: &mut Vec<String>,
) -> Vec<(String, PathBuf)> {
    let mut mapped = Vec::new();
    for extra in extras {
        let logical = extra.logical_path.as_str();
        if logical.starts_with("assets/") {
            if asset_paths.insert(logical.to_string()) {
                mapped.push((logical.to_string(), extra.source.clone()));
            }
            continue;
        }
        if !logical.contains('/') {
            if is_reserved_source_file(logical) {
                if dev_mode || logical == "pack.lock.cbor" {
                    mapped.push((logical.to_string(), extra.source.clone()));
                }
                continue;
            }
            let target = format!("assets/{logical}");
            if asset_paths.insert(target.clone()) {
                mapped.push((target, extra.source.clone()));
            } else {
                warnings.push(format!(
                    "skipping root asset {logical} because assets/{logical} already exists"
                ));
            }
            continue;
        }
        mapped.push((logical.to_string(), extra.source.clone()));
    }
    mapped
}

fn is_reserved_source_file(path: &str) -> bool {
    matches!(
        path,
        "pack.yaml"
            | "pack.manifest.json"
            | "pack.lock.cbor"
            | "manifest.json"
            | "manifest.cbor"
            | "sbom.json"
            | "sbom.cbor"
            | "provenance.json"
            | "secret-requirements.json"
            | "secrets_requirements.json"
    ) || path.ends_with(".ygtc")
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
    bundled_paths: &BTreeMap<String, String>,
    manifest_paths: Option<&std::collections::BTreeMap<String, String>>,
) -> Result<Option<BTreeMap<String, ExtensionRef>>> {
    let mut entries = Vec::new();
    for comp in lock.components.values() {
        let Some(reference) = comp.r#ref.as_ref() else {
            continue;
        };
        if reference.starts_with("file://") {
            continue;
        }
        let source = match ComponentSourceRef::from_str(reference) {
            Ok(parsed) => parsed,
            Err(_) => {
                eprintln!(
                    "warning: skipping pack.lock entry `{}` with unsupported ref {}",
                    comp.component_id, reference
                );
                continue;
            }
        };
        let manifest_path = manifest_paths.and_then(|paths| paths.get(&comp.component_id).cloned());
        let artifact = if let Some(wasm_path) = bundled_paths.get(&comp.component_id) {
            ArtifactLocationV1::Inline {
                wasm_path: wasm_path.clone(),
                manifest_path,
            }
        } else {
            ArtifactLocationV1::Remote
        };
        entries.push(ComponentSourceEntryV1 {
            name: comp.component_id.clone(),
            component_id: Some(ComponentId::new(comp.component_id.clone()).map_err(|err| {
                anyhow!(
                    "invalid component id {} in lock: {}",
                    comp.component_id,
                    err
                )
            })?),
            source,
            resolved: ResolvedComponentV1 {
                digest: comp.resolved_digest.clone(),
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
    dev_mode: bool,
) -> Result<Vec<String>> {
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
    let mut written_paths = BTreeSet::new();
    let mut warnings = Vec::new();
    let mut asset_paths = BTreeSet::new();
    record_sbom_entry(
        &mut sbom_entries,
        "manifest.cbor",
        manifest_bytes,
        "application/cbor",
    );
    written_paths.insert("manifest.cbor".to_string());
    write_zip_entry(&mut writer, "manifest.cbor", manifest_bytes, options)?;

    if dev_mode {
        let mut flow_files = build.flow_files.clone();
        flow_files.sort_by(|a, b| a.logical_path.cmp(&b.logical_path));
        for flow_file in flow_files {
            if written_paths.insert(flow_file.logical_path.clone()) {
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
        }
    }

    let mut component_wasm_paths = BTreeSet::new();
    if bundle != BundleMode::None {
        for comp in &build.components {
            component_wasm_paths.insert(format!("components/{}.wasm", comp.id));
        }
    }
    let mut manifest_component_ids = BTreeSet::new();
    for manifest in &build.component_manifest_files {
        manifest_component_ids.insert(manifest.component_id.clone());
    }

    let mut lock_components = build.lock_components.clone();
    lock_components.sort_by(|a, b| a.logical_path.cmp(&b.logical_path));
    for comp in lock_components {
        if component_wasm_paths.contains(&comp.logical_path) {
            continue;
        }
        if !written_paths.insert(comp.logical_path.clone()) {
            continue;
        }
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
        let describe_source = PathBuf::from(format!("{}.describe.cbor", comp.source.display()));
        if describe_source.exists() {
            let describe_bytes = fs::read(&describe_source).with_context(|| {
                format!(
                    "failed to read describe cache {}",
                    describe_source.display()
                )
            })?;
            let describe_logical = format!("{}.describe.cbor", comp.logical_path);
            if written_paths.insert(describe_logical.clone()) {
                record_sbom_entry(
                    &mut sbom_entries,
                    &describe_logical,
                    &describe_bytes,
                    "application/cbor",
                );
                write_zip_entry(&mut writer, &describe_logical, &describe_bytes, options)?;
            }
        }

        if manifest_component_ids.contains(&comp.component_id) {
            let alias_path = format!("components/{}.wasm", comp.component_id);
            if written_paths.insert(alias_path.clone()) {
                record_sbom_entry(&mut sbom_entries, &alias_path, &bytes, "application/wasm");
                write_zip_entry(&mut writer, &alias_path, &bytes, options)?;
            }
            let describe_source = PathBuf::from(format!("{}.describe.cbor", comp.source.display()));
            if describe_source.exists() {
                let describe_bytes = fs::read(&describe_source).with_context(|| {
                    format!(
                        "failed to read describe cache {}",
                        describe_source.display()
                    )
                })?;
                let alias_describe = format!("{alias_path}.describe.cbor");
                if written_paths.insert(alias_describe.clone()) {
                    record_sbom_entry(
                        &mut sbom_entries,
                        &alias_describe,
                        &describe_bytes,
                        "application/cbor",
                    );
                    write_zip_entry(&mut writer, &alias_describe, &describe_bytes, options)?;
                }
            }
        }
    }

    let mut lock_manifests = build.component_manifest_files.clone();
    lock_manifests.sort_by(|a, b| a.manifest_path.cmp(&b.manifest_path));
    for manifest in lock_manifests {
        if written_paths.insert(manifest.manifest_path.clone()) {
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
    }

    if bundle != BundleMode::None {
        let mut components = build.components.clone();
        components.sort_by(|a, b| a.id.cmp(&b.id));
        for comp in components {
            let logical_wasm = format!("components/{}.wasm", comp.id);
            let wasm_bytes = fs::read(&comp.source)
                .with_context(|| format!("failed to read component {}", comp.source.display()))?;
            if written_paths.insert(logical_wasm.clone()) {
                record_sbom_entry(
                    &mut sbom_entries,
                    &logical_wasm,
                    &wasm_bytes,
                    "application/wasm",
                );
                write_zip_entry(&mut writer, &logical_wasm, &wasm_bytes, options)?;
            }
            let describe_source = PathBuf::from(format!("{}.describe.cbor", comp.source.display()));
            if describe_source.exists() {
                let describe_bytes = fs::read(&describe_source).with_context(|| {
                    format!(
                        "failed to read describe cache {}",
                        describe_source.display()
                    )
                })?;
                let describe_logical = format!("{logical_wasm}.describe.cbor");
                if written_paths.insert(describe_logical.clone()) {
                    record_sbom_entry(
                        &mut sbom_entries,
                        &describe_logical,
                        &describe_bytes,
                        "application/cbor",
                    );
                    write_zip_entry(&mut writer, &describe_logical, &describe_bytes, options)?;
                }
            }

            if written_paths.insert(comp.manifest_path.clone()) {
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
    }

    let mut extra_entries: Vec<_> = Vec::new();
    for asset in &build.assets {
        let logical = format!("assets/{}", asset.logical_path);
        asset_paths.insert(logical.clone());
        extra_entries.push((logical, asset.source.clone()));
    }
    let mut mapped_extra = map_extra_files(
        &build.extra_files,
        &mut asset_paths,
        dev_mode,
        &mut warnings,
    );
    extra_entries.append(&mut mapped_extra);
    extra_entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (logical, source) in extra_entries {
        if !written_paths.insert(logical.clone()) {
            continue;
        }
        let bytes = fs::read(&source)
            .with_context(|| format!("failed to read extra file {}", source.display()))?;
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
    let sbom_bytes = canonical::to_canonical_cbor_allow_floats(&sbom_doc)
        .context("failed to encode canonical sbom.cbor")?;
    write_zip_entry(&mut writer, "sbom.cbor", &sbom_bytes, options)?;

    writer
        .finish()
        .context("failed to finalise gtpack archive")?;
    Ok(warnings)
}

async fn collect_lock_component_artifacts(
    lock: &greentic_pack::pack_lock::PackLockV1,
    runtime: &RuntimeContext,
    bundle: BundleMode,
    allow_missing: bool,
) -> Result<Vec<LockComponentBinary>> {
    let dist = DistClient::new(DistOptions {
        cache_dir: runtime.cache_dir(),
        allow_tags: true,
        offline: runtime.network_policy() == NetworkPolicy::Offline,
        allow_insecure_local_http: false,
        ..DistOptions::default()
    });

    let mut artifacts = Vec::new();
    let mut seen_paths = BTreeSet::new();
    for comp in lock.components.values() {
        let Some(reference) = comp.r#ref.as_ref() else {
            continue;
        };
        if reference.starts_with("file://") {
            continue;
        }
        let parsed = ComponentSourceRef::from_str(reference).ok();
        let is_tag = parsed.as_ref().map(|r| r.is_tag()).unwrap_or(false);
        let should_bundle = is_tag || bundle == BundleMode::Cache;
        if !should_bundle {
            continue;
        }

        let resolved = if is_tag {
            let item = if runtime.network_policy() == NetworkPolicy::Offline {
                dist.ensure_cached(&comp.resolved_digest)
                    .await
                    .map_err(|err| {
                        anyhow!(
                            "tag ref {} must be bundled but cache is missing ({})",
                            reference,
                            err
                        )
                    })?
            } else {
                dist.resolve_ref(reference)
                    .await
                    .map_err(|err| anyhow!("failed to resolve {}: {}", reference, err))?
            };
            let cache_path = item.cache_path.clone().ok_or_else(|| {
                anyhow!("tag ref {} resolved but cache path is missing", reference)
            })?;
            ResolvedLockItem { cache_path }
        } else {
            let mut resolved = dist
                .ensure_cached(&comp.resolved_digest)
                .await
                .ok()
                .and_then(|item| item.cache_path.clone().map(|path| (item, path)));
            if resolved.is_none()
                && runtime.network_policy() != NetworkPolicy::Offline
                && !allow_missing
                && reference.starts_with("oci://")
            {
                let item = dist
                    .resolve_ref(reference)
                    .await
                    .map_err(|err| anyhow!("failed to resolve {}: {}", reference, err))?;
                if let Some(path) = item.cache_path.clone() {
                    resolved = Some((item, path));
                }
            }
            let Some((_item, path)) = resolved else {
                if runtime.network_policy() == NetworkPolicy::Offline {
                    if allow_missing {
                        eprintln!(
                            "warning: component {} is not cached; skipping embed",
                            comp.component_id
                        );
                        continue;
                    }
                    anyhow::bail!(
                        "component {} requires network access ({}) but cache is missing; offline builds cannot download artifacts",
                        comp.component_id,
                        reference
                    );
                }
                eprintln!(
                    "warning: component {} is not cached; skipping embed",
                    comp.component_id
                );
                continue;
            };
            ResolvedLockItem { cache_path: path }
        };

        let cache_path = resolved.cache_path;
        let bytes = fs::read(&cache_path)
            .with_context(|| format!("failed to read cached component {}", cache_path.display()))?;
        let wasm_sha256 = format!("{:x}", Sha256::digest(&bytes));
        let logical_path = if is_tag {
            format!("blobs/sha256/{}.wasm", wasm_sha256)
        } else {
            format!("components/{}.wasm", comp.component_id)
        };

        if seen_paths.insert(logical_path.clone()) {
            artifacts.push(LockComponentBinary {
                component_id: comp.component_id.clone(),
                logical_path: logical_path.clone(),
                source: cache_path.clone(),
            });
        }
    }

    Ok(artifacts)
}

struct ResolvedLockItem {
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
    for (key, entry) in &pack_lock.components {
        lock_by_id.insert(key.clone(), entry);
    }

    let mut bundle_sources_by_component = BTreeMap::new();
    for entry in lock_components {
        bundle_sources_by_component.insert(entry.component_id.clone(), entry.source.clone());
    }

    let mut materialized_components = Vec::new();
    let mut manifest_files = Vec::new();
    let mut manifest_paths: BTreeMap<String, String> = BTreeMap::new();

    for component_id in referenced {
        if existing.contains(&component_id) {
            continue;
        }

        let lock_entry = lock_by_id.get(&component_id).copied();
        let Some(lock_entry) = lock_entry else {
            handle_missing_component_manifest(&component_id, None, require_component_manifests)?;
            continue;
        };
        let bundled_source = bundle_sources_by_component.get(&component_id);
        if bundled_source.is_none() {
            if require_component_manifests {
                anyhow::bail!(
                    "component {} is not bundled; cannot materialize manifest without local artifacts",
                    lock_entry.component_id
                );
            }
            eprintln!(
                "warning: component {} is not bundled; pack will emit PACK_COMPONENT_NOT_EXPLICIT",
                lock_entry.component_id
            );
            continue;
        }

        let manifest =
            load_component_manifest_for_lock(pack_dir, &lock_entry.component_id, bundled_source)?;

        let Some(manifest) = manifest else {
            handle_missing_component_manifest(&component_id, None, require_component_manifests)?;
            continue;
        };

        if manifest.id.as_str() != lock_entry.component_id.as_str() {
            anyhow::bail!(
                "component manifest id {} does not match pack.lock component_id {}",
                manifest.id.as_str(),
                lock_entry.component_id.as_str()
            );
        }

        let manifest_file = component_manifest_file_from_manifest(&manifest)?;
        manifest_paths.insert(
            manifest.id.as_str().to_string(),
            manifest_file.manifest_path.clone(),
        );
        manifest_paths.insert(
            lock_entry.component_id.clone(),
            manifest_file.manifest_path.clone(),
        );

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
            if !id.is_empty() && !is_builtin_component_id(id) {
                ids.insert(id.to_string());
            }
        }
    }
    ids
}

fn is_builtin_component_id(id: &str) -> bool {
    matches!(id, "session.wait" | "flow.call" | "provider.invoke") || id.starts_with("emit.")
}

fn load_component_manifest_for_lock(
    pack_dir: &Path,
    component_id: &str,
    bundled_source: Option<&PathBuf>,
) -> Result<Option<ComponentManifest>> {
    let mut search_paths = Vec::new();
    search_paths.extend(component_manifest_search_paths(pack_dir, component_id));
    if let Some(source) = bundled_source
        && let Some(parent) = source.parent()
    {
        search_paths.push(parent.join("component.manifest.cbor"));
        search_paths.push(parent.join("component.manifest.json"));
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
    let manifest_bytes = canonical::to_canonical_cbor_allow_floats(manifest)
        .context("encode component manifest to canonical cbor")?;
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

fn resolve_secret_requirements_override(
    pack_root: &Path,
    override_path: Option<&PathBuf>,
) -> Option<PathBuf> {
    if let Some(path) = override_path {
        return Some(path.clone());
    }
    find_secret_requirements_file(pack_root)
}

fn find_secret_requirements_file(pack_root: &Path) -> Option<PathBuf> {
    for name in ["secrets_requirements.json", "secret-requirements.json"] {
        let candidate = pack_root.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BootstrapConfig;
    use crate::runtime::resolve_runtime;
    use greentic_pack::pack_lock::{LockedComponent, PackLockV1};
    use greentic_types::cbor::canonical;
    use greentic_types::decode_pack_manifest;
    use greentic_types::flow::FlowKind;
    use greentic_types::schemas::common::schema_ir::{AdditionalProperties, SchemaIr};
    use greentic_types::schemas::component::v0_6_0::{
        ComponentDescribe, ComponentInfo, ComponentOperation, ComponentRunInput,
        ComponentRunOutput, schema_hash,
    };
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs::File;
    use std::io::Read;
    use std::path::Path;
    use std::{fs, path::PathBuf};
    use tempfile::tempdir;
    use zip::ZipArchive;

    fn sample_hex(ch: char) -> String {
        std::iter::repeat_n(ch, 64).collect()
    }

    fn sample_lock_component(
        component_id: &str,
        reference: Option<&str>,
        digest_hex: char,
    ) -> LockedComponent {
        LockedComponent {
            component_id: component_id.to_string(),
            r#ref: reference.map(|value| value.to_string()),
            abi_version: "0.6.0".to_string(),
            resolved_digest: format!("sha256:{}", sample_hex(digest_hex)),
            describe_hash: sample_hex(digest_hex),
            operations: Vec::new(),
            world: None,
            component_version: None,
            role: None,
        }
    }

    fn write_describe_sidecar(wasm_path: &Path, component_id: &str) {
        let input_schema = SchemaIr::String {
            min_len: None,
            max_len: None,
            regex: None,
            format: None,
        };
        let output_schema = SchemaIr::String {
            min_len: None,
            max_len: None,
            regex: None,
            format: None,
        };
        let config_schema = SchemaIr::Object {
            properties: BTreeMap::new(),
            required: Vec::new(),
            additional: AdditionalProperties::Forbid,
        };
        let hash = schema_hash(&input_schema, &output_schema, &config_schema).expect("schema hash");
        let operation = ComponentOperation {
            id: "run".to_string(),
            display_name: None,
            input: ComponentRunInput {
                schema: input_schema,
            },
            output: ComponentRunOutput {
                schema: output_schema,
            },
            defaults: BTreeMap::new(),
            redactions: Vec::new(),
            constraints: BTreeMap::new(),
            schema_hash: hash,
        };
        let describe = ComponentDescribe {
            info: ComponentInfo {
                id: component_id.to_string(),
                version: "0.1.0".to_string(),
                role: "tool".to_string(),
                display_name: None,
            },
            provided_capabilities: Vec::new(),
            required_capabilities: Vec::new(),
            metadata: BTreeMap::new(),
            operations: vec![operation],
            config_schema,
        };
        let bytes = canonical::to_canonical_cbor_allow_floats(&describe).expect("encode describe");
        let describe_path = PathBuf::from(format!("{}.describe.cbor", wasm_path.display()));
        fs::write(describe_path, bytes).expect("write describe cache");
    }

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

    fn write_sample_manifest(path: &Path, component_id: &str) {
        let manifest: ComponentManifest = serde_json::from_value(json!({
            "id": component_id,
            "version": "0.1.0",
            "supports": [],
            "world": "greentic:component/component@0.5.0",
            "profiles": { "default": "stateless", "supported": ["stateless"] },
            "capabilities": { "wasi": {}, "host": {} },
            "operations": [],
            "resources": {},
            "dev_flows": {}
        }))
        .expect("manifest");
        let bytes = serde_cbor::to_vec(&manifest).expect("encode manifest");
        fs::write(path, bytes).expect("write manifest");
    }

    #[test]
    fn load_component_manifest_from_disk_supports_id_specific_files() {
        let temp = tempdir().expect("temp dir");
        let components = temp.path().join("components");
        fs::create_dir_all(&components).expect("create components dir");
        let wasm = components.join("component.wasm");
        fs::write(&wasm, b"wasm").expect("write wasm");
        let manifest_name = components.join("foo.component.manifest.cbor");
        write_sample_manifest(&manifest_name, "foo.component");

        let manifest =
            load_component_manifest_from_disk(&wasm, "foo.component").expect("load manifest");
        let manifest = manifest.expect("manifest present");
        assert_eq!(manifest.id.to_string(), "foo.component");
    }

    #[test]
    fn load_component_manifest_from_disk_accepts_generic_names() {
        let temp = tempdir().expect("temp dir");
        let components = temp.path().join("components");
        fs::create_dir_all(&components).expect("create components dir");
        let wasm = components.join("component.wasm");
        fs::write(&wasm, b"wasm").expect("write wasm");
        let manifest_name = components.join("component.manifest.cbor");
        write_sample_manifest(&manifest_name, "component");

        let manifest =
            load_component_manifest_from_disk(&wasm, "component").expect("load manifest");
        let manifest = manifest.expect("manifest present");
        assert_eq!(manifest.id.to_string(), "component");
    }

    #[test]
    fn load_component_manifest_from_disk_walks_up_from_nested_target_paths() {
        let temp = tempdir().expect("temp dir");
        let component_root = temp.path().join("components/demo-component");
        let release_dir = component_root.join("target/wasm32-wasip2/release");
        fs::create_dir_all(&release_dir).expect("create release dir");
        let wasm = release_dir.join("demo_component.wasm");
        fs::write(&wasm, b"wasm").expect("write wasm");
        let manifest_name = component_root.join("component.manifest.cbor");
        write_sample_manifest(&manifest_name, "dev.local.demo-component");

        let manifest = load_component_manifest_from_disk(&wasm, "dev.local.demo-component")
            .expect("load manifest");
        let manifest = manifest.expect("manifest present");
        assert_eq!(manifest.id.to_string(), "dev.local.demo-component");
    }

    #[test]
    fn resolve_component_artifacts_requires_manifest_unless_migration_flag_set() {
        let temp = tempdir().expect("temp dir");
        let wasm = temp.path().join("component.wasm");
        fs::write(&wasm, [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]).expect("write wasm");

        let cfg: ComponentConfig = serde_json::from_value(json!({
            "id": "demo.component",
            "version": "0.1.0",
            "world": "greentic:component/component@0.6.0",
            "supports": [],
            "profiles": { "default": "stateless", "supported": ["stateless"] },
            "capabilities": { "wasi": {}, "host": {} },
            "operations": [],
            "wasm": wasm.to_string_lossy()
        }))
        .expect("component config");

        let err = match resolve_component_artifacts(&cfg, false) {
            Ok(_) => panic!("missing manifest must fail"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("missing component.manifest.json"),
            "unexpected error: {err}"
        );

        let (manifest, _binary) =
            resolve_component_artifacts(&cfg, true).expect("migration flag allows fallback");
        assert_eq!(manifest.id.to_string(), "demo.component");
    }

    #[test]
    fn collect_extra_dir_files_skips_hidden_and_known_dirs() {
        let temp = tempdir().expect("temp dir");
        let root = temp.path();
        fs::create_dir_all(root.join("schemas")).expect("schemas dir");
        fs::create_dir_all(root.join("schemas").join(".nested")).expect("nested hidden dir");
        fs::create_dir_all(root.join(".hidden")).expect("hidden dir");
        fs::create_dir_all(root.join("assets")).expect("assets dir");
        fs::write(root.join("README.txt"), b"root").expect("root file");
        fs::write(root.join("schemas").join("config.schema.json"), b"{}").expect("schema file");
        fs::write(
            root.join("schemas").join(".nested").join("skip.json"),
            b"{}",
        )
        .expect("nested hidden file");
        fs::write(root.join(".hidden").join("secret.txt"), b"nope").expect("hidden file");
        fs::write(root.join("assets").join("asset.txt"), b"nope").expect("asset file");

        let collected = collect_extra_dir_files(root).expect("collect extra dirs");
        let paths: BTreeSet<_> = collected.iter().map(|e| e.logical_path.as_str()).collect();
        assert!(paths.contains("README.txt"));
        assert!(paths.contains("schemas/config.schema.json"));
        assert!(!paths.contains("schemas/.nested/skip.json"));
        assert!(!paths.contains(".hidden/secret.txt"));
        assert!(paths.contains("assets/asset.txt"));
    }

    #[test]
    fn collect_extra_dir_files_skips_reserved_sbom_files() {
        let temp = tempdir().expect("temp dir");
        let root = temp.path();
        fs::write(root.join("sbom.cbor"), b"binary").expect("sbom file");
        fs::write(root.join("sbom.json"), b"{}").expect("sbom json");
        fs::write(root.join("README.md"), b"hello").expect("root file");

        let collected = collect_extra_dir_files(root).expect("collect extra dirs");
        let paths: BTreeSet<_> = collected.iter().map(|e| e.logical_path.as_str()).collect();
        assert!(paths.contains("README.md"));
        assert!(!paths.contains("sbom.cbor"));
        assert!(!paths.contains("sbom.json"));
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
            component_manifest_files: Vec::new(),
            flow_files: Vec::new(),
            assets: Vec::new(),
            extra_files: Vec::new(),
        };

        let out = temp.path().join("demo.gtpack");
        let warnings = package_gtpack(&out, &manifest_bytes, &build, BundleMode::Cache, false)
            .expect("package gtpack");
        assert!(warnings.is_empty(), "expected no packaging warnings");

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
    fn prod_gtpack_excludes_forbidden_files() {
        let component = manifest_with_dev_flow();
        let pack_manifest = pack_manifest_with_component(component.clone());
        let manifest_bytes = encode_pack_manifest(&pack_manifest).expect("encode manifest");

        let temp = tempdir().expect("temp dir");
        let wasm_path = temp.path().join("component.wasm");
        write_stub_wasm(&wasm_path).expect("write stub wasm");

        let pack_yaml = temp.path().join("pack.yaml");
        fs::write(&pack_yaml, "pack").expect("write pack.yaml");
        let pack_manifest_json = temp.path().join("pack.manifest.json");
        fs::write(&pack_manifest_json, "{}").expect("write manifest json");

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
            component_manifest_files: Vec::new(),
            flow_files: Vec::new(),
            assets: Vec::new(),
            extra_files: vec![
                ExtraFile {
                    logical_path: "pack.yaml".to_string(),
                    source: pack_yaml,
                },
                ExtraFile {
                    logical_path: "pack.manifest.json".to_string(),
                    source: pack_manifest_json,
                },
            ],
        };

        let out = temp.path().join("prod.gtpack");
        let warnings = package_gtpack(&out, &manifest_bytes, &build, BundleMode::Cache, false)
            .expect("package gtpack");
        assert!(
            warnings.is_empty(),
            "no warnings expected for forbidden drop"
        );

        let mut archive = ZipArchive::new(fs::File::open(&out).expect("open gtpack"))
            .expect("read gtpack archive");
        assert!(archive.by_name("pack.yaml").is_err());
        assert!(archive.by_name("pack.manifest.json").is_err());
    }

    #[test]
    fn asset_mapping_prefers_assets_version_on_conflict() {
        let component = manifest_with_dev_flow();
        let pack_manifest = pack_manifest_with_component(component.clone());
        let manifest_bytes = encode_pack_manifest(&pack_manifest).expect("encode manifest");

        let temp = tempdir().expect("temp dir");
        let wasm_path = temp.path().join("component.wasm");
        write_stub_wasm(&wasm_path).expect("write stub wasm");

        let assets_dir = temp.path().join("assets");
        fs::create_dir_all(&assets_dir).expect("create assets dir");
        let asset_file = assets_dir.join("README.md");
        fs::write(&asset_file, "asset").expect("write asset");
        let root_asset = temp.path().join("README.md");
        fs::write(&root_asset, "root").expect("write root file");

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
            component_manifest_files: Vec::new(),
            flow_files: Vec::new(),
            assets: Vec::new(),
            extra_files: vec![
                ExtraFile {
                    logical_path: "assets/README.md".to_string(),
                    source: asset_file,
                },
                ExtraFile {
                    logical_path: "README.md".to_string(),
                    source: root_asset,
                },
            ],
        };

        let out = temp.path().join("conflict.gtpack");
        let warnings = package_gtpack(&out, &manifest_bytes, &build, BundleMode::Cache, false)
            .expect("package gtpack");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("skipping root asset README.md"))
        );

        let mut archive = ZipArchive::new(fs::File::open(&out).expect("open gtpack"))
            .expect("read gtpack archive");
        assert!(archive.by_name("README.md").is_err());
        assert!(archive.by_name("assets/README.md").is_ok());
    }

    #[test]
    fn root_files_map_under_assets_directory() {
        let component = manifest_with_dev_flow();
        let pack_manifest = pack_manifest_with_component(component.clone());
        let manifest_bytes = encode_pack_manifest(&pack_manifest).expect("encode manifest");

        let temp = tempdir().expect("temp dir");
        let wasm_path = temp.path().join("component.wasm");
        write_stub_wasm(&wasm_path).expect("write stub wasm");
        let root_asset = temp.path().join("notes.txt");
        fs::write(&root_asset, "notes").expect("write root asset");

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
            component_manifest_files: Vec::new(),
            flow_files: Vec::new(),
            assets: Vec::new(),
            extra_files: vec![ExtraFile {
                logical_path: "notes.txt".to_string(),
                source: root_asset,
            }],
        };

        let out = temp.path().join("root-assets.gtpack");
        let warnings = package_gtpack(&out, &manifest_bytes, &build, BundleMode::Cache, false)
            .expect("package gtpack");
        assert!(
            warnings.iter().all(|w| !w.contains("notes.txt")),
            "root asset mapping should not warn without conflict"
        );

        let mut archive = ZipArchive::new(fs::File::open(&out).expect("open gtpack"))
            .expect("read gtpack archive");
        assert!(archive.by_name("assets/notes.txt").is_ok());
        assert!(archive.by_name("notes.txt").is_err());
    }

    #[test]
    fn prod_gtpack_embeds_secret_requirements_cbor_only() {
        let component = manifest_with_dev_flow();
        let mut pack_manifest = pack_manifest_with_component(component.clone());
        let secret_requirement: SecretRequirement = serde_json::from_value(json!({
            "key": "demo/token",
            "required": true,
            "description": "demo secret",
            "scope": { "env": "dev", "tenant": "demo" }
        }))
        .expect("parse secret requirement");
        pack_manifest.secret_requirements = vec![secret_requirement.clone()];
        let manifest_bytes = encode_pack_manifest(&pack_manifest).expect("encode manifest");

        let temp = tempdir().expect("temp dir");
        let wasm_path = temp.path().join("component.wasm");
        write_stub_wasm(&wasm_path).expect("write stub wasm");
        let secret_file = temp.path().join("secret-requirements.json");
        fs::write(&secret_file, "[{}]").expect("write secret json");

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
            component_manifest_files: Vec::new(),
            flow_files: Vec::new(),
            assets: Vec::new(),
            extra_files: vec![ExtraFile {
                logical_path: "secret-requirements.json".to_string(),
                source: secret_file,
            }],
        };

        let out = temp.path().join("secrets.gtpack");
        package_gtpack(&out, &manifest_bytes, &build, BundleMode::Cache, false)
            .expect("package gtpack");

        let mut archive = ZipArchive::new(fs::File::open(&out).expect("open gtpack"))
            .expect("read gtpack archive");
        assert!(archive.by_name("secret-requirements.json").is_err());
        assert!(archive.by_name("assets/secret-requirements.json").is_err());
        assert!(archive.by_name("secrets_requirements.json").is_err());
        assert!(archive.by_name("assets/secrets_requirements.json").is_err());

        let mut manifest_entry = archive
            .by_name("manifest.cbor")
            .expect("manifest.cbor present");
        let mut manifest_buf = Vec::new();
        manifest_entry
            .read_to_end(&mut manifest_buf)
            .expect("read manifest bytes");
        let decoded = decode_pack_manifest(&manifest_buf).expect("decode manifest");
        assert_eq!(decoded.secret_requirements, vec![secret_requirement]);
    }

    #[test]
    fn component_sources_extension_respects_bundle() {
        let mut components = BTreeMap::new();
        components.insert(
            "demo.tagged".to_string(),
            sample_lock_component(
                "demo.tagged",
                Some("oci://ghcr.io/demo/component:1.0.0"),
                'a',
            ),
        );
        let lock_tag = PackLockV1::new(components);

        let mut bundled_paths = BTreeMap::new();
        bundled_paths.insert(
            "demo.tagged".to_string(),
            "blobs/sha256/deadbeef.wasm".to_string(),
        );

        let ext_none =
            merge_component_sources_extension(None, &lock_tag, &bundled_paths, None).expect("ext");
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

        let mut components = BTreeMap::new();
        components.insert(
            "demo.component".to_string(),
            sample_lock_component(
                "demo.component",
                Some("oci://ghcr.io/demo/component@sha256:deadbeef"),
                'b',
            ),
        );
        let lock_digest = PackLockV1::new(components);

        let ext_none =
            merge_component_sources_extension(None, &lock_digest, &BTreeMap::new(), None)
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

        let mut components = BTreeMap::new();
        components.insert(
            "demo.component".to_string(),
            sample_lock_component(
                "demo.component",
                Some("oci://ghcr.io/demo/component@sha256:deadbeef"),
                'c',
            ),
        );
        let lock_digest_bundled = PackLockV1::new(components);

        let mut bundled_paths = BTreeMap::new();
        bundled_paths.insert(
            "demo.component".to_string(),
            "components/demo.component.wasm".to_string(),
        );

        let ext_cache =
            merge_component_sources_extension(None, &lock_digest_bundled, &bundled_paths, None)
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
        let mut components = BTreeMap::new();
        components.insert(
            "local.component".to_string(),
            sample_lock_component("local.component", Some("file:///tmp/component.wasm"), 'd'),
        );
        let lock = PackLockV1::new(components);

        let ext_none =
            merge_component_sources_extension(None, &lock, &BTreeMap::new(), None).expect("ext");
        assert!(ext_none.is_none(), "file refs should be omitted");

        let mut components = BTreeMap::new();
        components.insert(
            "local.component".to_string(),
            sample_lock_component("local.component", Some("file:///tmp/component.wasm"), 'e'),
        );
        components.insert(
            "remote.component".to_string(),
            sample_lock_component(
                "remote.component",
                Some("oci://ghcr.io/demo/component:2.0.0"),
                'f',
            ),
        );
        let lock = PackLockV1::new(components);

        let ext_some =
            merge_component_sources_extension(None, &lock, &BTreeMap::new(), None).expect("ext");
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
            write_describe_sidecar(&cache_path, "dummy.component");

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
                lock_path: pack_dir.join("pack.lock.cbor"),
                bundle: BundleMode::Cache,
                dry_run: false,
                secrets_req: None,
                default_secret_scope: None,
                allow_oci_tags: false,
                require_component_manifests: false,
                no_extra_dirs: false,
                dev: false,
                runtime,
                skip_update: false,
                allow_pack_schema: true,
            };

            run(&opts).await.expect("build");

            let gtpack_path = opts.gtpack_out.expect("gtpack path");
            let mut archive = ZipArchive::new(File::open(&gtpack_path).expect("open gtpack"))
                .expect("read gtpack");
            assert!(
                archive.by_name("components/dummy.component.wasm").is_ok(),
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
                lock_path: pack_dir.join("pack.lock.cbor"),
                bundle: BundleMode::Cache,
                dry_run: false,
                secrets_req: None,
                default_secret_scope: None,
                allow_oci_tags: false,
                require_component_manifests: false,
                no_extra_dirs: false,
                dev: false,
                runtime,
                skip_update: false,
                allow_pack_schema: true,
            };

            run(&opts).await.expect("build");

            let gtpack_path = opts.gtpack_out.expect("gtpack path");
            let mut archive =
                ZipArchive::new(File::open(&gtpack_path).expect("open gtpack"))
                    .expect("read gtpack");
            assert!(
                archive
                    .by_name("components/dummy.component.wasm")
                    .is_ok(),
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
            name: None,
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
            name: None,
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

    #[tokio::test]
    async fn offline_build_requires_cached_remote_component() {
        let temp = tempdir().expect("temp dir");
        let cache_dir = temp.path().join("cache");
        fs::create_dir_all(&cache_dir).expect("create cache dir");
        let project_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root");
        let runtime = resolve_runtime(Some(project_root), Some(cache_dir.as_path()), true, None)
            .expect("resolve runtime");

        let mut components = BTreeMap::new();
        components.insert(
            "remote.component".to_string(),
            LockedComponent {
                component_id: "remote.component".to_string(),
                r#ref: Some("oci://example/remote@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string()),
                abi_version: "0.6.0".to_string(),
                resolved_digest: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    .to_string(),
                describe_hash: sample_hex('a'),
                operations: Vec::new(),
                world: None,
                component_version: None,
                role: None,
            },
        );
        let lock = PackLockV1::new(components);

        let err = match collect_lock_component_artifacts(&lock, &runtime, BundleMode::Cache, false)
            .await
        {
            Ok(_) => panic!("expected offline build to fail without cached component"),
            Err(err) => err,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("requires network access"),
            "error message should describe missing network access, got {}",
            msg
        );
    }
}
