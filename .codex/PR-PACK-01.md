# PR-PACK-01 — Pack contract lock (CBOR-only) for component@0.6.0 self-describing components

Repo: `greentic-pack`

## Goal
Make pack builds deterministic and contract-driven for component@0.6.0 by emitting a **CBOR-only lock** capturing:
- resolved component digests
- ABI version
- `describe_hash` (sha256 over canonical CBOR of typed describe payload)
- per-operation `schema_hash`
- optional `world` / `component_version` / `role`

This is the “locksmith” foundation: packs can later be validated strictly and drift becomes detectable.

## Decisions locked (2026-02-11)
- **Lock file format:** CBOR only. Canonical lock file is `pack.lock.cbor` (no JSON/TOML variants).
- **Doctor mode:** offline strict by default; network re-resolve only with explicit `--online`.
- **Describe source of truth:** WASM `describe()` is authoritative. Build/doctor run WASM by default. Optional cached describe may be used only when explicitly requested and verifiable.
- **SchemaIR validation:** enforce the strict subset (types/required/additionalProperties/enums/bounds/items). Regex/format are best-effort with diagnostics (warn if unsupported; never silently ignore).
- **Answers default location:** inside pack dir at `answers/<mode>.answers.json` + derived `answers/<mode>.answers.cbor`; external `--answers` still supported.
- **Wizard output formats:** pack descriptor/metadata is CBOR-first (`pack.cbor`); optional human-readable views may be printed by `inspect` commands but are not written by default.
- **Fixture format:** align with greentic-flow fixture registry layout (`tests/fixtures/registry/index.json` + per-component folders).
- **Inspect output:** `inspect-*` commands emit stable sorted-key JSON to stdout (pretty-printed), machine-diffable across runs.


## Scope
### In-scope
- Define/extend a typed pack lock structure and emit it as **canonical CBOR** (`pack.lock.cbor`).
- Populate lock entries by resolving each component and introspecting WASM `describe()` (CBOR).
- Introduce or formalize a resolver abstraction (`ComponentResolver`) suitable for distribution and `fixture://`.
- Deterministic ordering guarantees (BTreeMap + sorted operation lists).
- Tests for canonical encoding + deterministic output.

### Out-of-scope
- Strict doctor enforcement (PR-PACK-02)
- Wizard skeleton generation (PR-PACK-03)
- Pack QA + answers (PR-PACK-04)
- Fixture resolver + CI harness (PR-PACK-05)

## Files / formats
### Canonical lock output
- `pack.lock.cbor` (CBOR only)

Optional human inspection is via CLI (`inspect-lock`) printing **stable sorted-key JSON** to stdout; no extra files are written.

## Lock schema (v1)
`PackLock` (CBOR):
- `version: u32` (start at 1)
- `components: BTreeMap<String, LockedComponent>`

`LockedComponent`:
- `component_id: String`
- `ref: Option<String>` (oci://… / store://… / file://…)
- `abi_version: String` (e.g. "0.6.0")
- `resolved_digest: String`
- `describe_hash: String`
- `operations: Vec<LockedOperation>` (sorted by operation_id)
- `world: Option<String>`
- `component_version: Option<String>`
- `role: Option<String>`

`LockedOperation`:
- `operation_id: String`
- `schema_hash: String`

Determinism rules:
- maps are BTreeMap
- operation list sorted by `operation_id`
- no timestamps in canonical lock (avoid non-reproducible output)

## Implementation tasks
1) **Add lock structs**
- `src/lock.rs` (or existing model module)
- serde defaults for additive evolution (unknown fields ignored)

2) **Resolver abstraction**
- Introduce `trait ComponentResolver { fn resolve(&self, req: ResolveReq) -> Result<ResolvedComponent> }`.
- Provide a real resolver impl using existing distribution/OCI resolution logic.
- Keep API compatible with future `fixture://`.
 - `ResolvedComponent` should return:
   - wasm bytes (or a path/handle)
   - resolved_digest
   - component_id
   - abi_version
   - optional world, component_version (if cheaply derivable; otherwise fill after describe())

3) **Introspection during build**
For each included component:
- resolve ref → digest + wasm bytes (or wasm path)
- run WASM `describe()` (authoritative)
- decode typed `ComponentDescribe` (greentic-types)
- compute:
  - `describe_hash = sha256(canonical_cbor(typed_describe))`
  - `schema_hash` per op (recompute and/or trust embedded then verify later in PR-PACK-02)
- store into `pack.lock.cbor`

4) **Canonical encoding**
- Ensure CBOR encoding is canonical (map ordering etc.)
- Ensure stable bytes output across runs/machines

5) **Tests**
- roundtrip: encode→decode→encode yields identical bytes for known fixture lock
- deterministic ordering tests for components + operations

## CLI changes
- `greentic-pack build` now emits `pack.lock.cbor`.
- Add `greentic-pack inspect-lock --lock pack.lock.cbor` printing **stable sorted-key JSON** to stdout (pretty-printed). This is machine-diffable across runs. If a human summary is desired, add `--human` later; default stays stable JSON.

## Acceptance criteria
- `pack.lock.cbor` is emitted and deterministic.
- Lock contains digest + describe_hash + per-op schema_hash.
- `inspect-lock` output is stable pretty JSON with sorted keys (deterministic across runs).
- `cargo test` passes.
