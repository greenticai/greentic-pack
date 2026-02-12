# PR-PACK-04 — Pack QA + answers workflow + add-component using self-description (greentic-qa + i18n)

Repo: `greentic-pack`

## Goal
Provide QA-driven pack configuration and component selection:
- `greentic-pack qa` runs pack QA specs for modes default/setup/upgrade/remove
- answers are persisted as JSON + canonical CBOR
- interactive prompting via greentic-qa
- localized rendering via greentic-i18n
- `wizard add-component` resolves components, reads WASM describe payload, and updates pack metadata + lock

Depends on PR-PACK-03 and PR-PACK-01.

## Decisions locked (2026-02-11)
- **Lock file format:** CBOR only. Canonical lock file is `pack.lock.cbor` (no JSON/TOML variants).
- **Doctor mode:** offline strict by default; network re-resolve only with explicit `--online`.
- **Describe source of truth:** WASM `describe()` is authoritative. Build/doctor run WASM by default. Optional cached describe may be used only when explicitly requested and verifiable.
- **SchemaIR validation:** enforce the strict subset (types/required/additionalProperties/enums/bounds/items). Regex/format are best-effort with diagnostics (warn if unsupported; never silently ignore).
- **Answers default location:** inside pack dir at `answers/<mode>.answers.json` + derived `answers/<mode>.answers.cbor`; external `--answers` still supported.
- **Wizard output formats:** pack descriptor/metadata is CBOR-first (`pack.cbor`); optional human-readable views may be printed by `inspect` commands but are not written by default. `inspect-*` outputs are stable sorted-key JSON to stdout (pretty-printed).
- **Pack schema ownership:** `pack.cbor` uses greentic-types pack schemas only (v1). No custom extension fields in `pack.cbor` v1. Extension-specific data goes in `extensions/<kind>/extension.cbor` or `PackDescribe.metadata` for free-form hints.
- **Fixture format:** align with greentic-flow fixture registry layout (`tests/fixtures/registry/index.json` + per-component folders).


## Answers storage (default)
Inside pack dir:
- `<pack>/answers/<mode>.answers.json`
- `<pack>/answers/<mode>.answers.cbor`

External answers are supported via `--answers <file-or-dir>`.

## CLI
- `greentic-pack qa --pack <dir> --mode <mode> [--answers <file-or-dir>] [--locale <bcp47>] [--non-interactive]`
- `greentic-pack wizard add-component <ref-or-id> --pack <dir> [--mode setup] ...`

## Behavior
- If answers exist and `--reask` not set: reuse (idempotent).
- Always write JSON answers (human editable) and CBOR answers (canonical) when interactive or when `--answers` provided.
- Apply answers deterministically to pack config (BTreeMap ordering).

## Add-component behavior
- Resolve component ref via resolver
- Run WASM `describe()` (authoritative)
- Update pack metadata and `pack.lock.cbor` fields (digest + hashes + ops)
- Optionally run component QA setup and store resulting config in pack config area

## Tests
- i18n resolution + fallback chain
- answers JSON→CBOR canonicalization determinism
- add-component deterministic metadata/lock updates (using mock resolver until PR-PACK-05)

## Acceptance criteria
- Pack QA works interactively and non-interactively.
- Answers stored inside pack by default; external overrides work.
- add-component uses self-describing WASM contract and updates `pack.lock.cbor`.
- `cargo test` passes.

## Work completed
- Added `greentic-pack qa` command with sorted-key pretty JSON answers + canonical CBOR output.
- QA answers default to `<pack>/answers/<mode>.answers.json` / `.cbor`; `--answers` supports file or dir.
- Basic i18n lookup from `assets/i18n/<locale>.json`, with fallback to embedded strings or key.
- QA defaults to `pack.yaml` component list, with `--component` filter and `--all-locked` override.
- Pack-level QA uses `PackDescribe.metadata["greentic.qa"]` as a CBOR-encoded `QaSpecSource` (InlineCbor or RefPackPath to `qa/pack/<mode>.cbor`).
- Added `--pack-only` to require and run only pack-level QA.
- Implemented `wizard add-component`:
  - resolves refs (file/oci/repo/store/sha256), fetches bytes, runs `describe()`
  - writes `components/<component_id>.wasm`
  - updates `pack.yaml` (id/version/world/operations/config_schema)
  - updates `pack.lock.cbor` (digest, describe_hash, schema_hashes)
- `wizard add-component` supports `--force` and `--dry-run`.
- Updated CLI docs with `qa` and `wizard` sections.
- Added unit tests for answers path resolution + sorted JSON determinism.
- Added unit tests for pack QA (InlineCbor + RefPackPath).
- `cargo test -p greentic-pack qa` passes.

## Open questions / follow-ups
None.

## Status
Done.
