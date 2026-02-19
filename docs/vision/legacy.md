# Legacy and Compatibility Notes

This page collects compatibility behavior that is intentionally not part of the
primary v0.6 learning path.

## CLI aliases and migration-only switches

- `greentic-pack inspect` is a deprecated alias of `greentic-pack doctor`.
  Use: `greentic-pack doctor`.
- `greentic-pack qa --mode upgrade` is a deprecated alias of `--mode update`.
  Use: `greentic-pack qa --mode update`.
- `greentic-pack build --allow-pack-schema` is migration-only.
  Use component manifests on the v0.6 path instead.

## Legacy build outputs

- `greentic-pack build --out` writes a legacy stub Wasm output path.
  Canonical artifact: `.gtpack` via `--gtpack-out`.
- `greentic-pack build --sbom` is a legacy JSON SBOM output path.
  Canonical archive inventory is `sbom.cbor` in the `.gtpack`.

## Legacy provider-extension track

The schema-core provider-extension flow is a legacy track and is not part of the
default component v0.6 QA runner path.

See:

- `docs/extension-provider-packs-howto.md`
- `docs/provider_extension.md`

Use canonical v0.6 docs for new packs unless you are maintaining existing
provider-extension deployments.
