#![forbid(unsafe_code)]

use std::io::Write;
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use greentic_pack::validate::{
    ComponentReferencesExistValidator, ProviderReferencesExistValidator,
    ReferencedFilesExistValidator, SbomConsistencyValidator, ValidateCtx, run_validators,
};
use greentic_pack::{PackLoad, SigningPolicy, open_pack};
use greentic_types::component_source::ComponentSourceRef;
use greentic_types::pack::extensions::component_sources::{
    ArtifactLocationV1, ComponentSourcesV1, EXT_COMPONENT_SOURCES_V1,
};
use greentic_types::pack_manifest::PackManifest;
use greentic_types::provider::ProviderDecl;
use greentic_types::validate::{Diagnostic, Severity, ValidationReport};
use serde::Serialize;
use serde_cbor;
use serde_json::Value;
use tempfile::TempDir;

use crate::build;
use crate::runtime::RuntimeContext;
use crate::validator::{
    DEFAULT_VALIDATOR_ALLOW, ValidatorConfig, ValidatorPolicy, run_wasm_validators,
};

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

    /// Allowed OCI prefixes for validator refs
    #[arg(long, value_name = "PREFIX", default_value = DEFAULT_VALIDATOR_ALLOW)]
    pub validator_allow: Vec<String>,

    /// Validator cache directory
    #[arg(long, value_name = "DIR", default_value = ".greentic/cache/validators")]
    pub validator_cache_dir: PathBuf,

    /// Validator loading policy
    #[arg(long, value_enum, default_value = "optional")]
    pub validator_policy: ValidatorPolicy,
}

pub async fn handle(args: InspectArgs, json: bool, runtime: &RuntimeContext) -> Result<()> {
    let mode = resolve_mode(&args)?;
    let format = resolve_format(&args, json);
    let validate_enabled = if args.no_validate {
        false
    } else {
        args.validate
    };

    let load = match mode {
        InspectMode::Archive(path) => inspect_pack_file(&path)?,
        InspectMode::Source(path) => {
            inspect_source_dir(&path, runtime, args.allow_oci_tags).await?
        }
    };
    let validation = if validate_enabled {
        let mut output = run_pack_validation(&load, &args, runtime).await?;
        let mut doctor_diagnostics = Vec::new();
        let mut doctor_errors = false;
        if args.flow_doctor {
            doctor_errors |= run_flow_doctors(&load, &mut doctor_diagnostics)?;
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
                "manifest": load.manifest,
                "report": {
                    "signature_ok": load.report.signature_ok,
                    "sbom_ok": load.report.sbom_ok,
                    "warnings": load.report.warnings,
                },
                "sbom": load.sbom,
            });
            if let Some(report) = validation.as_ref() {
                payload["validation"] = serde_json::to_value(report)?;
            }
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        InspectFormat::Human => {
            print_human(&load, validation.as_ref());
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

fn run_flow_doctors(load: &PackLoad, diagnostics: &mut Vec<Diagnostic>) -> Result<bool> {
    if load.manifest.flows.is_empty() {
        return Ok(false);
    }

    let mut has_errors = false;

    for flow in &load.manifest.flows {
        let Some(bytes) = load.files.get(&flow.file_yaml) else {
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

        let mut command = Command::new("greentic-flow");
        command
            .args(["doctor", "--json", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warn,
                    code: "PACK_FLOW_DOCTOR_UNAVAILABLE".to_string(),
                    message: "greentic-flow not available; skipping flow doctor checks".to_string(),
                    path: None,
                    hint: Some("install greentic-flow or pass --no-flow-doctor".to_string()),
                    data: Value::Null,
                });
                return Ok(false);
            }
            Err(err) => return Err(err).context("run greentic-flow doctor"),
        };
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(bytes)
                .context("write flow content to greentic-flow stdin")?;
        }
        let output = child
            .wait_with_output()
            .context("wait for greentic-flow doctor")?;

        if !output.status.success() {
            if flow_doctor_unsupported(&output) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warn,
                    code: "PACK_FLOW_DOCTOR_UNAVAILABLE".to_string(),
                    message: "greentic-flow does not support --stdin; skipping flow doctor checks"
                        .to_string(),
                    path: None,
                    hint: Some("upgrade greentic-flow or pass --no-flow-doctor".to_string()),
                    data: json_diagnostic_data(&output),
                });
                return Ok(false);
            }
            has_errors = true;
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "PACK_FLOW_DOCTOR_FAILED".to_string(),
                message: "flow doctor failed".to_string(),
                path: Some(flow.file_yaml.clone()),
                hint: Some("run `greentic-flow doctor` for details".to_string()),
                data: json_diagnostic_data(&output),
            });
        }
    }

    Ok(has_errors)
}

fn flow_doctor_unsupported(output: &std::process::Output) -> bool {
    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(&output.stdout));
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    let combined = combined.to_lowercase();
    combined.contains("--stdin") && combined.contains("unknown")
        || combined.contains("found argument '--stdin'")
        || combined.contains("unexpected argument '--stdin'")
        || combined.contains("unrecognized option '--stdin'")
}

fn run_component_doctors(load: &PackLoad, diagnostics: &mut Vec<Diagnostic>) -> Result<bool> {
    if load.manifest.components.is_empty() {
        return Ok(false);
    }

    let temp = TempDir::new().context("allocate temp dir for component doctor")?;
    let mut has_errors = false;

    let mut manifests = std::collections::HashMap::new();
    if let Some(gpack_manifest) = load.gpack_manifest.as_ref() {
        for component in &gpack_manifest.components {
            if let Ok(bytes) = serde_json::to_vec_pretty(component) {
                manifests.insert(component.id.to_string(), bytes);
            }
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

        let manifest_bytes = if let Some(bytes) = manifests.get(&component.name) {
            Some(bytes.clone())
        } else if let Some(path) = component.manifest_file.as_deref()
            && let Some(bytes) = load.files.get(path)
        {
            Some(bytes.clone())
        } else {
            None
        };

        let Some(manifest_bytes) = manifest_bytes else {
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

        let output = match Command::new("greentic-component")
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
            Err(err) => return Err(err).context("run greentic-component doctor"),
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

fn json_diagnostic_data(output: &std::process::Output) -> Value {
    serde_json::json!({
        "status": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout).trim_end(),
        "stderr": String::from_utf8_lossy(&output.stderr).trim_end(),
    })
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
        runtime: runtime.clone(),
        skip_update: false,
    };

    build::run(&opts).await?;

    inspect_pack_file(&gtpack_out)
}

fn print_human(load: &PackLoad, validation: Option<&ValidationOutput>) {
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
    args: &InspectArgs,
    runtime: &RuntimeContext,
) -> Result<ValidationOutput> {
    let ctx = ValidateCtx::from_pack_load(load);
    let validators: Vec<Box<dyn greentic_types::validate::PackValidator>> = vec![
        Box::new(ReferencedFilesExistValidator::new(ctx.clone())),
        Box::new(SbomConsistencyValidator::new(ctx.clone())),
        Box::new(ProviderReferencesExistValidator::new(ctx.clone())),
        Box::new(ComponentReferencesExistValidator),
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
    };

    let wasm_result = run_wasm_validators(load, &config, runtime).await?;
    report.diagnostics.extend(wasm_result.diagnostics);

    let has_errors = has_error_diagnostics(&report.diagnostics) || wasm_result.missing_required;

    Ok(ValidationOutput {
        report,
        has_errors,
        sources: wasm_result.sources,
    })
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
