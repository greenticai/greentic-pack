#![forbid(unsafe_code)]

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use greentic_pack::archive_shape::{
    CANONICAL_MANIFEST_ENTRY, DW_MANIFEST_ENTRY, PackArchiveShape, archive_shape_is_ambiguous,
    detect_archive_shape,
};
use greentic_pack::reader::read_pack_archive;
use greentic_pack::static_routes::{StaticRouteV1, parse_static_routes_extension};
use greentic_pack::validate::{
    ComponentReferencesExistValidator, OauthCapabilityRequirementsValidator,
    ProviderReferencesExistValidator, ReferencedFilesExistValidator, SbomConsistencyValidator,
    SecretRequirementsValidator, StaticRoutesValidator, ValidateCtx, run_validators,
};
use greentic_pack::{PackLoad, SigningPolicy, open_pack};
use greentic_types::component_source::ComponentSourceRef;
use greentic_types::pack::extensions::component_manifests::{
    ComponentManifestIndexV1, EXT_COMPONENT_MANIFEST_INDEX_V1,
};
use greentic_types::pack::extensions::component_sources::{
    ArtifactLocationV1, ComponentSourcesV1, EXT_COMPONENT_SOURCES_V1,
};
use greentic_types::pack_manifest::{ExtensionInline as PackManifestExtensionInline, PackManifest};
use greentic_types::provider::ProviderDecl;
use greentic_types::validate::{Diagnostic, Severity, ValidationReport};
use serde::Serialize;
use serde_cbor;
use serde_json::Value;
use tempfile::TempDir;

use crate::build;
use crate::cli::doctor_dw::{DwPackReport, check_dw_pack};
use crate::cli::flow_doctor::{FlowDoctorOutcome, json_diagnostic_data, run_flow_doctor};
use crate::extension_refs::{
    default_extensions_file_path, default_extensions_lock_file_path, read_extensions_file,
    read_extensions_lock_file, validate_extensions_lock_alignment,
};
use crate::extensions::DEPLOYER_EXTENSION_KEY;
use crate::pack_lock_doctor::{PackLockDoctorInput, run_pack_lock_doctor};
use crate::runtime::RuntimeContext;
use crate::validator::{
    DEFAULT_VALIDATOR_ALLOW, LocalValidator, ValidatorConfig, ValidatorPolicy, run_wasm_validators,
};

const EXT_BUILD_MODE_ID: &str = "greentic.pack-mode.v1";

#[derive(Clone, Copy, PartialEq, Eq)]
enum PackBuildMode {
    Prod,
    Dev,
}

#[derive(Debug, Parser)]
pub struct InspectArgs {
    /// Path to a pack (.gtpack or source dir). Defaults to current directory.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Path to a compiled .gtpack archive
    #[arg(long, value_name = "FILE", conflicts_with = "input")]
    pub pack: Option<PathBuf>,

    /// Path to a pack source directory containing pack.yaml
    #[arg(long = "in", value_name = "DIR", conflicts_with = "pack")]
    pub input: Option<PathBuf>,

    /// Force archive inspection (disables auto-detection)
    #[arg(long)]
    pub archive: bool,

    /// Force source inspection (disables auto-detection)
    #[arg(long)]
    pub source: bool,

    /// Allow OCI component refs in extensions to be tag-based (default requires sha256 digest)
    #[arg(long = "allow-oci-tags", default_value_t = false)]
    pub allow_oci_tags: bool,

    /// Disable per-flow doctor checks
    #[arg(long = "no-flow-doctor", default_value_t = true, action = clap::ArgAction::SetFalse)]
    pub flow_doctor: bool,

    /// Disable per-component doctor checks
    #[arg(long = "no-component-doctor", default_value_t = true, action = clap::ArgAction::SetFalse)]
    pub component_doctor: bool,

    /// Output format
    #[arg(long, value_enum, default_value = "human")]
    pub format: InspectFormat,

    /// Enable validation (default)
    #[arg(long, default_value_t = true)]
    pub validate: bool,

    /// Disable validation
    #[arg(long = "no-validate", default_value_t = false)]
    pub no_validate: bool,

    /// Directory containing validator packs (.gtpack)
    #[arg(long, value_name = "DIR", default_value = ".greentic/validators")]
    pub validators_root: PathBuf,

    /// Validator pack or component reference (path or oci://...)
    #[arg(long, value_name = "REF")]
    pub validator_pack: Vec<String>,

    /// Validator component wasm (format: <COMPONENT_ID>=<FILE>)
    #[arg(long, value_name = "COMPONENT=FILE")]
    pub validator_wasm: Vec<String>,

    /// Allowed OCI prefixes for validator refs
    #[arg(long, value_name = "PREFIX", default_value = DEFAULT_VALIDATOR_ALLOW)]
    pub validator_allow: Vec<String>,

    /// Validator cache directory
    #[arg(long, value_name = "DIR", default_value = ".greentic/cache/validators")]
    pub validator_cache_dir: PathBuf,

    /// Validator loading policy
    #[arg(long, value_enum, default_value = "optional")]
    pub validator_policy: ValidatorPolicy,

    /// Allow online resolution of component refs during pack lock checks (default: offline)
    #[arg(long, default_value_t = false)]
    pub online: bool,

    /// Allow describe cache fallback when components cannot execute describe()
    #[arg(long = "use-describe-cache", default_value_t = false)]
    pub use_describe_cache: bool,
}

pub async fn handle(args: InspectArgs, json: bool, runtime: &RuntimeContext) -> Result<()> {
    let mode = resolve_mode(&args)?;
    let format = resolve_format(&args, json);
    let validate_enabled = if args.no_validate {
        false
    } else {
        args.validate
    };

    // Archives come in more than one shape, and only the canonical one can be
    // loaded as a `PackLoad`. Derive the shape from the archive's own entry
    // names before trying to decode anything, so a healthy pack of another
    // shape is named rather than reported as a missing file.
    let mut shape_notes: Vec<String> = Vec::new();
    let shape = match &mode {
        InspectMode::Archive(path) => {
            let entries = read_pack_archive(path)
                .with_context(|| format!("failed to open pack {}", path.display()))?;
            let entry_names = entries.entry_names();
            let shape = detect_archive_shape(&entry_names);
            if archive_shape_is_ambiguous(&entry_names) {
                shape_notes.push(format!(
                    "archive contains both `{CANONICAL_MANIFEST_ENTRY}` and `{DW_MANIFEST_ENTRY}`; \
                     treating it as a canonical pack (`{CANONICAL_MANIFEST_ENTRY}` wins) — \
                     a pack should carry exactly one manifest, so this is a producer bug"
                ));
            }
            match shape {
                PackArchiveShape::DwAnswerDoc => {
                    return handle_dw_pack(path, &entries.files, &args, format);
                }
                PackArchiveShape::Unrecognised => {
                    bail!(unrecognised_archive_message(path, &entry_names));
                }
                PackArchiveShape::Canonical => shape,
            }
        }
        // A source directory is always built into a canonical pack first.
        InspectMode::Source(_) => PackArchiveShape::Canonical,
    };

    let load = match &mode {
        InspectMode::Archive(path) => inspect_pack_file(path)?,
        InspectMode::Source(path) => inspect_source_dir(path, runtime, args.allow_oci_tags).await?,
    };
    let build_mode = detect_pack_build_mode(&load);
    if matches!(mode, InspectMode::Archive(_)) && build_mode == PackBuildMode::Prod {
        let forbidden = find_forbidden_source_paths(&load.files);
        if !forbidden.is_empty() {
            bail!(
                "production pack contains forbidden source files: {}",
                forbidden.join(", ")
            );
        }
    }
    let validation = if validate_enabled {
        let mut output =
            run_pack_validation(&load, source_mode_pack_dir(&mode), &args, runtime).await?;
        let mut doctor_diagnostics = Vec::new();
        let mut doctor_errors = false;
        if args.component_doctor {
            let use_describe_cache = args.use_describe_cache
                || std::env::var("GREENTIC_PACK_USE_DESCRIBE_CACHE").is_ok()
                || cfg!(test);
            let pack_dir = match &mode {
                InspectMode::Source(path) => Some(path.as_path()),
                InspectMode::Archive(_) => None,
            };
            let pack_lock_output = run_pack_lock_doctor(PackLockDoctorInput {
                load: &load,
                pack_dir,
                runtime,
                allow_oci_tags: args.allow_oci_tags,
                use_describe_cache,
                online: args.online,
            })?;
            doctor_errors |= pack_lock_output.has_errors;
            doctor_diagnostics.extend(pack_lock_output.diagnostics);
        }
        if args.flow_doctor {
            doctor_errors |= run_flow_doctors(&load, &mut doctor_diagnostics, build_mode)?;
        }
        if args.component_doctor {
            doctor_errors |= run_component_doctors(&load, &mut doctor_diagnostics)?;
        }
        output.report.diagnostics.extend(doctor_diagnostics);
        output.has_errors |= doctor_errors;
        Some(output)
    } else {
        None
    };

    match format {
        InspectFormat::Json => {
            let mut payload = serde_json::json!({
                "archive_shape": shape.as_slug(),
                "shape_notes": shape_notes,
                "manifest": load.manifest,
                "report": {
                    "signature_ok": load.report.signature_ok,
                    "sbom_ok": load.report.sbom_ok,
                    "warnings": load.report.warnings,
                },
                "sbom": load.sbom,
                "static_routes": load_static_routes(&load),
            });
            if let Some(report) = validation.as_ref() {
                payload["validation"] = serde_json::to_value(report)?;
            }
            println!("{}", to_sorted_json(payload)?);
        }
        InspectFormat::Human => {
            print_human(&load, validation.as_ref(), shape, &shape_notes);
        }
    }

    if validate_enabled
        && validation
            .as_ref()
            .map(|report| report.has_errors)
            .unwrap_or(false)
    {
        bail!("pack validation failed");
    }

    Ok(())
}

fn to_sorted_json(value: Value) -> Result<String> {
    let sorted = sort_json(value);
    Ok(serde_json::to_string_pretty(&sorted)?)
}

pub(crate) fn sort_json(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut sorted = serde_json::Map::new();
            for (key, value) in entries {
                sorted.insert(key, sort_json(value));
            }
            Value::Object(sorted)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(sort_json).collect()),
        other => other,
    }
}

fn run_flow_doctors(
    load: &PackLoad,
    diagnostics: &mut Vec<Diagnostic>,
    build_mode: PackBuildMode,
) -> Result<bool> {
    if load.manifest.flows.is_empty() {
        return Ok(false);
    }

    let mut has_errors = false;

    for flow in &load.manifest.flows {
        let Some(bytes) = load.files.get(&flow.file_yaml) else {
            if build_mode == PackBuildMode::Prod {
                continue;
            }
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "PACK_FLOW_DOCTOR_MISSING_FLOW".to_string(),
                message: "flow file missing from pack".to_string(),
                path: Some(flow.file_yaml.clone()),
                hint: Some("rebuild the pack to include flow sources".to_string()),
                data: Value::Null,
            });
            has_errors = true;
            continue;
        };

        match run_flow_doctor(bytes)? {
            FlowDoctorOutcome::Ok => {}
            FlowDoctorOutcome::Failed { data } => {
                has_errors = true;
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "PACK_FLOW_DOCTOR_FAILED".to_string(),
                    message: "flow doctor failed".to_string(),
                    path: Some(flow.file_yaml.clone()),
                    hint: Some("run `greentic-flow doctor` for details".to_string()),
                    data,
                });
            }
            // The cause applies to every flow, not just this one — report once
            // and stop asking.
            FlowDoctorOutcome::Unavailable {
                message,
                hint,
                data,
            } => {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warn,
                    code: "PACK_FLOW_DOCTOR_UNAVAILABLE".to_string(),
                    message: message.to_string(),
                    path: None,
                    hint: Some(hint.to_string()),
                    data,
                });
                return Ok(false);
            }
        }
    }

    Ok(has_errors)
}

fn run_component_doctors(load: &PackLoad, diagnostics: &mut Vec<Diagnostic>) -> Result<bool> {
    if load.manifest.components.is_empty() {
        return Ok(false);
    }

    let temp = TempDir::new().context("allocate temp dir for component doctor")?;
    let mut has_errors = false;

    let mut manifest_paths = std::collections::HashMap::new();
    if let Some(gpack_manifest) = load.gpack_manifest.as_ref()
        && let Some(manifest_extension) = gpack_manifest
            .extensions
            .as_ref()
            .and_then(|map| map.get(EXT_COMPONENT_MANIFEST_INDEX_V1))
            .and_then(|entry| entry.inline.as_ref())
            .and_then(|inline| match inline {
                PackManifestExtensionInline::Other(value) => Some(value),
                _ => None,
            })
            .and_then(|value| ComponentManifestIndexV1::from_extension_value(value).ok())
    {
        for entry in manifest_extension.entries {
            manifest_paths.insert(entry.component_id, entry.manifest_file);
        }
    }

    for component in &load.manifest.components {
        let Some(wasm_bytes) = load.files.get(&component.file_wasm) else {
            diagnostics.push(Diagnostic {
                severity: Severity::Warn,
                code: "PACK_COMPONENT_DOCTOR_MISSING_WASM".to_string(),
                message: "component wasm missing from pack; skipping component doctor".to_string(),
                path: Some(component.file_wasm.clone()),
                hint: Some("rebuild with --bundle=cache or supply cached artifacts".to_string()),
                data: Value::Null,
            });
            continue;
        };

        if component.manifest_file.is_none() {
            if manifest_paths.contains_key(&component.name) {
                continue;
            }
            diagnostics.push(component_manifest_missing_diag(&component.manifest_file));
            continue;
        }

        let manifest_bytes = if let Some(path) = component.manifest_file.as_deref()
            && let Some(bytes) = load.files.get(path)
        {
            bytes.clone()
        } else {
            diagnostics.push(component_manifest_missing_diag(&component.manifest_file));
            continue;
        };

        let component_dir = temp.path().join(sanitize_component_id(&component.name));
        fs::create_dir_all(&component_dir)
            .with_context(|| format!("create temp dir for {}", component.name))?;
        let wasm_path = component_dir.join("component.wasm");
        let manifest_value = match serde_json::from_slice::<Value>(&manifest_bytes) {
            Ok(value) => value,
            Err(_) => match serde_cbor::from_slice::<Value>(&manifest_bytes) {
                Ok(value) => value,
                Err(err) => {
                    diagnostics.push(component_manifest_missing_diag(&component.manifest_file));
                    tracing::debug!(
                        manifest = %component.name,
                        "failed to parse component manifest for doctor: {err}"
                    );
                    continue;
                }
            },
        };

        if !component_manifest_has_required_fields(&manifest_value) {
            diagnostics.push(component_manifest_missing_diag(&component.manifest_file));
            continue;
        }

        let manifest_bytes =
            serde_json::to_vec_pretty(&manifest_value).context("serialize component manifest")?;

        let manifest_path = component_dir.join("component.manifest.json");
        fs::write(&wasm_path, wasm_bytes)?;
        fs::write(&manifest_path, manifest_bytes)?;

        let component_bin = crate::external_tools::resolve("greentic-component")
            .unwrap_or_else(|| PathBuf::from("greentic-component"));
        let output = match Command::new(&component_bin)
            .args(["doctor"])
            .arg(&wasm_path)
            .args(["--manifest"])
            .arg(&manifest_path)
            .output()
        {
            Ok(output) => output,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warn,
                    code: "PACK_COMPONENT_DOCTOR_UNAVAILABLE".to_string(),
                    message: "greentic-component not available; skipping component doctor checks"
                        .to_string(),
                    path: None,
                    hint: Some(
                        "install greentic-component or pass --no-component-doctor".to_string(),
                    ),
                    data: Value::Null,
                });
                return Ok(false);
            }
            Err(err) => {
                return Err(err).with_context(|| format!("run {} doctor", component_bin.display()));
            }
        };

        if !output.status.success() {
            has_errors = true;
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "PACK_COMPONENT_DOCTOR_FAILED".to_string(),
                message: "component doctor failed".to_string(),
                path: Some(component.name.clone()),
                hint: Some("run `greentic-component doctor` for details".to_string()),
                data: json_diagnostic_data(&output),
            });
        }
    }

    Ok(has_errors)
}

fn component_manifest_missing_diag(manifest_file: &Option<String>) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warn,
        code: "PACK_COMPONENT_DOCTOR_MISSING_MANIFEST".to_string(),
        message: "component manifest missing or incomplete; skipping component doctor".to_string(),
        path: manifest_file.clone(),
        hint: Some("rebuild the pack to include component manifests".to_string()),
        data: Value::Null,
    }
}

fn component_manifest_has_required_fields(manifest: &Value) -> bool {
    manifest.get("name").is_some()
        && manifest.get("artifacts").is_some()
        && manifest.get("hashes").is_some()
        && manifest.get("describe_export").is_some()
        && manifest.get("config_schema").is_some()
}

fn sanitize_component_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn inspect_pack_file(path: &Path) -> Result<PackLoad> {
    let load = open_pack(path, SigningPolicy::DevOk)
        .map_err(|err| anyhow!(err.message))
        .with_context(|| format!("failed to open pack {}", path.display()))?;
    Ok(load)
}

fn detect_pack_build_mode(load: &PackLoad) -> PackBuildMode {
    if let Some(manifest) = load.gpack_manifest.as_ref()
        && let Some(mode) = manifest_build_mode(manifest)
    {
        return mode;
    }
    if load.files.keys().any(|path| path.ends_with(".ygtc")) {
        return PackBuildMode::Dev;
    }
    PackBuildMode::Prod
}

fn manifest_build_mode(manifest: &PackManifest) -> Option<PackBuildMode> {
    let extensions = manifest.extensions.as_ref()?;
    let entry = extensions.get(EXT_BUILD_MODE_ID)?;
    let inline = entry.inline.as_ref()?;
    if let PackManifestExtensionInline::Other(value) = inline
        && let Some(mode) = value.get("mode").and_then(|value| value.as_str())
    {
        if mode.eq_ignore_ascii_case("dev") {
            return Some(PackBuildMode::Dev);
        }
        return Some(PackBuildMode::Prod);
    }
    None
}

fn find_forbidden_source_paths(files: &HashMap<String, Vec<u8>>) -> Vec<String> {
    files
        .keys()
        .filter(|path| is_forbidden_source_path(path))
        .cloned()
        .collect()
}

fn is_forbidden_source_path(path: &str) -> bool {
    if matches!(path, "pack.yaml" | "pack.manifest.json") {
        return true;
    }
    if matches!(
        path,
        "secret-requirements.json" | "secrets_requirements.json"
    ) {
        return true;
    }
    if path.ends_with(".ygtc") {
        return true;
    }
    if path.starts_with("flows/") && path.ends_with(".json") {
        return true;
    }
    // Component manifest sources live under `components/<id>/component.manifest.json`
    // or `components/<id>.manifest.json`. Earlier the rule was a blanket
    // `path.ends_with("manifest.json")`, which incorrectly flagged generic asset
    // index files like `assets/i18n/_manifest.json` as forbidden.
    if path.starts_with("components/")
        && (path.ends_with("/component.manifest.json") || path.ends_with(".manifest.json"))
    {
        return true;
    }
    false
}

enum InspectMode {
    Archive(PathBuf),
    Source(PathBuf),
}

fn resolve_mode(args: &InspectArgs) -> Result<InspectMode> {
    if args.archive && args.source {
        bail!("--archive and --source are mutually exclusive");
    }
    if args.pack.is_some() && args.input.is_some() {
        bail!("exactly one of --pack or --in may be supplied");
    }

    if let Some(path) = &args.pack {
        return Ok(InspectMode::Archive(path.clone()));
    }
    if let Some(path) = &args.input {
        return Ok(InspectMode::Source(path.clone()));
    }
    if let Some(path) = &args.path {
        let meta =
            fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
        if args.archive || (path.extension() == Some(std::ffi::OsStr::new("gtpack"))) {
            return Ok(InspectMode::Archive(path.clone()));
        }
        if args.source || meta.is_dir() {
            return Ok(InspectMode::Source(path.clone()));
        }
        if meta.is_file() {
            return Ok(InspectMode::Archive(path.clone()));
        }
    }
    Ok(InspectMode::Source(
        std::env::current_dir().context("determine current directory")?,
    ))
}

fn source_mode_pack_dir(mode: &InspectMode) -> Option<&Path> {
    match mode {
        InspectMode::Source(path) => Some(path.as_path()),
        InspectMode::Archive(_) => None,
    }
}

async fn inspect_source_dir(
    dir: &Path,
    runtime: &RuntimeContext,
    allow_oci_tags: bool,
) -> Result<PackLoad> {
    let pack_dir = dir
        .canonicalize()
        .with_context(|| format!("failed to resolve pack dir {}", dir.display()))?;

    let temp = TempDir::new().context("failed to allocate temp dir for inspect")?;
    let manifest_out = temp.path().join("manifest.cbor");
    let gtpack_out = temp.path().join("pack.gtpack");

    let opts = build::BuildOptions {
        pack_dir,
        component_out: None,
        manifest_out,
        sbom_out: None,
        gtpack_out: Some(gtpack_out.clone()),
        lock_path: gtpack_out.with_extension("lock.json"), // use temp lock path under temp dir
        bundle: build::BundleMode::Cache,
        dry_run: false,
        secrets_req: None,
        default_secret_scope: None,
        allow_oci_tags,
        require_component_manifests: false,
        no_extra_dirs: false,
        dev: true,
        runtime: runtime.clone(),
        skip_update: false,
        allow_pack_schema: false,
        validate_extension_refs: false,
    };

    build::run(&opts).await?;

    inspect_pack_file(&gtpack_out)
}

fn print_human(
    load: &PackLoad,
    validation: Option<&ValidationOutput>,
    shape: PackArchiveShape,
    shape_notes: &[String],
) {
    println!(
        "Pack shape: {}{}",
        shape.describe(),
        canonical_dw_sidecar_note(load)
    );
    for note in shape_notes {
        println!("warning: {note}");
    }
    let manifest = &load.manifest;
    let report = &load.report;
    println!(
        "Pack: {} ({})",
        manifest.meta.pack_id, manifest.meta.version
    );
    println!("Name: {}", manifest.meta.name);
    println!("Flows: {}", manifest.flows.len());
    if manifest.flows.is_empty() {
        println!("Flows list: none");
    } else {
        println!("Flows list:");
        for flow in &manifest.flows {
            println!(
                "  - {} (entry: {}, kind: {})",
                flow.id, flow.entry, flow.kind
            );
        }
    }
    println!("Components: {}", manifest.components.len());
    if manifest.components.is_empty() {
        println!("Components list: none");
    } else {
        println!("Components list:");
        for component in &manifest.components {
            println!("  - {} ({})", component.name, component.version);
        }
    }
    if let Some(gmanifest) = load.gpack_manifest.as_ref()
        && let Some(value) = gmanifest
            .extensions
            .as_ref()
            .and_then(|m| m.get(EXT_COMPONENT_SOURCES_V1))
            .and_then(|ext| ext.inline.as_ref())
            .and_then(|inline| match inline {
                greentic_types::ExtensionInline::Other(v) => Some(v),
                _ => None,
            })
        && let Ok(cs) = ComponentSourcesV1::from_extension_value(value)
    {
        let mut inline = 0usize;
        let mut remote = 0usize;
        let mut oci = 0usize;
        let mut repo = 0usize;
        let mut store = 0usize;
        let mut file = 0usize;
        for entry in &cs.components {
            match entry.artifact {
                ArtifactLocationV1::Inline { .. } => inline += 1,
                ArtifactLocationV1::Remote => remote += 1,
            }
            match entry.source {
                ComponentSourceRef::Oci(_) => oci += 1,
                ComponentSourceRef::Repo(_) => repo += 1,
                ComponentSourceRef::Store(_) => store += 1,
                ComponentSourceRef::File(_) => file += 1,
            }
        }
        println!(
            "Component sources: {} total (origins: oci {}, repo {}, store {}, file {}; artifacts: inline {}, remote {})",
            cs.components.len(),
            oci,
            repo,
            store,
            file,
            inline,
            remote
        );
        if cs.components.is_empty() {
            println!("Component source entries: none");
        } else {
            println!("Component source entries:");
            for entry in &cs.components {
                println!(
                    "  - {} source={} artifact={}",
                    entry.name,
                    format_component_source(&entry.source),
                    format_component_artifact(&entry.artifact)
                );
            }
        }
    } else {
        println!("Component sources: none");
    }

    if let Some(gmanifest) = load.gpack_manifest.as_ref() {
        let providers = providers_from_manifest(gmanifest);
        if providers.is_empty() {
            println!("Providers: none");
        } else {
            println!("Providers:");
            for provider in providers {
                println!(
                    "  - {} ({}) {}",
                    provider.provider_type,
                    provider_kind(&provider),
                    summarize_provider(&provider)
                );
            }
        }
    } else {
        println!("Providers: none");
    }

    let static_routes = load_static_routes(load);
    if static_routes.is_empty() {
        println!("Static routes: none");
    } else {
        println!("Static routes:");
        for route in &static_routes {
            println!(
                "  - {} -> {} [{}]",
                route.id, route.public_path, route.source_root
            );
            println!(
                "    scope: tenant={} team={}",
                route.scope.tenant, route.scope.team
            );
            println!(
                "    index_file: {}",
                route.index_file.as_deref().unwrap_or("none")
            );
            println!(
                "    spa_fallback: {}",
                route.spa_fallback.as_deref().unwrap_or("none")
            );
            println!(
                "    cache: {}",
                route
                    .cache
                    .as_ref()
                    .map(|cache| match cache.max_age_seconds {
                        Some(max_age) => format!("{} ({max_age}s)", cache.strategy),
                        None => cache.strategy.clone(),
                    })
                    .unwrap_or_else(|| "none".to_string())
            );
            if route.exports.is_empty() {
                println!("    exports: none");
            } else {
                let exports = route
                    .exports
                    .iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("    exports: {exports}");
            }
        }
    }

    if !report.warnings.is_empty() {
        println!("Warnings:");
        for warning in &report.warnings {
            println!("  - {}", warning);
        }
    }

    if let Some(report) = validation {
        print_validation(report);
    }
}

fn load_static_routes(load: &PackLoad) -> Vec<StaticRouteV1> {
    load.gpack_manifest
        .as_ref()
        .and_then(|manifest| {
            parse_static_routes_extension(&manifest.extensions)
                .ok()
                .flatten()
        })
        .map(|payload| payload.routes)
        .unwrap_or_default()
}

#[derive(Clone, Debug, Serialize)]
struct ValidationOutput {
    #[serde(flatten)]
    report: ValidationReport,
    has_errors: bool,
    sources: Vec<crate::validator::ValidatorSourceReport>,
}

fn has_error_diagnostics(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diag| matches!(diag.severity, Severity::Error))
}

async fn run_pack_validation(
    load: &PackLoad,
    source_pack_dir: Option<&Path>,
    args: &InspectArgs,
    runtime: &RuntimeContext,
) -> Result<ValidationOutput> {
    let ctx = ValidateCtx::from_pack_load(load);
    let validators: Vec<Box<dyn greentic_types::validate::PackValidator>> = vec![
        Box::new(ReferencedFilesExistValidator::new(ctx.clone())),
        Box::new(SbomConsistencyValidator::new(ctx.clone())),
        Box::new(ProviderReferencesExistValidator::new(ctx.clone())),
        Box::new(SecretRequirementsValidator),
        Box::new(StaticRoutesValidator::new(ctx.clone())),
        Box::new(ComponentReferencesExistValidator),
        Box::new(OauthCapabilityRequirementsValidator),
    ];

    let mut report = if let Some(manifest) = load.gpack_manifest.as_ref() {
        run_validators(manifest, &ctx, &validators)
    } else {
        ValidationReport {
            pack_id: None,
            pack_version: None,
            diagnostics: vec![Diagnostic {
                severity: Severity::Warn,
                code: "PACK_MANIFEST_UNSUPPORTED".to_string(),
                message: "Pack manifest is not in the greentic-types format; skipping validation."
                    .to_string(),
                path: Some("manifest.cbor".to_string()),
                hint: Some(
                    "Rebuild the pack with greentic-pack build to enable validation.".to_string(),
                ),
                data: Value::Null,
            }],
        }
    };

    let config = ValidatorConfig {
        validators_root: args.validators_root.clone(),
        validator_packs: args.validator_pack.clone(),
        validator_allow: args.validator_allow.clone(),
        validator_cache_dir: args.validator_cache_dir.clone(),
        policy: args.validator_policy,
        local_validators: parse_validator_wasm_args(&args.validator_wasm)?,
    };

    let wasm_result = run_wasm_validators(load, &config, runtime).await?;
    report.diagnostics.extend(wasm_result.diagnostics);
    if let Some(pack_dir) = source_pack_dir {
        report
            .diagnostics
            .extend(collect_extension_dependency_diagnostics(pack_dir));
    }

    let has_errors = has_error_diagnostics(&report.diagnostics) || wasm_result.missing_required;

    Ok(ValidationOutput {
        report,
        has_errors,
        sources: wasm_result.sources,
    })
}

fn collect_extension_dependency_diagnostics(pack_dir: &Path) -> Vec<Diagnostic> {
    let source_path = default_extensions_file_path(pack_dir);
    let lock_path = default_extensions_lock_file_path(pack_dir);
    let mut diagnostics = Vec::new();

    let source = if source_path.exists() {
        match read_extensions_file(&source_path) {
            Ok(file) => Some(file),
            Err(err) => {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "PACK_EXTENSION_DEPENDENCY_SOURCE_INVALID".to_string(),
                    message: err.to_string(),
                    path: Some(path_display(pack_dir, &source_path)),
                    hint: Some("fix pack.extensions.json and rerun doctor".to_string()),
                    data: Value::Null,
                });
                None
            }
        }
    } else {
        None
    };

    let lock = if lock_path.exists() {
        match read_extensions_lock_file(&lock_path) {
            Ok(file) => Some(file),
            Err(err) => {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "PACK_EXTENSION_DEPENDENCY_LOCK_INVALID".to_string(),
                    message: err.to_string(),
                    path: Some(path_display(pack_dir, &lock_path)),
                    hint: Some("rerun `greentic-pack extensions-lock --in <DIR>`".to_string()),
                    data: Value::Null,
                });
                None
            }
        }
    } else {
        None
    };

    match (source.as_ref(), lock.as_ref()) {
        (Some(_), None) => diagnostics.push(Diagnostic {
            severity: Severity::Warn,
            code: "PACK_EXTENSION_DEPENDENCY_LOCK_MISSING".to_string(),
            message: "pack.extensions.json exists but pack.extensions.lock.json is missing"
                .to_string(),
            path: Some(path_display(pack_dir, &source_path)),
            hint: Some("run `greentic-pack extensions-lock --in <DIR>`".to_string()),
            data: Value::Null,
        }),
        (None, Some(_)) => diagnostics.push(Diagnostic {
            severity: Severity::Warn,
            code: "PACK_EXTENSION_DEPENDENCY_SOURCE_MISSING".to_string(),
            message: "pack.extensions.lock.json exists but pack.extensions.json is missing"
                .to_string(),
            path: Some(path_display(pack_dir, &lock_path)),
            hint: Some(
                "restore pack.extensions.json or regenerate the lock from the intended source file"
                    .to_string(),
            ),
            data: Value::Null,
        }),
        (Some(source), Some(lock)) => {
            if let Err(err) = validate_extensions_lock_alignment(source, lock) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "PACK_EXTENSION_DEPENDENCY_LOCK_STALE".to_string(),
                    message: err.to_string(),
                    path: Some(path_display(pack_dir, &lock_path)),
                    hint: Some("rerun `greentic-pack extensions-lock --in <DIR>` after editing pack.extensions.json".to_string()),
                    data: Value::Null,
                });
            }
        }
        (None, None) => {}
    }

    if let Some(source) = source.as_ref() {
        for extension in &source.extensions {
            if extension.id == DEPLOYER_EXTENSION_KEY && extension.role != "deployer" {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "PACK_DEPLOYER_EXTENSION_ROLE_INVALID".to_string(),
                    message: format!(
                        "extension `{}` must use role `deployer`, found `{}`",
                        extension.id, extension.role
                    ),
                    path: Some(path_display(pack_dir, &source_path)),
                    hint: Some("set the dependency role to `deployer`".to_string()),
                    data: Value::Null,
                });
            }
        }
    }

    if let Some(lock) = lock.as_ref() {
        for extension in &lock.extensions {
            if extension.media_type.is_none() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warn,
                    code: "PACK_EXTENSION_DEPENDENCY_LOCK_MISSING_MEDIA_TYPE".to_string(),
                    message: format!(
                        "extension `{}` lock entry is missing media_type metadata",
                        extension.id
                    ),
                    path: Some(path_display(pack_dir, &lock_path)),
                    hint: Some("rerun `greentic-pack extensions-lock --in <DIR>` with a resolver that reports content type".to_string()),
                    data: Value::Null,
                });
            }
            if extension.size_bytes.is_none() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warn,
                    code: "PACK_EXTENSION_DEPENDENCY_LOCK_MISSING_SIZE".to_string(),
                    message: format!(
                        "extension `{}` lock entry is missing size metadata",
                        extension.id
                    ),
                    path: Some(path_display(pack_dir, &lock_path)),
                    hint: Some("rerun `greentic-pack extensions-lock --in <DIR>` with a resolver that reports content length".to_string()),
                    data: Value::Null,
                });
            }
        }
    }

    diagnostics
}

fn path_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn print_validation(report: &ValidationOutput) {
    let (info, warn, error) = validation_counts(&report.report);
    println!("Validation:");
    println!("  Info: {info} Warn: {warn} Error: {error}");
    if report.report.diagnostics.is_empty() {
        println!("  - none");
        return;
    }
    for diag in &report.report.diagnostics {
        let sev = match diag.severity {
            Severity::Info => "INFO",
            Severity::Warn => "WARN",
            Severity::Error => "ERROR",
        };
        if let Some(path) = diag.path.as_deref() {
            println!("  - [{sev}] {} {} - {}", diag.code, path, diag.message);
        } else {
            println!("  - [{sev}] {} - {}", diag.code, diag.message);
        }
        if matches!(
            diag.code.as_str(),
            "PACK_FLOW_DOCTOR_FAILED" | "PACK_COMPONENT_DOCTOR_FAILED"
        ) {
            print_doctor_failure_details(&diag.data);
        }
        if let Some(hint) = diag.hint.as_deref() {
            println!("    hint: {hint}");
        }
    }
}

fn parse_validator_wasm_args(args: &[String]) -> Result<Vec<LocalValidator>> {
    let mut local_validators = Vec::new();
    for entry in args {
        let mut segments = entry.splitn(2, '=');
        let component_id = segments.next().unwrap_or_default().trim().to_string();
        let path = segments
            .next()
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .ok_or_else(|| {
                anyhow!(
                    "invalid --validator-wasm argument `{}` (expected format COMPONENT_ID=FILE)",
                    entry
                )
            })?;
        if component_id.is_empty() {
            return Err(anyhow!(
                "validator component id must not be empty in `{}`",
                entry
            ));
        }
        local_validators.push(LocalValidator {
            component_id,
            path: PathBuf::from(path),
        });
    }
    Ok(local_validators)
}

fn print_doctor_failure_details(data: &Value) {
    let Some(obj) = data.as_object() else {
        return;
    };
    let stdout = obj.get("stdout").and_then(|value| value.as_str());
    let stderr = obj.get("stderr").and_then(|value| value.as_str());
    let status = obj.get("status").and_then(|value| value.as_i64());
    if let Some(status) = status {
        println!("    status: {status}");
    }
    if let Some(stderr) = stderr {
        let trimmed = stderr.trim();
        if !trimmed.is_empty() {
            println!("    stderr: {trimmed}");
        }
    }
    if let Some(stdout) = stdout {
        let trimmed = stdout.trim();
        if !trimmed.is_empty() {
            println!("    stdout: {trimmed}");
        }
    }
}

fn validation_counts(report: &ValidationReport) -> (usize, usize, usize) {
    let mut info = 0;
    let mut warn = 0;
    let mut error = 0;
    for diag in &report.diagnostics {
        match diag.severity {
            Severity::Info => info += 1,
            Severity::Warn => warn += 1,
            Severity::Error => error += 1,
        }
    }
    (info, warn, error)
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum InspectFormat {
    Human,
    Json,
}

fn resolve_format(args: &InspectArgs, json: bool) -> InspectFormat {
    if json {
        InspectFormat::Json
    } else {
        args.format
    }
}

fn providers_from_manifest(manifest: &PackManifest) -> Vec<ProviderDecl> {
    let mut providers = manifest
        .provider_extension_inline()
        .map(|inline| inline.providers.clone())
        .unwrap_or_default();
    providers.sort_by(|a, b| a.provider_type.cmp(&b.provider_type));
    providers
}

fn provider_kind(provider: &ProviderDecl) -> String {
    provider
        .runtime
        .world
        .split('@')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn summarize_provider(provider: &ProviderDecl) -> String {
    let caps = provider.capabilities.len();
    let ops = provider.ops.len();
    let mut parts = vec![format!("caps:{caps}"), format!("ops:{ops}")];
    parts.push(format!("config:{}", provider.config_schema_ref));
    if let Some(docs) = provider.docs_ref.as_deref() {
        parts.push(format!("docs:{docs}"));
    }
    parts.join(" ")
}

fn format_component_source(source: &ComponentSourceRef) -> String {
    match source {
        ComponentSourceRef::Oci(value) => format_source_ref("oci", value),
        ComponentSourceRef::Repo(value) => format_source_ref("repo", value),
        ComponentSourceRef::Store(value) => format_source_ref("store", value),
        ComponentSourceRef::File(value) => format_source_ref("file", value),
    }
}

fn format_source_ref(scheme: &str, value: &str) -> String {
    if value.contains("://") {
        value.to_string()
    } else {
        format!("{scheme}://{value}")
    }
}

fn format_component_artifact(artifact: &ArtifactLocationV1) -> String {
    match artifact {
        ArtifactLocationV1::Inline { wasm_path, .. } => format!("inline ({})", wasm_path),
        ArtifactLocationV1::Remote => "remote".to_string(),
    }
}

// --- pack shape awareness ---------------------------------------------------

/// Sidecars that mark a *canonical* pack built from `kind: dw-application`.
///
/// This is the second producer of "Agentic Worker pack": `greentic-pack build`
/// emits these alongside a normal `manifest.cbor`, so such a pack is canonical,
/// not the designer's answer-doc shape. Naming them keeps the two apart at a
/// glance.
const PACKC_DW_SIDECARS: [&str; 2] = ["dw-agents.json", "secrets-policy.json"];

fn canonical_dw_sidecar_note(load: &PackLoad) -> String {
    let present: Vec<&str> = PACKC_DW_SIDECARS
        .iter()
        .copied()
        .filter(|entry| load.files.contains_key(*entry))
        .collect();
    if present.is_empty() {
        return String::new();
    }
    format!("; dw-application sidecars: {}", present.join(", "))
}

/// The replacement for `manifest.cbor missing from archive`.
///
/// Names every shape doctor knows and the entry that decides each, then lists
/// what the archive actually holds, so the operator can reach the same verdict
/// with `unzip -l`. The old message named one entry and implied it was the only
/// possibility, which is how a healthy pack came to look corrupt.
fn unrecognised_archive_message(
    path: &Path,
    entry_names: &std::collections::BTreeSet<String>,
) -> String {
    const SHOWN: usize = 8;

    let mut top_level: Vec<String> = entry_names
        .iter()
        .map(|name| match name.split_once('/') {
            Some((dir, _)) => format!("{dir}/"),
            None => name.clone(),
        })
        .collect();
    top_level.dedup();

    let found = if top_level.is_empty() {
        "  Top-level entries found: none — the archive is empty.".to_string()
    } else if top_level.len() > SHOWN {
        format!(
            "  Top-level entries found ({} of {} shown): {}",
            SHOWN,
            top_level.len(),
            top_level[..SHOWN].join(", ")
        )
    } else {
        format!("  Top-level entries found: {}", top_level.join(", "))
    };

    format!(
        "unrecognised .gtpack shape: {}\n\n\
         \x20 The archive opened as a valid ZIP but matches no pack shape doctor knows.\n\
         \x20 doctor derives the shape from the archive's top-level entries, in this order:\n\n\
         \x20   canonical pack      -> a top-level `{CANONICAL_MANIFEST_ENTRY}` entry\n\
         \x20   DW application pack -> a top-level `{DW_MANIFEST_ENTRY}` entry (greentic-designer export)\n\n\
         {found}\n\n\
         \x20 If this should be a canonical pack, rebuild it: `greentic-pack build`.\n\
         \x20 If this should be a designer export, re-export it — a DW application pack\n\
         \x20 must carry a top-level `{DW_MANIFEST_ENTRY}`.",
        path.display()
    )
}

/// Report a DW application pack.
///
/// Deliberately does not run the canonical validators, the pack-lock doctor or
/// the component doctor: none of the artefacts they read exist in this shape.
fn handle_dw_pack(
    path: &Path,
    files: &HashMap<String, Vec<u8>>,
    args: &InspectArgs,
    format: InspectFormat,
) -> Result<()> {
    let validate_enabled = if args.no_validate {
        false
    } else {
        args.validate
    };
    let report = if validate_enabled {
        check_dw_pack(files, args.flow_doctor)
    } else {
        DwPackReport::default()
    };

    match format {
        InspectFormat::Json => {
            let payload = serde_json::json!({
                "archive_shape": PackArchiveShape::DwAnswerDoc.as_slug(),
                "pack": {
                    "pack_id": report.pack_id,
                    "manifest_id": report.manifest_id,
                    "display_name": report.display_name,
                    "tenant": report.tenant,
                    "locale": report.locale,
                    "executing_flow": report.executing_flow,
                },
                "validation": {
                    "diagnostics": report.diagnostics,
                    "has_errors": report.has_errors(),
                },
            });
            println!("{}", to_sorted_json(payload)?);
        }
        InspectFormat::Human => print_dw_human(path, &report, validate_enabled, args.flow_doctor),
    }

    if validate_enabled && report.has_errors() {
        bail!("pack validation failed");
    }
    Ok(())
}

fn print_dw_human(path: &Path, report: &DwPackReport, validate_enabled: bool, flow_doctor: bool) {
    println!("Pack shape: {}", PackArchiveShape::DwAnswerDoc.describe());
    println!("File: {}", path.display());
    if let Some(pack_id) = &report.pack_id {
        println!("Pack: {pack_id}");
    }
    if let Some(manifest_id) = &report.manifest_id {
        println!("Manifest id: {manifest_id}");
    }
    if let Some(display_name) = &report.display_name {
        println!("Display name: {display_name}");
    }
    if let Some(tenant) = &report.tenant {
        println!("Tenant: {tenant}");
    }
    if let Some(locale) = &report.locale {
        println!("Locale: {locale}");
    }
    match &report.executing_flow {
        Some(flow) => println!("Executing flow: {flow}"),
        None => println!("Executing flow: none"),
    }
    if report.knowledge.is_empty() {
        println!("Knowledge: none");
    } else {
        for summary in &report.knowledge {
            println!(
                "Knowledge: {} ({}, {} asset(s))",
                summary.sidecar,
                summary.strategy.as_deref().unwrap_or("unknown strategy"),
                summary.asset_count
            );
        }
    }

    if !validate_enabled {
        println!("\nValidation disabled (--no-validate).");
        return;
    }

    let mut checked = vec![
        "manifest.json schema",
        "metadata.json",
        "declared kind",
        "knowledge sidecar asset references",
        "archive entry paths",
    ];
    // Only claim the flow was checked when there was one and checking was on —
    // an over-broad "Checked:" line is its own kind of misleading report.
    if report.executing_flow.is_some() && flow_doctor {
        checked.insert(3, "flows/main.ygtc (greentic-flow doctor)");
    }
    println!("\nChecked: {}", checked.join(", "));
    println!(
        "Not checked (not part of this pack shape): SBOM, signature, component lock, \
         component manifests, static routes"
    );

    if report.diagnostics.is_empty() {
        println!("\nOK: no problems found.");
        return;
    }
    println!("\nDiagnostics:");
    for diagnostic in &report.diagnostics {
        let severity = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warn => "warn",
            Severity::Info => "info",
        };
        let location = diagnostic
            .path
            .as_deref()
            .map(|path| format!(" [{path}]"))
            .unwrap_or_default();
        println!(
            "  {severity}: {} ({}){location}",
            diagnostic.message, diagnostic.code
        );
        if let Some(hint) = &diagnostic.hint {
            println!("    hint: {hint}");
        }
    }
    if !report.has_errors() {
        println!("\nOK: no errors (warnings and notes above are not fatal).");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn sample_args() -> InspectArgs {
        InspectArgs {
            path: None,
            pack: None,
            input: None,
            archive: false,
            source: false,
            allow_oci_tags: false,
            flow_doctor: true,
            component_doctor: true,
            format: InspectFormat::Human,
            validate: true,
            no_validate: false,
            validators_root: PathBuf::from(".greentic/validators"),
            validator_pack: Vec::new(),
            validator_wasm: Vec::new(),
            validator_allow: vec![DEFAULT_VALIDATOR_ALLOW.to_string()],
            validator_cache_dir: PathBuf::from(".greentic/cache/validators"),
            validator_policy: ValidatorPolicy::Optional,
            online: false,
            use_describe_cache: false,
        }
    }

    #[test]
    fn sort_json_orders_object_keys_recursively() {
        let value = serde_json::json!({
            "z": 1,
            "a": { "b": 2, "a": 1 },
            "list": [{ "d": 4, "c": 3 }]
        });

        let sorted = to_sorted_json(value).expect("json serialization should succeed");
        let root_a = sorted.find("\"a\"").expect("root a key");
        let root_z = sorted.find("\"z\"").expect("root z key");
        let nested_a = sorted.find("\"a\": 1").expect("nested a key");
        let nested_b = sorted.find("\"b\": 2").expect("nested b key");

        assert!(root_a < root_z, "root keys should be sorted: {sorted}");
        assert!(
            nested_a < nested_b,
            "nested keys should be sorted: {sorted}"
        );
    }

    #[test]
    fn sanitize_component_id_replaces_path_like_characters() {
        assert_eq!(
            sanitize_component_id("demo/component:beta@1"),
            "demo_component_beta_1"
        );
    }

    #[test]
    fn forbidden_source_paths_match_dev_only_inputs() {
        assert!(is_forbidden_source_path("pack.yaml"));
        assert!(is_forbidden_source_path("pack.manifest.json"));
        assert!(is_forbidden_source_path("flows/main.json"));
        assert!(is_forbidden_source_path("flows/main.ygtc"));
        assert!(is_forbidden_source_path("components/demo.manifest.json"));
        assert!(is_forbidden_source_path(
            "components/demo/component.manifest.json"
        ));
        assert!(!is_forbidden_source_path("gui/assets/index.html"));
        // Asset index files are not source-only; the bundle/runtime rely on them.
        assert!(!is_forbidden_source_path("assets/i18n/_manifest.json"));
        assert!(!is_forbidden_source_path("assets/i18n/en/_manifest.json"));
        assert!(!is_forbidden_source_path("assets/cards/_manifest.json"));
    }

    #[test]
    fn find_forbidden_source_paths_returns_only_matching_entries() {
        let files = HashMap::from([
            ("pack.yaml".to_string(), Vec::new()),
            ("flows/main.ygtc".to_string(), Vec::new()),
            ("gui/assets/index.html".to_string(), Vec::new()),
        ]);

        let forbidden = find_forbidden_source_paths(&files);
        assert_eq!(forbidden.len(), 2);
        assert!(forbidden.contains(&"pack.yaml".to_string()));
        assert!(forbidden.contains(&"flows/main.ygtc".to_string()));
    }

    #[test]
    fn resolve_mode_prefers_pack_and_input_flags() {
        let pack_args = InspectArgs {
            pack: Some(PathBuf::from("demo.gtpack")),
            ..sample_args()
        };
        let source_args = InspectArgs {
            input: Some(PathBuf::from("demo")),
            ..sample_args()
        };

        assert!(matches!(
            resolve_mode(&pack_args).expect("pack mode"),
            InspectMode::Archive(path) if path.as_path() == std::path::Path::new("demo.gtpack")
        ));
        assert!(matches!(
            resolve_mode(&source_args).expect("source mode"),
            InspectMode::Source(path) if path.as_path() == std::path::Path::new("demo")
        ));
    }

    #[test]
    fn resolve_mode_auto_detects_dir_and_gtpack_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("pack");
        let file = temp.path().join("pack.gtpack");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(&file, b"stub").expect("file");

        let dir_args = InspectArgs {
            path: Some(dir.clone()),
            ..sample_args()
        };
        let file_args = InspectArgs {
            path: Some(file.clone()),
            ..sample_args()
        };

        assert!(matches!(
            resolve_mode(&dir_args).expect("dir mode"),
            InspectMode::Source(path) if path == dir
        ));
        assert!(matches!(
            resolve_mode(&file_args).expect("file mode"),
            InspectMode::Archive(path) if path == file
        ));
    }

    #[test]
    fn parse_validator_wasm_args_rejects_missing_paths() {
        let err = parse_validator_wasm_args(&["demo.component=".to_string()])
            .expect_err("missing validator path should fail");
        assert!(
            err.to_string()
                .contains("expected format COMPONENT_ID=FILE")
        );
    }

    #[test]
    fn parse_validator_wasm_args_parses_component_pairs() {
        let validators = parse_validator_wasm_args(&[
            "demo.component=validators/demo.wasm".to_string(),
            "other.component = validators/other.wasm".to_string(),
        ])
        .expect("validator args should parse");

        assert_eq!(validators.len(), 2);
        assert_eq!(validators[0].component_id, "demo.component");
        assert_eq!(validators[1].path, PathBuf::from("validators/other.wasm"));
    }

    #[test]
    fn format_helpers_preserve_existing_schemes_and_inline_paths() {
        assert_eq!(format_source_ref("oci", "oci://example"), "oci://example");
        assert_eq!(
            format_source_ref("file", "components/demo.wasm"),
            "file://components/demo.wasm"
        );
        assert_eq!(
            format_component_artifact(&ArtifactLocationV1::Inline {
                wasm_path: "components/demo.wasm".to_string(),
                manifest_path: None,
            }),
            "inline (components/demo.wasm)"
        );
        assert_eq!(
            format_component_artifact(&ArtifactLocationV1::Remote),
            "remote"
        );
    }
}
