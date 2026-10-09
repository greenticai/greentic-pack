# Greentic Pack Format

This document describes the on-disk representation for `.gtpack` archives
produced by `greentic-pack`.

## Archive Layout

`*.gtpack` files are deterministic ZIP archives. Entries are written in lexical
order with DOS timestamps pinned to `1980-01-01T00:00:00Z` and permissions
normalised to `0644`. The root contains the following directories:

```
manifest.cbor              # canonical CBOR manifest
sbom.cbor                  # CBOR inventory of every payload stored in the archive
schemas/<name>@<ver>/...   # optional node schema
components/<name>@<ver>/component.wasm
components/<name>@<ver>/manifest.cbor (optional)
assets/...                 # mapped root files plus other auxiliary assets
signatures/pack.sig        # JSON signature envelope covering the manifest + SBOM
signatures/chain.pem       # signing certificate chain
```

Production `.gtpack` archives include only the canonical CBOR manifest, its SBOM,
runtime artifacts (WASM + component manifests), asset material, and the signature
bundle. Flow definitions and pack sources live inside `manifest.cbor`; the
individual `.ygtc`/JSON files are generated only when `greentic-pack build --dev`
is requested (or when `greentic-pack doctor` inspects a source directory).

Any non-reserved file at the pack root is remapped to `assets/<filename>` inside
the archive, while complete directories (e.g. `schemas/`, `templates/`) are
included as-is. The contents of an existing `assets/` directory are bundled
verbatim and take precedence: if a root file would conflict with `assets/<name>`,
the archive keeps the `assets` version and emits a warning instead of overwriting.

Static route mounts are declared separately from asset packaging. Packs that
need tenant-facing hosted assets should keep the payload under `assets/...` and
declare the mount metadata through the `greentic.static-routes.v1` extension in
the pack manifest.

Only regular files are allowed—directories, symlinks, and special entries are
rejected by the reader before any manifest parsing occurs.

## Hashing & SBOM

Every payload file (excluding `signatures/*`) is recorded in `sbom.cbor` as a
`SbomEntry` with the relative path, byte length, media type, and BLAKE3 digest.
During verification the reader recomputes all hashes and also ensures that every
file present in the archive is listed in the SBOM. This SBOM is also part of the
signature input.

The CycloneDX file tracked as `dist/sbom.cdx.json` is derived from this same
inventory. When you build a `.gtpack` via `packc --gtpack-out`, the CycloneDX
artifact and the archive’s own `sbom.cbor` are produced from the same flows and
templates even though their formatting differs.

Common media types:

- `application/cbor` – `manifest.cbor`
- `application/json` – JSON payloads and schemas
- `application/yaml` – flow sources
- `application/wasm` – WASI components
- `application/octet-stream` – arbitrary assets

## Signing

`signatures/pack.sig` is a JSON envelope containing the signing algorithm,
decoded signature (URL-safe Base64), digest, timestamp, and optional key
fingerprint. The digest covers:

1. The canonical manifest (`manifest.cbor`).
2. The SBOM document (`sbom.json`).
3. Every SBOM entry, concatenating `path + "\n" + blake3` in lexical order.

`signatures/chain.pem` carries the certificate chain. Dev builds generate an
ephemeral Ed25519 key and a single self-signed certificate with
`CN=greentic-dev-local`. Production builds should bundle the full trust chain.

## Pack kinds

Supported `kind` values include:

- `application`
- `source-provider`
- `scanner`
- `signing`
- `attestation`
- `policy-engine`
- `oci-provider`
- `billing-provider`
- `search-provider`
- `recommendation-provider`
- `distribution-bundle` (offline bundle GT pack)

`rollout-strategy` remains reserved for future phases and must not be used.

### Archive shapes

`kind` above is a field *inside* a canonical manifest. Separately from it, a
`.gtpack` file comes in one of two **archive shapes**, and they carry different
schemas from different crates:

| shape | deciding entry | manifest type | crate | producer |
|---|---|---|---|---|
| canonical | `manifest.cbor` | `PackManifest` | `greentic_types` | `greentic-pack build` |
| DW application pack | `manifest.json` | `AnswerDocPackSpec` | local to greentic-designer | greentic-designer `write_gtpack` |

These are **different schemas, not two encodings of one schema**. Re-encoding an
`AnswerDocPackSpec` as CBOR and naming it `manifest.cbor` is therefore not a
migration: the reader would find the entry and then fail to decode it as a
`PackManifest`, turning a clear "this is another shape" into a misleading
"this file is malformed".

A DW application pack carries:

```
manifest.json              # AnswerDocPackSpec (pretty-printed JSON)
metadata.json              # pack_id, kind, created_at, source
knowledge_base.json        # optional, static-injection KB index
knowledge_corpus.json      # optional, embedding-retrieval corpus index
assets/knowledge/<slug>.txt  # optional, indexed by the sidecars above
flows/main.ygtc            # optional, emitted only for an executing node
```

Note that an Agentic Worker pack built by `greentic-pack build` with
`kind: dw-application` is a **canonical** archive: it has `manifest.cbor` plus
`dw-agents.json` and `secrets-policy.json` sidecars. "Agentic Worker pack" alone
is therefore ambiguous; the archive shape is what determines how a pack is read.

#### How the shape is determined

From the archive's own top-level entry names, never from a declared field:

1. a top-level `manifest.cbor` entry -> canonical;
2. otherwise a top-level `manifest.json` entry -> DW application pack;
3. otherwise -> unrecognised, which is a hard failure.

Matching is exact and top-level only — a nested `assets/i18n/_manifest.json` is
an asset index, not a pack manifest.

`metadata.json` carries a `kind` marker (`"DwApplication"`). It is **advisory**:
`doctor` cross-checks it against the derived shape and reports a mismatch, but
never uses it to decide the shape. A manifest field can drift out of sync with
the zip it rides in; the zip's own contents cannot.

If an archive somehow carries both manifests, `manifest.cbor` wins and `doctor`
warns — a pack should carry exactly one manifest.

### Distribution bundles

Use `kind: distribution-bundle` with a `distribution` section:

```yaml
kind: distribution-bundle
distribution:
  bundle_id: bundle-123          # optional; defaults to pack id if omitted
  tenant: {}                     # opaque JSON map; conventionally serialized TenantCtx
  environment_ref: env-prod
  desired_state_version: v1
  components:
    - component_id: app.component
      version: 1.0.0
      digest: sha256:deadbeef
      artifact_path: artifacts/app.component.wasm
      kind: software
      artifact_type: binary/linux-x86_64
      tags: [runner-dependency]
      platform: linux-x86_64
      entrypoint: install.sh
  platform_components:
    - component_id: greentic-runner
      version: 1.2.3
      digest: sha256:cafebabe
      artifact_path: artifacts/runner.wasm
```

`tenant` is validated only as a JSON object; downstream tooling interprets it as a serialized TenantCtx.

### Component descriptors and software installs

Components may carry an optional `kind` (e.g. `software`), optional `artifact_type` hint, `tags`, `platform`, and `entrypoint`. `artifact_path` is a generic path inside the `.gtpack`; the pack format does not assume WASM. Downstream tooling decides how to execute or install.

## Capabilities

Each compiled `manifest.capabilities` entry is a `ComponentCapability { name, description? }`. The array has two layers of meaning that share the wire format:

- **Per-component derived** (auto): `derive_pack_capabilities` walks every component's host/WASI permission set and emits entries like `host:http`, `host:state:read`, `wasi:random`. These describe *what host APIs the WASM needs at runtime*. Authors do not write them by hand.
- **Pack-declared opt-ins** (author-supplied): write a top-level `capabilities:` block in `pack.yaml`. These are *feature switches the pack opts into* (e.g. fast2flow free-text routing, observer hooks). The runtime gates downstream behaviour on matching exact strings.

The two layers are distinguished by name convention: `greentic.cap.*` for pack-level opt-ins, `host:*` / `wasi:*` for derived permissions.

```yaml
# pack.yaml
pack_id: example.pack
version: 0.1.0
kind: application
publisher: Greentic
capabilities:
  - name: greentic.cap.fast2flow.v1
    description: opts into fast2flow free-text routing
```

At build time, author-declared entries union with the derived entries — author entries land first so their descriptions survive a collision.

At deploy time the runtime's `BundleCapabilityGate` matches the exact `greentic.cap.fast2flow.v1` string and enables the routing pipeline. The pack may also ship a pre-seeded routing index at `assets/intent-index.json`; the runtime materializes it under the indexes path automatically, so no environment configuration is required for a local run. See [fast2flow / Opting In](https://docs.greentic.ai/components/fast2flow/#opting-in-from-a-pack).

## Verification Semantics

`open_pack(path, policy)` reads the archive, enforces size limits, rejects
path traversal ("zip slip"), and only accepts regular files. The SBOM is
validated before any manifest processing. Signature handling depends on the
requested `SigningPolicy`:

- `DevOk` – accepts the self-signed dev certificate, warning when the chain
  contains more than one entry.
- `Strict` – rejects dev/self-signed chains and requires a non-dev certificate.

The function returns the decoded `PackManifest` together with a
`VerifyReport { signature_ok, sbom_ok, warnings }` so callers can surface
warnings while still treating the pack as verified.

## Static Routes Extension

Use `extensions.greentic.static-routes.v1` when a pack needs runtime to mount
packaged files as public static surfaces.

The extension does not change how files are bundled. Assets remain ordinary
files under `assets/...`; the extension only declares mount metadata.

### Shape

```yaml
extensions:
  greentic.static-routes.v1:
    kind: greentic.static-routes.v1
    version: 1.0.0
    inline:
      version: 1
      routes:
        - id: webchat-gui
          public_path: /v1/web/webchat/{tenant}
          source_root: assets/webchat-gui
          scope:
            tenant: true
            team: false
          index_file: index.html
          spa_fallback: index.html
          cache:
            strategy: public-max-age
            max_age_seconds: 3600
          exports:
            base_url: webchat_gui_base_url
            entry_url: webchat_gui_entry_url
```

### V1 rules

- `public_path` must start with `/v1/web/`.
- `public_path` supports only literal path segments plus `{tenant}` and `{team}`.
- Wildcards, regex-like segments, arbitrary placeholders, query strings, and fragments are rejected.
- `source_root` must be a directory-backed logical path under `assets/`.
- `index_file` and `spa_fallback`, when present, are relative to `source_root`.
- Exported URL names must be unique across the whole pack.
- `scope.team=true` is invalid unless `scope.tenant=true`.

### Cache policy

Supported `cache.strategy` values in v1:

- `none`
- `public-max-age`

When `strategy: public-max-age`, `max_age_seconds` is required. For `none`,
`max_age_seconds` must be omitted.

## Deterministic Builds

`PackBuilder` always emits deterministic archives:

- Entries sorted lexically.
- Stable DOS timestamps and permissions.
- Stored compression mode (no deflate variance).
- Media types recorded for every SBOM entry.

The `examples/build_demo.rs` example and the CI workflow both build the same
pack twice and ensure the resulting archives are byte-identical, guaranteeing
the determinism contract.
