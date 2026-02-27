# Greentic Pack Usage Guide

This guide expands on the README with end-to-end instructions for building
Greentic packs and integrating them with the MCP runtime.

For compatibility-only behavior and deprecated aliases, see
`docs/vision/legacy.md`.

## Installing the CLI

Fetch the prebuilt release binaries via `cargo-binstall`:

```bash
cargo install cargo-binstall   # run once
cargo binstall greentic-pack packc
```

The install provides the canonical `greentic-pack` CLI on your `PATH`.

## Workflow overview

1. **Author a pack manifest** - create `pack.yaml` with metadata, `flow_files`,
   and optional `template_dirs`.
2. **Write flows** – author `.ygtc` files that orchestrate conversation
   behaviour. Flows should reference MCP tools using `mcp.exec` nodes so the
   host can negotiate tool execution at runtime.
3. **Add templates** – drop supplementary assets (markdown, prompts, UI
   fragments) under directories listed in `template_dirs`.
4. **Run `greentic-pack build`** - build the pack artifacts locally. The CLI validates the
   manifest, fingerprints flows/templates, writes a CBOR manifest, and emits a
   Wasm component backed by the generated `data.rs` payload.
5. **Ship the artifacts** - publish the resulting `.gtpack` and related outputs
   to the desired distribution channel.

For declaring providers inside `pack.yaml`, see
`docs/extension-provider-packs-howto.md`. The provider extension is optional and
validated by `greentic-pack lint`; scaffold a starter pack with
`greentic-pack new` and inspect it with
`greentic-pack providers list --pack <path>`.
For pack localization and QA prompt translation patterns, see
`docs/internationalise-pack-howto.md`.

For repo-oriented packs (source/scanner/signing/attestation/policy/oci),
see `docs/repo-pack-types.md` for the schema, capabilities, and bindings
requirements.

## CLI reference

`greentic-pack build` exposes a structured build interface:

```text
Usage: greentic-pack build --in <DIR> [--manifest <FILE>] [--gtpack-out <FILE>]
                           [--bundle <cache|none>] [--dry-run] [--log <LEVEL>]
```

- `--in` – path to the pack directory containing `pack.yaml`.
- `--manifest` – CBOR manifest output (default `dist/manifest.cbor`).
- `--gtpack-out` – optional path to the `.gtpack` archive that packages the
  manifest, SBOM, flows, templates, and compiled component.
- `--bundle` - bundle strategy (`cache` to embed runtime artifacts, `none` for refs-only).
- `--dry-run` – validate inputs without writing artifacts or compiling Wasm.
- `--secrets-req` – optional JSON/YAML file with additional secret
  requirements.
- `--default-secret-scope` – dev-only helper to fill missing secret scopes
  (format: `ENV/TENANT[/TEAM]`).
- `--log` – customise the tracing filter (defaults to `info`).
- `--offline` – hard-disable any network activity (highest precedence; also see `GREENTIC_PACK_OFFLINE`).
- `--cache-dir` – override the packc cache root (default: `<pack_dir>/.packc/`; env: `GREENTIC_PACK_CACHE_DIR`).
- `--no-update` – skip the automatic `packc update` that normally runs before a build.
- `--no-extra-dirs` – only bundle `flows/`, `components/`, and `assets/` (skip other top-level dirs).
- `--dev` – keep `pack.yaml`, `pack.manifest.json`, flow sources, and other pack artifacts inside the `.gtpack` for debugging.

When a component’s `wasm` path points to a directory, `packc build` only
packages runtime artifacts: the resolved Wasm (`*.component.wasm` preferred)
and the component manifest (`component.json` converted to CBOR). Source files
(README, src/, tmp/, etc.) are deliberately excluded from the `.gtpack`.

By default the generated `.gtpack` contains just `manifest.cbor`, `sbom.cbor`,
runtime artifacts, and the `assets/` tree, with non-reserved root files
remapped to `assets/<name>`. Existing files inside `assets/` take priority,
and conflicts emit a warning rather than overwriting. Pass `--dev` when you
need to bundle the original source artifacts (`pack.yaml`, `.ygtc`, JSON
manifests, etc.) for debugging.

`greentic-pack` writes structured progress logs to stderr. When invoking inside CI, pass
`--dry-run` to skip Wasm compilation if the target toolchain is unavailable.
Use `greentic-pack config` to print the resolved configuration, provenance, and any
warnings (add `--json` for machine-readable output).

### Inspecting packs

Use `greentic-pack doctor` to read a
`.gtpack` archive (`--pack`) or a source directory (`--in`, containing
`pack.yaml`). Source mode shells out to `packc build --gtpack-out` in a temp
dir to guarantee parity with archive inspection. Examples:

```bash
# Inspect a built archive
greentic-pack doctor dist/demo.gtpack --json

# Inspect a source tree without prebuilding artifacts
greentic-pack doctor --in examples/weather-demo
```

Output defaults to a human-readable summary (pack id/version/name, messaging
adapters, component count, warnings). Pass `--json` to emit the manifest,
verification report, and SBOM as JSON. Signature verification uses the dev
policy when inspecting archives.

By default, `doctor` also runs `greentic-flow doctor` on each flow and
`greentic-component doctor` on each component when the binaries are available.
Disable these with `--no-flow-doctor` or `--no-component-doctor`.

### Flow resolve sidecars and pack.lock

Authoring flows may be accompanied by optional `*.ygtc.resolve.json` sidecars
that map flow nodes to component sources (`local`, `oci`, `repo`, or `store`).
`greentic-pack update` now ensures every flow has a sidecar (creates an empty
one if missing). `--strict` forces update to error when node mappings are
absent. Builds require sidecars to map every node, so resolution is explicit
instead of guessed.

Packs can also carry a deterministic `pack.lock.cbor` (version 1) beside
`pack.yaml`:

```json
{
  "schema_version": 1,
  "components": [
    { "name": "demo", "ref": "oci://example/demo:1.0.0", "digest": "sha256:..." }
  ]
}
```

Use `greentic-pack resolve --in <pack>` to aggregate sidecar refs, resolve
remote digests via greentic-distributor-client (honouring `--offline`), and
write a deterministic `pack.lock.cbor` (override output with `--lock <path>`).
Builds expect this lockfile; if it is missing the error directs you to run
`greentic-pack resolve`.

### Bundling policy: refs-only vs cache

`greentic-pack build` supports two bundle modes:

- `--bundle=none` (refs-only): the `.gtpack` carries component source refs +
  digests via the component_sources extension; no wasm blobs are embedded.
- `--bundle=cache` (default): embed only runtime artifacts (wasm +
  manifest.cbor) for each component; directories are **never** copied in full.
  Remote refs are pulled from cache when available.

Doctor output surfaces how many components are inline vs remote to make this
mode visible.

Recommended workflow:

1. `greentic-pack update --strict` (create/validate sidecars)
2. `greentic-pack resolve` (writes pack.lock.cbor; use `--offline` in CI if pre-resolved)
3. `greentic-pack build --bundle=cache` (or `--bundle=none` for refs-only)
4. `greentic-pack doctor dist/*.gtpack` (CI smoke)

### GUI pack converter (Loveable)

`packc gui loveable-convert` turns a Loveable-generated app into a GUI `.gtpack`.

Key flags:

- `--pack-kind <layout|auth|feature|skin|telemetry>`
- `--id <pack-id>` and `--version <semver>`
- one of `--repo-url`, `--dir`, or `--assets-dir` (skip build)
- optional build overrides: `--package-dir`, `--install-cmd`, `--build-cmd`, `--build-dir`
- routing overrides: repeat `--route /path:file.html` (or `--routes path:html,...`)
- `--spa true|false` to override SPA detection
- `--out` (alias `--output`) for the resulting `.gtpack`

Example (using prebuilt assets):

```bash
packc gui loveable-convert \
  --pack-kind feature \
  --id greentic.demo.gui \
  --version 0.1.0 \
  --assets-dir ./dist \
  --out ./dist/demo.gtpack
```

## Scaffolding new packs

`greentic-pack new` bootstraps a directory that already matches the expected manifest
and flow layout:

```bash
greentic-pack new hello-pack --dir ./hello-pack
cd hello-pack
greentic-pack build --in . --dry-run
```

The command writes `pack.yaml`, `flows/main.ygtc`, and an empty
`components/` directory (no stub `.wasm` is generated). Populate
`components/` with your compiled Wasm components and add additional flows as
needed, then run `greentic-pack update --in .` to refresh `pack.yaml`.

## Example build

```bash
rustup target add wasm32-wasip2   # run once
cargo run -p greentic-pack --bin greentic-pack -- build \
  --in examples/weather-demo \
  --manifest dist/manifest.cbor \
  --gtpack-out dist/demo.gtpack
```

Outputs:

- `dist/manifest.cbor` – canonical pack manifest suitable for transmission.
- `crates/pack_component/src/data.rs` – regenerated Rust source containing raw
  bytes for the manifest, flow sources, and templates.
- `.packc/mcp/<id>/component.wasm` – merged MCP adapter+router components for
  each `mcp_components` entry.

When you pass `--gtpack-out`, the build writes the
canonical `.gtpack` archive. Use
`cargo run -p greentic-pack --bin greentic-pack -- --json doctor dist/demo.gtpack`
to inspect the archive, confirm the SBOM entries have media types, and ensure
the flows/templates match what was written into `manifest.cbor`.

## Planning deployments

`greentic-pack` ships a complementary CLI for inspecting archives and producing
provider-agnostic deployment plans:

```bash
greentic-pack plan dist/demo.gtpack \
  --tenant tenant-demo \
  --environment prod
```

The planner always consumes a `.gtpack` archive to guarantee parity between
local dev, CI, and operators. For convenience `plan` also accepts a pack source
directory; in that case it invokes `packc build --gtpack-out` internally to
create a temporary archive before running the planner. Set the
`GREENTIC_PACK_PLAN_PACKC` environment variable if `packc` is not on `PATH`.
When available, the planner pulls aggregated secret requirements from the
`secretRequirements` field inside `manifest.cbor` (falling back to
`secret-requirements.json` if the manifest is missing the data); otherwise it
falls back to the component manifests bundled in the pack.

## MCP components and flows

- Declare MCP routers under `mcp_components` in `pack.yaml` with an `id`,
  `router_ref`, and optional `protocol`/`adapter_template` (defaults:
  `25.06.18` + `default`).
- `router_ref` must be a local file path (relative to the pack root). OCI or
  remote router references are not supported yet.
- `packc build` composes the MCP adapter template for the chosen protocol with
  each router using `wasm-tools compose`, emitting merged
  `greentic:component@0.4.0` artifacts under `.packc/mcp/<id>/component.wasm`.
- Override the default adapter by setting
  `GREENTIC_PACK_ADAPTER_25_06_18=/path/to/adapter.component.wasm` when needed.
- packc pins a specific MCP adapter reference internally (`MCP_ADAPTER_25_06_18`);
  current image: `ghcr.io/greenticai/greentic-mcp-adapter:25.06.18-v0.4.4`
  (digest to be added when published).
- Use `mcp.exec` nodes to describe remote actions. Set the `component` field to
  the `mcp_components.id` you defined; the merged component handles the
  adapter-to-router wiring.
- Pipe user input into node arguments through the `in` variables and reference
  pack parameters for defaults (e.g. `parameters.days_default`).

Example snippet from the bundled weather demo:

```yaml
forecast_weather:
  mcp.exec:
    component: "weather_api"
    action: "forecast_weather"
    args:
      q: in.q_location
      days: parameters.days_default
routing:
  - to: weather_text
```

### Flow-to-flow invocation

Introduce hierarchical logic by delegating to another flow via the reserved
`flow.call` component. It takes a `flow_id` (the `id` of another entry in
`flow_files`) plus an optional `input` object. The called flow executes with a
fresh context and returns its final payload to the caller:

```yaml
call_specialist:
  flow.call:
    flow_id: parameters.answer_flow_id
    input:
      question: collect_question.payload.q_problem
      profile: lookup_context.payload.value
  routing:
    - to: respond_to_user
```

### Session-aware prompts

Components such as `qa.process` issue a `session.update` under the hood when
they need real user input. The runner persists the execution state, pauses the
flow, and resumes once the user replies. Pack authors simply route the node’s
outputs; no special wiring is required beyond ensuring the incoming activity
includes a user identifier so the runner can find the paused session.

### Multi-message replies

Any node may yield an array payload to emit multiple outbound activities from a
single execution pass. This is handy for “thinking” style responses where an
LLM first shares intermediate reasoning before the final answer:

```yaml
respond_to_user:
  messaging.emit:
    messages: call_specialist.payload    # array of message objects
  routing:
    - out: true
```

The runner preserves order, sending each entry to the channel sequentially.
See `examples/qa-demo` for a complete pack that combines all three patterns.

## Distribution bundles and software components

- Use `kind: distribution-bundle` when producing offline bundles; include a `distribution` section with `bundle_id` (optional), `tenant` (opaque JSON map, conventionally serialized TenantCtx), `environment_ref`, `desired_state_version`, `components`, and optional `platform_components`.
- Components may carry `kind: software` plus optional `artifact_type`/`tags`/`platform`/`entrypoint`; `artifact_path` is a generic path inside the `.gtpack`. The pack tooling does not assume WASM—downstream tools decide how to execute or install.

## Component integration

The generated `pack_component` crate exposes helper functions for host runtimes
and targets `wasm32-wasip2`, so it can be instantiated using the WASI Preview 2
ABI. It depends on the host bindings from `greentic-interfaces-host` and
implements the `greentic:pack-export` WIT (see
`greentic_interfaces_host::bindings::exports::greentic::interfaces_pack::component_api::ProviderMeta`),
meaning no extra WIT files are required in this repository.

- `manifest_cbor()` – raw CBOR manifest bytes.
- `manifest_value()` / `manifest_as<T>()` – JSON/typed views of the manifest.
- `flows()` / `templates()` – iterate embedded resources.
- `Component` – an implementation of the `greentic:pack-export` interface with
  stubbed execution hooks ready for future expansion.

Hosts are expected to load `pack.wasm`, instantiate the component, call
`list_flows`, and use MCP to execute the declared `mcp.exec` nodes.

## Pack QA

Pack-level QA is optional and is described by storing a `QaSpecSource` in
`pack.cbor` metadata under the key `greentic.qa`. The value is CBOR encoded and
points to a `schemas::pack::v0_6_0::PackQaSpec` (either inline or via a pack
path).

Recommended pack path convention:

```
qa/pack/default.cbor
qa/pack/setup.cbor
qa/pack/update.cbor
qa/pack/remove.cbor
```

Each file contains canonical CBOR for `PackQaSpec`.

For component QA on the 0.6 path, `greentic-pack qa` runs:
`describe -> qa-spec -> ask -> apply-answers -> strict validation against describe.config_schema`.
Compatibility aliases are documented in `docs/vision/legacy.md`.

## CI tips

- Run `cargo fmt --all` and `cargo clippy --workspace` locally before pushing.
- Add `--dry-run` to CI invocations of `packc build` if the Wasm toolchain is
  not provisioned.
- Keep example packs up to date; tests use `examples/weather-demo` as a contract
  to ensure generated artifacts capture MCP nodes correctly.

## Troubleshooting

| Issue | Resolution |
| ----- | ---------- |
| `Rust target 'wasm32-wasip2' is not installed` | Run `rustup target add wasm32-wasip2` once before building without `--dry-run`. |
| CLI fails with duplicate flow/template IDs | Ensure each entry in `flow_files` and `template_dirs` maps to unique logical paths. |
| Missing MCP tool at runtime | Confirm the host has loaded the proper MCP component; packs should never embed the tool implementation. |
| `schema_hash mismatch` / contract drift | Re-resolve component metadata (`greentic-pack resolve`) and verify component describe payloads match the expected operation/schema pair. |
| `apply-answers output failed strict schema validation` | Fix answers or component QA logic so returned config matches `describe.config_schema`; errors include field paths and aggregated violations. |
| Capability denied at runtime | Capability enforcement is owned by runtime/operator layers; ensure granted host profile matches component requirements. |

