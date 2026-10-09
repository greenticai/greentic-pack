# greentic-pack CLI Reference

This document describes every `greentic-pack` command and flag, along with
common usage patterns. The CLI is published as the `greentic-pack` binary.

If you want the easiest human-facing introduction, start with `README.md`.
If you are a coding agent or you are building AnswerDocuments, start with
`docs/coding-agents.md`.

Compatibility-only aliases and migration switches are documented in
`docs/vision/legacy.md`.

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
- `--manifest <FILE>`: manifest output path (default: `dist/manifest.cbor`).
- `--gtpack-out <FILE>`: `.gtpack` output (default: `dist/<pack_dir>.gtpack`).
- `--lock <FILE>`: pack.lock.cbor path (default: `<pack_dir>/pack.lock.cbor`).
- `--bundle <cache|none>`: embed component artifacts (`cache`) or keep refs only (`none`).
- `--dry-run`: validate without writing outputs.
- `--secrets-req <FILE>`: JSON file with extra secret requirements.
- `--default-secret-scope <ENV/TENANT[/TEAM]>`: fill missing secret scopes.
- `--allow-oci-tags`: allow tag-based OCI refs in extensions.
- `--no-extra-dirs`: only include `flows/`, `components/`, and `assets/` in the archive (skip extra directories and root files).

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

Resolve flow sidecars into `pack.lock.cbor`.

```
greentic-pack resolve --in <DIR> [--lock <FILE>]
```

Options:
- `--in <DIR>`: pack root (default: `.`).
- `--lock <FILE>`: custom lockfile path.

### `inspect-lock`

Print `pack.lock.cbor` as stable, sorted-key pretty JSON (machine-diffable).

```
greentic-pack inspect-lock --in <DIR> [--lock <FILE>]
```

Options:
- `--in <DIR>`: pack root (default: `.`).
- `--lock <FILE>`: custom lockfile path.

### `qa`

Run component QA specs and store answers as JSON + canonical CBOR.
The 0.6 runner path executes: `describe -> qa-spec -> ask -> apply-answers -> strict schema validation`.

```
greentic-pack qa --pack <DIR> --mode <default|setup|update|remove> [options]
```

Options:
- `--pack <DIR>`: pack root (default: `.`).
- `--mode <MODE>`: QA mode to run (default: `default`).
- `--answers <FILE_OR_DIR>`: override answers location (file or directory).
- `--locale <BCP47>`: locale tag for i18n lookup (default: `en`).
- `--non-interactive`: disable prompts; fail if required answers missing.
- `--reask`: re-ask questions even if answers exist.
- `--component <ID>`: run QA for specific component id(s).
- `--all-locked`: run QA for every entry in `pack.lock.cbor`.
- `--pack-only`: run pack-level QA only (requires `pack.cbor` metadata `greentic.qa`).

Example:

```
greentic-pack qa --pack examples/qa-demo --mode setup
```

Pack-level QA is optional; if `pack.cbor` includes metadata key `greentic.qa`,
it should be a CBOR-encoded `QaSpecSource` (InlineCbor or RefPackPath). When
using `RefPackPath`, place canonical `PackQaSpec` CBOR at:

```
qa/pack/default.cbor
qa/pack/setup.cbor
qa/pack/update.cbor
qa/pack/remove.cbor
```

### `doctor`

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
- `--no-flow-doctor`: disable per-flow doctor checks.
- `--no-component-doctor`: disable per-component doctor checks.
- `--validator-pack <REF>`: validator pack or component reference (path or `oci://`).
- `--validator-wasm <COMPONENT_ID=FILE>`: load a local validator component binary.

Example:

```
greentic-pack doctor dist/weather-demo.gtpack
```

#### Pack shapes

`doctor` derives the archive's shape from its own top-level entry names before
decoding anything, and reports which shape it found (see
[pack-format.md](./pack-format.md#archive-shapes)). What it checks depends on
the shape:

**Canonical pack** (`manifest.cbor`) — unchanged: manifest decoding, SBOM
consistency, signatures, referenced files, static routes, secret requirements,
the pack-lock component doctor and the per-flow flow doctor.

**DW application pack** (`manifest.json`, a greentic-designer export) —
`manifest.json` schema (`manifest_id`, `manifest`, and the optional
`display_name` / `locale` / `tenant` / `provider_overrides` types),
`metadata.json` (`pack_id`, and the declared `kind` cross-checked against the
derived shape), `flows/main.ygtc` through `greentic-flow doctor`, knowledge
sidecar asset references in both directions, and archive entry paths.

SBOM, signature, component lock, component manifests and static routes are
**not** checked for a DW application pack — it carries none of them, so
reporting them as missing would be noise. The report says so explicitly.

**Neither shape** — a hard failure listing both shapes, the entry that decides
each, and what the archive actually contains. Kind-awareness never turns a
corrupt archive into a pass.

`--format json` reports the shape in a top-level `archive_shape` field
(`canonical`, `dw-application`).

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

LEGACY TRACK: provider-extension/schema-core guidance is maintained for
compatibility. For v0.6-first authoring, start from `docs/usage.md`.

```
greentic-pack providers <subcommand> [options]
```

Subcommands:
- `list --pack <PATH> [--json]`
- `info <PROVIDER_ID> --pack <PATH> [--json]`
- `validate --pack <PATH> [--strict] [--json]`

### `add-extension provider`

Add or amend the provider extension entry stored in `pack.yaml`.

LEGACY TRACK: this command updates provider-extension/schema-core metadata used
by existing deployments. For details, see `docs/vision/legacy.md`.

```
greentic-pack add-extension provider [options]
```

Options:
- `--pack-dir <DIR>`: update a source directory containing `pack.yaml`.
- `--dry-run`: show the updated `pack.yaml` without persisting changes.
- `--id <PROVIDER_ID>`: provider type identifier to insert or update.
- `--kind <KIND>`: provider kind (e.g. `messaging`, `events`) used to populate `capabilities`.
- `--title <STRING>` / `--description <STRING>`: optional metadata stored alongside the provider.
- `--route <STRING>` / `--flow <FLOW_ID>`: convenience hints stored with the provider (useful for routing schemas).
- `--validator-ref <REF>` / `--validator-digest <DIGEST>`: optional validator reference and digest stored with the provider for strict validation.

Example:

```
greentic-pack add-extension provider --pack-dir examples/weather-demo \
  --id messaging.dummy --kind messaging --title "Dummy Messaging Provider"
```

### `add-extension capability`

Add or amend a capability offer in `extensions.greentic.ext.capabilities.v1`.

```
greentic-pack add-extension capability [options]
```

Options:
- `--pack-dir <DIR>`: update a source directory containing `pack.yaml`.
- `--dry-run`: show the updated `pack.yaml` without persisting changes.
- `--offer-id <ID>`: stable capability offer id.
- `--cap-id <CAP_ID>`: capability identifier (for example `greentic.cap.op_hook.pre`).
- `--version <VERSION>`: capability contract version (default `v1`).
- `--component-ref <COMPONENT_ID>`: provider component id from `components[].id`.
- `--op <OP_ID>`: provider operation to invoke.
- `--priority <INT>`: deterministic selection priority (ascending).
- `--requires-setup`: mark offer as requiring setup.
- `--qa-ref <PACK_REL_PATH>`: required with `--requires-setup`; must exist in pack sources.
- `--hook-op-name <OP_NAME>`: repeatable exact operation names for hook applicability.

Validation notes (`build`/`lint`):
- `requires_setup=true` requires a non-empty `setup.qa_ref`.
- `setup.qa_ref` must point to an existing file in the pack source.
- `provider.component_ref` must reference an existing component id from `pack.yaml`.

Example:

```
greentic-pack add-extension capability --pack-dir examples/weather-demo \
  --offer-id policy.pre.10 \
  --cap-id greentic.cap.op_hook.pre \
  --component-ref policy.hook \
  --op hook.evaluate \
  --priority 10 \
  --hook-op-name send
```

### `add-extension deployer`

Add or amend a generic deployer extension in `extensions.greentic.deployer.v1`.

```
greentic-pack add-extension deployer [options]
```

Options:
- `--pack-dir <DIR>`: update a source directory containing `pack.yaml`.
- `--dry-run`: show the updated `pack.yaml` without persisting changes.
- `--contract-id <ID>`: deployer contract identifier.
- `--op <OP>`: supported deployer operation (repeatable). Defaults to
  `generate`, `plan`, `apply`, `destroy`, `status`, `rollback`.
- `--flow-ref <OP=PATH>`: optional explicit flow ref mapping written into
  deployer metadata and used by validation.

Validation notes (`build`/`lint`):
- deployer metadata must include a non-empty `version`.
- `provides[].capability`, `provides[].contract`, and at least one op are required.
- any declared `flow_refs` must point to existing pack-relative files.

Example:

```
greentic-pack add-extension deployer --pack-dir examples/weather-demo \
  --contract-id greentic.deployer.v1 \
  --op generate \
  --flow-ref generate=flows/generate.ygtc
```

### `add-extension dependency`

Add or update an external extension dependency ref in `pack.extensions.json`.

```
greentic-pack add-extension dependency [OPTIONS]
```

Options:
- `--pack-dir <DIR>`: update a source directory containing `pack.yaml`.
- `--dry-run`: show the updated `pack.extensions.json` without persisting changes.
- `--id <ID>`: logical dependency id.
- `--role <ROLE>`: logical dependency role such as `deployer`.
- `--ref <REF>`: source reference such as `oci://...`, `file://...`, `repo://...`, or `store://...`.
- `--allow-tags`: allow author-edited tag refs in the source file.

Example:

```
greentic-pack add-extension dependency --pack-dir examples/weather-demo \
  --id greentic.deployer.v1 \
  --role deployer \
  --ref oci://ghcr.io/greenticai/packs/deployer:0.6.0 \
  --allow-tags
```

### `extensions-lock`

Resolve `pack.extensions.json` refs and write `pack.extensions.lock.json`.

```
greentic-pack extensions-lock [OPTIONS] --in <DIR>
```

Options:
- `--in <DIR>`: pack root containing `pack.extensions.json`.
- `--file <FILE>`: override the source file path.
- `--out <FILE>`: override the lock file path.

Lock notes:
- this command is separate from `resolve` and does not replace `pack.lock.cbor`
- `pack.extensions.json` remains human-edited and may allow tag refs
- `pack.extensions.lock.json` stores resolved digest-pinned refs plus media type and size when available
- `lint` and `build` validate that the lock file still matches the current
  `pack.extensions.json` entries
- `doctor --in <DIR>` surfaces stale or incomplete extension lock state as
  normal validation diagnostics

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

### `wizard`

Run the interactive wizard.

```
greentic-pack wizard
```

AnswerDocument modes:

```
greentic-pack wizard run [--answers <FILE>] [--emit-answers <FILE>] [--schema-version <VER>] [--migrate] [--dry-run]
greentic-pack wizard validate --answers <FILE> [--emit-answers <FILE>] [--schema-version <VER>] [--migrate]
greentic-pack wizard apply --answers <FILE> [--emit-answers <FILE>] [--schema-version <VER>] [--migrate]
```

- `run`:
  - default interactive behavior when no subcommand is passed
  - with `--answers`, runs non-interactive apply semantics
  - with `--dry-run`, records choices and emits answers without executing side effects
- `validate`:
  - validates AnswerDocument content only (no side effects)
- `apply`:
  - executes side effects from AnswerDocument (`greentic-flow`, `greentic-component`, `doctor`, `build`, optional `sign`)
  - stages any declared `answers.asset_staging` files/directories into `pack_dir` after the pack root exists and before delegate/build steps
- `--emit-answers` writes the normalized/migrated AnswerDocument envelope.
- `--migrate` allows missing/older schema metadata to be normalized to the target schema version.

Main menu:
- Create application pack
- Update application pack
- Create extension pack
- Update extension pack
- Add extension to existing pack
- Exit

Navigation contract:
- Main menu: `0) Exit`
- Submenus: `0) Back`, `M) Main Menu`

Create application pack flow:
- asks pack id and pack dir (`./<pack-id>` default)
- setup menu: `Edit flows`, `Add/edit components`, `Finalize`
- delegates:
  - `Edit flows` -> `greentic-flow wizard` (cwd = pack dir)
  - `Add/edit components` -> `greentic-component wizard` (cwd = pack dir)
- finalize pipeline:
  - `greentic-pack doctor --in <DIR>`
  - `greentic-pack build --in <DIR>`
  - optional sign prompt (`greentic-pack sign --pack <DIR> --key <FILE>`)

Update application pack flow:
- asks pack dir (`.` default)
- menu: `Edit flows`, `Add/edit components`, `Run update & validate`, `Sign`
- `Run update & validate` executes `doctor --in <DIR>` then `build --in <DIR>` then optional sign
- after successful delegate from flows/components, wizard auto-runs update & validate

Create extension pack flow:
- asks `Check for a new version [Y/n]`
- `Enter` / `Y` opens a second prompt for catalog URL with default:
  `https://github.com/greenticai/greentic-pack/blob/master/docs/extensions_capability_packs.catalog.v1.json`
- `n` uses the bundled/local default catalog ref:
  `file://docs/extensions_capability_packs.catalog.v1.json`
- direct refs still work when pasted at the first prompt (`fixture://...`, `file://<path>`, `https://...`, `oci://...`)
- if the default GitHub URL cannot be fetched, the wizard falls back to the bundled default catalog
- choose extension type (with explanation), choose template, choose pack dir
- records selected type, template, template QA answers, and edit answers in the AnswerDocument for replay
- catalog labels can be provided via i18n keys in catalog (`name_key`, `description_key`)
- creates a full base extension-pack scaffold before applying the selected template:
  `flows/`, `components/`, `i18n/`, `assets/`, `qa/`, `extensions/`
- seeds `assets/README.md` and `qa/README.md` when absent
- catalog templates may interpolate `{{edit.*}}` placeholders in file paths and
  contents, and may write binary scaffold artifacts with `write_binary_files`
- the default catalog includes a `Deployer` type that scaffolds placeholder
  deployer flows, schemas, examples, and a component bundle under
  `components/{{edit.component_ref}}/`
- deployer metadata is persisted under `extensions/deployer.json` and merged
  into `pack.yaml -> extensions.greentic.deployer.v1`
- deployer validation checks generic metadata and declared flow refs without
  introducing target-specific deployer fields
- the default catalog also includes:
  - `Runtime Capability` for component-backed capability runtime packs
  - `Contract` for schema/rules/policy-oriented packs
  - `Ops` for ops metadata and execution-hook packs
- these additional scaffold families remain capability-first and merge through
  `pack.yaml -> extensions.greentic.ext.capabilities.v1`
- applies scaffold plan, then runs finalize (`doctor --in`, `build --in`, optional sign)
- includes a required `Custom extension` scaffold path
- on catalog/template/delegate failures: localized error + `0) Back` / `M) Main Menu`

Update extension pack flow:
- asks pack dir + catalog ref
- menu: `Edit extension entries`, `Edit flows`, `Add/edit components`, `Run update & validate`, `Sign`
- `Run update & validate` executes `doctor --in <DIR>` then `build --in <DIR>` then optional sign
- `Edit extension entries` writes catalog answers under `extensions/<type>.json`
  and merges canonical extension data into `pack.yaml`
  (`greentic.ext.capabilities.v1` for capability packs,
  `greentic.deployer.v1` for deployer packs)

Add extension to existing pack flow:
- asks pack dir + the same catalog prompt flow used by create/update extension
- chooses extension type and asks edit questions
- writes catalog answers under `extensions/<type>.json` and merges canonical
  extension data into `pack.yaml`

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
- `docs/vision/legacy.md` for deprecated aliases and migration-only switches.
