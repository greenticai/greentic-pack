# Repository Overview

## 1. High-Level Purpose
- Rust workspace for building, packaging, and inspecting Greentic packs. Packs bundle flow definitions, templates, and metadata into `.gtpack` archives that can be verified, planned, and distributed.
- Provides developer tooling (`packc`) to validate pack sources, generate manifests/SBOMs, embed assets into a Wasm component, and sign/verify packs; an operator CLI/library (`greentic-pack`) to inspect archives and derive deployment plans; and a generated component crate (`pack_component`) that exposes the embedded pack via the `greentic:pack-export` interface.

## 2. Main Components and Functionality
- **Path:** `crates/packc`  
  **Role:** Builder CLI for authoring and validating Greentic packs; now also hosts the canonical `greentic-pack` binary plus a deprecated `packc` shim.  
  **Key functionality:** Validates `pack.yaml`, enforces pack version/kind constraints (including `distribution-bundle`), loads flow and template assets, builds `.gtpack` archives with manifests and SBOM entries, generates Wasm components via `pack_component_template`, composes MCP router + adapter components, supports component descriptors (including `kind: software` with arbitrary artifact paths/types), and handles signing/verification of manifests. Provides subcommands for build/lint/components/update/new/sign/verify/gui/doctor(aka inspect)/plan/providers/config; telemetry setup; exposes library helpers (`BuildArgs`, signing APIs).

- **Path:** `crates/greentic-pack`  
  **Role:** Library for inspecting packs and producing deployment plans (binaries now live in `packc`).  
  **Key functionality:** `reader` parses `.gtpack` archives, verifies hashes/signatures, and exposes manifest contents (including component manifest index helpers and SBOM reading); `plan` builds deployment plans (optionally shelling out to `packc` when given a source directory); `builder` defines pack metadata (now includes `distribution-bundle` kind, distribution section, and component descriptors with optional `software` kind/`artifact_type` labels), SBOM entries, signing helpers, and archive writing; `events`/`messaging`/`repo` schemas validate sections.

- **Path:** `crates/pack_component`  
  **Role:** Generated Wasm component that embeds manifest, flows, and templates produced by `packc`.  
  **Key functionality:** Exposes `manifest_*` helpers (CBOR/raw/typed), accessors for embedded flows/templates, and a `PackExport` implementation with C ABI shims; `run_flow` returns an `ok` status with the flow source payload plus echoed input for quick inspection of embedded flows without full execution.

- **Path:** `crates/pack_component_template`  
  **Role:** Template strings used by `packc` when generating the component crate; includes placeholder `data.rs`, `Cargo.toml`, and `lib.rs` mirroring the packaged `PackExport` behaviour (flow/source introspection with input echo, not full execution).

- **Path:** `docs/` and `examples/`  
  **Role:** Usage guides (CLI, publishing, pack format) and sample packs demonstrating pack structure and flows; examples include weather, QA, billing, search, and recommendation demos.

- **Path:** `.github/workflows/`  
  **Role:** CI for lint/test, publishing to crates.io, and binstall release artifacts; pushes to `master` (or manual dispatch) run CI, publish crates, build binstall archives (`.tgz`), and upload them to a GitHub Release—no tag gating.

## 3. Work In Progress, TODOs, and Stubs
- Component manifest index extension fully surfaced (reader/inspect, verification via `--verify-manifest-files`; tests for happy-path, missing file, hash-mismatch).
- `packc build` now runs `packc update` by default (skip with `--no-update`) and, when a component `wasm` path is a directory, only packages runtime artifacts (resolved wasm + manifest.cbor converted from component.json). Tests guard against copying ancillary files and ensure update runs unless explicitly skipped.
- Unified CLI surface: canonical `greentic-pack` binary (hosted in `packc`) now exposes the full packc command set plus `doctor` (inspect alias); `packc` binary emits a deprecation warning and delegates to the same runner. Local checks use `greentic-pack doctor` in place of `gtpack-inspect`.
- New read-only helpers for flow resolve sidecars (`*.ygtc.resolve.json`) and deterministic `pack.lock.json` (schema_version=1) parsing/writing; discovery emits warnings when sidecars are missing but does not alter build behaviour.
- New `greentic-pack resolve` command: reads flow resolve sidecars, resolves remote refs via greentic-distributor-client (supports `--offline`), computes digests for locals, and writes deterministic `pack.lock.json` (configurable via `--lock`).

## 4. Broken, Failing, or Conflicting Areas
- No failing tests or known broken areas after the latest run.

## 5. Notes for Future Work
- None outstanding beyond normal maintenance.
