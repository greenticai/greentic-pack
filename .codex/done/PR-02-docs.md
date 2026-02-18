# PR-02: Docs for 0.6 QA runner + component-authoritative schema

## Supersession Note
- This doc track supersedes the earlier broad outlines in `.codex/done/PR-01.md` and `.codex/done/PR-02.md` for the 0.6 QA runner path.
- Current source of truth for this effort: `.codex/PR-01-pack-as-qa-runner.md` and `.codex/PR-02-docs.md`.

## Goals
- Update docs to reflect pack’s 0.6 QA runner responsibility and component-authoritative schema model.

## Implementation Steps
1) Update `cli.md` / `usage.md`:
   - Modes: `default`/`setup`/`update`/`remove` (+ deprecated `upgrade` alias warning in CLI)
   - Internally/outputs always use `update`
   - Add deprecation note: `upgrade` alias will be removed in a future 0.6.x/0.7 release (no date/version committed)
   - Explain: schemas/secret requirements come from component describe, not pack assets
   - Explain: schema-core world is legacy-only and routed through explicit legacy path (module/command/flag), not default 0.6
   - Explain: 0.6 path hard-errors on pack-originated schema; mention migration-only `--allow-pack-schema` if present

2) Add a troubleshooting section:
   - schema hash mismatch
   - denied capabilities (owned by runtime/operator)
   - validation failures with field paths + aggregated violations
   - canonical CBOR expectations for persisted `.cbor` artifacts

## Acceptance Criteria
- Docs reflect 0.6 runner behavior and remove contradictory schema-core requirements for 0.6.

## Status
Done.

## Remaining items
None.
