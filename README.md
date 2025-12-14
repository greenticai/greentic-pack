# Greentic Pack

`greentic-pack` provides two core capabilities for building Greentic packages:

- `packc`: a developer-facing CLI that validates pack manifests, normalises
  metadata, generates simple SBOM reports, and emits the `data.rs` payload that
  powers the Greentic pack component.
- `pack_component`: a reusable Wasm component crate that exposes the
  `greentic:pack-export` interface using the artefacts produced by `packc`.

## Installation

Install the prebuilt CLI binaries with `cargo-binstall`:

```bash
cargo install cargo-binstall   # run once
cargo binstall greentic-pack
```

## Repository layout

```
greentic-pack/
├── Cargo.toml                # Cargo workspace manifest
├── crates/
│   ├── packc/                # Builder CLI
│   └── pack_component/       # Wasm component library
├── docs/                     # Additional guides
├── examples/                 # Sample packs
└── .github/workflows/        # CI automation
```

### packc

The CLI expects a pack directory that contains `pack.yaml` alongside its flow
files and templates. Example:

```bash
cargo run -p packc --bin packc -- build \
  --in examples/weather-demo \
  --out dist/pack.wasm \
  --manifest dist/manifest.cbor \
  --sbom dist/sbom.cdx.json \
  --gtpack-out dist/demo.gtpack
```

Running the command performs validation, emits the CBOR manifest, generates a
CycloneDX SBOM, regenerates `crates/pack_component/src/data.rs`, and compiles
`pack_component` to the requested Wasm artifact. Use `--dry-run` to skip writes
while still validating the pack inputs.

Passing `--gtpack-out dist/demo.gtpack` generates the canonical `.gtpack`
archive; inspect it with `cargo run -p greentic-pack --bin gtpack-inspect -- --policy devok --json dist/demo.gtpack`
to confirm the SBOM entries, flows, and templates embedded inside the archive.

> ℹ️ The build step expects the `wasm32-wasip2` Rust target. Install it
> once with `rustup target add wasm32-wasip2`.

#### MCP components

- Declare MCP routers in `pack.yaml` via `mcp_components` (id, router_ref,
  optional `protocol` defaulting to `25.06.18`, and optional
  `adapter_template` defaulting to `default`).
- During `packc build`, the MCP adapter template is composed with each router
  using `wasm-tools compose`, producing merged `greentic:component@0.4.0`
  artifacts written under `.packc/mcp/<id>/component.wasm`.
- `router_ref` must point to a local component file (paths relative to the pack
  root). OCI/remote router references are not supported yet.
- The merged components are embedded in the `.gtpack` manifest; router
  components are not exposed separately unless explicitly requested.
- The default adapter can be overridden via
  `GREENTIC_PACK_ADAPTER_25_06_18=/path/to/adapter.component.wasm` if you need
  to pin or test a specific adapter build.
- packc pins a specific MCP adapter reference internally (see
  `MCP_ADAPTER_25_06_18` in code); current image:
  `ghcr.io/greentic-ai/greentic-mcp-adapter:25.06.18-v0.4.4` (digest pending).

Use `packc new` to bootstrap a fresh pack directory that already matches the
current schema:

```bash
packc new greentic.demo --dir ./greentic-demo
cd greentic-demo
./scripts/build.sh
```

The scaffold command writes `pack.yaml`, a starter flow under `flows/`, a helper
build script, and (optionally) a development Ed25519 keypair when `--sign` is
specified.

Greentic packs only transport flows and templates. Execution-time tools are
resolved by the host through the MCP runtime, so flows should target
`mcp.exec` nodes rather than embedding tool adapters. The `tools` field remains
in `PackSpec` for compatibility but new packs should rely on MCP.

Schemas for `pack.yaml` live under `crates/packc/schemas/` as both
`pack.v1.schema.{json,yaml}` and `pack.schema.v1.{json,yaml}`; the Rust
validation in `packc` remains the source of truth.

Note: rollout-strategy (Distributor-oriented) pack kinds are reserved for a
future phase and must be rejected in v1; only the documented provider kinds are
accepted.

### greentic-pack

Operators inspect and plan published packs via the `greentic-pack` CLI:

```bash
greentic-pack inspect dist/demo.gtpack --policy devok
greentic-pack plan dist/demo.gtpack --tenant tenant-demo --environment prod
```

`plan` always operates on a `.gtpack` archive so that CI, dev machines, and
operators see identical behaviour. For convenience you can also point it at a
pack source directory; the CLI shells out to `packc build --gtpack-out` to
create a temporary archive before running the planner (set
`GREENTIC_PACK_PLAN_PACKC=/path/to/packc` if `packc` is not on `PATH`).

### Flow patterns

- **Flow-to-flow invocation** – use the special `flow.call` component to jump
  into another flow within the same pack. The component accepts a `flow_id`
  plus an optional `input` payload, returning whatever the target flow yields.
- **Session-aware prompts** – nodes such as `qa.process` rely on the host’s
  session store to pause execution while they wait for user replies. No extra
  wiring is required in the pack; simply route their outputs like any other
  node and the runner resumes the flow when input arrives.
- **Multi-message replies** – any node may return an array payload to emit
  multiple outbound messages. The runner normalises each array element into a
  message in the order provided, enabling richer “thinking + answer” patterns.

### Telemetry configuration

`packc` initialises Greentic's telemetry stack automatically. Configure the
following environment variables as needed:

- `OTEL_EXPORTER_OTLP_ENDPOINT` (defaults to `http://localhost:4317`)
- `RUST_LOG` (standard filtering for tracing; `PACKC_LOG` still overrides when set)
- `OTEL_RESOURCE_ATTRIBUTES` (recommend `deployment.environment=dev` for local work)

### pack_component

`pack_component` is a thin wrapper around the generated `data.rs`. It exposes
helpers for inspecting the embedded manifest and flow assets. The component
depends on the shared bindings from `greentic-interfaces`; no WIT files are
vendored in this repository. Re-run `packc build` whenever the manifest or flow
assets change to ensure `data.rs` stays in sync.

#### Loveable → GUI packs

Use `packc gui loveable-convert` to package a Loveable-generated app into a GUI
`.gtpack`. The command supports cloning a repo (`--repo-url`), pointing at a
local checkout (`--dir`), or reusing an existing build (`--assets-dir`). It
generates `gui/manifest.json`, copies `gui/assets/**`, writes a canonical
`pack.yaml`, and emits the `.gtpack` via the existing pack builder. Override
routes with `--route /path:file.html`, force SPA/MPA with `--spa`, and set the
output via `--out`.

#### Offline and cache controls

- Pass `--offline` to hard-disable any network activity during pack builds
  (e.g., git clones or dependency downloads). `GREENTIC_PACK_OFFLINE=1` enables
  the same guard, but the CLI flag always wins and emits a warning when it
  overrides the environment.
- Override the packc cache root with `--cache-dir <path>` or
  `GREENTIC_PACK_CACHE_DIR`; otherwise the cache defaults to `<pack_dir>/.packc/`.

#### Secret requirements

`packc build` now aggregates component secret requirements, dedupes them, and
embeds `secret-requirements.json` in the `.gtpack`. Use `--secrets-req` to
inject additional requirements during migration, and `--default-secret-scope
ENV/TENANT[/TEAM]` (dev-only) to fill missing scopes. `gtpack-inspect` surfaces
the aggregated list in both human and JSON output. `greentic-pack plan` reads
the embedded `secret-requirements.json` (falling back to component manifests)
so deployment plans reflect the migration data.

## Examples

- `examples/weather-demo` – a toy conversational pack demonstrating the
  expected directory structure. Use this sample to smoke test `packc` or
  bootstrap new packs.
- `examples/qa-demo` – showcases a multi-turn QA assistant that pauses for user
  input, invokes a specialist subflow via `flow.call`, and emits multiple
  outbound messages from a single run.
- `examples/billing-demo`, `examples/search-demo`, `examples/reco-demo` – minimal
  manifests showing the new billing/search/recommendation provider kinds,
  packVersion usage, and interface bindings.

## Further documentation

- `docs/usage.md` – CLI flags, build workflow, and guidance for designing MCP
  aware flows.
- `docs/publishing.md` – notes on publishing the crates to crates.io.
- `docs/pack-format.md` – on-disk `.gtpack` layout, hashing rules, and
  verification semantics.

## Local CI Checks

Mirror the GitHub Actions flow locally with:

```bash
ci/local_check.sh
```

Toggles:

- `LOCAL_CHECK_ONLINE=1` – allow steps that need the network.
- `LOCAL_CHECK_STRICT=1` – fail when optional tools are missing.
- `LOCAL_CHECK_VERBOSE=1` – echo every command.

The script runs automatically via `.git/hooks/pre-push`; remove or edit that
hook if you need custom behavior.

## Releases & Publishing

Version numbers come from each crate’s `Cargo.toml`. When changes land on
`master`, the automation tags any crate whose manifest version changed with
`<crate-name>-v<semver>` (for single-crate repos this matches the repo name).
The publish workflow then runs `cargo fmt`, `cargo clippy`, `cargo build`, and
`cargo test --workspace --all-features` before invoking
`katyo/publish-crates@v2`. Publishing is idempotent—reruns succeed even when
all crates are already uploaded—while still requiring `CARGO_REGISTRY_TOKEN`
for new releases.

## Signing & Verification

Greentic packs can now embed developer signatures directly inside their
`pack.toml` manifest. Signatures allow downstream tooling (including the
runner) to verify that the pack contents have not been tampered with.

### Generating keys

Ed25519 keys are managed using industry standard PKCS#8 PEM files. You can
generate a developer keypair with OpenSSL:

```bash
openssl genpkey -algorithm ed25519 -out sk.pem
openssl pkey -in sk.pem -pubout -out pk.pem
```

The private key (`sk.pem`) is used for signing, while the public key (`pk.pem`)
is distributed to verifiers.

### Signing manifests

Use `packc sign` to produce a signature and embed it into the pack manifest:

```bash
packc sign \
  --pack examples/weather-demo \
  --key ./sk.pem
```

By default the manifest is updated in place. Provide `--out` to write the
signed manifest to a separate location and `--kid` to override the derived key
identifier. The command prints the key id, digest, and timestamp, and it can
emit JSON with the `--json` flag.

After signing, the manifest contains a new block:

```toml
[greentic.signature]
alg = "ed25519"
key_id = "1f2c3d4e5f6a7b8c9d0e1f2c3d4e5f6a"
created_at = "2025-01-01T12:34:56Z"
digest = "sha256:c0ffee..."
sig = "l4dbase64urlsig..."
```

The digest covers a canonical view of the pack directory that excludes build
artifacts, VCS metadata, `.packignore` entries, and the signature block itself.

### Verifying manifests

Verification is available from both the CLI and the library API:

```bash
packc verify --pack examples/weather-demo --pub ./pk.pem
```

The `--allow-unsigned` flag lets verification succeed when no signature is
present (returning a synthetic signature with `alg = "none"`). Library users
can call `packc::verify_pack_dir` with `VerifyOptions` to replicate the same
behaviour. The runner will use this API with `allow_unsigned = false` by
default, providing an `--allow-unsigned` escape hatch for development flows.

## Licensing

`greentic-pack` is licensed under the terms of the MIT license. See
[LICENSE](LICENSE) for details.
