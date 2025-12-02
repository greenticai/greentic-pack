use crate::flows::FlowAsset;
use crate::templates::TemplateAsset;
use crate::{BuildArgs, embed, flows, manifest, mcp, sbom, templates};
use anyhow::{Context, Result};
use greentic_pack::builder::{
    ComponentArtifact, ImportRef, PACK_VERSION, PackBuilder, PackMeta, Provenance, Signing,
};
use semver::Version;
use serde_json::Value as JsonValue;
use std::fs;
use std::path::{Path, PathBuf};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tracing::{debug, info};

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub pack_dir: PathBuf,
    pub component_out: PathBuf,
    pub manifest_out: PathBuf,
    pub sbom_out: PathBuf,
    pub gtpack_out: Option<PathBuf>,
    pub component_data: PathBuf,
    pub dry_run: bool,
}

impl From<BuildArgs> for BuildOptions {
    fn from(args: BuildArgs) -> Self {
        let pack_dir = normalize(args.input);
        let component_out = normalize(args.component_out);
        let manifest_out = normalize(args.manifest);
        let sbom_out = normalize(args.sbom);
        let gtpack_out = args.gtpack_out.map(normalize);
        let default_component_data = pack_dir
            .join(".packc")
            .join("pack_component")
            .join("src")
            .join("data.rs");
        let component_data = args
            .component_data
            .map(normalize)
            .unwrap_or(default_component_data);

        Self {
            pack_dir,
            component_out,
            manifest_out,
            sbom_out,
            gtpack_out,
            component_data,
            dry_run: args.dry_run,
        }
    }
}

pub fn run(opts: &BuildOptions) -> Result<()> {
    info!(
        pack_dir = %opts.pack_dir.display(),
        component_out = %opts.component_out.display(),
        manifest_out = %opts.manifest_out.display(),
        sbom_out = %opts.sbom_out.display(),
        component_data = %opts.component_data.display(),
        gtpack_out = ?opts.gtpack_out,
        dry_run = opts.dry_run,
        "building greentic pack"
    );

    let spec_bundle = manifest::load_spec(&opts.pack_dir)?;
    info!(id = %spec_bundle.spec.id, version = %spec_bundle.spec.version, "loaded pack spec");

    let flows = flows::load_flows(&opts.pack_dir, &spec_bundle.spec)?;
    info!(count = flows.len(), "loaded flows");

    let templates = templates::collect_templates(&opts.pack_dir, &spec_bundle.spec)?;
    info!(count = templates.len(), "collected templates");

    let pack_version = Version::parse(&spec_bundle.spec.version)
        .with_context(|| format!("invalid pack version {}", spec_bundle.spec.version))?;

    let mcp_components = mcp::compose_all(&opts.pack_dir, &spec_bundle, &pack_version)?;

    let pack_manifest = manifest::build_manifest(&spec_bundle, &flows, &templates);
    let manifest_bytes = manifest::encode_manifest(&pack_manifest)?;
    info!(len = manifest_bytes.len(), "encoded manifest");

    let component_src = embed::generate_component_data(&manifest_bytes, &flows, &templates)?;
    let sbom_model = sbom::generate(&spec_bundle, &flows, &templates);
    let sbom_json = serde_json::to_string_pretty(&sbom_model)?;

    if opts.dry_run {
        debug!("component_data=\n{}", component_src);
        info!("dry-run complete; no files written");
        return Ok(());
    }

    write_if_changed(&opts.manifest_out, &manifest_bytes)?;
    write_if_changed(&opts.sbom_out, sbom_json.as_bytes())?;
    write_if_changed(&opts.component_data, component_src.as_bytes())?;

    embed::compile_component(&opts.component_data, &opts.component_out)?;

    maybe_build_gtpack(
        opts,
        &spec_bundle,
        &flows,
        &templates,
        &pack_version,
        &mcp_components,
    )?;

    info!("build complete");
    Ok(())
}

fn normalize(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        cwd.join(path)
    }
}

fn write_if_changed(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    let mut needs_write = true;
    if let Ok(current) = fs::read(path)
        && current == contents
    {
        needs_write = false;
    }

    if needs_write {
        fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))?;
        info!(path = %path.display(), "wrote file");
    } else {
        debug!(path = %path.display(), "unchanged");
    }

    Ok(())
}

fn maybe_build_gtpack(
    opts: &BuildOptions,
    spec_bundle: &manifest::SpecBundle,
    flows: &[FlowAsset],
    templates: &[TemplateAsset],
    pack_version: &Version,
    mcp_components: &[mcp::ComposedMcpComponent],
) -> Result<()> {
    if opts.dry_run {
        info!("dry-run requested; skipping .gtpack generation");
        return Ok(());
    }

    let Some(gtpack_path) = opts.gtpack_out.as_ref() else {
        return Ok(());
    };

    info!(gtpack_out = %gtpack_path.display(), "packaging .gtpack archive");

    let created_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());

    let entry_flows = if spec_bundle.spec.entry_flows.is_empty() {
        flows.iter().map(|flow| flow.bundle.id.clone()).collect()
    } else {
        spec_bundle.spec.entry_flows.clone()
    };

    let mut annotations = spec_bundle.spec.annotations.clone();
    if !spec_bundle.spec.imports_required.is_empty()
        && !annotations.contains_key("imports_required")
    {
        annotations.insert(
            "imports_required".to_string(),
            JsonValue::Array(
                spec_bundle
                    .spec
                    .imports_required
                    .iter()
                    .map(|entry| JsonValue::String(entry.clone()))
                    .collect(),
            ),
        );
    }

    let imports = spec_bundle
        .spec
        .imports_required
        .iter()
        .map(|entry| ImportRef {
            pack_id: entry.clone(),
            version_req: "*".into(),
        })
        .collect();

    let name = spec_bundle
        .spec
        .name
        .clone()
        .unwrap_or_else(|| spec_bundle.spec.id.clone());
    let meta = PackMeta {
        pack_version: PACK_VERSION,
        pack_id: spec_bundle.spec.id.clone(),
        version: pack_version.clone(),
        name,
        kind: spec_bundle.spec.kind.clone(),
        description: spec_bundle.spec.description.clone(),
        authors: spec_bundle.spec.authors.clone(),
        license: spec_bundle.spec.license.clone(),
        homepage: spec_bundle.spec.homepage.clone(),
        support: spec_bundle.spec.support.clone(),
        vendor: spec_bundle.spec.vendor.clone(),
        imports,
        entry_flows,
        created_at_utc: created_at.clone(),
        events: spec_bundle.spec.events.clone(),
        repo: spec_bundle.spec.repo.clone(),
        messaging: spec_bundle.spec.messaging.clone(),
        interfaces: spec_bundle.spec.interfaces.clone(),
        annotations,
        distribution: spec_bundle.spec.distribution.clone(),
        components: spec_bundle.spec.components.clone(),
    };

    let mut builder = PackBuilder::new(meta);
    for flow in flows {
        builder = builder.with_flow(flow.bundle.clone());
    }

    let component_version = Version::parse(env!("CARGO_PKG_VERSION"))
        .context("failed to parse package version for component metadata")?;
    let component = ComponentArtifact {
        name: "pack_component".into(),
        version: component_version,
        wasm_path: opts.component_out.clone(),
        schema_json: None,
        manifest_json: None,
        capabilities: None,
        world: None,
        hash_blake3: None,
    };
    builder = builder.with_component(component);

    for mcp in mcp_components {
        builder = builder.with_component(ComponentArtifact {
            name: mcp.id.clone(),
            version: mcp.version.clone(),
            wasm_path: mcp.artifact_path.clone(),
            schema_json: None,
            manifest_json: None,
            capabilities: None,
            world: Some("greentic:component@0.4.0".to_string()),
            hash_blake3: None,
        });
    }

    for template in templates {
        builder = builder.with_asset_bytes(template.logical_path.clone(), template.bytes.clone());
    }

    let provenance = Provenance {
        builder: format!("packc@{}", env!("CARGO_PKG_VERSION")),
        git_commit: None,
        git_repo: None,
        toolchain: None,
        built_at_utc: created_at,
        host: None,
        notes: None,
    };

    builder = builder
        .with_provenance(provenance)
        .with_signing(Signing::Dev);

    builder.build(gtpack_path)?;

    info!(gtpack_out = %gtpack_path.display(), "gtpack archive ready");
    Ok(())
}
