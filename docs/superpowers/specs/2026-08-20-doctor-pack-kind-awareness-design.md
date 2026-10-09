# `doctor` pack-shape awareness: stop calling a valid DW pack corrupt

Date: 2026-08-20
Status: implemented (see the corrections at the end)
Base lane: `research` (`origin/research` @ `1.3.0-research.9`; `main` is ~10 days stale — do not branch off it)
Repos: `greentic-pack` (all required work). `greentic-designer` (one optional, non-blocking hardening change).

> **Docs layout note.** The task brief asked for `docs/specs/` + `docs/plans/`. This repo already has an
> established layout for design/plan pairs — `docs/superpowers/specs/<date>-<slug>-design.md` and
> `docs/superpowers/plans/<date>-<slug>.md` (see the `subscribes_to`, `auto-derive-credential-setup`, and
> `greentic-pack-i18n-build` pairs). This design follows the existing layout, per the brief's instruction to
> do so and say which was used.

---

## Problem

A partner exported an Agentic Worker pack from greentic-designer. `greentic-pack doctor <pack>.gtpack`
answered:

```
manifest.cbor missing from archive
```

The pack is not broken. The same file was accepted by `greentic-setup bundle add`, built into a
`.gtbundle`, and passed `greentic-setup env-deploy --dry-run`. The message reads like corruption and it
cost the partner time.

### Where the message comes from

`doctor` is not a separate command — it shares `InspectArgs` and the whole `inspect` code path:

- `crates/packc/src/cli/mod.rs:90` — `Doctor(self::inspect::InspectArgs)`
- `crates/packc/src/cli/mod.rs:103-104` — `Inspect` is documented as the *deprecated alias for `doctor`*
- `crates/packc/src/cli/mod.rs:354` — `Command::Inspect(args) | Command::Doctor(args) => …handle(args, …)`

`handle` resolves archive mode and immediately opens the pack:

- `crates/packc/src/cli/inspect.rs:134` — `InspectMode::Archive(path) => inspect_pack_file(path)?`
- `crates/packc/src/cli/inspect.rs:522-527` — `inspect_pack_file` calls `open_pack(path, SigningPolicy::DevOk)`
  and turns any failure into an `anyhow` error

`open_pack` → `open_pack_inner` reads every zip entry and then hard-requires the canonical manifest:

- `crates/greentic-pack/src/reader.rs:378-381`
  ```rust
  let manifest_bytes = files
      .get("manifest.cbor")
      .cloned()
      .ok_or_else(|| anyhow!("manifest.cbor missing from archive"))?;
  ```

That `?` aborts `handle` before the diagnostic machinery is ever constructed. **This is the root cause of
the bad UX:** the failure happens in the loader, not in the validator, so none of the repo's existing
`Diagnostic` / `Severity` / `ValidationReport` reporting applies. Note that
`crates/greentic-pack/src/validate/mod.rs:184` already defines a `PACK_MISSING_MANIFEST_CBOR` diagnostic —
it is *unreachable* from archive mode, because `open_pack` bails first.

### What the archive actually contains

Confirmed by reading greentic-designer (`develop` checkout,
`src/orchestrate/dw_application_pack.rs`):

| entry | written at | contents |
|---|---|---|
| `manifest.json` | `:441` | pretty-printed `AnswerDocPackSpec` (`:97-113`) |
| `metadata.json` | `:454` | `PackMetadata` (`:116-122`), including `kind: "DwApplication"` (`:448`) |
| `knowledge_base.json` | `:469` | optional; static-KB sidecar |
| `knowledge_corpus.json` | `:493` | optional; embedding-retrieval sidecar |
| `assets/knowledge/<slug>.txt` | KB/corpus loops | optional; one per KB file, deduped across both sidecars |
| `flows/main.ygtc` | `:524` | optional; emitted only when `spec.executing_node.is_some()` |

`AnswerDocPackSpec` is a struct **local to greentic-designer** (`dw_application_pack.rs:97`), not a shared
type. `PackManifest` (behind `manifest.cbor`) comes from `greentic_types`. These are two different schemas,
not two encodings of one schema.

### The brief's core claim is correct, and it matters

`dw_application_pack.rs:10-16` claims migrating to CBOR is "a one-line swap
(`ciborium::into_writer`)". That claim is misleading and this design deliberately does not build on it.
Swapping only the encoder yields an `AnswerDocPackSpec` encoded as CBOR under the name `manifest.cbor`.
`doctor` would then *find* the entry at `reader.rs:379` and fail one line later at `reader.rs:382`
(`decode_manifest(...).context("manifest.cbor is invalid")`). The operator-visible error moves from
"missing" to "malformed" — strictly worse, because "malformed" removes even the hint that a *different*
shape exists. **The fix belongs in `doctor`.**

---

## Findings: where the brief is incomplete or imprecise

These are corrections to the brief, discovered while reading the code. Two of them change the design.

**F1 — `doctor` is an alias, not a command.** Everything below applies to `inspect` too, because they are
literally the same function (`cli/mod.rs:354`). There is no way to fix one without the other, and no reason
to want to.

**F2 — this repo already builds `dw-application` packs, and they are *canonical* packs.** `greentic-pack
build` accepts `kind: dw-application` in `pack.yaml` and emits `dw-agents.json` + `secrets-policy.json`
sidecars (`crates/packc/src/build.rs:384-397`, `crates/packc/src/agent_pack.rs`). Crucially,
`crates/packc/src/build.rs:1413-1427` maps `"dw-application" => Ok(PackKind::Application)` with an explicit
comment that `greentic_types::PackKind` has no `DwApplication` variant. So a packc-built DW pack has
`manifest.cbor` and passes `doctor` today.

  There are therefore **two producers of "Agentic Worker pack" with two different archive shapes**:

  | producer | archive shape | doctor today |
  |---|---|---|
  | `greentic-pack build` (`kind: dw-application`) | canonical: `manifest.cbor` + `sbom.cbor` + `dw-agents.json` | passes |
  | `greentic-designer` `write_gtpack` | answer-doc: `manifest.json` + `metadata.json` | **fails, wrongly** |

  This is the single most important thing the design has to keep straight, and it drives the naming
  decision below. Saying "DW pack" without qualification is ambiguous; the two are named apart throughout.

**F3 — a `kind` marker already exists in the designer output.** `metadata.json` already carries
`kind: "DwApplication"` (`dw_application_pack.rs:448`). The brief's fourth question ("should a `kind`
marker *also* be written into future DW packs?") is therefore already half-answered by the artefact: the
marker exists. What is missing is a *consumer* that reads it — and, per the `.gtxtpl` lesson, that consumer
must treat it as advisory. This changes the recommendation from "add a marker" to "cross-check the marker
that is already there".

**F4 — `greentic-pack-lib` already has a `PackKind::DwApplication` variant** (`crates/greentic-pack/src/kind.rs:8-13`,
rendered at `crates/packc/src/cli/info/report.rs:114`). It is currently unreachable from any built pack
because the builder downgrades to `PackKind::Application` (F2). That inconsistency is real but **out of
scope** here — noted so the next person does not assume `PackKind::DwApplication` means "designer export".
It does not; it is the manifest-level kind field of a canonical pack.

**F5 — there are four "manifest … missing from archive" strings, and only one is ours.** The partner hit
`reader.rs:381`. The others are `reader.rs:294` (a *component* manifest inside the index extension — a
different thing entirely), `crates/packc/src/cli/providers.rs:247`, and
`crates/greentic-pack/src/bin/common/providers.rs:225` (both `.context(...)` on provider packs). Only
`reader.rs:381` is in scope; the providers ones inherit the improvement for free (see "Blast radius").

**F6 — the zip reader already does the hard safety work.** `read_archive_entries`
(`crates/greentic-pack/src/reader.rs:780-830`) already rejects non-regular files, unsafe/traversing paths
(`enclosed_name`), duplicate entries, oversized files and oversized archives. The DW path reuses it
verbatim rather than re-implementing zip hygiene. This makes the DW branch small.

---

## Decision 1 — how `doctor` detects the shape

**Derive it from the archive's own entry names, before decoding anything.**

### Precedent being followed

greentic-designer-admin learned this for `.gtxtpl` templates. Its manifest carries a `kindTarget` field,
documented at `src/store_artifact/template.rs:55-59` as:

> *"Advisory. The artifact's contents decide the kind; this only has to agree with them. Absent on every
> artifact built before this existed."*

The real kind is derived from the zip at `src/store_artifact/template.rs:82-86`:

```rust
let derived_kind_target = if archive.file_names().any(|n| n == DW_FORM_ENTRY) { … };
```

and a declared value that disagrees is a hard rejection (`template.rs:122-126`,
`src/store_artifact/mod.rs:50-56`). The reasoning holds here unchanged: **a manifest field can drift out of
sync with the zip it rides in, but the zip's own contents cannot lie about what they contain.** It also
survives the "absent on every artifact built before this existed" case, which matters because every DW pack
already in the field predates any change we make.

### The detector

A new pure function in `greentic-pack-lib`:

```
crates/greentic-pack/src/archive_shape.rs

pub enum PackArchiveShape {
    Canonical,       // greentic-pack build output
    DwAnswerDoc,     // greentic-designer write_gtpack output
    Unrecognised,
}

pub fn detect_archive_shape(entry_names: &BTreeSet<String>) -> PackArchiveShape
```

Rules, in order:

1. `manifest.cbor` present → `Canonical`.
2. else `manifest.json` present → `DwAnswerDoc`.
3. else → `Unrecognised`.

Four properties this design commits to:

- **Exact top-level match only.** The predicate is `entry_names.contains("manifest.json")`, never a suffix
  or `ends_with` test. This repo has already been bitten by suffix matching: `is_forbidden_source_path`
  carries the comment at `crates/packc/src/cli/inspect.rs:576-586` explaining that a blanket
  `path.ends_with("manifest.json")` wrongly flagged `assets/i18n/_manifest.json`. The same trap applies
  here and is closed by construction.

- **`manifest.cbor` wins when both are present**, and doctor emits a `PACK_ARCHIVE_SHAPE_AMBIGUOUS` warning
  naming both entries. Canonical wins because `manifest.cbor` is the contract every downstream consumer
  (`verify`, `sign`, `plan`, the runner) reads. The warning exists because such an archive is a producer
  bug that nobody should ship silently.

- **`manifest.json` alone is the DW discriminant; `metadata.json` is a *check*, not part of the
  discriminant.** The designer writes both unconditionally (`:441`, `:454`), so an archive with
  `manifest.json` but no `metadata.json` is a *broken DW pack*, not an unknown artefact. Classifying it as
  `DwAnswerDoc` and then failing check **D3** below produces "detected DW application pack; metadata.json is
  missing" — far more actionable than "unrecognised archive". This mirrors the `.gtxtpl` precedent, which
  also discriminates on a single distinctive entry (`DW_FORM_ENTRY`, `template.rs:21`).

- **Pure, no IO, no `native` feature gate.** `crates/greentic-pack/src/lib.rs:9-19` puts `reader`,
  `validate`, `path_safety` and `resolver` behind `#[cfg(feature = "native")]` while `kind` is unconditional.
  `archive_shape` takes a `&BTreeSet<String>` and returns an enum, so it goes next to `kind` — unconditional,
  unit-testable without a fixture, and compiles for `wasm32-wasip2` (required by `rust-toolchain.toml`).

### Rejected alternatives

- **Trust `metadata.json`'s `kind` field as the discriminant.** Rejected — it is exactly the drift-prone
  declared field the `.gtxtpl` lesson warns about, and it does not exist in an archive that is missing
  `metadata.json`. It is used only as a cross-check (D3).
- **Sniff `manifest.cbor`'s CBOR bytes to tell `PackManifest` from a CBOR-encoded `AnswerDocPackSpec`.**
  Rejected — that is designing around the misleading "one-line swap" comment. If the designer ever does
  make that swap, the correct answer is that it broke the contract and must be reverted, not that doctor
  should guess. Detection stays on entry names, which are unambiguous.
- **Add a `--kind` / `--shape` CLI flag.** Rejected — it puts the burden on the operator who, by definition,
  does not yet know what they have. The whole complaint is that the tool would not tell them.

---

## Decision 2 — what `doctor` validates for a DW application pack

"Has a decodable `PackManifest`" does not apply. Neither do SBOM hashing, Ed25519 signatures, the pack-lock
component doctor, the component-manifest index, static routes, or the forbidden-source-path check — none of
those artefacts exist in this shape. The full set of meaningful checks:

| id | check | severity on failure | code |
|---|---|---|---|
| **D1** | Archive opens as a zip; entries are regular files with safe paths, no duplicates, within size caps | Error (fatal, pre-shape) | existing `read_archive_entries` errors |
| **D2a** | `manifest.json` is valid UTF-8 JSON and the root is an object | Error | `PACK_DW_MANIFEST_INVALID_JSON` |
| **D2b** | `manifest_id` present, a string, non-empty | Error | `PACK_DW_MANIFEST_MISSING_FIELD` |
| **D2c** | `manifest` present and an object (the composer's DW manifest payload) | Error | `PACK_DW_MANIFEST_MISSING_FIELD` |
| **D2d** | Optional fields well-typed when present: `display_name`/`locale`/`tenant` strings, `provider_overrides` object | Error | `PACK_DW_MANIFEST_FIELD_TYPE` |
| **D3a** | `metadata.json` present and valid JSON object | Error | `PACK_DW_METADATA_MISSING` |
| **D3b** | `metadata.json.pack_id` present, string, non-empty | Error | `PACK_DW_METADATA_MISSING_FIELD` |
| **D3c** | `metadata.json.kind` — when present, must equal `"DwApplication"` | **Error** (declared-vs-derived drift) | `PACK_DW_KIND_MISMATCH` |
| **D3d** | `metadata.json.created_at` — when present, parses as RFC 3339 | Warn | `PACK_DW_METADATA_TIMESTAMP` |
| **D4a** | If `flows/main.ygtc` is present, it passes `greentic-flow doctor --json --stdin` | Error | `PACK_FLOW_DOCTOR_FAILED` (reused) |
| **D4b** | If `greentic-flow` is absent or lacks `--stdin` | Warn, skip | `PACK_FLOW_DOCTOR_UNAVAILABLE` (reused) |
| **D4c** | If `flows/main.ygtc` is absent | Info | `PACK_DW_NO_EXECUTING_FLOW` |
| **D5** | If `knowledge_base.json` / `knowledge_corpus.json` present: valid JSON, and every asset path they index exists as an archive entry | Error | `PACK_DW_KNOWLEDGE_DANGLING_ASSET` |
| **D6** | `assets/knowledge/*` entries referenced by neither sidecar | Warn | `PACK_DW_KNOWLEDGE_ORPHAN_ASSET` |
| **D7** | Top-level entries outside the known set are listed, not rejected | Info | `PACK_DW_UNKNOWN_ENTRY` |

Rationale for the non-obvious ones:

- **D3c is an Error, not a Warn.** This is the point where the `.gtxtpl` precedent is followed most
  literally: `template.rs:122-126` returns `ArtifactError::KindTargetMismatch` and the upload is rejected.
  A `metadata.json` saying `"kind": "Flow"` inside an archive whose contents say DW is a producer bug that
  will mislead every downstream reader. **Absence** of the field is *not* an error — pre-existing packs and
  future producers may omit it, and detection never depended on it.

- **D4 reuses `run_flow_doctors` machinery verbatim** (`crates/packc/src/cli/inspect.rs:246-333`), including
  its "greentic-flow not installed → warn and skip" behaviour. `flows/main.ygtc` is a real YGTC flow written
  by `inject_dw_agent_graph_node` / `inject_operala_call_node` (`dw_application_pack.rs:512-527`); it is the
  single highest-value check available for this shape, because it is the one entry that can be *semantically*
  wrong rather than merely absent. The only difference from the canonical path is where the flow list comes
  from: canonical reads `load.manifest.flows`, DW hardcodes the one known path.

- **D5 mirrors `ReferencedFilesExistValidator`** (`crates/greentic-pack/src/validate/mod.rs`, wired at
  `crates/packc/src/cli/inspect.rs:874`). "Manifest references a file the archive does not contain" is the
  canonical path's most useful class of finding; the DW shape has exactly one analogue and it should get the
  same treatment.

- **D7 is Info, deliberately.** The designer is on a research lane and adds sidecars (KB in Phase 2.0,
  corpus in Phase 2.1/W5). An unknown entry must not fail a pack built by a newer designer against an older
  packc. Listing them keeps the operator informed without turning forward compatibility into breakage.

- **What is explicitly *not* reported.** The DW report must never emit "sbom.cbor missing",
  "signature files missing", or `PACK_MANIFEST_UNSUPPORTED`
  (`crates/packc/src/cli/inspect.rs:882-898`). Those are canonical-shape statements. Suppressing them is
  half the fix — a report full of inapplicable "missing" lines is the same failure mode as the original
  complaint, just quieter.

**Exit-code contract is unchanged**: any `Severity::Error` diagnostic makes `handle` `bail!("pack
validation failed")` (`crates/packc/src/cli/inspect.rs:213-220`), exit 1. A clean DW pack exits 0.

---

## Decision 3 — the archive matching neither shape

`Unrecognised` is a **hard, loud failure**. Exit 1, no diagnostics report, no partial success. Kind-awareness
must not turn a genuinely corrupt pack into a pass, and the way to guarantee that is structural: the shape
enum has exactly three arms and the third one has no validation path at all. There is no "best effort"
branch to leak through.

Two failure classes stay distinct, because they have different fixes:

1. **Not a zip / unreadable zip** — keeps the existing message from
   `crates/greentic-pack/src/reader.rs:342-352` (`"{path} is not a valid gtpack archive"`), plus the
   existing entry-level errors from `read_archive_entries` (unsafe path, duplicate entry, size cap).
   Nothing changes here; these were already clear.
2. **Valid zip, no recognised shape** — the new `PACK_ARCHIVE_SHAPE_UNRECOGNISED` failure, with the message
   in Decision 5.

---

## Decision 4 — should a `kind` marker also be written into future DW packs?

**One already is** (F3): `metadata.json` carries `kind: "DwApplication"` (`dw_application_pack.rs:448`).

**Decision: no new marker is required, and detection still derives from contents first.** The existing
marker is promoted from "purely informational for the UI" (its own doc comment at
`dw_application_pack.rs:115`) to an *advisory cross-check* — check **D3c**. Declared-vs-derived agreement is
enforced; declared-vs-derived *precedence* never is. This is exactly the `.gtxtpl` arrangement.

### Which half lands in which repo

| half | repo | required? | why |
|---|---|---|---|
| Shape detection, DW validation, all message text, `--format json` shape field | **`greentic-pack`** (this repo) | **Required** | Fixes the partner's problem with zero designer changes, and works on every DW pack already exported and sitting in the field. Nothing about the fix may depend on a designer release. |
| Add `schema_version: "dw-pack-v1"` to `PackMetadata` (`dw_application_pack.rs:116-122`) | `greentic-designer` | Optional, later, non-blocking | Lets a future packc distinguish DW shape revisions without guessing from entry names. Purely additive; absent on existing packs, so D3 must tolerate its absence forever. |
| Correct the misleading module-header comment (`dw_application_pack.rs:10-16`) | `greentic-designer` | Recommended | The "one-line swap" claim is what would produce the strictly-worse "malformed" error. Leaving it in place invites exactly the change this design exists to prevent. Replace it with a pointer to this document. |

The designer half is **not** on the critical path and must not gate the greentic-pack PR. If it never ships,
the fix still works, because detection never depended on the marker.

---

## Decision 5 — message text

The current message is the whole complaint, so this is a load-bearing part of the design. Every replacement
states **what shape was detected** and **what was checked**.

### 5a. Canonical pack (unchanged behaviour, one new line)

One line is prepended to `print_human` (`crates/packc/src/cli/inspect.rs:675`); everything below it is
today's output verbatim.

```
Pack shape: canonical (manifest.cbor)
Pack: acme.weather (1.2.0)
Name: Weather Assistant
Flows: 2
...
```

When packc-built DW sidecars are present (F2), the line names them, so the two DW producers are
distinguishable at a glance:

```
Pack shape: canonical (manifest.cbor); dw-application sidecars: dw-agents.json, secrets-policy.json
```

### 5b. DW application pack (new)

```
Pack shape: DW application pack (manifest.json + metadata.json, written by greentic-designer)
Pack: pack.dw.support-triage.9f2a1c
Manifest id: support-triage
Display name: Support Triage Worker
Executing flow: flows/main.ygtc
Knowledge: knowledge_corpus.json (4 assets under assets/knowledge/)

Checked: manifest.json schema, metadata.json, declared kind, flows/main.ygtc (greentic-flow doctor),
         knowledge sidecar asset references, archive entry paths
Not checked (not part of this pack shape): SBOM, signature, component lock, component manifests, static routes

OK: no problems found.
```

With findings, the `OK:` line is replaced by the standard diagnostics block already used by
`print_human`, so severity rendering, `--format json`, and exit codes stay uniform across shapes.

### 5c. Unrecognised archive (replaces `manifest.cbor missing from archive`)

```
error: unrecognised .gtpack shape: /home/ops/exports/support-triage.gtpack

  The archive opened as a valid ZIP but matches no pack shape doctor knows.
  doctor derives the shape from the archive's top-level entries, in this order:

    canonical pack      -> a top-level `manifest.cbor` entry
    DW application pack -> a top-level `manifest.json` entry (greentic-designer export)

  Top-level entries found (3 of 7 shown): metadata.json, flows/, assets/

  If this should be a canonical pack, rebuild it: `greentic-pack build`.
  If this should be a designer export, re-export it — a DW application pack
  must carry a top-level `manifest.json`.
```

Design notes on the text:

- It names both shapes and the exact entry that decides each, so the operator can check the archive
  themselves with `unzip -l` and reach the same conclusion the tool did. The old message named one entry
  and implied it was the only possibility.
- It prints what *was* found, truncated. This is what turns "your pack is corrupt" into "your pack is
  something else"; in the partner's case the listing would have shown `metadata.json` and answered the
  question outright.
- It never says "missing" or "corrupt". Both were wrong.

### 5d. Ambiguous archive (both manifests present)

```
warning: archive contains both `manifest.cbor` and `manifest.json`
         treating it as a canonical pack (manifest.cbor wins)
         a pack should carry exactly one manifest — this is a producer bug
```

### 5e. Non-doctor callers of `open_pack`

`verify`, `sign`, `plan`, and the two provider paths (`crates/packc/src/cli/providers.rs:247`,
`crates/greentic-pack/src/bin/common/providers.rs:225`) genuinely require a canonical pack. They keep
failing, but the bare `reader.rs:381` string becomes shape-aware:

```
this archive is a DW application pack (it carries `manifest.json`, not `manifest.cbor`);
`greentic-pack verify` requires a canonical pack — run `greentic-pack doctor <pack>` to inspect it
```

and, for the genuinely unrecognised case:

```
`manifest.cbor` missing from archive, and no other known pack shape matched;
run `greentic-pack doctor <pack>` for details
```

This is a one-call-site change reusing `detect_archive_shape`, and it is why the improvement reaches
`verify`/`sign`/`providers` without touching them.

---

## Architecture

```
crates/greentic-pack/src/archive_shape.rs        NEW  pure detector + unit tests, no `native` gate
crates/greentic-pack/src/lib.rs                  edit `pub mod archive_shape;` + re-export
crates/greentic-pack/src/reader.rs:378-381       edit shape-aware error text (5e)
crates/greentic-pack/src/reader.rs               edit split entry-reading from canonical decoding so the
                                                      DW path reuses `read_archive_entries` (F6)
crates/packc/src/cli/inspect.rs:130-137          edit branch on shape before `inspect_pack_file`
crates/packc/src/cli/inspect.rs:675              edit `print_human` prepends the shape line (5a)
crates/packc/src/cli/doctor_dw.rs                NEW  D2-D7 -> Vec<Diagnostic>, plus DW human report (5b)
```

Three constraints the implementation must hold:

1. **The DW branch reuses `Diagnostic` / `Severity` / `ValidationReport`** from `greentic_types::validate`.
   It does not invent a parallel report type. `--format json` gains one field — `"archive_shape"` — and its
   `"validation"` block keeps the same schema for every shape.
2. **`open_pack`'s signature and canonical-path semantics do not change.** Its callers
   (`verify`, `sign`, `plan`, `providers`, `info`) are correct to demand a canonical pack. Only the error
   *text* at `reader.rs:381` changes. Doctor gets a new entry point rather than a widened `open_pack`.
3. **`archive_shape.rs` stays IO-free.** `detect_archive_shape(&BTreeSet<String>)` is a total function over
   entry names, so shape tests need no fixture files and the module compiles for `wasm32-wasip2`.

### Blast radius

- `inspect` — same change (F1), by construction.
- `verify` / `sign` / `plan` / `providers` — better error text (5e), unchanged pass/fail behaviour.
- `info` — unchanged in this design. It calls `open_pack` and will still refuse a DW pack, now with the
  better message. Making `info` shape-aware is a reasonable follow-up, explicitly out of scope.
- Canonical packs — behaviour identical apart from one new leading line. Every existing test that asserts on
  canonical `doctor` output must be checked against that line.

---

## Verification approach

Per `CLAUDE.md`, the gate is `ci/local_check.sh` (fmt, interfaces-bindings import guard, clippy
`-D warnings`, build, tests, builder-demo determinism, canonical gtpack generation), plus
`cargo check --target wasm32-wasip2` for the WASM lane. Tests follow the repo's conventions:
`tempfile::tempdir()` for isolation, `assert_cmd` for CLI integration, fixtures under
`crates/packc/tests/fixtures/`.

Coverage the plan must produce:

- Unit tests on `detect_archive_shape` — canonical, DW, both-present, neither, and the
  `assets/i18n/_manifest.json`-must-not-match regression (the `inspect.rs:576-586` trap).
- A DW fixture `.gtpack` built in-test with the `zip` crate, byte-faithful to
  `dw_application_pack.rs:422-530`: `manifest.json` + `metadata.json`, plus optional
  `flows/main.ygtc` and knowledge sidecar variants.
- CLI tests asserting: DW pack exits 0 and prints the shape line; a DW pack with a dangling knowledge asset
  exits 1 with `PACK_DW_KNOWLEDGE_DANGLING_ASSET`; a `metadata.json` with a wrong `kind` exits 1 with
  `PACK_DW_KIND_MISMATCH`; a zip with neither manifest exits 1 and the message names both shapes and does
  **not** contain the string `manifest.cbor missing from archive`.
- A regression test that a canonical pack's diagnostics are unchanged and that DW output contains none of
  `sbom`, `signature`, or `PACK_MANIFEST_UNSUPPORTED`.

`greentic-flow` may not be installed in every environment; D4b's existing warn-and-skip path
(`crates/packc/src/cli/inspect.rs:264-277`) means flow-doctor coverage must be asserted through the
skip-warning, not by requiring the binary.

---

## Out of scope

- Making `greentic-pack build` consume an `AnswerDocPackSpec`, or converting a DW pack into a canonical one.
- Reconciling F4 (`PackKind::DwApplication` exists in `greentic-pack-lib` but is unreachable because
  `build.rs:1422` downgrades to `Application`).
- Making `info`, `verify`, or `sign` succeed on a DW pack. They get better errors, not new capabilities.
- Any change to `greentic-setup`, which already accepts these packs and is not part of the complaint.
- Signing or SBOM-ing DW application packs.

---

## Corrections found during implementation

Recorded here rather than edited in place, so the design and what it cost stay
comparable.

**C1 — the provider readers do not inherit the message "for free".** The
"Blast radius" section said `verify` / `sign` / `plan` / `providers` pick up the
better error via the one change at `reader.rs:381`. `verify`, `sign` and `plan`
do, because they go through `open_pack`. The two provider readers do not:
`crates/packc/src/cli/providers.rs` and
`crates/greentic-pack/src/bin/common/providers.rs` walk the zip themselves and
call `archive.by_name("manifest.cbor")` directly. Both were updated explicitly.
This is why the message lives in `archive_shape` as a public
`non_canonical_archive_message(&BTreeSet<String>)` rather than as a private
helper in `reader`.

**C2 — `cargo check --target wasm32-wasip2` fails on the workspace default
features, and did so before this work.** `greentic-pack-lib`'s default feature
is `native`, which pulls in tokio, and tokio refuses to build for wasm with
those features. The meaningful check for the wasm lane is
`cargo check --target wasm32-wasip2 -p greentic-pack-lib --no-default-features`,
which passes with `archive_shape` included. The plan's Task 1 Step 3 command
should have said so.

**C3 — `Severity::Info` exists**, so the Info-severity rows in the D2-D7 table
(D4c, D7) map directly onto `greentic_types::validate::Severity` with no
workaround. The design did not check this; it happened to be true.

**C4 — the knowledge sidecar indexes two paths per file, not one.** The design's
D5 said "every asset path it indexes". The real annotation shape
(greentic-designer `orchestrate::kb_attacher::KbAnnotationFile`) is
`{asset_path, original_name, chars, vectors_asset_path?}` — the optional
`vectors_asset_path` points at a precomputed vectors asset and is equally
capable of dangling. Both are checked.

**C5 — the shared flow-doctor spawn became its own module.** The plan said to
factor it out; it landed as `crates/packc/src/cli/flow_doctor.rs` exposing a
three-arm `FlowDoctorOutcome`. The canonical path's behaviour is preserved
exactly, including "report `PACK_FLOW_DOCTOR_UNAVAILABLE` once and stop asking",
which is a property of the caller loop rather than of the spawn.
