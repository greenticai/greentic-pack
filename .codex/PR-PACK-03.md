# PR-PACK-03 — Pack wizard: deterministic skeletons for app + extension packs (CBOR + i18n-first)
Status: done (2026-02-12)

Repo: `greentic-pack`

## Goal
Add a pack wizard that generates deterministic skeletons for:
- application packs
- extension packs (provider extensions, etc.)

Outputs are CBOR-first and i18n-first.

## Decisions locked (2026-02-11)
- **Lock file format:** CBOR only. Canonical lock file is `pack.lock.cbor` (no JSON/TOML variants).
- **Doctor mode:** offline strict by default; network re-resolve only with explicit `--online`.
- **Describe source of truth:** WASM `describe()` is authoritative. Build/doctor run WASM by default. Optional cached describe may be used only when explicitly requested and verifiable.
- **SchemaIR validation:** enforce the strict subset (types/required/additionalProperties/enums/bounds/items). Regex/format are best-effort with diagnostics (warn if unsupported; never silently ignore).
- **Answers default location:** inside pack dir at `answers/<mode>.answers.json` + derived `answers/<mode>.answers.cbor`; external `--answers` still supported.
- **Wizard output formats:** pack descriptor/metadata is CBOR-first (`pack.cbor`); optional human-readable views may be printed by `inspect` commands but are not written by default. `inspect-*` outputs are stable sorted-key JSON to stdout (pretty-printed).
- **Pack schema ownership:** `pack.cbor` uses greentic-types pack schemas only (v1). No custom extension fields in `pack.cbor` v1. Extension-specific data goes in `extensions/<kind>/extension.cbor` or `PackDescribe.metadata` for free-form hints.
- **Fixture format:** align with greentic-flow fixture registry layout (`tests/fixtures/registry/index.json` + per-component folders).


## CLI
- `greentic-pack wizard new-app <id> --out <dir> [--locale <bcp47>]`
- `greentic-pack wizard new-extension <id> --kind <kind> --out <dir> [--locale <bcp47>]`

## Generated structure (recommended)
<pack>/
  pack.cbor
  assets/i18n/en.json
  components/
  flows/
  extensions/<kind>/
  README.md

Notes:
- `pack.cbor` is canonical typed metadata (PackInfo/PackDescribe).
- Human-readable views are produced by `greentic-pack inspect-pack --pack <dir>` as **stable sorted-key JSON** to stdout (pretty-printed), not written by default.

## Template requirements
- All user-facing strings are stored as `I18nText` keys (with fallback in the i18n bundle).
- Deterministic file emitter (no timestamps).
- Extension packs include `extensions/<kind>/extension.cbor` (or similar typed CBOR) describing extension metadata.

## Implementation tasks
1) Add `wizard` module + deterministic file writer.
2) Implement CBOR pack descriptor writer using greentic-types pack schemas.
3) Generate i18n keys + initial `en.json`.
4) Tests:
   - golden snapshot tests for generated tree and `pack.cbor` decode
   - deterministic output across runs

## Acceptance criteria
- Wizard generates deterministic pack skeletons.
- Pack metadata is CBOR-first (`pack.cbor`) and i18n-first.
- `inspect-pack` output is stable pretty JSON with sorted keys (deterministic across runs).
- `cargo test` passes.

## Implementation summary
- Added `greentic-pack wizard` with `new-app` and `new-extension` subcommands.
- Generates deterministic skeletons: `pack.cbor`, `assets/i18n/<locale>.json`, `components/`, `flows/`, and `extensions/<kind>/` for extensions.
- Uses typed `PackDescribe` (greentic-types v0.6.0) encoded as canonical CBOR.
- Added tests in `crates/packc/tests/wizard.rs` for file creation + deterministic output.
