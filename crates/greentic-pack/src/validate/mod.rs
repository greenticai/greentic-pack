use std::collections::BTreeSet;
use std::path::PathBuf;

use greentic_types::ComponentId;
use greentic_types::pack::extensions::component_manifests::{
    ComponentManifestIndexV1, EXT_COMPONENT_MANIFEST_INDEX_V1,
};
use greentic_types::pack::extensions::component_sources::{
    ComponentSourcesV1, EXT_COMPONENT_SOURCES_V1,
};
use greentic_types::pack_manifest::{ExtensionInline, PackManifest};
use greentic_types::provider::ProviderDecl;
use greentic_types::validate::{
    Diagnostic, PackValidator, Severity, ValidationReport, validate_pack_manifest_core,
};
use serde_json::Value;

use crate::PackLoad;

#[derive(Clone, Debug, Default)]
pub struct ValidateCtx {
    pub pack_paths: BTreeSet<String>,
    pub sbom_paths: BTreeSet<String>,
    pub referenced_paths: BTreeSet<String>,
    pub pack_root: Option<PathBuf>,
}

impl ValidateCtx {
    pub fn from_pack_load(load: &PackLoad) -> Self {
        let pack_paths = load.files.keys().cloned().collect();
        let sbom_paths = load.sbom.iter().map(|entry| entry.path.clone()).collect();

        let mut referenced_paths = BTreeSet::new();
        for flow in &load.manifest.flows {
            referenced_paths.insert(flow.file_yaml.clone());
            referenced_paths.insert(flow.file_json.clone());
        }
        for component in &load.manifest.components {
            referenced_paths.insert(component.file_wasm.clone());
            if let Some(schema_file) = component.schema_file.as_ref() {
                referenced_paths.insert(schema_file.clone());
            }
            if let Some(manifest_file) = component.manifest_file.as_ref() {
                referenced_paths.insert(manifest_file.clone());
            }
        }
        if let Some(manifest) = load.gpack_manifest.as_ref()
            && let Some(value) = manifest
                .extensions
                .as_ref()
                .and_then(|exts| exts.get(EXT_COMPONENT_MANIFEST_INDEX_V1))
                .and_then(|ext| ext.inline.as_ref())
                .and_then(|inline| match inline {
                    ExtensionInline::Other(value) => Some(value),
                    _ => None,
                })
            && let Ok(index) = ComponentManifestIndexV1::from_extension_value(value)
        {
            for entry in index.entries {
                referenced_paths.insert(entry.manifest_file);
            }
        }

        Self {
            pack_paths,
            sbom_paths,
            referenced_paths,
            pack_root: None,
        }
    }
}

pub fn run_validators(
    manifest: &PackManifest,
    _ctx: &ValidateCtx,
    validators: &[Box<dyn PackValidator>],
) -> ValidationReport {
    let mut report = ValidationReport {
        pack_id: Some(manifest.pack_id.clone()),
        pack_version: Some(manifest.version.clone()),
        diagnostics: Vec::new(),
    };

    report
        .diagnostics
        .extend(validate_pack_manifest_core(manifest));

    for validator in validators {
        if validator.applies(manifest) {
            report.diagnostics.extend(validator.validate(manifest));
        }
    }

    report
}

#[derive(Clone, Debug)]
pub struct ReferencedFilesExistValidator {
    ctx: ValidateCtx,
}

impl ReferencedFilesExistValidator {
    pub fn new(ctx: ValidateCtx) -> Self {
        Self { ctx }
    }
}

impl PackValidator for ReferencedFilesExistValidator {
    fn id(&self) -> &'static str {
        "pack.referenced-files-exist"
    }

    fn applies(&self, _manifest: &PackManifest) -> bool {
        true
    }

    fn validate(&self, _manifest: &PackManifest) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        for path in &self.ctx.referenced_paths {
            let missing_in_pack = !self.ctx.pack_paths.contains(path);
            let missing_in_sbom = !self.ctx.sbom_paths.contains(path);

            if missing_in_pack || missing_in_sbom {
                let message = if missing_in_pack && missing_in_sbom {
                    "Referenced file is missing from the pack archive and SBOM."
                } else if missing_in_pack {
                    "Referenced file is missing from the pack archive."
                } else {
                    "Referenced file is missing from the SBOM."
                };
                diagnostics.push(missing_file_diagnostic(
                    "PACK_MISSING_FILE",
                    message,
                    Some(path.clone()),
                ));
            }
        }

        diagnostics
    }
}

#[derive(Clone, Debug)]
pub struct SbomConsistencyValidator {
    ctx: ValidateCtx,
}

impl SbomConsistencyValidator {
    pub fn new(ctx: ValidateCtx) -> Self {
        Self { ctx }
    }
}

impl PackValidator for SbomConsistencyValidator {
    fn id(&self) -> &'static str {
        "pack.sbom-consistency"
    }

    fn applies(&self, _manifest: &PackManifest) -> bool {
        true
    }

    fn validate(&self, _manifest: &PackManifest) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let manifest_path = "manifest.cbor";

        if !self.ctx.pack_paths.contains(manifest_path)
            || !self.ctx.sbom_paths.contains(manifest_path)
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "PACK_MISSING_MANIFEST_CBOR".to_string(),
                message: "manifest.cbor must be present in the pack and listed in the SBOM."
                    .to_string(),
                path: Some(manifest_path.to_string()),
                hint: Some(
                    "Rebuild the pack so manifest.cbor is included in the SBOM.".to_string(),
                ),
                data: Value::Null,
            });
        }

        for path in &self.ctx.sbom_paths {
            if !self.ctx.pack_paths.contains(path) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "PACK_SBOM_DANGLING_PATH".to_string(),
                    message: "SBOM entry references a path missing from the pack archive."
                        .to_string(),
                    path: Some(path.clone()),
                    hint: Some("Remove stale SBOM entries or rebuild the pack.".to_string()),
                    data: Value::Null,
                });
            }
        }

        diagnostics
    }
}

#[derive(Clone, Debug)]
pub struct ProviderReferencesExistValidator {
    ctx: ValidateCtx,
}

impl ProviderReferencesExistValidator {
    pub fn new(ctx: ValidateCtx) -> Self {
        Self { ctx }
    }
}

impl PackValidator for ProviderReferencesExistValidator {
    fn id(&self) -> &'static str {
        "pack.provider-references-exist"
    }

    fn applies(&self, manifest: &PackManifest) -> bool {
        manifest.provider_extension_inline().is_some()
            || self
                .ctx
                .pack_paths
                .contains("assets/secret-requirements.json")
    }

    fn validate(&self, manifest: &PackManifest) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        for provider in providers_from_manifest(manifest) {
            let config_path = provider.config_schema_ref.as_str();
            if !config_path.is_empty() {
                diagnostics.extend(check_pack_path(
                    &self.ctx,
                    config_path,
                    "provider config schema",
                ));
            }
            if let Some(docs_path) = provider.docs_ref.as_deref()
                && !docs_path.is_empty()
            {
                diagnostics.extend(check_pack_path(&self.ctx, docs_path, "provider docs"));
            }
        }

        let secret_requirements_path = "assets/secret-requirements.json";
        if self.ctx.pack_paths.contains(secret_requirements_path)
            && !self.ctx.sbom_paths.contains(secret_requirements_path)
        {
            diagnostics.push(missing_file_diagnostic(
                "PACK_MISSING_FILE",
                "Secret requirements asset is missing from the SBOM.",
                Some(secret_requirements_path.to_string()),
            ));
        }

        diagnostics
    }
}

#[derive(Clone, Debug, Default)]
pub struct ComponentReferencesExistValidator;

impl PackValidator for ComponentReferencesExistValidator {
    fn id(&self) -> &'static str {
        "pack.component-references-exist"
    }

    fn applies(&self, _manifest: &PackManifest) -> bool {
        true
    }

    fn validate(&self, manifest: &PackManifest) -> Vec<Diagnostic> {
        let mut known: BTreeSet<ComponentId> = BTreeSet::new();
        for component in &manifest.components {
            known.insert(component.id.clone());
        }

        let mut source_ids: BTreeSet<ComponentId> = BTreeSet::new();
        if let Some(value) = manifest
            .extensions
            .as_ref()
            .and_then(|exts| exts.get(EXT_COMPONENT_SOURCES_V1))
            .and_then(|ext| ext.inline.as_ref())
            .and_then(|inline| match inline {
                ExtensionInline::Other(value) => Some(value),
                _ => None,
            })
            && let Ok(cs) = ComponentSourcesV1::from_extension_value(value)
        {
            for entry in cs.components {
                if let Some(id) = entry.component_id {
                    source_ids.insert(id);
                }
            }
        }

        let mut diagnostics = Vec::new();
        for flow in &manifest.flows {
            for (node_id, node) in &flow.flow.nodes {
                if node.component.pack_alias.is_some() {
                    continue;
                }
                let component_id = &node.component.id;
                if !known.contains(component_id) && !source_ids.contains(component_id) {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "PACK_MISSING_COMPONENT_REFERENCE".to_string(),
                        message: format!(
                            "Flow references component '{}' missing from manifest/component sources.",
                            component_id
                        ),
                        path: Some(format!(
                            "flows.{}.nodes.{}.component",
                            flow.id.as_str(),
                            node_id.as_str()
                        )),
                        hint: Some(
                            "Add the component to the pack manifest or component sources."
                                .to_string(),
                        ),
                        data: Value::Null,
                    });
                }
            }
        }

        diagnostics
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

fn check_pack_path(ctx: &ValidateCtx, path: &str, label: &str) -> Vec<Diagnostic> {
    if path.trim().is_empty() {
        return Vec::new();
    }

    let mut diagnostics = Vec::new();
    if !ctx.pack_paths.contains(path) {
        diagnostics.push(missing_file_diagnostic(
            "PACK_MISSING_FILE",
            &format!("{label} is missing from the pack archive."),
            Some(path.to_string()),
        ));
    } else if !ctx.sbom_paths.contains(path) {
        diagnostics.push(missing_file_diagnostic(
            "PACK_MISSING_FILE",
            &format!("{label} is missing from the SBOM."),
            Some(path.to_string()),
        ));
    }

    diagnostics
}

fn missing_file_diagnostic(code: &str, message: &str, path: Option<String>) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code: code.to_string(),
        message: message.to_string(),
        path,
        hint: None,
        data: Value::Null,
    }
}
