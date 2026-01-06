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
