# PR-01 Public WIT Completion

Date: 2026-02-18

## What was implemented
- Removed committed canonical `greentic:component@0.6.0` WIT duplicates from this repo.
- Ensured v0.6 test fixture uses the canonical guest wrapper macro:
  - `crates/packc/tests/fixtures/components/noop-component-v06-src/src/lib.rs`
  - Uses `greentic_interfaces_guest::export_component_v060!`.
- Added guard script to prevent future canonical WIT copies:
  - `ci/check_no_duplicate_canonical_wit.sh`
- Verified duplicate guard passes:
  - `bash ci/check_no_duplicate_canonical_wit.sh` => `OK: No canonical greentic:component WIT found.`
- Ensured CI wiring calls the guard script in workflow steps.

## Validation status
- `cargo build`: passes.
- `./ci/local_check.sh`: executed; formatting/clippy/build and many test suites passed during this run.
- Note: the active local-check session includes long-running integration tests and was still running at time of writing.
