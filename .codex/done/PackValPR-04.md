# PackValPR-04 — Add Validator-as-Pack/WASM support to `greentic-pack doctor` (no hardcoded domains)

**Repo:** `greenticai/greentic-pack`

## Goal
Extend `greentic-pack doctor` so it can discover and run **validator components** shipped as WASM (optionally inside validator packs), without hardcoding messaging/events/secrets validators into greentic-pack.

Validators are discovered from:
1) The **target pack itself**, via provider extensions that declare validator references; and/or
2) Optional `--validator-pack` / `--validators-root` inputs.

Validators implement the WIT world `greentic:pack-validate@0.1.0/pack-validator` and return diagnostics that map to `greentic-types` validation model.

## Non-goals
- Do not implement messaging/events/secrets rules here.
- Do not redesign pack formats.
- Do not require network; OCI fetching must be optional and cacheable.

## Deliverables

### A) CLI and configuration
Add flags to `greentic-pack doctor`:

- `--validate` (default ON)
- `--no-validate`
- `--format human|json` (keep existing `--json` alias)
- `--validators-root <dir>` (default: `.greentic/validators`)
- `--validator-pack <path|oci://...>` (repeatable)
- `--validator-allow <prefix>` (repeatable; allowlist for OCI namespaces, default `oci://ghcr.io/greenticai/validators/`)
- `--validator-cache-dir <dir>` (default `.greentic/cache/validators`)
- `--validator-policy required|optional` (default `optional`)
  - `required`: if a pack declares a validator ref and it cannot be loaded, fail validation
  - `optional`: load if possible, otherwise warn

### B) Discovery: “validator refs” from provider extensions
Implement a generic mechanism to extract validator references from the target pack manifest:
- If the manifest contains provider extension blocks, support a field like:
  - `validator_ref` (string) and optional `validator_digest` (sha256)
- If the manifest does not yet have explicit validator fields, support annotations:
  - `meta.annotations["greentic.validators"]` = JSON array of refs
This PR should support at least one mechanism that works immediately without requiring domain-specific code.

### C) Fetching and caching validator packs/components
Support loading validators from:
- Local `.gtpack` path
- Local directory (`--validators-root`) containing `.gtpack` files
- OCI refs (`oci://...`) when HTTP is enabled and allowed by allowlist

Caching:
- Cache downloaded validator packs under `--validator-cache-dir`
- Use digest pinning when provided (`validator_digest`); if digest mismatches, treat as error

### D) Executing validator WASM safely
Run validators with Wasmtime component model:
- No filesystem access
- No network access
- Provide only `PackInputs`:
  - `manifest_cbor` bytes from target pack
  - `sbom.json` string from target pack
  - `file_index` list from target pack zip entries
- Add resource limits:
  - max memory (configurable, default reasonable)
  - epoch interruption / timeout (configurable, default e.g. 2s per validator)

Validator selection:
- For each loaded validator component:
  - call `applies(inputs)`; if false, skip `validate`
  - call `validate(inputs)`; collect diagnostics

### E) Reporting
Extend doctor JSON output to include:
- `"validation": { "diagnostics": [...], "has_errors": true/false, "sources": [...] }`
Where `sources` lists which validators ran and where they came from (path/oci).

Human output:
- Add a `Validation:` section after existing warnings.

Exit codes:
- Non-zero if:
  - core validators have errors OR
  - validator packs produce Error diagnostics OR
  - policy is `required` and a declared validator could not be loaded

### F) Tests
Add fixtures:
1) `tests/fixtures/validators/noop-validator.gtpack` — a validator pack whose validator returns one WARN
2) A target pack fixture (can reuse hello2) to run doctor against
Test cases:
- doctor loads local validator pack via `--validators-root` and includes its diagnostic
- allowlist blocks unknown OCI refs (unit test without network)
- `--validator-policy required` fails when validator missing

## Acceptance criteria
- `greentic-pack doctor --pack X` still works with no validators present.
- `greentic-pack doctor --pack X --validators-root tests/fixtures/validators` runs the validator and reports its diagnostics.
- No domain hardcoding in greentic-pack.
- Uses `greentic-types` validation model for output.

