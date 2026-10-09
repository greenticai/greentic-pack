//! Derive which shape of `.gtpack` an archive is, from the archive's own entry
//! names.
//!
//! Two producers emit `.gtpack` files with two different archive shapes:
//!
//! - `greentic-pack build` writes the canonical shape: a `manifest.cbor`
//!   holding a `greentic_types::PackManifest`, alongside `sbom.cbor` and
//!   optional signatures.
//! - `greentic-designer` (`orchestrate::dw_application_pack::write_gtpack`)
//!   writes a DW application pack: a `manifest.json` holding an
//!   `AnswerDocPackSpec` — a struct local to the designer — alongside
//!   `metadata.json` and optional knowledge/flow entries.
//!
//! These are different schemas, not two encodings of one schema. Re-encoding
//! the designer's manifest as CBOR under the name `manifest.cbor` would not
//! make it readable as a `PackManifest`; it would only move the failure from
//! "missing" to "malformed".
//!
//! # Why detection derives from contents
//!
//! The shape is derived from the entry names and never from a declared field
//! inside a manifest. greentic-designer-admin reached the same conclusion for
//! `.gtxtpl` templates, where a manifest's `kindTarget` is documented as
//! advisory and the real kind comes from whether the zip carries a
//! `dw-form.json` entry: a manifest field can drift out of sync with the zip it
//! rides in, but the zip's own contents cannot lie about what they contain.
//!
//! A declared marker — the designer's `metadata.json` already carries
//! `"kind": "DwApplication"` — is therefore only ever cross-checked against the
//! derived shape, never used to determine it.
//!
//! # Why matches are exact
//!
//! Every predicate here is an exact lookup of a top-level entry name. Suffix
//! matching is a trap this repo has already fallen into once: a blanket
//! `path.ends_with("manifest.json")` wrongly classified asset index files such
//! as `assets/i18n/_manifest.json`. Exact matching closes that by construction.

use std::collections::BTreeSet;

/// The canonical pack manifest entry (`greentic_types::PackManifest`, CBOR).
pub const CANONICAL_MANIFEST_ENTRY: &str = "manifest.cbor";

/// The DW application pack manifest entry (designer `AnswerDocPackSpec`, JSON).
pub const DW_MANIFEST_ENTRY: &str = "manifest.json";

/// The DW application pack metadata sidecar.
///
/// Not part of the discriminant: the designer writes it unconditionally, so an
/// archive carrying [`DW_MANIFEST_ENTRY`] without this one is a *broken DW
/// pack*, not an unknown artefact, and is better reported as such.
pub const DW_METADATA_ENTRY: &str = "metadata.json";

/// The shape of a `.gtpack` archive, derived from its entry names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackArchiveShape {
    /// Canonical pack written by `greentic-pack build`. Carries
    /// [`CANONICAL_MANIFEST_ENTRY`].
    Canonical,
    /// DW application pack written by greentic-designer. Carries
    /// [`DW_MANIFEST_ENTRY`].
    DwAnswerDoc,
    /// A readable archive that matches no known shape. Must stay a loud
    /// failure — there is deliberately no validation path for this variant.
    Unrecognised,
}

impl PackArchiveShape {
    /// Stable machine-readable slug, used for `--format json` output.
    pub fn as_slug(&self) -> &'static str {
        match self {
            Self::Canonical => "canonical",
            Self::DwAnswerDoc => "dw-application",
            Self::Unrecognised => "unrecognised",
        }
    }

    /// Operator-facing description naming the entry that decided the shape.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Canonical => "canonical (manifest.cbor)",
            Self::DwAnswerDoc => {
                "DW application pack (manifest.json + metadata.json, written by greentic-designer)"
            }
            Self::Unrecognised => "unrecognised",
        }
    }
}

/// Derive the archive shape from the set of entry names an archive contains.
///
/// Precedence is `manifest.cbor`, then `manifest.json`, then unrecognised.
/// `manifest.cbor` wins when both are present because it is the contract every
/// downstream consumer (`verify`, `sign`, `plan`, the runner) reads; callers
/// that want to flag the oddity should ask [`archive_shape_is_ambiguous`].
pub fn detect_archive_shape(entry_names: &BTreeSet<String>) -> PackArchiveShape {
    if entry_names.contains(CANONICAL_MANIFEST_ENTRY) {
        return PackArchiveShape::Canonical;
    }
    if entry_names.contains(DW_MANIFEST_ENTRY) {
        return PackArchiveShape::DwAnswerDoc;
    }
    PackArchiveShape::Unrecognised
}

/// Whether the archive carries both manifests.
///
/// Such an archive is a producer bug: a pack should carry exactly one manifest.
/// [`detect_archive_shape`] still resolves it to [`PackArchiveShape::Canonical`]
/// so behaviour stays predictable, but the caller should warn.
pub fn archive_shape_is_ambiguous(entry_names: &BTreeSet<String>) -> bool {
    entry_names.contains(CANONICAL_MANIFEST_ENTRY) && entry_names.contains(DW_MANIFEST_ENTRY)
}

/// Explain why an archive without a canonical manifest cannot be read as one.
///
/// Several commands (`verify`, `sign`, `plan`, the provider readers) genuinely
/// require a canonical pack and are right to fail without one. But the archive
/// may still be a perfectly healthy pack of another shape, and saying only
/// "manifest.cbor missing" reads as corruption — that message is what made a
/// valid greentic-designer export look broken in the field. Name the shape
/// instead, and point at the command that can actually inspect it.
///
/// Callers pass the archive's entry names; the shape is derived, never declared.
pub fn non_canonical_archive_message(entry_names: &BTreeSet<String>) -> String {
    match detect_archive_shape(entry_names) {
        PackArchiveShape::DwAnswerDoc => format!(
            "this archive is a DW application pack (it carries `{DW_MANIFEST_ENTRY}`, not \
             `{CANONICAL_MANIFEST_ENTRY}`); this command requires a canonical pack — run \
             `greentic-pack doctor <pack>` to inspect it"
        ),
        // `Canonical` is unreachable in practice: callers only build this
        // message after failing to find the canonical manifest. Fold it into
        // the unrecognised arm rather than asserting.
        PackArchiveShape::Canonical | PackArchiveShape::Unrecognised => format!(
            "`{CANONICAL_MANIFEST_ENTRY}` missing from archive, and no other known pack shape \
             matched; run `greentic-pack doctor <pack>` for details"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn manifest_cbor_yields_canonical() {
        let names = entries(&["manifest.cbor", "sbom.cbor", "flows/main.ygtc"]);
        assert_eq!(detect_archive_shape(&names), PackArchiveShape::Canonical);
        assert!(!archive_shape_is_ambiguous(&names));
    }

    #[test]
    fn manifest_json_yields_dw_answer_doc() {
        let names = entries(&["manifest.json", "metadata.json"]);
        assert_eq!(detect_archive_shape(&names), PackArchiveShape::DwAnswerDoc);
        assert!(!archive_shape_is_ambiguous(&names));
    }

    #[test]
    fn both_manifests_yield_canonical_and_report_ambiguous() {
        let names = entries(&["manifest.cbor", "manifest.json", "metadata.json"]);
        assert_eq!(detect_archive_shape(&names), PackArchiveShape::Canonical);
        assert!(archive_shape_is_ambiguous(&names));
    }

    #[test]
    fn neither_manifest_yields_unrecognised() {
        let names = entries(&["metadata.json", "flows/main.ygtc", "assets/knowledge/a.txt"]);
        assert_eq!(detect_archive_shape(&names), PackArchiveShape::Unrecognised);
    }

    /// Regression: a blanket `ends_with("manifest.json")` used to classify asset
    /// index files as pack manifests. Matching must stay exact and top-level.
    #[test]
    fn nested_i18n_manifest_json_does_not_match() {
        let names = entries(&["assets/i18n/_manifest.json", "assets/i18n/en.json"]);
        assert_eq!(detect_archive_shape(&names), PackArchiveShape::Unrecognised);
    }

    #[test]
    fn nested_component_manifest_json_does_not_match() {
        let names = entries(&[
            "components/foo/component.manifest.json",
            "components/foo.manifest.json",
        ]);
        assert_eq!(detect_archive_shape(&names), PackArchiveShape::Unrecognised);
    }

    #[test]
    fn empty_archive_yields_unrecognised() {
        let names = entries(&[]);
        assert_eq!(detect_archive_shape(&names), PackArchiveShape::Unrecognised);
        assert!(!archive_shape_is_ambiguous(&names));
    }

    #[test]
    fn slugs_are_stable() {
        assert_eq!(PackArchiveShape::Canonical.as_slug(), "canonical");
        assert_eq!(PackArchiveShape::DwAnswerDoc.as_slug(), "dw-application");
        assert_eq!(PackArchiveShape::Unrecognised.as_slug(), "unrecognised");
    }
}
