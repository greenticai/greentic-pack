# `doctor` pack-shape awareness — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use `superpowers:subagent-driven-development` (recommended)
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax
> for tracking.

**Design:** [`../specs/2026-08-20-doctor-pack-kind-awareness-design.md`](../specs/2026-08-20-doctor-pack-kind-awareness-design.md).
Read it first — every decision (D1–D7, message text, naming) is fixed there and must not be re-litigated here.

**Goal:** `greentic-pack doctor` (and its alias `inspect`) derives the pack's archive shape from the zip's
own entry names, validates a greentic-designer DW application pack against checks that actually apply to it,
and fails loudly and informatively on an archive matching no known shape — replacing the
`manifest.cbor missing from archive` message that made a healthy partner pack look corrupt.

**Architecture:** a new pure detector in `greentic-pack-lib` (`archive_shape.rs`) is called from three
places: `doctor`'s archive branch (new DW validation path), `print_human` (one shape line), and
`reader.rs:381` (shape-aware error text that every `open_pack` caller inherits). `open_pack`'s signature and
canonical semantics are unchanged.

**Tech stack:** Rust 2024, rustc 1.95.0 (`rust-toolchain.toml`), `anyhow` + `thiserror`, `serde_json`,
`zip`, `greentic_types::validate::{Diagnostic, Severity, ValidationReport}`, `tempfile` + `assert_cmd` for
tests. Additionally targets `wasm32-wasip2`.

## Global Constraints

- **One repo, one worktree.** All required work is in `greentic-pack`. Branch off `origin/research`
  (`1.3.0-research.9`) — **not** `main`, which is ~10 days stale. Current worktree:
  `/home/bima-pangestu/projects/orca/workspaces/greentic-pack/doctor-pack-kind-awareness`, branch
  `BimaPangestu28/doctor-pack-kind-awareness`.
- The greentic-designer half (Task 8) is **optional and non-blocking**. It must not gate this PR, and
  nothing in Tasks 1–7 may depend on it shipping.
- `#![forbid(unsafe_code)]`. No `unwrap()` / `panic!()` outside `#[cfg(test)]`.
- Use `greentic_interfaces::canonical`; never import `greentic_interfaces::bindings::*`
  (enforced by `ci/check_no_interfaces_bindings_imports.sh`).
- `archive_shape.rs` must be **IO-free and not behind `#[cfg(feature = "native")]`** — it has to compile for
  `wasm32-wasip2`. Validate with `cargo check --target wasm32-wasip2`.
- **Never match manifest entries by suffix.** Exact top-level `BTreeSet::contains` only. See the
  `assets/i18n/_manifest.json` trap documented at `crates/packc/src/cli/inspect.rs:576-586`.
- Conventional commits (`feat:` / `fix:` / `refactor:`). English only in everything committed. No AI/Claude
  co-author attribution on commits or the PR.
- Update `.codex/repo_overview.md` **before and after** PR work (repo convention in `CLAUDE.md`).
- Gate: `bash ci/local_check.sh` green before the PR is declared done.
- Update the Orca worktree comment at each task boundary:
  `orca-ide worktree set --worktree active --comment "<short status>" --json`

---

## Task 1: The shape detector (pure, tested in isolation)

The detector is the whole design in one function. It ships first, alone, with its own tests, so every later
task builds on something already proven. No caller is wired up yet.

**Files:**
- Create: `crates/greentic-pack/src/archive_shape.rs`
- Modify: `crates/greentic-pack/src/lib.rs:3-25` (add `pub mod archive_shape;` next to `kind`, **not**
  under a `native` gate; re-export the type alongside `pub use kind::PackKind;`)

**Interfaces produced:**
```rust
pub enum PackArchiveShape { Canonical, DwAnswerDoc, Unrecognised }
pub const CANONICAL_MANIFEST_ENTRY: &str = "manifest.cbor";
pub const DW_MANIFEST_ENTRY: &str = "manifest.json";
pub const DW_METADATA_ENTRY: &str = "metadata.json";
pub fn detect_archive_shape(entry_names: &BTreeSet<String>) -> PackArchiveShape;
pub fn archive_shape_is_ambiguous(entry_names: &BTreeSet<String>) -> bool;
```

- [ ] **Step 1: Write the failing unit tests first** (TDD; `superpowers:test-driven-development`)

  In `crates/greentic-pack/src/archive_shape.rs`, `#[cfg(test)] mod tests`:
  - `manifest_cbor_yields_canonical`
  - `manifest_json_yields_dw_answer_doc`
  - `both_manifests_yield_canonical_and_report_ambiguous`
  - `neither_manifest_yields_unrecognised`
  - `nested_i18n_manifest_json_does_not_match` — entries `{"assets/i18n/_manifest.json"}` → `Unrecognised`.
    This is the regression guard for the suffix-matching trap.
  - `nested_component_manifest_json_does_not_match` — `{"components/foo/component.manifest.json"}` →
    `Unrecognised`.
  - `empty_archive_yields_unrecognised`

- [ ] **Step 2: Implement `detect_archive_shape`** with the exact precedence from the design
      (Decision 1): `manifest.cbor` → `Canonical`; else `manifest.json` → `DwAnswerDoc`; else
      `Unrecognised`. Exact `contains` only. Document on the enum *why* precedence is contents-first,
      citing the `.gtxtpl` precedent (`greentic-designer-admin/src/store_artifact/template.rs:55-59, 82-86`).

- [ ] **Step 3: Verify**
```bash
cargo test -p greentic-pack-lib archive_shape -- --nocapture
cargo check --target wasm32-wasip2 -p greentic-pack-lib
cargo clippy -p greentic-pack-lib --all-targets -- -D warnings
```
  Expect: all shape tests pass, wasm target compiles, no clippy warnings.

- [ ] **Step 4:** `orca-ide worktree set --worktree active --comment "Task 1 done: archive_shape detector + unit tests" --json`

---

## Task 2: Shape-aware `open_pack` error text

The smallest possible change that improves `verify`, `sign`, `plan`, and both provider paths at once — one
call site. Does not change any pass/fail outcome.

**Files:**
- Modify: `crates/greentic-pack/src/reader.rs:378-381` (the `ok_or_else(|| anyhow!("manifest.cbor missing
  from archive"))`)
- Test: `crates/greentic-pack/tests/` (new `archive_shape_errors.rs`)

- [ ] **Step 1: Write the failing test.** Build a zip in-test containing only `manifest.json` +
      `metadata.json`, call `open_pack(path, SigningPolicy::DevOk)`, assert the returned
      `PackVerifyResult.message` contains `"DW application pack"` and `"greentic-pack doctor"`, and does
      **not** contain the literal `"manifest.cbor missing from archive"`.

- [ ] **Step 2: Implement.** Replace the bare `anyhow!` with a match on
      `detect_archive_shape(&files.keys().cloned().collect())`, emitting the two texts from design §5e.
      `open_pack` still returns `Err` in both cases — behaviour unchanged, text improved.

- [ ] **Step 3: Verify**
```bash
cargo test -p greentic-pack-lib --test archive_shape_errors -- --nocapture
cargo test -p greentic-pack --locked -- --nocapture     # nothing asserting on the old string may break
```
  If any existing test asserts on `"manifest.cbor missing from archive"`, update it to the new text —
  do not weaken the assertion to a substring that would also match the old message.

- [ ] **Step 4:** Orca comment: `"Task 2 done: shape-aware open_pack error"`

---

## Task 3: Split entry reading from canonical decoding

Refactor only — no behaviour change. The DW path must reuse `read_archive_entries`
(`crates/greentic-pack/src/reader.rs:780-830`), which already enforces regular-file-only, safe paths via
`enclosed_name`, duplicate detection, `MAX_FILE_BYTES` and `MAX_ARCHIVE_BYTES` (design **D1**). Re-implementing
zip hygiene in the DW branch would be the single most likely place to introduce a security regression.

**Files:**
- Modify: `crates/greentic-pack/src/reader.rs:340-390` (`open_pack_inner`)

**Interfaces produced:**
```rust
/// Opens the zip, reads every entry with the existing safety checks, and enforces
/// the archive size cap. Shape-agnostic: performs no manifest decoding.
pub struct PackArchiveEntries { pub files: HashMap<String, Vec<u8>>, pub total_bytes: u64 }
pub fn read_pack_archive(path: &Path) -> Result<PackArchiveEntries>;
```

- [ ] **Step 1:** Extract the zip-open + `read_archive_entries` + `MAX_ARCHIVE_BYTES` block from
      `open_pack_inner` into `read_pack_archive`. Keep every existing error message byte-identical —
      including `"{} is not a valid gtpack archive"` and the "check that no build artifacts … were
      accidentally included" hint.
- [ ] **Step 2:** Make `open_pack_inner` call `read_pack_archive` and keep the rest of its body unchanged.
- [ ] **Step 3: Verify — this task must move zero tests.**
```bash
cargo test -p greentic-pack --locked -- --nocapture
cargo test -p greentic-pack-lib --locked -- --nocapture
```
  Expect: identical pass set to before the refactor. Any change here means the refactor was not pure.
- [ ] **Step 4:** Orca comment: `"Task 3 done: read_pack_archive extracted, no behaviour change"`

---

## Task 4: DW validation checks (D2–D7)

The core of the feature. Produces `Vec<Diagnostic>` using the repo's existing types so `--format json`,
severity rendering and exit codes stay uniform across shapes (design "Architecture", constraint 1).

**Files:**
- Create: `crates/packc/src/cli/doctor_dw.rs`
- Modify: `crates/packc/src/cli/mod.rs` (add `mod doctor_dw;`)
- Test: `crates/packc/tests/doctor_dw_pack.rs` (new)

**Interfaces produced:**
```rust
pub struct DwPackReport {
    pub pack_id: Option<String>,
    pub manifest_id: Option<String>,
    pub display_name: Option<String>,
    pub executing_flow: Option<String>,
    pub knowledge: Option<DwKnowledgeSummary>,
    pub diagnostics: Vec<greentic_types::validate::Diagnostic>,
}
pub fn check_dw_pack(files: &HashMap<String, Vec<u8>>, flow_doctor: bool) -> DwPackReport;
```

- [ ] **Step 1: Build the fixture helper first.** In `crates/packc/tests/doctor_dw_pack.rs`, a
      `build_dw_pack(...)` helper writing a zip byte-faithful to
      `greentic-designer/src/orchestrate/dw_application_pack.rs:422-530`: `manifest.json` (pretty-printed
      `AnswerDocPackSpec` shape — `manifest_id`, `manifest`, optional `display_name`/`locale`/`tenant`/
      `provider_overrides`) + `metadata.json` (`pack_id`, `kind: "DwApplication"`, `created_at`, `source`),
      with switches for `flows/main.ygtc`, `knowledge_base.json`, `knowledge_corpus.json`, and
      `assets/knowledge/*.txt`.

- [ ] **Step 2: Write the failing tests**, one per design check:
  - D2a `manifest_json_not_json_is_error` → `PACK_DW_MANIFEST_INVALID_JSON`
  - D2b `manifest_id_missing_is_error` / `manifest_id_empty_is_error` → `PACK_DW_MANIFEST_MISSING_FIELD`
  - D2c `manifest_payload_missing_is_error` → `PACK_DW_MANIFEST_MISSING_FIELD`
  - D2d `display_name_wrong_type_is_error` → `PACK_DW_MANIFEST_FIELD_TYPE`
  - D3a `metadata_json_absent_is_error` → `PACK_DW_METADATA_MISSING`
  - D3b `metadata_pack_id_missing_is_error` → `PACK_DW_METADATA_MISSING_FIELD`
  - D3c `metadata_kind_mismatch_is_error` → `PACK_DW_KIND_MISMATCH`
  - D3c `metadata_kind_absent_is_not_an_error` — **explicitly asserts absence is tolerated**; this is the
    forward/backward-compat guarantee for every DW pack already in the field
  - D3d `metadata_created_at_unparseable_is_warn` → `PACK_DW_METADATA_TIMESTAMP`
  - D4c `no_executing_flow_is_info` → `PACK_DW_NO_EXECUTING_FLOW`
  - D5 `knowledge_sidecar_dangling_asset_is_error` → `PACK_DW_KNOWLEDGE_DANGLING_ASSET`
  - D6 `unreferenced_knowledge_asset_is_warn` → `PACK_DW_KNOWLEDGE_ORPHAN_ASSET`
  - D7 `unknown_top_level_entry_is_info` → `PACK_DW_UNKNOWN_ENTRY`
  - `clean_dw_pack_has_no_error_diagnostics`
  - **Negative-space test** `dw_report_never_mentions_sbom_or_signature`: assert no diagnostic code or
    message contains `sbom`, `signature`, or `PACK_MANIFEST_UNSUPPORTED`. Suppressing inapplicable
    "missing" lines is half the fix (design, Decision 2).

- [ ] **Step 3: Implement `check_dw_pack`** exactly per the design's D2–D7 table — codes, severities and
      hint text as specified. Every diagnostic sets `path` to the entry it concerns and a `hint` naming the
      concrete fix. Errors are collected, never short-circuited: one bad field must not hide the rest of the
      report.

- [ ] **Step 4: Wire D4 (flow doctor).** Reuse the `greentic-flow doctor --json --stdin` spawn logic from
      `crates/packc/src/cli/inspect.rs:246-333`, including the `ErrorKind::NotFound` →
      `PACK_FLOW_DOCTOR_UNAVAILABLE` warn-and-skip branch (`inspect.rs:264-277`) and the
      `flow_doctor_unsupported` fallback. Factor the shared spawn into a helper rather than copying it;
      the DW path differs only in that it has one hardcoded flow path instead of `load.manifest.flows`.
      Test D4 through the **skip warning**, not by requiring the binary to be installed.

- [ ] **Step 5: Verify**
```bash
cargo test -p greentic-pack --test doctor_dw_pack -- --nocapture
cargo clippy -p greentic-pack --all-targets -- -D warnings
```

- [ ] **Step 6:** Orca comment: `"Task 4 done: DW checks D2-D7 with diagnostics"`

---

## Task 5: Wire the shape branch into `doctor` / `inspect`

**Files:**
- Modify: `crates/packc/src/cli/inspect.rs:130-137` (the `InspectMode::Archive` arm in `handle`)
- Modify: `crates/packc/src/cli/inspect.rs:196-221` (JSON + human output; exit-code block)
- Modify: `crates/packc/src/cli/inspect.rs:675` (`print_human` — prepend the shape line)

- [ ] **Step 1:** In `handle`, for `InspectMode::Archive(path)`: call `read_pack_archive` (Task 3), then
      `detect_archive_shape`, then branch.
  - `Canonical` → today's `inspect_pack_file` path, unchanged. If `archive_shape_is_ambiguous`, push the
    §5d warning first.
  - `DwAnswerDoc` → `check_dw_pack`, render per §5b, exit 1 iff any `Severity::Error`.
  - `Unrecognised` → the §5c error, exit 1. **No diagnostics report, no partial success** (design,
    Decision 3).
  - `InspectMode::Source(_)` is untouched — source dirs always build a canonical pack.

- [ ] **Step 2:** `print_human` prepends `Pack shape: canonical (manifest.cbor)`, appending
      `; dw-application sidecars: …` when `dw-agents.json` / `secrets-policy.json` are present (design §5a,
      finding F2 — this is what distinguishes a packc-built DW pack from a designer-exported one).

- [ ] **Step 3:** `--format json` gains a top-level `"archive_shape"` field
      (`"canonical" | "dw-application" | "unrecognised"`). For `DwAnswerDoc`, emit `"validation"` with the
      same `ValidationReport` schema as canonical and **omit** `manifest`, `sbom`, `report.signature_ok`,
      `report.sbom_ok`, `static_routes` — those describe a shape this archive does not have. Keep the
      output sorted via the existing `to_sorted_json`.

- [ ] **Step 4: Verify**
```bash
cargo test -p greentic-pack --locked -- --nocapture
```
  Expect: some existing canonical-output assertions fail on the new leading shape line
  (`crates/packc/tests/{inspect,doctor_validation,cli_smoke,readme_examples}.rs` are the likely sites).
  Update them to expect the line — do **not** remove the line to keep tests quiet.

- [ ] **Step 5:** Orca comment: `"Task 5 done: doctor branches on archive shape"`

---

## Task 6: End-to-end CLI tests

**Files:**
- Modify: `crates/packc/tests/doctor_dw_pack.rs` (add an `assert_cmd` section)

- [ ] **Step 1: The field-report regression.** Build a valid DW pack, run
      `greentic-pack doctor <pack>.gtpack`, assert exit **0**, stdout contains
      `Pack shape: DW application pack`, and stdout+stderr contain **neither** `manifest.cbor missing from
      archive` **nor** the word `corrupt`. This test is the partner's bug; it must fail before Task 5 and
      pass after.
- [ ] **Step 2: Unrecognised stays loud.** A zip with only `README.txt`: assert exit **1**, and that the
      message names both shapes, both deciding entry names, and lists the entries found (design §5c).
- [ ] **Step 3: Corrupt is still corrupt.** Random non-zip bytes with a `.gtpack` extension: assert exit
      **1** with the existing `is not a valid gtpack archive` message. Kind-awareness must not have turned a
      genuinely broken file into a pass.
- [ ] **Step 4: Canonical unchanged.** Run `doctor` on an existing fixture pack; assert exit 0 and that the
      diagnostics set matches pre-change behaviour.
- [ ] **Step 5: Ambiguous.** Zip with both `manifest.cbor` and `manifest.json`: canonical path runs, the
      §5d warning appears.
- [ ] **Step 6: JSON shape.** `--format json` on a DW pack: `archive_shape == "dw-application"`, and the
      canonical-only keys are absent.
- [ ] **Step 7: Alias parity.** `greentic-pack inspect <dw-pack>` behaves identically to `doctor` (finding
      F1); the only difference is the existing deprecation warning (`cli/mod.rs:355-357`).
- [ ] **Step 8: Verify**
```bash
cargo test -p greentic-pack --test doctor_dw_pack -- --nocapture
```
- [ ] **Step 9:** Orca comment: `"Task 6 done: e2e CLI coverage incl. field-report regression"`

---

## Task 7: Docs, repo overview, and the PR

**Files:**
- Modify: `docs/pack-format.md` (new subsection under `## Pack kinds`, line ~78)
- Modify: `docs/cli.md` (the `doctor` entry)
- Modify: `.codex/repo_overview.md`
- Modify: `CHANGELOG.md` if the repo keeps one on this lane

- [ ] **Step 1:** `docs/pack-format.md` — add **"Archive shapes"** distinguishing the canonical archive from
      the DW application pack, with the entry tables from the design's "What the archive actually contains".
      State plainly that `manifest.json` and `manifest.cbor` carry **different schemas**
      (`AnswerDocPackSpec` vs `PackManifest`), not two encodings of one schema, and that re-encoding one as
      the other is not a valid migration.
- [ ] **Step 2:** `docs/cli.md` — document what `doctor` checks per shape and what it deliberately does not
      check for a DW pack.
- [ ] **Step 3:** `.codex/repo_overview.md` — record `crates/greentic-pack/src/archive_shape.rs` and
      `crates/packc/src/cli/doctor_dw.rs`.
- [ ] **Step 4: Full gate**
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --target wasm32-wasip2
bash ci/local_check.sh
```
  All four must be green. `ci/local_check.sh` also runs the interfaces-bindings import guard, the builder
  demo determinism check, and canonical gtpack generation.
- [ ] **Step 5: Open the PR** against `research` (not `main`). Body must state: the field report and its
      date, the root cause with `file:line` (`crates/greentic-pack/src/reader.rs:378-381`), the
      contents-first detection rule and its `.gtxtpl` precedent, and an explicit note that no
      greentic-designer change is required for this fix to work on packs already exported.
- [ ] **Step 6:** Orca comment: `"Task 7 done: docs + local_check green, PR open"`

---

## Task 8 (OPTIONAL, separate repo, does not gate the PR): greentic-designer hardening

Land only after Tasks 1–7 have merged. Nothing above depends on this.

**Repo:** `greentic-designer` (checkout: `/home/bima-pangestu/projects/orca/workspaces/greentic-designer/develop`)

- [ ] **Step 1:** `src/orchestrate/dw_application_pack.rs:10-16` — replace the "migration to CBOR is a
      one-line swap (`ciborium::into_writer`)" claim. It is wrong: `manifest.cbor` must decode as
      `greentic_types::PackManifest`, so re-encoding `AnswerDocPackSpec` as CBOR under that name would move
      `doctor`'s error from "missing" to "malformed" — strictly worse. Point the comment at
      `greentic-pack/docs/superpowers/specs/2026-08-20-doctor-pack-kind-awareness-design.md`.
- [ ] **Step 2:** `src/orchestrate/dw_application_pack.rs:116-122` — add
      `schema_version: &'static str` (`"dw-pack-v1"`) to `PackMetadata`. Purely additive; packc's D3 must
      tolerate its absence forever, so **no packc change is required** to accept it.
- [ ] **Step 3:** Update the designer's `write_gtpack` tests
      (`src/orchestrate/dw_application_pack_tests.rs`) for the new field.
- [ ] **Step 4:** Keep `kind: "DwApplication"` exactly as it is (`:448`). packc cross-checks it (D3c) but
      never derives from it — changing or dropping the string would now trip `PACK_DW_KIND_MISMATCH`.
- [ ] **Step 5:** Designer's own CI gate green; separate PR on the designer's lane.

---

## Risks and mitigations

| risk | mitigation |
|---|---|
| Suffix matching sneaks back in and `assets/i18n/_manifest.json` is misdetected | Task 1 Step 1 has a dedicated regression test; the trap is already documented at `crates/packc/src/cli/inspect.rs:576-586` |
| The Task 3 refactor silently weakens zip safety | Task 3 asserts a **zero-test-delta** verification; error strings kept byte-identical |
| Kind-awareness turns a corrupt pack into a pass | Task 6 Steps 2–3 assert exit 1 for both the unrecognised-zip and not-a-zip cases; `Unrecognised` structurally has no validation path |
| The new shape line breaks canonical golden-output tests | Expected and planned for in Task 5 Step 4 — update the assertions, never drop the line |
| `greentic-flow` absent in CI makes D4 flaky | Assert through the existing `PACK_FLOW_DOCTOR_UNAVAILABLE` skip-warning, never by requiring the binary |
| Reviewers conflate the two DW producers (finding F2) | The shape line names packc-built sidecars explicitly (§5a); the PR body and `docs/pack-format.md` name the two producers apart |
