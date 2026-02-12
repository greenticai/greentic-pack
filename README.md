# Greentic Pack

Greentic packs are portable, signed bundles of flows, components, assets, and
metadata. They let you ship a complete capability as a single `.gtpack` file,
then run, validate, or deploy it anywhere with consistent results.

A pack can describe an **application** flow, **infrastructure** configuration,
or **provider** extension. You can evolve each pack independently, publish it to
registries, and compose packs into larger systems without rebuilding your app.

## Why packs are useful

- **Portable**: one archive captures flows, manifests, component sources, and
  assets.
- **Composable**: pack dependencies let teams reuse shared capabilities.
- **Inspectable**: `greentic-pack doctor` surfaces flows, components, sources,
  providers, and SBOM state.
- **Verifiable**: packs include checksums, SBOMs, and optional signatures.
- **Extensible**: provider extensions let you add new capabilities without
  reworking the core runtime.

## Quickstart (try a demo pack)

Install the CLI:

```bash
cargo install cargo-binstall
cargo binstall greentic-pack
```

Build and inspect the demo pack in this repo:

```bash
greentic-pack lint --in examples/weather-demo
greentic-pack build --in examples/weather-demo --gtpack-out dist/weather-demo.gtpack
greentic-pack doctor dist/weather-demo.gtpack
```

Create a new pack scaffold:

```bash
greentic-pack new acme.weather --dir ./acme-weather
```

## Pack types at a glance

- **Application packs**: flows, templates, and component references that power
  user-facing experiences.
- **Infrastructure packs**: operational configuration, telemetry, and deployment
  defaults for a platform team.
- **Provider packs**: add new capabilities via provider extensions and
  component-backed runtimes.

For the full taxonomy and rules, see:
- `docs/repo-pack-types.md`
- `docs/provider_extension.md`

## How packs work (short version)

1. Author a `pack.yaml` and flows (`.ygtc`).
2. Resolve component sources into `pack.lock.cbor`.
3. Build a canonical `.gtpack` that embeds the manifest, SBOM, and assets.
4. Distribute the `.gtpack` and inspect it with `greentic-pack doctor` or
   derive a deployment plan with `greentic-pack plan`.

Deep dives:
- `docs/pack-format.md`
- `docs/usage.md`
- `docs/pack_extensions_components.md`
- `docs/events-provider-packs.md`

## Example packs

- `examples/weather-demo` – application pack with a simple conversational flow.
- `examples/qa-demo` – multi-turn QA flow with subflow calls.
- `examples/billing-demo`, `examples/search-demo`, `examples/reco-demo` –
  provider-oriented manifests and pack kinds.

## CLI reference

See `docs/cli.md` for a complete reference of commands, flags, and workflows.

## Local checks & publishing

- Local CI wrapper: `ci/local_check.sh`
- Publishing guidance: `docs/publishing.md`

## Contributing & security

- Contributing guidelines: `CONTRIBUTING.md`
- Security policy: `SECURITY.md`
