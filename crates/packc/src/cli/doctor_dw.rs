//! Doctor checks for a DW application pack — the shape greentic-designer's
//! `orchestrate::dw_application_pack::write_gtpack` emits.
//!
//! "Has a decodable `PackManifest`" does not apply to this shape, and neither
//! do SBOM hashing, Ed25519 signatures, the pack-lock component doctor, the
//! component-manifest index, static routes, or the forbidden-source-path check:
//! none of those artefacts exist here. Reporting them as missing would be the
//! same failure mode as the message this work replaces, only quieter. The
//! checks below are the ones that are actually meaningful for the shape.
//!
//! Diagnostics reuse `greentic_types::validate::{Diagnostic, Severity}` so
//! `--format json`, severity rendering and exit codes stay uniform across pack
//! shapes.

use std::collections::{BTreeSet, HashMap};

use greentic_pack::archive_shape::{DW_MANIFEST_ENTRY, DW_METADATA_ENTRY};
use greentic_types::validate::{Diagnostic, Severity};
use serde_json::Value;

use crate::cli::flow_doctor::{FlowDoctorOutcome, run_flow_doctor};

/// The one flow entry the designer emits, and only when the answer document
/// carries an executing node.
const DW_FLOW_ENTRY: &str = "flows/main.ygtc";
/// Static-KB sidecar (Phase 2.0).
const DW_KNOWLEDGE_BASE_ENTRY: &str = "knowledge_base.json";
/// Embedding-retrieval corpus sidecar (Phase 2.1 / W5).
const DW_KNOWLEDGE_CORPUS_ENTRY: &str = "knowledge_corpus.json";
/// Directory the knowledge sidecars index into.
const DW_KNOWLEDGE_ASSET_PREFIX: &str = "assets/knowledge/";
/// The value `metadata.json` must declare when it declares anything.
const DW_DECLARED_KIND: &str = "DwApplication";

/// What a DW application pack turned out to be, for the human report.
#[derive(Debug, Default)]
pub(crate) struct DwPackReport {
    pub pack_id: Option<String>,
    pub manifest_id: Option<String>,
    pub display_name: Option<String>,
    pub tenant: Option<String>,
    pub locale: Option<String>,
    pub executing_flow: Option<String>,
    pub knowledge: Vec<DwKnowledgeSummary>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub(crate) struct DwKnowledgeSummary {
    pub sidecar: &'static str,
    pub strategy: Option<String>,
    pub asset_count: usize,
}

impl DwPackReport {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diag| matches!(diag.severity, Severity::Error))
    }
}

fn diag(
    severity: Severity,
    code: &str,
    message: impl Into<String>,
    path: Option<&str>,
    hint: impl Into<String>,
) -> Diagnostic {
    Diagnostic {
        severity,
        code: code.to_string(),
        message: message.into(),
        path: path.map(str::to_string),
        hint: Some(hint.into()),
        data: Value::Null,
    }
}

/// Run every DW check over an archive's entries.
///
/// Never short-circuits: one bad field must not hide the rest of the report.
/// `flow_doctor` mirrors the `--no-flow-doctor` switch.
pub(crate) fn check_dw_pack(files: &HashMap<String, Vec<u8>>, flow_doctor: bool) -> DwPackReport {
    let mut report = DwPackReport::default();

    check_manifest(files, &mut report);
    check_metadata(files, &mut report);
    let referenced = check_knowledge(files, &mut report);
    check_flow(files, flow_doctor, &mut report);
    check_orphan_assets(files, &referenced, &mut report);
    check_unknown_entries(files, &mut report);

    report
}

// --- D2: manifest.json -----------------------------------------------------

fn check_manifest(files: &HashMap<String, Vec<u8>>, report: &mut DwPackReport) {
    // Presence is guaranteed: `manifest.json` is what selected this shape.
    let Some(bytes) = files.get(DW_MANIFEST_ENTRY) else {
        return;
    };

    let value: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(err) => {
            report.diagnostics.push(diag(
                Severity::Error,
                "PACK_DW_MANIFEST_INVALID_JSON",
                format!("{DW_MANIFEST_ENTRY} is not valid JSON: {err}"),
                Some(DW_MANIFEST_ENTRY),
                "re-export the pack from greentic-designer",
            ));
            return;
        }
    };

    let Some(object) = value.as_object() else {
        report.diagnostics.push(diag(
            Severity::Error,
            "PACK_DW_MANIFEST_INVALID_JSON",
            format!("{DW_MANIFEST_ENTRY} must be a JSON object"),
            Some(DW_MANIFEST_ENTRY),
            "re-export the pack from greentic-designer",
        ));
        return;
    };

    match object.get("manifest_id") {
        Some(Value::String(id)) if !id.trim().is_empty() => {
            report.manifest_id = Some(id.clone());
        }
        Some(Value::String(_)) => report.diagnostics.push(diag(
            Severity::Error,
            "PACK_DW_MANIFEST_MISSING_FIELD",
            format!("{DW_MANIFEST_ENTRY} field `manifest_id` is empty"),
            Some(DW_MANIFEST_ENTRY),
            "re-export the pack; every DW pack needs a non-empty manifest_id",
        )),
        Some(other) => report.diagnostics.push(diag(
            Severity::Error,
            "PACK_DW_MANIFEST_FIELD_TYPE",
            format!(
                "{DW_MANIFEST_ENTRY} field `manifest_id` must be a string, found {}",
                value_kind(other)
            ),
            Some(DW_MANIFEST_ENTRY),
            "re-export the pack from greentic-designer",
        )),
        None => report.diagnostics.push(diag(
            Severity::Error,
            "PACK_DW_MANIFEST_MISSING_FIELD",
            format!("{DW_MANIFEST_ENTRY} is missing the required field `manifest_id`"),
            Some(DW_MANIFEST_ENTRY),
            "re-export the pack from greentic-designer",
        )),
    }

    match object.get("manifest") {
        Some(Value::Object(_)) => {}
        Some(other) => report.diagnostics.push(diag(
            Severity::Error,
            "PACK_DW_MANIFEST_FIELD_TYPE",
            format!(
                "{DW_MANIFEST_ENTRY} field `manifest` must be an object, found {}",
                value_kind(other)
            ),
            Some(DW_MANIFEST_ENTRY),
            "re-export the pack from greentic-designer",
        )),
        None => report.diagnostics.push(diag(
            Severity::Error,
            "PACK_DW_MANIFEST_MISSING_FIELD",
            format!(
                "{DW_MANIFEST_ENTRY} is missing the required field `manifest` \
                 (the composed DW manifest payload)"
            ),
            Some(DW_MANIFEST_ENTRY),
            "re-export the pack from greentic-designer",
        )),
    }

    for field in ["display_name", "locale", "tenant"] {
        match object.get(field) {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) => match field {
                "display_name" => report.display_name = Some(text.clone()),
                "locale" => report.locale = Some(text.clone()),
                _ => report.tenant = Some(text.clone()),
            },
            Some(other) => report.diagnostics.push(diag(
                Severity::Error,
                "PACK_DW_MANIFEST_FIELD_TYPE",
                format!(
                    "{DW_MANIFEST_ENTRY} field `{field}` must be a string, found {}",
                    value_kind(other)
                ),
                Some(DW_MANIFEST_ENTRY),
                "re-export the pack from greentic-designer",
            )),
        }
    }

    match object.get("provider_overrides") {
        None | Some(Value::Null) | Some(Value::Object(_)) => {}
        Some(other) => report.diagnostics.push(diag(
            Severity::Error,
            "PACK_DW_MANIFEST_FIELD_TYPE",
            format!(
                "{DW_MANIFEST_ENTRY} field `provider_overrides` must be an object, found {}",
                value_kind(other)
            ),
            Some(DW_MANIFEST_ENTRY),
            "re-export the pack from greentic-designer",
        )),
    }
}

// --- D3: metadata.json -----------------------------------------------------

fn check_metadata(files: &HashMap<String, Vec<u8>>, report: &mut DwPackReport) {
    let Some(bytes) = files.get(DW_METADATA_ENTRY) else {
        // Not a detection failure: the designer writes this unconditionally, so
        // its absence is a broken DW pack rather than an unknown artefact, and
        // saying so is far more actionable than "unrecognised archive".
        report.diagnostics.push(diag(
            Severity::Error,
            "PACK_DW_METADATA_MISSING",
            format!("{DW_METADATA_ENTRY} missing; a DW application pack must carry one"),
            Some(DW_METADATA_ENTRY),
            "re-export the pack from greentic-designer",
        ));
        return;
    };

    let value: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(err) => {
            report.diagnostics.push(diag(
                Severity::Error,
                "PACK_DW_METADATA_MISSING",
                format!("{DW_METADATA_ENTRY} is not valid JSON: {err}"),
                Some(DW_METADATA_ENTRY),
                "re-export the pack from greentic-designer",
            ));
            return;
        }
    };

    let Some(object) = value.as_object() else {
        report.diagnostics.push(diag(
            Severity::Error,
            "PACK_DW_METADATA_MISSING",
            format!("{DW_METADATA_ENTRY} must be a JSON object"),
            Some(DW_METADATA_ENTRY),
            "re-export the pack from greentic-designer",
        ));
        return;
    };

    match object.get("pack_id") {
        Some(Value::String(id)) if !id.trim().is_empty() => report.pack_id = Some(id.clone()),
        _ => report.diagnostics.push(diag(
            Severity::Error,
            "PACK_DW_METADATA_MISSING_FIELD",
            format!("{DW_METADATA_ENTRY} needs a non-empty string field `pack_id`"),
            Some(DW_METADATA_ENTRY),
            "re-export the pack from greentic-designer",
        )),
    }

    // The declared kind is advisory: the archive's contents already decided the
    // shape. It only has to agree. Absence is fine — packs built before the
    // field existed, and any future producer that omits it, stay valid.
    match object.get("kind") {
        None | Some(Value::Null) => {}
        Some(Value::String(kind)) if kind == DW_DECLARED_KIND => {}
        Some(other) => report.diagnostics.push(diag(
            Severity::Error,
            "PACK_DW_KIND_MISMATCH",
            format!(
                "{DW_METADATA_ENTRY} declares kind {} but the archive's contents say \
                 {DW_DECLARED_KIND} (it carries `{DW_MANIFEST_ENTRY}`)",
                render_scalar(other)
            ),
            Some(DW_METADATA_ENTRY),
            "the producer wrote a kind that disagrees with what it packed; re-export the pack",
        )),
    }

    if let Some(created_at) = object.get("created_at") {
        let parsed = created_at.as_str().is_some_and(|text| {
            time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
                .is_ok()
        });
        if !parsed {
            report.diagnostics.push(diag(
                Severity::Warn,
                "PACK_DW_METADATA_TIMESTAMP",
                format!(
                    "{DW_METADATA_ENTRY} field `created_at` is not an RFC 3339 timestamp: {}",
                    render_scalar(created_at)
                ),
                Some(DW_METADATA_ENTRY),
                "informational only; the pack is still usable",
            ));
        }
    }
}

// --- D5: knowledge sidecars ------------------------------------------------

/// Returns every asset path the sidecars claim, so [`check_orphan_assets`] can
/// report the reverse direction.
fn check_knowledge(
    files: &HashMap<String, Vec<u8>>,
    report: &mut DwPackReport,
) -> BTreeSet<String> {
    let mut referenced = BTreeSet::new();

    for sidecar in [DW_KNOWLEDGE_BASE_ENTRY, DW_KNOWLEDGE_CORPUS_ENTRY] {
        let Some(bytes) = files.get(sidecar) else {
            continue;
        };

        let value: Value = match serde_json::from_slice(bytes) {
            Ok(value) => value,
            Err(err) => {
                report.diagnostics.push(diag(
                    Severity::Error,
                    "PACK_DW_KNOWLEDGE_INVALID_JSON",
                    format!("{sidecar} is not valid JSON: {err}"),
                    Some(sidecar),
                    "re-export the pack from greentic-designer",
                ));
                continue;
            }
        };

        let strategy = value
            .get("strategy")
            .and_then(Value::as_str)
            .map(str::to_string);

        let entries = value.get("files").and_then(Value::as_array);
        let Some(entries) = entries else {
            report.diagnostics.push(diag(
                Severity::Error,
                "PACK_DW_KNOWLEDGE_INVALID_JSON",
                format!("{sidecar} must carry a `files` array"),
                Some(sidecar),
                "re-export the pack from greentic-designer",
            ));
            continue;
        };

        let mut asset_count = 0usize;
        for entry in entries {
            for field in ["asset_path", "vectors_asset_path"] {
                let Some(path) = entry.get(field).and_then(Value::as_str) else {
                    continue;
                };
                referenced.insert(path.to_string());
                if field == "asset_path" {
                    asset_count += 1;
                }
                if !files.contains_key(path) {
                    report.diagnostics.push(diag(
                        Severity::Error,
                        "PACK_DW_KNOWLEDGE_DANGLING_ASSET",
                        format!("{sidecar} references `{path}`, which is not in the archive"),
                        Some(sidecar),
                        "re-export the pack; the knowledge index and its assets disagree",
                    ));
                }
            }
        }

        report.knowledge.push(DwKnowledgeSummary {
            sidecar,
            strategy,
            asset_count,
        });
    }

    referenced
}

// --- D6: orphan assets -----------------------------------------------------

fn check_orphan_assets(
    files: &HashMap<String, Vec<u8>>,
    referenced: &BTreeSet<String>,
    report: &mut DwPackReport,
) {
    let mut orphans: Vec<&String> = files
        .keys()
        .filter(|path| path.starts_with(DW_KNOWLEDGE_ASSET_PREFIX))
        .filter(|path| !referenced.contains(*path))
        .collect();
    orphans.sort();

    for path in orphans {
        report.diagnostics.push(diag(
            Severity::Warn,
            "PACK_DW_KNOWLEDGE_ORPHAN_ASSET",
            format!("`{path}` is in the archive but no knowledge sidecar references it"),
            Some(path),
            "harmless, but it will never be read; re-export to drop it",
        ));
    }
}

// --- D4: the executing flow ------------------------------------------------

fn check_flow(files: &HashMap<String, Vec<u8>>, flow_doctor: bool, report: &mut DwPackReport) {
    let Some(bytes) = files.get(DW_FLOW_ENTRY) else {
        report.diagnostics.push(diag(
            Severity::Info,
            "PACK_DW_NO_EXECUTING_FLOW",
            format!("no {DW_FLOW_ENTRY} in the archive; this pack declares no executing node"),
            None,
            "expected when the answer document has no agent_graph or operala.call node",
        ));
        return;
    };
    report.executing_flow = Some(DW_FLOW_ENTRY.to_string());

    if !flow_doctor {
        return;
    }

    match run_flow_doctor(bytes) {
        Ok(FlowDoctorOutcome::Ok) => {}
        Ok(FlowDoctorOutcome::Failed { data }) => report.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "PACK_FLOW_DOCTOR_FAILED".to_string(),
            message: "flow doctor failed".to_string(),
            path: Some(DW_FLOW_ENTRY.to_string()),
            hint: Some("run `greentic-flow doctor` for details".to_string()),
            data,
        }),
        Ok(FlowDoctorOutcome::Unavailable {
            message,
            hint,
            data,
        }) => report.diagnostics.push(Diagnostic {
            severity: Severity::Warn,
            code: "PACK_FLOW_DOCTOR_UNAVAILABLE".to_string(),
            message: message.to_string(),
            path: None,
            hint: Some(hint.to_string()),
            data,
        }),
        Err(err) => report.diagnostics.push(diag(
            Severity::Warn,
            "PACK_FLOW_DOCTOR_UNAVAILABLE",
            format!("could not run greentic-flow doctor: {err}"),
            None,
            "install greentic-flow or pass --no-flow-doctor",
        )),
    }
}

// --- D7: unknown entries ---------------------------------------------------

fn check_unknown_entries(files: &HashMap<String, Vec<u8>>, report: &mut DwPackReport) {
    let mut unknown: Vec<&String> = files
        .keys()
        .filter(|path| !is_known_dw_entry(path))
        .collect();
    unknown.sort();

    for path in unknown {
        // Info, deliberately: the designer adds sidecars over time (KB in Phase
        // 2.0, corpus in Phase 2.1). A pack built by a newer designer must not
        // fail against an older packc.
        report.diagnostics.push(diag(
            Severity::Info,
            "PACK_DW_UNKNOWN_ENTRY",
            format!("`{path}` is not a DW application pack entry this version knows"),
            Some(path),
            "ignored; it may come from a newer greentic-designer",
        ));
    }
}

fn is_known_dw_entry(path: &str) -> bool {
    matches!(
        path,
        DW_MANIFEST_ENTRY
            | DW_METADATA_ENTRY
            | DW_KNOWLEDGE_BASE_ENTRY
            | DW_KNOWLEDGE_CORPUS_ENTRY
            | DW_FLOW_ENTRY
    ) || path.starts_with(DW_KNOWLEDGE_ASSET_PREFIX)
}

// --- shared helpers --------------------------------------------------------

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn render_scalar(value: &Value) -> String {
    match value {
        Value::String(text) => format!("\"{text}\""),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(entries: &[(&str, &str)]) -> HashMap<String, Vec<u8>> {
        entries
            .iter()
            .map(|(name, body)| ((*name).to_string(), body.as_bytes().to_vec()))
            .collect()
    }

    fn valid_manifest() -> &'static str {
        r#"{"manifest_id":"support-triage","display_name":"Support Triage","manifest":{"agents":{}}}"#
    }

    fn valid_metadata() -> &'static str {
        r#"{"pack_id":"pack.dw.support-triage.9f2a","kind":"DwApplication",
            "created_at":"2026-08-20T06:00:00Z","source":"designer/research/dw_application_pack"}"#
    }

    fn codes(report: &DwPackReport) -> Vec<&str> {
        report
            .diagnostics
            .iter()
            .map(|diag| diag.code.as_str())
            .collect()
    }

    #[test]
    fn clean_dw_pack_has_no_error_diagnostics() {
        let report = check_dw_pack(
            &files(&[
                (DW_MANIFEST_ENTRY, valid_manifest()),
                (DW_METADATA_ENTRY, valid_metadata()),
            ]),
            false,
        );

        assert!(!report.has_errors(), "unexpected: {:?}", report.diagnostics);
        assert_eq!(report.manifest_id.as_deref(), Some("support-triage"));
        assert_eq!(report.display_name.as_deref(), Some("Support Triage"));
        assert_eq!(
            report.pack_id.as_deref(),
            Some("pack.dw.support-triage.9f2a")
        );
    }

    #[test]
    fn manifest_json_not_json_is_error() {
        let report = check_dw_pack(
            &files(&[
                (DW_MANIFEST_ENTRY, "{not json"),
                (DW_METADATA_ENTRY, valid_metadata()),
            ]),
            false,
        );

        assert!(codes(&report).contains(&"PACK_DW_MANIFEST_INVALID_JSON"));
    }

    #[test]
    fn manifest_id_missing_is_error() {
        let report = check_dw_pack(
            &files(&[
                (DW_MANIFEST_ENTRY, r#"{"manifest":{}}"#),
                (DW_METADATA_ENTRY, valid_metadata()),
            ]),
            false,
        );

        assert!(codes(&report).contains(&"PACK_DW_MANIFEST_MISSING_FIELD"));
    }

    #[test]
    fn manifest_id_empty_is_error() {
        let report = check_dw_pack(
            &files(&[
                (DW_MANIFEST_ENTRY, r#"{"manifest_id":"  ","manifest":{}}"#),
                (DW_METADATA_ENTRY, valid_metadata()),
            ]),
            false,
        );

        assert!(codes(&report).contains(&"PACK_DW_MANIFEST_MISSING_FIELD"));
    }

    #[test]
    fn manifest_payload_missing_is_error() {
        let report = check_dw_pack(
            &files(&[
                (DW_MANIFEST_ENTRY, r#"{"manifest_id":"x"}"#),
                (DW_METADATA_ENTRY, valid_metadata()),
            ]),
            false,
        );

        assert!(codes(&report).contains(&"PACK_DW_MANIFEST_MISSING_FIELD"));
    }

    #[test]
    fn display_name_wrong_type_is_error() {
        let report = check_dw_pack(
            &files(&[
                (
                    DW_MANIFEST_ENTRY,
                    r#"{"manifest_id":"x","manifest":{},"display_name":42}"#,
                ),
                (DW_METADATA_ENTRY, valid_metadata()),
            ]),
            false,
        );

        assert!(codes(&report).contains(&"PACK_DW_MANIFEST_FIELD_TYPE"));
    }

    #[test]
    fn metadata_json_absent_is_error() {
        let report = check_dw_pack(&files(&[(DW_MANIFEST_ENTRY, valid_manifest())]), false);

        assert!(codes(&report).contains(&"PACK_DW_METADATA_MISSING"));
    }

    #[test]
    fn metadata_pack_id_missing_is_error() {
        let report = check_dw_pack(
            &files(&[
                (DW_MANIFEST_ENTRY, valid_manifest()),
                (DW_METADATA_ENTRY, r#"{"kind":"DwApplication"}"#),
            ]),
            false,
        );

        assert!(codes(&report).contains(&"PACK_DW_METADATA_MISSING_FIELD"));
    }

    #[test]
    fn metadata_kind_mismatch_is_error() {
        let report = check_dw_pack(
            &files(&[
                (DW_MANIFEST_ENTRY, valid_manifest()),
                (DW_METADATA_ENTRY, r#"{"pack_id":"p","kind":"Flow"}"#),
            ]),
            false,
        );

        assert!(codes(&report).contains(&"PACK_DW_KIND_MISMATCH"));
    }

    /// Detection never depended on the declared kind, so a pack that omits it —
    /// every pack built before the field existed — must stay valid.
    #[test]
    fn metadata_kind_absent_is_not_an_error() {
        let report = check_dw_pack(
            &files(&[
                (DW_MANIFEST_ENTRY, valid_manifest()),
                (DW_METADATA_ENTRY, r#"{"pack_id":"p"}"#),
            ]),
            false,
        );

        assert!(!report.has_errors(), "unexpected: {:?}", report.diagnostics);
        assert!(!codes(&report).contains(&"PACK_DW_KIND_MISMATCH"));
    }

    #[test]
    fn metadata_created_at_unparseable_is_warn() {
        let report = check_dw_pack(
            &files(&[
                (DW_MANIFEST_ENTRY, valid_manifest()),
                (
                    DW_METADATA_ENTRY,
                    r#"{"pack_id":"p","created_at":"last tuesday"}"#,
                ),
            ]),
            false,
        );

        assert!(codes(&report).contains(&"PACK_DW_METADATA_TIMESTAMP"));
        assert!(!report.has_errors(), "must not be fatal");
    }

    #[test]
    fn no_executing_flow_is_info() {
        let report = check_dw_pack(
            &files(&[
                (DW_MANIFEST_ENTRY, valid_manifest()),
                (DW_METADATA_ENTRY, valid_metadata()),
            ]),
            false,
        );

        assert!(codes(&report).contains(&"PACK_DW_NO_EXECUTING_FLOW"));
        assert!(!report.has_errors());
        assert!(report.executing_flow.is_none());
    }

    #[test]
    fn knowledge_sidecar_dangling_asset_is_error() {
        let report = check_dw_pack(
            &files(&[
                (DW_MANIFEST_ENTRY, valid_manifest()),
                (DW_METADATA_ENTRY, valid_metadata()),
                (
                    DW_KNOWLEDGE_BASE_ENTRY,
                    r#"{"version":1,"strategy":"static_injection","files":[
                        {"asset_path":"assets/knowledge/gone.txt","original_name":"gone.txt","chars":1}]}"#,
                ),
            ]),
            false,
        );

        assert!(codes(&report).contains(&"PACK_DW_KNOWLEDGE_DANGLING_ASSET"));
        assert!(report.has_errors());
    }

    #[test]
    fn knowledge_sidecar_vectors_asset_is_checked_too() {
        let report = check_dw_pack(
            &files(&[
                (DW_MANIFEST_ENTRY, valid_manifest()),
                (DW_METADATA_ENTRY, valid_metadata()),
                ("assets/knowledge/a.txt", "text"),
                (
                    DW_KNOWLEDGE_CORPUS_ENTRY,
                    r#"{"version":1,"strategy":"embedding_retrieval","files":[
                        {"asset_path":"assets/knowledge/a.txt","original_name":"a.txt","chars":4,
                         "vectors_asset_path":"assets/knowledge/a.vec.json"}]}"#,
                ),
            ]),
            false,
        );

        assert!(codes(&report).contains(&"PACK_DW_KNOWLEDGE_DANGLING_ASSET"));
    }

    #[test]
    fn unreferenced_knowledge_asset_is_warn() {
        let report = check_dw_pack(
            &files(&[
                (DW_MANIFEST_ENTRY, valid_manifest()),
                (DW_METADATA_ENTRY, valid_metadata()),
                ("assets/knowledge/stray.txt", "text"),
            ]),
            false,
        );

        assert!(codes(&report).contains(&"PACK_DW_KNOWLEDGE_ORPHAN_ASSET"));
        assert!(!report.has_errors(), "an orphan asset must not be fatal");
    }

    #[test]
    fn unknown_top_level_entry_is_info() {
        let report = check_dw_pack(
            &files(&[
                (DW_MANIFEST_ENTRY, valid_manifest()),
                (DW_METADATA_ENTRY, valid_metadata()),
                ("future-sidecar.json", "{}"),
            ]),
            false,
        );

        assert!(codes(&report).contains(&"PACK_DW_UNKNOWN_ENTRY"));
        assert!(!report.has_errors(), "forward compat must not be fatal");
    }

    /// The report must not describe artefacts this shape never has. Reporting a
    /// missing SBOM or signature is the same failure mode as the message this
    /// work replaces, only quieter.
    #[test]
    fn dw_report_never_mentions_sbom_or_signature() {
        let report = check_dw_pack(
            &files(&[
                (DW_MANIFEST_ENTRY, "{}"),
                (DW_METADATA_ENTRY, r#"{"kind":"Flow"}"#),
                ("assets/knowledge/stray.txt", "text"),
                ("weird.bin", "x"),
            ]),
            false,
        );

        for diagnostic in &report.diagnostics {
            let haystack =
                format!("{} {}", diagnostic.code, diagnostic.message).to_ascii_lowercase();
            for forbidden in ["sbom", "signature", "pack_manifest_unsupported"] {
                assert!(
                    !haystack.contains(forbidden),
                    "DW report must not mention {forbidden}: {diagnostic:?}"
                );
            }
        }
    }
}
