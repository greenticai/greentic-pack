# greentic-pack CLI Reference

This document describes every `greentic-pack` command and flag, along with
common usage patterns. The CLI is published as the `greentic-pack` binary.

## Command structure

```
greentic-pack [global options] <command> [command options]
```

### Global options

- `--log <LEVEL>`: logging filter (default: `info`, overrides `PACKC_LOG`).
- `--offline`: hard-disable any network access (resolving refs, cloning repos,
  GUI asset builds). Equivalent to `GREENTIC_PACK_OFFLINE=1` but the flag wins.
- `--cache-dir <DIR>`: override the cache root (default: `<pack_dir>/.packc/` or
  `GREENTIC_PACK_CACHE_DIR`).
- `--config-override <FILE>`: TOML/JSON overrides for greentic-config.
- `--json`: emit machine-readable JSON where applicable.

## Commands

### `new`

Scaffold a new pack directory.

```
greentic-pack new <PACK_ID> --dir <DIR>
```

Options:
- `--dir <DIR>`: directory to create the pack in.
- `<PACK_ID>`: required positional pack id.

Example:

```
greentic-pack new acme.weather --dir ./acme-weather
```

### `build`

Build a pack and emit artifacts (manifest, optional SBOM, `.gtpack`).

```
greentic-pack build --in <DIR> [options]
```

Options:
- `--in <DIR>`: pack root containing `pack.yaml`.
- `--no-update`: skip the pre-build `update` sync.
- `--out <FILE>`: write a stub Wasm component (legacy).
- `--manifest <FILE>`: manifest output path (default: `dist/manifest.cbor`).
- `--sbom <FILE>`: SBOM output path (legacy JSON stub).
- `--gtpack-out <FILE>`: `.gtpack` output (default: `dist/<pack_dir>.gtpack`).
- `--lock <FILE>`: pack.lock.json path (default: `<pack_dir>/pack.lock.json`).
- `--bundle <cache|none>`: embed component artifacts (`cache`) or keep refs only (`none`).
- `--dry-run`: validate without writing outputs.
- `--secrets-req <FILE>`: JSON file with extra secret requirements.
- `--default-secret-scope <ENV/TENANT[/TEAM]>`: fill missing secret scopes.
- `--allow-oci-tags`: allow tag-based OCI refs in extensions.

Example:

```
greentic-pack build --in examples/weather-demo --gtpack-out dist/weather-demo.gtpack
```

### `lint`

Validate `pack.yaml` and compile flows.

```
greentic-pack lint --in <DIR> [--allow-oci-tags]
```

Options:
- `--in <DIR>`: pack root.
- `--allow-oci-tags`: allow tag-based OCI refs in extensions.

### `components`

Sync `pack.yaml` components with files under `components/`.

```
greentic-pack components --in <DIR>
```

### `update`

Sync `pack.yaml` components and flows with `components/` and `flows/`.

```
greentic-pack update --in <DIR> [--strict]
```

Options:
- `--in <DIR>`: pack root.
- `--strict`: require resolve sidecars for all flow nodes.

### `resolve`

Resolve flow sidecars into `pack.lock.json`.

```
greentic-pack resolve --in <DIR> [--lock <FILE>]
```

Options:
- `--in <DIR>`: pack root (default: `.`).
- `--lock <FILE>`: custom lockfile path.

### `doctor` (alias: `inspect`)

Inspect a pack archive or source directory.

```
greentic-pack doctor [PATH] [options]
```

Options:
- `PATH`: pack directory or `.gtpack` path (default: current directory).
- `--pack <FILE>`: force archive path.
- `--in <DIR>`: force source directory.
- `--archive`: treat `PATH` as archive.
- `--source`: treat `PATH` as source.
- `--allow-oci-tags`: allow tag-based OCI refs in extensions.

Example:

```
greentic-pack doctor dist/weather-demo.gtpack
```

### `plan`

Generate a deployment plan from a pack archive or source directory.

```
greentic-pack plan <PATH> [options]
```

Options:
- `<PATH>`: `.gtpack` archive or pack dir.
- `--tenant <ID>`: tenant id (default: `tenant-local`).
- `--environment <ID>`: environment id (default: `local`).
- `--json`: compact JSON output.
- `--verbose`: extra diagnostics when building from source.

### `providers`

Inspect or validate provider extensions.

```
greentic-pack providers <subcommand> [options]
```

Subcommands:
- `list --pack <PATH> [--json]`
- `info <PROVIDER_ID> --pack <PATH> [--json]`
- `validate --pack <PATH> [--strict] [--json]`

### `sign`

Sign a manifest with an Ed25519 private key.

```
greentic-pack sign --pack <DIR> --key <FILE> [--manifest <FILE>] [--key-id <ID>]
```

### `verify`

Verify a signed manifest with an Ed25519 public key.

```
greentic-pack verify --pack <DIR> --key <FILE> [--manifest <FILE>]
```

### `config`

Print resolved greentic-config (provenance + warnings).

```
greentic-pack config [--json]
```

### `gui loveable-convert`

Convert a Loveable build into a GUI `.gtpack`.

```
greentic-pack gui loveable-convert --pack-kind <layout|auth|feature|skin|telemetry> \
  --id <PACK_ID> --version <SEMVER> --out <FILE> [options]
```

Options:
- `--pack-kind <KIND>`: GUI pack kind (`layout`, `auth`, `feature`, `skin`, `telemetry`).
- `--id <PACK_ID>`: pack id to embed in `pack.yaml`.
- `--version <SEMVER>`: pack version.
- `--pack-manifest-kind <KIND>`: `application|provider|infrastructure|library`.
- `--publisher <STRING>`: publisher (default: `greentic.gui`).
- `--name <STRING>`: display name for the GUI pack.
- `--repo-url <URL>`: clone and build a repo (mutually exclusive with `--dir`, `--assets-dir`).
- `--branch <BRANCH>`: git branch (default: `main`).
- `--dir <DIR>`: local repo path (mutually exclusive with `--repo-url`, `--assets-dir`).
- `--assets-dir <DIR>`: prebuilt assets dir (skips build).
- `--package-dir <DIR>`: build subdirectory inside the repo.
- `--install-cmd <CMD>`: override install command.
- `--build-cmd <CMD>`: override build command.
- `--build-dir <DIR>`: override build output directory.
- `--spa <true|false>`: force SPA/MPA mode.
- `--route <path:html>`: route overrides (repeatable).
- `--routes <CSV>`: comma-separated route overrides.
- `--out <FILE>`: output `.gtpack` path.

Example:

```
greentic-pack gui loveable-convert --pack-kind layout \
  --id acme.gui.layout --version 0.1.0 --dir ./my-app --out dist/gui.gtpack
```

## Related docs

- `docs/usage.md` for workflows and best practices.
- `docs/pack-format.md` for `.gtpack` internals.
- `docs/provider_extension.md` for provider metadata.
- `docs/pack_extensions_components.md` for component source extensions.
