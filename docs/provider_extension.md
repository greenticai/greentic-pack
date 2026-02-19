# Provider extension validation

> LEGACY TRACK
> This page documents the provider-extension/schema-core compatibility path.
> For canonical component v0.6 guidance, start at `docs/usage.md` and
> `docs/vision/README.md`.

This document describes the legacy/provider-extension path. The default
component 0.6 QA runner path does not depend on `schema-core`.
Use legacy provider commands (`greentic-pack add-extension provider`, `greentic-pack providers ...`)
for this track; do not route 0.6 QA runner flows through this schema-core world.

`packc` supports generic pack extensions via `PackManifest.extensions`. The known provider extension is keyed by `greentic.ext.provider`.

## Inline shape

For `greentic.ext.provider`, an inline payload is required:

```json
{
  "providers": [
    {
      "provider_type": "messaging.telegram.bot",
      "capabilities": ["send", "receive"],
      "ops": ["send", "reply"],
      "config_schema_ref": "schemas/messaging/telegram/config.schema.json",
      "state_schema_ref": "schemas/messaging/telegram/state.schema.json",
      "runtime": {
        "component_ref": "telegram-provider",
        "export": "provider",
        "world": "greentic:provider/schema-core@1.0.0"
      },
      "docs_ref": "schemas/messaging/telegram/README.md"
    }
  ]
}
```

Validation checks that:

- `inline.providers` exists and is non-empty.
- Each provider includes the required fields above.
- `runtime.world` must equal `greentic:provider/schema-core@1.0.0` (legacy provider-extension path).

Other extension kinds are accepted without additional shape validation.

## Validator references

Providers can optionally point to a validator pack/component:

- `validator_ref`: a pack/component reference (path or `oci://...`).
- `validator_digest`: optional digest for the validator ref (required in strict mode).

Validator packs built with `greentic-pack build` may embed the validator Wasm at
`components/<id>.wasm`; validator loading accepts both `components/<id>.wasm`
and `components/<id>@<version>/component.wasm`.

## Strict pinning

Set `GREENTIC_PACK_STRICT_EXTENSIONS=1` to enforce deterministic pinning:

- If an extension sets `location`, a `digest` is required.
- Allowed `location` schemes: `oci://`, `file://`, or `https://` (only with a digest).
