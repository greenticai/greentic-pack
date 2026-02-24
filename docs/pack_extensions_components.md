# Components Extension (OCI refs)

Use the `extensions.greentic.components` entry to declare OCI component references that should be resolved externally (for example by a distributor). This keeps the core pack schema stable while allowing packs to point at registry-hosted components without embedding them.

## Shape

```yaml
extensions:
  greentic.components:
    kind: greentic.components
    version: v1
    inline:
      refs:
        - ghcr.io/org/name@sha256:<64-hex>   # required, may be empty list
      mode: eager | lazy                     # optional, preserved only
      allow_tags: false                      # optional, preserved only
```

- `refs` must be an array of strings. By default each entry must be digest pinned (`...@sha256:<64-hex>`).
- `mode` and `allow_tags` are advisory hints for installers; `packc`/`greentic-pack` only preserve them.

## Validation rules

- Digest-pinned refs are accepted by default.
- Tag refs (`ghcr.io/org/name:tag`) are rejected unless you opt in with `--allow-oci-tags`.
- Invalid shapes (missing `refs`, non-string entries, bad digest/tag formats) fail validation with actionable errors.

## CLI flag

When building/linting/inspecting from source:

```bash
packc build --in . --allow-oci-tags       # permit tag refs in the extension
packc lint --in . --allow-oci-tags
packc inspect --in . --allow-oci-tags
```

Archives (`.gtpack`) preserve the extension exactly; no download/pull occurs. Resolution is expected to be handled by installers/distributors that understand this extension.

## Capabilities Extension (v1)

Use `extensions.greentic.ext.capabilities.v1` to declare capability offers consumed by operator/runtime capability resolution.

### Shape

```yaml
extensions:
  greentic.ext.capabilities.v1:
    kind: greentic.ext.capabilities.v1
    version: 1.0.0
    inline:
      schema_version: 1
      offers:
        - offer_id: policy.pre.10
          cap_id: greentic.cap.op_hook.pre
          version: v1
          provider:
            component_ref: policy.hook
            op: hook.evaluate
          priority: 10
          requires_setup: false
          applies_to:
            op_names: [send]
```

### Validation rules

- `schema_version` must be `1`.
- Each offer `provider.component_ref` must match an id from `components[].id` in `pack.yaml`.
- If `requires_setup: true`:
  - `setup` must be present;
  - `setup.qa_ref` must be non-empty and reference an existing file under the pack root.

These checks run in `greentic-pack build`/`lint` paths for source packs.
