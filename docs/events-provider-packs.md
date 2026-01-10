# Provider Extension Packs

Packs can describe providers so runtimes can discover brokers/sources/sinks/bridges without hard-coding transports. The provider extension lives under `extensions.greentic.provider-extension.v1` in `pack.yaml` and is optional/compatible with existing packs.

## Schema

```yaml
extensions:
  greentic.provider-extension.v1:
    kind: greentic.provider-extension.v1
    version: 1.0.0
    inline:
      providers:
        - provider_type: "nats-core"            # required, unique per pack
          capabilities: ["messages"]            # optional hints
          ops: ["publish", "subscribe"]
          config_schema_ref: "schemas/nats-config.json" # local or remote refs
          state_schema_ref: "schemas/nats-state.json"
          docs_ref: "docs/nats.md"
          runtime:
            component_ref: "nats-provider@1.0.0"
            export: "run"
            world: "greentic:provider/schema-core@1.0.0"
```

Fields map directly to the shared `ProviderDecl` model:

- `provider_type` – identifier used in diagnostics and registries.
- `capabilities` / `ops` – optional hints exposed by the provider.
- `config_schema_ref` / `state_schema_ref` – references to JSON Schemas (local or remote).
- `runtime` – component+export binding that implements the provider runtime.
- `docs_ref` – optional docs reference for tooling.

## Examples

NATS broker:

```yaml
extensions:
  greentic.provider-extension.v1:
    kind: greentic.provider-extension.v1
    version: 1.0.0
    inline:
      providers:
        - provider_type: "nats-core"
          capabilities: ["messaging"]
          ops: ["publish", "subscribe"]
          config_schema_ref: "schemas/nats.json"
          runtime:
            component_ref: "nats-provider@1.0.0"
            export: "run"
            world: "greentic:provider/schema-core@1.0.0"
```

Kafka broker (conceptual):

```yaml
extensions:
  greentic.provider-extension.v1:
    kind: greentic.provider-extension.v1
    version: 1.0.0
    inline:
      providers:
        - provider_type: "kafka-core"
          capabilities: ["messaging"]
          ops: ["publish", "subscribe"]
          config_schema_ref: "schemas/kafka.json"
          runtime:
            component_ref: "kafka-provider@1.0.0"
            export: "run"
            world: "greentic:provider/schema-core@1.0.0"
```

## Validation and discovery

- `packc lint --in <pack-dir>` validates the provider extension alongside flows/templates.
- `greentic-pack providers list --pack <path> [--json]` lists declared providers from a source directory or `.gtpack`.
- `greentic-pack providers info <id> --pack <path> [--json]` prints a specific provider declaration.
- `greentic-pack providers validate --pack <path> [--strict]` validates the provider extension contents and local references.

Treat the provider extension as optional; packs without it continue to parse and build normally.
