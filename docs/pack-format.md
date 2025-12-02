# Greentic Pack Format

This document describes the on-disk representation for `.gtpack` archives
produced by `greentic-pack`.

## Archive Layout

`*.gtpack` files are deterministic ZIP archives. Entries are written in lexical
order with DOS timestamps pinned to `1980-01-01T00:00:00Z` and permissions
normalised to `0644`. The root contains the following directories:

```
manifest.cbor              # canonical CBOR manifest
manifest.json              # human readable manifest
sbom.json                  # SPDX-like file catalogue
provenance.json            # build metadata (builder, git sha, toolchain)
flows/<id>/flow.ygtc       # canonical YAML source
flows/<id>/flow.json       # normalised JSON derived from the `.ygtc` source
schemas/<name>@<ver>/...   # optional node schema
components/<name>@<ver>/component.wasm
components/<name>@<ver>/manifest.json (optional)
assets/...                 # optional additional assets
signatures/pack.sig        # JSON envelope over digests
signatures/chain.pem       # signing certificate chain
```

Flow sources are kept as `.ygtc` files; the canonical JSON under each
`flows/<id>/flow.json` is computed by `greentic-flow` and reflects the same
data used inside the `.gtpack`.

Only regular files are allowed—directories, symlinks, and special entries are
rejected by the reader before any manifest parsing occurs.

## Hashing & SBOM

Every payload file (excluding `signatures/*`) is recorded in `sbom.json` as a
`SbomEntry` with the relative path, byte length, media type, and BLAKE3 digest.
During verification the reader recomputes all hashes and also ensures that every
file present in the archive is listed in the SBOM. This SBOM is also part of the
signature input.

The CycloneDX file tracked as `dist/sbom.cdx.json` is derived from this same
inventory. When you build a `.gtpack` via `packc --gtpack-out`, the CycloneDX
artifact and the archive’s own `sbom.json` are produced from the same flows and
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

## Deterministic Builds

`PackBuilder` always emits deterministic archives:

- Entries sorted lexically.
- Stable DOS timestamps and permissions.
- Stored compression mode (no deflate variance).
- Media types recorded for every SBOM entry.

The `examples/build_demo.rs` example and the CI workflow both build the same
pack twice and ensure the resulting archives are byte-identical, guaranteeing
the determinism contract.
