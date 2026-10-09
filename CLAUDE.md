# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Rust workspace for building, packaging, and inspecting Greentic packs — portable, signed bundles of flows, components, assets, and metadata shipped as `.gtpack` archives. The CLI binary is `greentic-pack` (published from `crates/packc`).

## Build & Development Commands

```bash
# Build
cargo build --workspace --locked

# Test (all)
cargo test --workspace --locked -- --nocapture

# Test (single test file, e.g. build_pipeline)
cargo test --test build_pipeline --locked -- --nocapture

# Test (single test function)
cargo test --test build_pipeline build_does_not_copy_component_directory -- --nocapture

# Test (single crate)
cargo test -p greentic-pack --locked -- --nocapture

# Lint
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Full local CI (run before submitting PRs)
ci/local_check.sh
```

`ci/local_check.sh` runs: format check, interfaces bindings import guard, clippy, build, tests, builder demo determinism, and canonical gtpack generation. It auto-installs `greentic-component` via `cargo binstall` if missing. Env-var defaults: `LOCAL_CHECK_ONLINE=1` (on), `LOCAL_CHECK_STRICT=0` (off), `LOCAL_CHECK_VERBOSE=0` (off). Override as needed.

`ci/check_no_duplicate_canonical_wit.sh` guards against accidentally defining canonical `greentic:component` WIT in this repo (greentic-pack must not own that package).

`scripts/version-tools.sh` provides version helper functions used during release workflows. `tools/i18n.sh` manages i18n string extraction/updates for pack wizard prompts.

## Workspace Structure

| Crate | Package Name | Role |
|-------|-------------|------|
| `crates/packc` | `greentic-pack` | CLI binary and library — build, lint, sign, verify, doctor, plan, wizard |
| `crates/greentic-pack` | `greentic-pack-lib` | Core library — pack reader, builder, plan generator, archive parsing |
| `crates/pack_component` | `pack_component` | Generated Wasm component exposing pack via `greentic:pack-export` interface |
| `crates/pack_component_template` | `pack_component_template` | Template strings for generating component crate code |

`examples/` contains demo packs (weather, qa, billing, search, reco, adaptive-mcp-oauth-demo). `docs/` has usage guides, pack format specs, and CLI reference.

## Architecture

- **Pack format**: `.gtpack` is a ZIP archive containing `manifest.cbor`, `sbom.cbor`, flows (`.ygtc`), component Wasm binaries, assets, and optional signatures.
- **Build pipeline**: `pack.yaml` → resolve components into `pack.lock.cbor` → compile flows → **i18n materialisation** (see below) → generate Wasm component from template → produce canonical `.gtpack`.
- **i18n materialisation step**: if the `wizard apply` answers include a `langs` field (a JSON string array, e.g. `["id","ja"]`), the build extracts translatable strings from `assets/cards/*.json` into `assets/i18n/en.json`, then invokes the `greentic-i18n-translator` binary once per requested language to produce `assets/i18n/<lang>.json`, and finally writes `assets/i18n/_manifest.json` (a sorted JSON array of all locale codes, always including `en`). Existing locale files (hand-authored or from a previous build) are kept as-is (carry-over). The translator binary is located via `GREENTIC_I18N_TRANSLATOR_BIN` / `GREENTIC_I18N_TRANSLATOR_DEV_BIN`, then `PATH`. The step is **non-fatal**: if the binary is absent or a language fails, the build succeeds and the skipped languages are reported on stderr.
- **Pack kinds**: application, infrastructure, provider, distribution-bundle.
- **Component model**: Uses WebAssembly Component Model (wasmtime 45, wit-bindgen 0.54, wit-component 0.247). Components expose the `greentic:pack-export` interface.
- **Signing**: Ed25519 via `ed25519-dalek` for manifest signing/verification.

## Key Conventions

- **Workspace version**: `1.2.0-dev.0` (dev lane). Dual-role binary crate — subject to binary bifurcation on dev publish (`greentic-pack` lib publishes as pre-release, binary publishes as `greentic-pack-dev`).
- **WASM toolchain variant**: This repo targets `wasm32-wasip2` (see `rust-toolchain.toml`). Run `cargo check --target wasm32-wasip2` to validate WASM compilation alongside host checks.
- `#![forbid(unsafe_code)]` — no unsafe code allowed.
- Rust 2024 edition, pinned to rustc 1.95.0 (`rust-toolchain.toml`).
- Use `greentic_interfaces::canonical` — never import from `greentic_interfaces::bindings::*` (enforced by `ci/check_no_interfaces_bindings_imports.sh`).
- Prefer existing Greentic shared crates (interfaces, types, secrets, oauth, messaging, events) over re-defining types locally.
- Error handling: `anyhow::Result<T>` with `.context()` for propagation; `thiserror` for domain-specific errors.
- Tests use `tempfile::tempdir()` for isolation and `assert_cmd` for CLI integration tests.
- Update `.codex/repo_overview.md` before and after PR work. See also `.codex/global_rules.md` for Codex workflow conventions.

## Gotchas

- `examples/` contains workspace-excluded generated `pack_component` dirs (e.g. `examples/weather-demo/.packc/pack_component`, `examples/qa-demo/.packc/pack_component`); these are build artifacts, not source.
- CI build and test steps use `--locked` (`cargo build --workspace --locked`, `cargo test --workspace --locked`); local edits that change dependency resolution require updating `Cargo.lock` first.
- Dev-lane publish is dual-role bifurcated: the publish list is `greentic-pack-lib`, `pack_component`, `pack_component_template`, `greentic-pack`. The binary publishes as `greentic-pack-dev` on the dev lane; the library publishes as a pre-release under its canonical name.

## CI Workflows (`.github/workflows/`)

| File | Name | Triggers |
|------|------|----------|
| `branch-invariants.yml` | Branch Invariants | push, pull_request |
| `ci.yml` | CI & Publish | workflow_call, push, pull_request |
| `codeql.yml` | CodeQL | push, schedule |
| `codex-security-fix.yml` | Codex Security Fix | schedule, workflow_dispatch |
| `codex-semver-fix.yml` | Codex Semver Fix | schedule, workflow_dispatch |
| `crates-publish.yml` | Publish to crates.io | push, workflow_dispatch |
| `dependabot-automerge.yml` | Dependabot auto-merge | pull_request_target |
| `dependency-review.yml` | Dependency Review | pull_request |
| `dev-publish.yml` | Dev Publish | push, workflow_dispatch |
| `nightly-coverage.yml` | Nightly Coverage | schedule, workflow_dispatch |
| `notify-scheduled-failures.yml` | Notify scheduled failures | workflow_run |
| `perf.yml` | Perf (lightweight) | schedule, workflow_dispatch |
| `release-binaries.yml` | Release Binaries | push, workflow_dispatch |
| `tag-on-version-bump.yml` | Tag on Version Bump | push |
