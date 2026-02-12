# PR-PACK-02 — Pack doctor strict: enforce component@0.6.0 self-description + SchemaIR constraints + i18n/QA

Repo: `greentic-pack`

## Goal
Make `greentic-pack doctor` enforce “WASM is contract authority” for component@0.6.0:
- Validate packaged components are self-describing via WASM `describe()`
- Enforce inline SchemaIR is constrained (strict by default)
- Verify `describe_hash` and `schema_hash` against `pack.lock.cbor`
- Validate QA + i18n surface correctness

Depends on PR-PACK-01 (CBOR-only lock).

## Decisions locked (2026-02-11)
- **Lock file format:** CBOR only. Canonical lock file is `pack.lock.cbor` (no JSON/TOML variants).
- **Doctor mode:** offline strict by default; network re-resolve only with explicit `--online`.
- **Describe source of truth:** WASM `describe()` is authoritative. Build/doctor run WASM by default. Optional cached describe may be used only when explicitly requested and verifiable.
- **SchemaIR validation:** enforce the strict subset (types/required/additionalProperties/enums/bounds/items). Regex/format are best-effort with diagnostics (warn if unsupported; never silently ignore).
- **Wizard output formats:** `inspect-*` outputs are stable sorted-key JSON to stdout (pretty-printed).


## Doctor behavior
### Default: offline strict
- `greentic-pack doctor --pack <dir>` runs offline strict by default:
  - uses local packaged artifacts and `pack.lock.cbor`
  - does not network resolve unless `--online` set

### Optional: online drift check
- `--online` re-resolves components and confirms digest/describe_hash still match.

## Validations
For each locked component (abi 0.6.0):
1) Load component artifact bytes from pack (`components/<id>.wasm`) or component sources extension; fallback to cache or `file://` ref.
2) Load WASM via wasmtime and call:
   - `component-descriptor.describe()`
   - `component-i18n.i18n-keys()`
   - `component-qa.qa-spec(mode)` for all modes
3) Decode typed `ComponentDescribe` and validate:
   - schema hashes recompute cleanly
4) Hash checks:
   - recompute `describe_hash = sha256(canonical_cbor(typed_describe))` equals lock
   - recompute op `schema_hash = sha256(canonical_cbor({input,output,config}))` equals:
     - the schema_hash embedded in `describe()` for that op
     - the lock’s stored schema_hash
   Any mismatch is a failure (component didn’t lie + lock is accurate).
5) SchemaIR strict checks:
   - reject empty object with permissive additionalProperties
   - enforce required/types/enums/bounds/items/additionalProperties
   - regex/format best-effort: emit diagnostic if present but unsupported
6) QA + i18n:
   - `component-i18n.i18n-keys()` exists and is callable for all 0.6.0 components
   - QA specs decode for default/setup/upgrade/remove
   - i18n keys referenced by QA specs are a subset of i18n-keys

## Diagnostics
- `Diagnostic { code, severity, message, path?, hint? }`
- stable ordering by (component_id, code, path)
- `--format human|json` for output

## Implementation summary
- Added pack lock doctor in `crates/packc/src/pack_lock_doctor.rs`.
- Wired doctor into `greentic-pack doctor` when component doctor is enabled.
- Added CLI flags `--online` and `--use-describe-cache` for pack lock checks.
- Included `pack.lock.cbor` in built packs (even prod) and allowed it during prod inspection.
- Added targeted tests in `crates/packc/tests/pack_lock_doctor.rs`:
  - missing pack.lock emits `PACK_LOCK_MISSING`
  - invalid component bytes emit `PACK_LOCK_COMPONENT_DECODE_FAILED` (with describe cache)
- Resolved indexmap version conflicts by:
  - bumping `serde_yaml_bw` to `2.5.2`
  - downgrading `wit-component` to `0.244`
  - pinning workspace `indexmap` to `2.12.1` (direct dependency in pack crates)

## Acceptance criteria
- Doctor fails early on invalid self-description by default.
- Hashes verified deterministically vs `pack.lock.cbor`.
- Offline is default; online requires explicit flag.
- `cargo test` passes.
