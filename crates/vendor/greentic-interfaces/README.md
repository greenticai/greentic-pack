# Greentic Interfaces

Shared WebAssembly Interface Types (WIT) packages and Rust bindings for the Greentic next-gen platform. The crate is MIT licensed and evolves additively—new fields or functions land in minor releases, while breaking changes require a new package version.

> Canonical target for new work: `component@0.6.0` + `types-core@0.6.0` + `codec@0.6.0`.
> Legacy compatibility surfaces remain shipped but are tracked in `../docs/vision/legacy.md`.

## Overview

The repository serves two goals:

- Authoritative WIT contracts that describe how packs interact with the Greentic host. The contracts cover core types, host imports, the pack component API, and provider self-description metadata.
- Ergonomic Rust bindings with thin conversion helpers so runtime code can move between WIT-generated types and the rich structures from [`greentic-types`](../greentic-types).

All Greentic runtimes, components, and tools **must** depend on these WIT packages and the shared `greentic-types` crate. No other repository may re-declare the contracts or duplicate the shared models.

## WIT packages

The `wit/` directory contains additive packages:

| Package | Contents |
|---------|----------|
| `greentic:interfaces-types@0.1.0` | Canonical data structures (`TenantCtx`, `SessionCursor`, `Outcome`, `AllowList`, `NetworkPolicy`, `PackRef`, `SpanContext`, etc.). |
| `greentic:interfaces-provider@0.1.0` | Provider self-description (`ProviderMeta`). |
| `greentic:interfaces-pack@0.1.0` | Component world exporting `meta()` and `invoke()` for pack execution. |
| `greentic:provider-schema-core@1.0.0` | Provider-core schema world for describing provider capabilities/config via JSON Schema. |
| `greentic:secrets-store/store@1.0.0` | Read-only secret lookups (`get`) with structured errors. |
| `greentic:state/store@1.0.0` | Generic blob store keyed by `StateKey`. |
| `greentic:http/client@1.0.0` | HTTP client with structured request/response types. |
| `greentic:telemetry/logger@1.0.0` | Telemetry emitter keyed by `SpanContext`. |
| `greentic:repo-ui-actions@1.0.0` | UI action handler world for tenant-skinned consoles (`handle-action`). |
| `greentic:worker@1.0.0` | Generic worker envelope (WorkerRequest/WorkerResponse/messages) for assistants/workers; see [`docs/worker.md`](../docs/worker.md). |
| `greentic:oauth-broker@1.0.0` | Generic OAuth broker: build consent URLs, exchange codes, fetch tokens; provider semantics stay in host-side greentic-oauth/config. Hosts implement the exported `broker` world; wasm guests import via `broker-client`. |
| `greentic:distributor-api@1.0.0` | Runner/Deployer distributor API: resolve-component, pack status, warm-pack; canonical surface for runtime artifact lookup. |
| `greentic:distributor-api@1.1.0` | Adds ref-based resolution (`resolve-ref`) + digest fetching (`get-by-digest`) for OCI component references (tag or digest). |
| `greentic:distribution@1.0.0` | Experimental desired-state submission/fetch ABI (future-facing control plane surface, unused by current runner flows). |

### MCP router WIT

All MCP protocol WIT packages live in this repository. Routers must not redefine them elsewhere.

| WIT package | MCP spec revision | Link |
|-------------|-------------------|------|
| `wasix:mcp@24.11.5` | 2024-11-05 (+ legacy Greentic config/secrets/output descriptors) | https://modelcontextprotocol.io/specification/2024-11-05 |
| `wasix:mcp@25.3.26` | 2025-03-26 (annotations, audio content, completions, progress; metadata carries config/secrets/output hints) | https://modelcontextprotocol.io/specification/2025-03-26 |
| `wasix:mcp@25.6.18` | 2025-06-18 (structured output, resource/resource-link, elicitation, titles/_meta, tightened auth/resource metadata) | https://modelcontextprotocol.io/specification/2025-06-18 |

New development should target `wasix:mcp@25.6.18` (current spec). Older versions remain only for compatibility with existing routers.

### Using repo-ui-actions bindings

- Guest (UI action handler component): enable `repo-ui-actions` feature in `greentic-interfaces-guest`, then implement `handle_action` on the generated trait to parse JSON input and call into other WIT worlds as needed.
- Host (router): enable `repo-ui-actions-v1` in `greentic-interfaces` or import via `greentic-interfaces-host::ui_actions::repo_ui_worker`, instantiate the component with Wasmtime, and invoke `handle-action(tenant, page, action, payload-json)`.
- All components must target `wasm32-wasip2`; the host bindings are ABI-only and stay domain-agnostic.

The build script stages each package (plus dependencies) into `$OUT_DIR/wit-staging` so downstream tooling resolves imports deterministically. The absolute path is exported as `WIT_STAGING_DIR`, so consumers never need write access to the package directory even when building from crates.io.

### TenantCtx optional fields

Version `0.4.18` adds four optional identifiers to `greentic:types-core@0.4.x` and the mirrored ABI package:

- `session_id: option<string>` – runtime session handle (defaults to `None`).
- `flow_id: option<string>` – stable flow identifier for the current invocation.
- `node_id: option<string>` – node identifier inside the flow DAG.
- `provider_id: option<string>` – surface/runtime that accepted the invocation.

All four fields are additive and stay backwards compatible with existing 0.4.x users—structures that omit them continue to deserialize, and callers can opt in as soon as both sides adopt the newer version.

```rust
use greentic_types::{EnvId, TenantCtx, TenantId};

// Existing 0.4.x code keeps compiling; the new fields simply default to `None`.
let legacy = TenantCtx {
    env: EnvId::new("dev").expect("env id"),
    tenant: TenantId::new("tenant").expect("tenant id"),
    tenant_id: TenantId::new("tenant").expect("tenant id"),
    team: None,
    team_id: None,
    user: None,
    user_id: None,
    attributes: Default::default(),
    session_id: None,
    flow_id: None,
    node_id: None,
    provider_id: None,
    trace_id: None,
    i18n_id: None,
    correlation_id: None,
    deadline: None,
    attempt: 0,
    idempotency_key: None,
    impersonation: None,
};

// New code can opt into the richer metadata on the same struct.
let mut enriched = legacy.clone();
enriched.session_id = Some("s-1".into());
enriched.flow_id = Some("flow-welcome".into());
enriched.node_id = Some("node-enter".into());
enriched.provider_id = Some("telegram".into());
assert_eq!(enriched.session_id.as_deref(), Some("s-1"));
```

## Rust bindings

This crate is intentionally ABI-only: `greentic_interfaces::bindings::generated` exposes the raw `wit-bindgen` output for the `interfaces-pack` world, and the helper modules translate between those generated types and the richer models in [`greentic-types`](../greentic-types). No Wasmtime adapters ship here.

The bindings follow the `TenantCtx`, `Outcome`, `ProviderMeta`, and `AllowList` shapes defined in the design manifesto so runner, deployer, connectors, and packs all share the same ABI.

### Example: invoking a pack component

```rust
use greentic_interfaces::bindings::exports::greentic::interfaces_pack::component_api;
use greentic_interfaces::bindings;

fn empty_allow_list() -> bindings::greentic::interfaces_types::types::AllowList {
    bindings::greentic::interfaces_types::types::AllowList {
        domains: Vec::new(),
        ports: Vec::new(),
        protocols: Vec::new(),
    }
}

struct GreetingComponent;

impl component_api::Guest for GreetingComponent {
    fn meta() -> component_api::ProviderMeta {
        component_api::ProviderMeta {
            name: "hello-tool".into(),
            version: "0.1.0".into(),
            capabilities: vec!["invoke".into()],
            allow_list: empty_allow_list(),
            network_policy: bindings::greentic::interfaces_types::types::NetworkPolicy {
                egress: empty_allow_list(),
                deny_on_miss: false,
            },
        }
    }

    fn invoke(input: String, tenant: component_api::TenantCtx) -> component_api::Outcome {
        let message = format!("Hello {}, {}!", tenant.tenant, input);
        component_api::Outcome::Done(message)
    }
}
```

The packed component returns a `Outcome::Done(String)` which maps directly to `greentic_types::Outcome<String>` via the conversion helpers described below.

A minimal `examples/crates-io-consumer` binary shows how to depend on the published crate without any workspace patches.

## Conversion helpers

`src/mappers.rs` implements thin `From`/`TryFrom` conversions between WIT-generated types and their `greentic-types` equivalents:

- `TenantCtx ↔ greentic_types::TenantCtx`
- `SessionCursor ↔ greentic_types::SessionCursor`
- `Outcome<string> ↔ greentic_types::Outcome<String>`
- `AllowList ↔ greentic_types::policy::AllowList`
- `NetworkPolicy ↔ greentic_types::policy::NetworkPolicy`
- `SpanContext ↔ greentic_types::telemetry::SpanContext`

These helpers avoid business logic—each mapping is a total, lossless transformation so packs and hosts can interoperate without bespoke glue.

Unit tests under `src/mappers.rs` and integration tests in `tests/mapping_roundtrip.rs` ensure round-trips preserve the data the runner depends on (tenant identity, session cursors, expected input hints, etc.).

## Provider metadata validation

`greentic_interfaces::validate::validate_provider_meta` checks the minimal invariants for provider self-description:

- Non-empty provider name.
- Valid semantic version string.
- Allow-lists contain no empty hosts, zero ports, or unnamed custom protocols.
- Strict network policies (`deny_on_miss = true`) must include at least one allow rule.

Call this helper before accepting provider metadata to avoid surprising runtime failures.

## Testing, formatting, and linting

Quality gates are enforced locally and in CI:

```bash
# Format
cargo fmt --all

# Lint (covers library, tests, and examples)
cargo clippy --all-targets --all-features -- -D warnings

# Run the full test matrix (includes schema snapshots)
cargo test --all-features
```

The `wit_build` integration test parses every staged WIT package to ensure the build script emits well-formed bundles, and the schema snapshot (guarded by the `schema` feature) keeps provider metadata schemas stable.

CI mirrors these commands so pull requests fail fast if formatting drifts, clippy raises a regression, or contracts stop compiling.

## Releases & Publishing

Version numbers come from each crate's `Cargo.toml`. When a commit lands on `master`, the auto-tag workflow checks whether any crate manifests changed and creates lightweight tags in the form `<crate>-v<semver>` (for single-crate repos this matches the repository name). The publish workflow then runs the lint/test gate, and finally invokes `katyo/publish-crates` to publish changed crates to crates.io using the `CARGO_REGISTRY_TOKEN` secret. Publishing is idempotent, so rerunning on the same commit succeeds even if the versions are already available.

## Maintenance notes

- Updating WIT contracts only happens additively. Introduce a new version instead of breaking existing packages.
- Regenerate the Rust bindings by running `cargo build`; the build script handles staging and `wit-bindgen` output automatically.
- When adding new WIT structures, remember to extend the conversion helpers and the snapshot test so hosts and packs keep sharing the same ABI.
- The schema snapshot lives under `tests/snapshots/`. Run `INSTA_ACCEPT=auto cargo test --features schema` whenever the provider metadata shape changes to refresh the snapshot intentionally.

## Runtime support

Consumers that need to execute packs via Wasmtime should depend on the sibling `greentic-interfaces-wasmtime` crate (introduced in a follow-up PR) or wire Wasmtime directly. This package purposefully avoids runtime glue so it can stay focused on ABI contracts and type conversions.
