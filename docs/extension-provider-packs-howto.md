# Extension Provider Packs Howto

## What This Guide Covers

This guide explains:

- how to create extension packs with the wizard
- how extensions are declared and packaged
- how component operations are discovered and used
- how validation works
- how QA wizard questions and i18n work

## How Extensions Work

Extensions are extra metadata under `pack.yaml`:

```yaml
extensions:
  <extension-key>:
    kind: <extension-kind>
    version: <extension-version>
    inline: <extension-payload>
```

Source is YAML, but built pack artifacts are CBOR-first (`manifest.cbor`, optional
`pack.cbor`, optional `pack.lock.cbor` after resolve).

## Create an Extension Pack With the Wizard

1. Scaffold an extension pack.

```bash
greentic-pack wizard new-extension <PACK_ID> --kind <KIND> --out <DIR> --locale en --name "<Display Name>"
```

2. Add components to the pack.

```bash
greentic-pack wizard add-component <REF_OR_ID> --pack <DIR>
```

3. Sync `pack.yaml` with on-disk components/flows.

```bash
greentic-pack update --in <DIR>
```

4. Validate, resolve, build, inspect.

```bash
greentic-pack lint --in <DIR>
greentic-pack resolve --in <DIR>
greentic-pack build --in <DIR>
greentic-pack inspect --in <DIR>/dist/pack.gtpack --json
```

## Multiple Extension Types

### Provider extension (`greentic.provider-extension.v1`)

Use when you need provider declarations (types/capabilities/ops/runtime binding).

```yaml
extensions:
  greentic.provider-extension.v1:
    kind: greentic.provider-extension.v1
    version: 1.0.0
    inline:
      providers:
        - provider_type: "messaging.telegram.bot"
          capabilities: ["send", "receive"]
          ops: ["send", "reply"]
          config_schema_ref: "schemas/messaging/telegram/config.schema.json"
          state_schema_ref: "schemas/messaging/telegram/state.schema.json"
          runtime:
            component_ref: "telegram-provider@1.0.0"
            export: "run"
            world: "greentic:provider/schema-core@1.0.0"
```

Provider helper commands:

- `greentic-pack add-extension provider ...`
- `greentic-pack providers list --pack <path> [--json]`
- `greentic-pack providers info <id> --pack <path> [--json]`
- `greentic-pack providers validate --pack <path> [--strict]`

### Components extension (`greentic.components`)

Use when you want external OCI component refs.

```yaml
extensions:
  greentic.components:
    kind: greentic.components
    version: v1
    inline:
      refs:
        - ghcr.io/org/name@sha256:<64-hex>
```

Rules: `inline.refs` is required; digest pinning is default; tag refs need
`--allow-oci-tags`.

### Custom extension kinds

Unknown extension kinds are preserved so you can carry organization-specific
metadata.

## Operations: How They Are Determined

- Component operations come from component describe metadata.
- During `wizard add-component` / `resolve`, operations are collected and written
  into lock/manifests.
- Flow nodes using `component.exec` are normalized to explicit operations, so
  runtime behavior is deterministic.

## Validation: What Is Checked

- `greentic-pack lint` validates pack config, flows, and known extension shapes.
- `greentic-pack providers validate` validates provider-extension content and
  refs (use `--strict` for stronger pinning).
- `greentic-pack resolve` verifies component refs and writes deterministic lock
  data (`pack.lock.cbor`).
- `greentic-pack doctor` and `inspect` verify packaged output.

## QA Wizard Questions and i18n

`greentic-pack qa` runs the QA pipeline:

- `describe -> qa-spec -> ask -> apply-answers -> strict schema validation`

Outputs:

- `answers/<mode>.answers.json`
- `answers/<mode>.answers.cbor` (canonical)

I18n behavior:

- QA labels/help text are resolved from `assets/i18n/<locale>.json` (`--locale`
  controls locale).
- Components and pack QA use `I18nText` keys; missing keys fall back to inline
  default text where provided.
- Pack-level QA can be declared in `pack.cbor` metadata (`greentic.qa`) and can
  point to canonical CBOR files like `qa/pack/default.cbor`.
