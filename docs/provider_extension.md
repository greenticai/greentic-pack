# Provider extension validation

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
- `runtime.world` must equal `greentic:provider/schema-core@1.0.0`.

Other extension kinds are accepted without additional shape validation.

## Strict pinning

Set `GREENTIC_PACK_STRICT_EXTENSIONS=1` to enforce deterministic pinning:

- If an extension sets `location`, a `digest` is required.
- Allowed `location` schemes: `oci://`, `file://`, or `https://` (only with a digest).
