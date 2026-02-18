# PR-01: Make greentic-pack a complete 0.6 QA runner + isolate schema-core

## Supersession Note
- This PR track supersedes the earlier high-level outlines in `.codex/done/PR-01.md` and `.codex/done/PR-02.md` for the 0.6 QA runner path.
- Current source of truth for this work: `.codex/PR-01-pack-as-qa-runner.md` and `.codex/PR-02-docs.md`.

## Goals
- Implement **Option A (pack IS a 0.6 QA runner)**:
  - describe -> qa-spec -> collect answers -> apply-answers -> strict validate -> persist (canonical CBOR)
- Rename mode to `update` (alias `upgrade`).
- Remove schema-core dependency from the 0.6 path (isolate legacy/provider-extension if needed).
- Eliminate pack-originated schema for 0.6 (component is authoritative).

## Implementation Steps
1) Mode rename + alias:
   - Update `QaMode` enums / labels / CLI strings: `upgrade` -> `update`.
   - In greentic-pack CLI, accept `upgrade` as deprecated alias with warning.
   - Internally always emit/display `update`.
   - Docs note: alias will be removed in a future 0.6.x/0.7 release (no date/version committed now).

2) Complete 0.6 QA orchestration (P0):
   - In `qa.rs`, after retrieving `qa_spec`, implement:
     a) ask/collect answers (existing QA engine/hooks)
     b) call component `apply-answers`
     c) decode CBOR config output
     d) strict validate against **describe.config_schema** (component-provided)
     e) canonicalize persisted CBOR
   - Ensure failures are actionable (mode, component id, schema hash, field errors).
   - Validation errors must include field paths and aggregated violations by default.
   - Where available, include structured diagnostics (for example JSON `{path, message}` list) for tests/automation.

3) Remove/gate pack-originated schema (P1):
   - Find `manifest_from_config` / `config_schema: cfg.config_schema.clone()`.
   - For 0.6 path: hard-error when schema is supplied from pack config.
   - Optional migration-only escape hatch (only if trivial/needed): `--allow-pack-schema` with warning.
   - Default remains hard-error.

4) Schema-core bridge isolation (P0):
   - Locate `config.rs` and docs requiring `greentic:provider/schema-core@1.0.0`.
   - Make 0.6 component path not depend on schema-core world.
   - Keep schema-core only in legacy/provider-extension code paths.
   - Preferred isolation mechanism: separate legacy module/command path.
   - If a switch is required, use explicit CLI routing (subcommand or `--legacy-schema-core` flag), not cargo features as the primary mechanism.

5) Canonical CBOR everywhere persisted (P1):
   - Replace `serde_cbor::to_vec` / raw serializer for persisted manifests/metadata with canonical helper.
   - Scope includes everything persisted as CBOR on the 0.6 path:
     - QA answers/state persisted by runner (if any)
     - generated/derived manifests/metadata/sidecars
     - lock artifacts (pack locks / describe hashes / provenance CBOR records)
   - Rule: if it hits disk as `.cbor` (or CBOR bytes in lock/sidecar), it must go through canonical helper.

6) Tests:
   - Unit tests for QA runner:
     - mock component with known schema and apply-answers behavior
     - verify invalid config fails schema validation
     - verify canonical output bytes are stable
   - Integration test (if harness exists): run QA default/setup/update/remove for a dummy component fixture.

7) Run:
   - `cargo fmt`
   - `cargo clippy -D warnings`
   - `cargo test`

## Acceptance Criteria
- `greentic-pack` can run full 0.6 QA lifecycle including `apply-answers`.
- Post-apply strict validation uses component schema (describe.config_schema), not pack config.
- 0.6 path has no schema-core dependency; any schema-core usage is legacy-only and clearly isolated.
- Persisted CBOR artifacts are canonical.
- Tests pass.

## Status
Done.

## Remaining items
None in this repo.

## External compatibility note
- Resolved in this repo by upgrading to `greentic-types 0.4.51`, `greentic-flow 0.4.43`, and
  `greentic-interfaces-host 0.4.94`, then removing internal `*::Upgrade` enum mappings.
- Dependency resolution was unblocked by vendoring/patching `serde_yaml_bw` with a relaxed
  `indexmap` bound and vendoring `greentic-flow` to relax its strict `greentic-interfaces` pin.
