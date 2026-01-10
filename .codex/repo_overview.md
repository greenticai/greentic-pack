# Repository Overview

## 1. High-Level Purpose
- Rust workspace for building, packaging, and inspecting Greentic packs. Packs bundle flow definitions, templates, and metadata into `.gtpack` archives that can be verified, planned, and distributed.
- Provides the `greentic-pack` CLI (published from `crates/packc`) to validate pack sources, generate manifests/SBOMs, embed assets into a Wasm component, and sign/verify packs; the `greentic-pack-lib` library (in `crates/greentic-pack`) to inspect archives and derive deployment plans; and a generated component crate (`pack_component`) that exposes the embedded pack via the `greentic:pack-export` interface.

## 2. Main Components and Functionality
- **Path:** `crates/packc`  
  **Role:** Builder CLI for authoring and validating Greentic packs; publishes the canonical `greentic-pack` binary.  
  **Key functionality:** Validates `pack.yaml`, enforces pack version/kind constraints (including `distribution-bundle`), loads flow and template assets, builds `.gtpack` archives with manifests and SBOM entries (now `sbom.cbor`), generates Wasm components via `pack_component_template`, composes MCP router + adapter components, supports component descriptors (including `kind: software` with arbitrary artifact paths/types), and handles signing/verification of manifests. Provides subcommands for build/lint/components/update/new/sign/verify/gui/doctor(aka inspect)/plan/providers/config; telemetry setup; exposes library helpers (`BuildArgs`, signing APIs).

- **Path:** `crates/greentic-pack`  
  **Role:** `greentic-pack-lib` library for inspecting packs and producing deployment plans.  
  **Key functionality:** `reader` parses `.gtpack` archives, verifies hashes/signatures, and exposes manifest contents (including component manifest index helpers and SBOM reading); `plan` builds deployment plans (optionally shelling out to `greentic-pack` when given a source directory); `builder` defines pack metadata (now includes `distribution-bundle` kind, distribution section, and component descriptors with optional `software` kind/`artifact_type` labels), SBOM entries, signing helpers, and archive writing; `events`/`messaging`/`repo` schemas validate sections.

- **Path:** `crates/pack_component`  
  **Role:** Generated Wasm component that embeds manifest, flows, and templates produced by `greentic-pack`.  
  **Key functionality:** Exposes `manifest_*` helpers (CBOR/raw/typed), accessors for embedded flows/templates, and a `PackExport` implementation with C ABI shims; `run_flow` returns an `ok` status with the flow source payload plus echoed input for quick inspection of embedded flows without full execution.

- **Path:** `crates/pack_component_template`  
  **Role:** Template strings used by `greentic-pack` when generating the component crate; includes placeholder `data.rs`, `Cargo.toml`, and `lib.rs` mirroring the packaged `PackExport` behaviour (flow/source introspection with input echo, not full execution).

- **Path:** `docs/` and `examples/`  
  **Role:** Usage guides (CLI, publishing, pack format) and sample packs demonstrating pack structure and flows; examples include weather, QA, billing, search, and recommendation demos.

- **Path:** `.github/workflows/`  
  **Role:** CI for lint/test (now split into parallel fmt/clippy/test jobs), publishing to crates.io, and binstall release artifacts; pushes to `master` (or manual dispatch) run CI, publish crates, build `greentic-pack` binstall archives (`.tgz`), and upload them to a GitHub Release—no tag gating.

## 3. Work In Progress, TODOs, and Stubs
- None noted.

## 4. Broken, Failing, or Conflicting Areas
- None noted.

## 5. Notes for Future Work
- None outstanding beyond normal maintenance.
