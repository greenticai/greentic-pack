# Repository Overview

## 1. High-Level Purpose
- Rust workspace for building, packaging, and inspecting Greentic packs. Packs bundle flow definitions, templates, and metadata into `.gtpack` archives that can be verified, planned, and distributed.
- Provides the `greentic-pack` CLI (published from `crates/packc`) to validate pack sources, generate manifests/SBOMs, embed assets into a Wasm component, and sign/verify packs; the `greentic-pack-lib` library (in `crates/greentic-pack`) to inspect archives and derive deployment plans; and a generated component crate (`pack_component`) that exposes the embedded pack via the `greentic:pack-export` interface.

## 2. Main Components and Functionality
- **Path:** `crates/packc`  
  **Role:** Builder CLI for authoring and validating Greentic packs; publishes the canonical `greentic-pack` binary.  
  **Key functionality:** Validates `pack.yaml`, enforces pack version/kind constraints (including `distribution-bundle`), loads flow and template assets, builds `.gtpack` archives with manifests and SBOM entries (now `sbom.cbor`), generates Wasm components via `pack_component_template`, composes MCP router + adapter components, supports component descriptors (including `kind: software` with arbitrary artifact paths/types), and handles signing/verification of manifests. Provides subcommands for build/lint/components/update/new/sign/verify/gui/doctor(aka inspect)/plan/providers/config; telemetry setup; exposes library helpers (`BuildArgs`, signing APIs). `cli::doctor_dw` runs the doctor checks that apply to a DW application pack (manifest/metadata schema, advisory declared-kind cross-check, executing flow, knowledge sidecar references) and deliberately skips the canonical-only ones; `cli::flow_doctor` holds the shared `greentic-flow doctor --stdin` invocation used by both pack shapes.

- **Path:** `crates/greentic-pack`  
  **Role:** `greentic-pack-lib` library for inspecting packs and producing deployment plans.  
  **Key functionality:** `archive_shape` derives which shape a `.gtpack` is (canonical `manifest.cbor` vs greentic-designer's DW application pack `manifest.json`) from the archive's own entry names, and supplies the shape-aware message every canonical-only reader uses when it declines a pack; `reader` parses `.gtpack` archives via the shape-agnostic `read_pack_archive`, verifies hashes/signatures, and exposes manifest contents (including component manifest index helpers and SBOM reading); `plan` builds deployment plans (optionally shelling out to `greentic-pack` when given a source directory); `builder` defines pack metadata (now includes `distribution-bundle` kind, distribution section, and component descriptors with optional `software` kind/`artifact_type` labels), SBOM entries, signing helpers, and archive writing; `events`/`messaging`/`repo` schemas validate sections.

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
- `.codex/repo_overview_task.md` is referenced by `.codex/global_rules.md` but is not present in this checkout.

## 5. Notes for Future Work
- `ext://<id>#component` resolution (`crates/packc/src/cli/ext_resolver.rs`) now acquires the extension `.gtxpack` from the Store when its `pack.extensions.json` source is `store://<name>@<version>` (WS-D Phase 3a). The store artifact is fetched from `GET {GREENTIC_STORE_URL}/api/v1/extensions/{name}/{version}/artifact` (public, no auth), the body is integrity-checked against the `x-artifact-sha256` header, cached under `<cache_dir>/ext-store`, and the embedded component is extracted+digest-verified via `extract_and_verify_bytes`. An explicit version is required (tag/latest is out of scope); offline mode serves only the ref-keyed cache and errors on miss. `oci://` extension acquisition is intentionally guarded/deferred (bails with a clear message — no producer publishes extensions to OCI yet). `resolve.rs` wires the `ext://` branch through `resolve_ext_component_with_dist(pack_dir, raw_ref, cache_dir, offline, handle)`. The 3b capability flip (http/webhook → `ext://`) remains gated and is NOT done here.
- `greentic-pack wizard` extension flows now emit replay-complete AnswerDocuments for create/update/add-extension operations, persist deterministic `extensions/<type>.json` files, and merge canonical `extensions.greentic.ext.capabilities.v1` payloads into `pack.yaml` via the shared capability-offer path.
- `greentic-pack wizard apply` now also upserts `pack.extensions.json` from an `extension_dependencies` answers key (WS-D Phase 3b Task 2 — the WRITE side that pairs with the Phase 3a `ext://`/`store://` resolver). The key is an array of packc's existing `ExtensionDependency` serde shape (`{ "id", "role", "source": { "kind", "ref", "allow_tags" } }`). Parsing lives in `parse_extension_dependencies` and the merge in `upsert_extension_dependencies` (both in `crates/packc/src/cli/wizard.rs`), reusing `extension_refs::{read,write}_extensions_file`. Merge rule: dedupe by `id`, supplied wins (conflicting existing source is replaced and logged to stderr); unrelated existing entries (e.g. hand-authored) are preserved. Empty/absent `extension_dependencies` is a no-op (never creates or clobbers a file). `write_extensions_file` still validates, so version-tag `store://`/`oci://`/`repo://` refs require `allow_tags: true` unless digest-pinned. The catalog flip and publish are NOT done here.
- The default wizard catalog is now `file://docs/extensions_capability_packs.catalog.v1.json`; the fixture catalog remains available for tests/dev and includes fixture-only `runcli` entries.
- Cargo now resolves Greentic interface crates from published dependencies rather than the previously broken vendored-path patch overrides.
- The bare `mcp` flow op-key is now classified as a runner builtin
  (`crates/greentic-pack/src/builtin.rs`), so `resolve`/`build` no longer demand a
  resolve-sidecar or resolve-summary entry for it. It lives in a new
  `BUILTIN_EXACT_KINDS` list matched by EQUALITY ONLY, because the existing
  `BUILTIN_KINDS` prefix rule would also capture `mcp.exec` — a real component
  shipped in `examples/weather-demo` and `examples/adaptive-mcp-oauth-demo`,
  which would then silently skip resolution and drop out of the built pack.
- `var.set` joins `mcp` in `BUILTIN_EXACT_KINDS` (`crates/greentic-pack/src/builtin.rs`).
  greentic-designer's Set Variable palette node emits that op-key, so every flow
  using it failed `build` with "missing resolve summary entries". Exact-match for
  the same hazard as `mcp`: a prefix entry would capture a future `var.set.*`
  component and silently drop it. The module doc's earlier claim that this was
  blocked on the runner is corrected — `NATIVE_OP_KEYS` governs raw `.ygtc`
  loading, while a built pack reaches the engine through the compiled manifest,
  where a `"var."` prefix and a `"var.set"` arm already exist.
- `telco-x.call` joins `mcp` and `var.set` in `BUILTIN_EXACT_KINDS`
  (`crates/greentic-pack/src/builtin.rs`), so flows carrying a telco-x node build
  again. It is the runner's remote-dispatch node for the `"telco-x"` runtime
  (`NodeKind::TelcoXCall` → `execute_remote_dispatch`) and, unlike the state
  kinds, takes its `target` from `component.operation` — so its nodes carry an
  operation, which stops the engine dot-splitting the id and keeps the match arm
  reachable from a compiled manifest. No component id starting with `telco-x`
  exists in greentic-pack, greentic-designer or greentic-runner; the designer's
  `catalog.baseline.yaml` marks the capability `dispatch: true` with no
  `component_ref`. Exact-match regardless, because greentic-flow's
  `classify_node_type` reads a 3+-segment key as an adapter component, making
  `telco-x.call.*` exactly what a prefix entry would misclassify.
- **`state.get` / `state.set` are deliberately NOT builtins**, and the module doc
  now records that as a decision rather than as drift. Two independent blockers.
  (1) `state.get` is a REAL component here: `examples/qa-demo` resolves it from
  `repo://io.3bridges.components.state@1.0.0`, digest-pinned in `pack.lock.json`,
  declared in `pack.yaml` as `required_capabilities: [state:get]`, and given a wasm
  artifact by `packc/tests/run_examples_manifest.rs`. Classifying it as a builtin
  drops that component from the built pack — the `mcp.exec` hazard landing on the
  op-key ITSELF, so an exact entry is no safer than a prefix one. `state.set`
  shares that family and capability namespace. (2) The engine cannot dispatch
  either from a compiled manifest today: `BuiltinStateGet`/`BuiltinStateSet` are
  unit variants, so the node carries `operation: None`, and with no `state.` arm in
  the engine's `is_builtin` the id is dot-split into `"state"` — never a key in the
  pack's component map. Adding them would trade a loud build failure for a loud
  `component 'state' not found in pack` at dispatch. Closing this needs a `state.`
  arm in the runner's `is_builtin` first, plus a ruling on the contradiction
  between greentic-designer's `catalog.baseline.yaml` (`state.get` native,
  `component_ref: null`) and qa-demo (`state.get` a component); whichever wins,
  qa-demo's `pack.yaml`, `pack.lock.json`, both resolve sidecars and
  `run_examples_manifest.rs` move in the same change.
