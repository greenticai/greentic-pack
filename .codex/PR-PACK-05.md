# PR-PACK-05 — Fixture resolver + offline CI E2E for pack wizard + QA + doctor

Repo: `greentic-pack`

## Goal
Provide deterministic no-network test harness for pack workflows:
- wizard new-app/new-extension
- pack QA + answers
- wizard add-component using fixture resolver
- pack doctor strict validations

## Decisions locked (2026-02-11)
- **Lock file format:** CBOR only. Canonical lock file is `pack.lock.cbor` (no JSON/TOML variants).
- **Doctor mode:** offline strict by default; network re-resolve only with explicit `--online`.
- **Describe source of truth:** WASM `describe()` is authoritative. Build/doctor run WASM by default. Optional cached describe may be used only when explicitly requested and verifiable.
- **SchemaIR validation:** enforce the strict subset (types/required/additionalProperties/enums/bounds/items). Regex/format are best-effort with diagnostics (warn if unsupported; never silently ignore).
- **Answers default location:** inside pack dir at `answers/<mode>.answers.json` + derived `answers/<mode>.answers.cbor`; external `--answers` still supported.
- **Wizard output formats:** pack descriptor/metadata is CBOR-first (`pack.cbor`); optional human-readable views may be printed by `inspect` commands but are not written by default. `inspect-*` outputs are stable sorted-key JSON to stdout (pretty-printed).
- **Fixture format:** align with greentic-flow fixture registry layout (`tests/fixtures/registry/index.json` + per-component folders).


## Fixture registry layout (aligned with greentic-flow)
`tests/fixtures/registry/`
- `index.json`
- `components/<component_id>/describe.cbor`
- `components/<component_id>/qa_default.cbor`
- `components/<component_id>/qa_setup.cbor`
- `components/<component_id>/qa_upgrade.cbor`
- `components/<component_id>/qa_remove.cbor`
- `components/<component_id>/apply_setup_config.cbor`
- optional `components/<component_id>/component.wasm` (not required; mock-backed is default)

`tests/fixtures/packs/`
- base pack dirs and expected outputs including `pack.lock.cbor`

## Implementation tasks
1) Implement `fixture://` resolver as `ComponentResolver` impl.
2) E2E tests calling handlers directly (no subprocess):
   - create pack via wizard
   - run QA to write answers
   - add-component via fixture resolver
   - run pack doctor offline
   - compare outputs deterministically (canonical decode/encode compare)
3) CI job `cargo test -p greentic-pack` with offline guarantee.

## Acceptance criteria
- Offline CI validates pack wizard + QA + add-component + doctor end-to-end.
- Outputs deterministic; lock is CBOR-only.
- `cargo test` passes.

## Work completed
- Added `FixtureResolver` (fixture://) in `greentic-pack` and wired `wizard add-component` to use it via `GREENTIC_PACK_FIXTURE_DIR`.
- Added fixture registry under `crates/packc/tests/fixtures/registry` with a fixture component and describe cache.
- Added offline E2E test `crates/packc/tests/fixture_e2e.rs` that:
  - runs wizard new-app
  - writes pack QA spec + metadata
  - runs pack-only QA non-interactively
  - adds component via fixture resolver using describe cache
  - runs doctor (expects validation errors for fixture wasm)
- Adjusted QA to allow `--pack-only` without `pack.lock.cbor`.

## Tests
- `cargo test -p greentic-pack --test fixture_e2e`
