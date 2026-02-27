PR-PACK-01: Add greentic-pack wizard interactive entrypoint (QA-driven)
Status: Implemented (2026-02-27)

Completion checklist

- [x] PR-PACK-01 Interactive wizard entrypoint + main menu + nav contract
- [x] PR-PACK-02 i18n bundles embedded for wizard path + key-driven UI
- [x] PR-PACK-03 Create application pack flow (scaffold + delegates + finalize)
- [x] PR-PACK-04 Update application pack flow + update/validate menu path
- [x] PR-PACK-05 Extension catalog loader (`fixture://`, `file://`, `oci://`) + type selection
- [x] PR-PACK-06 Create extension pack from template plan + custom scaffold
- [x] PR-PACK-07 Update extension pack flow + catalog-driven entry edits
- [x] PR-PACK-08 Optional signing integrated into create/update finalize paths
- [x] PR-PACK-09 Guardrails (no direct print in wizard module, i18n key coverage, fixture key checks)

Implemented decisions captured

- Legacy wizard subcommands removed in favor of interactive `greentic-pack wizard`.
- Delegation uses spawned binaries with inherited stdio and `cwd` = pack root:
  - `greentic-flow wizard`
  - `greentic-component wizard`
- Delegate/process failure returns to current flow with localized error + `0) Back` / `M) Main Menu`.
- Finalize runs immediately (no extra confirmation): `doctor --in <DIR>` -> `build --in <DIR>` -> optional sign prompt.
- Sign key path is remembered in wizard session and reused as prompt default.
- OCI catalog fetch failure hard-fails with localized error + `0) Back` / `M) Main Menu`.
- Canonical extension key in generated scaffold uses `greentic.provider-extension.v1`.
- Wizard output is routed through centralized renderer adapter (`wizard_ui`), not direct print macros in wizard module.

Current scope notes

- Wizard finalize pipeline follows implemented decision path (`doctor` then `build`) and does not run explicit `update`/`resolve` stages.
- Catalog model currently lives in `packc` as a local typed struct (lift-to-types later if needed).

Goal

Introduce the interactive wizard command with the Main Menu only.

Scope

Add greentic-pack wizard (or expand existing wizard helpers into an interactive mode).

Main Menu choices:

Create application pack

Update application pack

Create extension pack

Update extension pack

Exit

Navigation rules:

Only main menu has 0) Exit

All other screens use 0) Back and M) Main Menu

Menus rendered + input collected via greentic-qa-lib.

Acceptance

greentic-pack wizard launches and you can exit.

No direct println! scattered in wizard paths (keep output centralized).

Tests

wizard_main_menu_nav_smoke: simulate answers (qa-lib non-TTY mode) and ensure correct state transitions.

PR-PACK-02: Wizard i18n baseline (ship bundles with binary via greentic-i18n)
Goal

Make the wizard UI 100% i18n-key driven using greentic-i18n, with bundles embedded in the binary.

Scope

Add embedded bundles:

i18n/pack_wizard/en-GB.json (baseline)

optional stubs: fr-FR.json, nl-NL.json

Load and resolve via greentic-i18n (embedded provider).

All wizard UI strings become i18n keys:

titles, menu items, back/main labels, errors.

Acceptance

Wizard UI contains no raw English literals (except debug-only missing-key markers).

Works offline (bundles embedded).

Tests

wizard_i18n_smoke_en_gb: render main menu and assert no missing keys.

Optional: grep-based “no raw print” + “no raw strings” guard for wizard modules.

PR-PACK-03: Create application pack (scaffold + optional flow edit + pipeline)
Goal

Implement Create application pack flow.

UX

Ask pack id

Ask pack dir [./<pack-id>]

Ask “Edit flows now?”:

1) Edit flows

2) Skip

(Selecting “Edit flows” goes to flow wizard; no explanatory text.)

Then finalize pipeline always runs:

update → resolve → doctor → build

After build: optional sign prompt.

Implementation

Call existing commands programmatically (or via internal functions if they exist):

new --dir <DIR> <PACK_ID>

update --in <DIR>

resolve --in <DIR>

doctor <DIR> (or --in)

build --in <DIR>

“Edit flows” jumps into greentic-flow wizard (spawn process, inherit stdio).

All UI via qa-lib + i18n keys.

Acceptance

One wizard run can create pack, optionally edit flows, and produce a build artifact.

Fail-fast if resolve/doctor/build fails (with localized error summary).

Tests

create_app_pack_happy_path (temp dir):

scaffold created

update/resolve invoked (mockable)

If hard to fully integrate, do a “dry-run mode” behind fixture:// resolver for tests.

PR-PACK-04: Update application pack (menu + always update & validate)
Goal

Implement Update application pack flow with your “merged” behavior.

UX

Ask pack directory [.]

Menu:

Edit flows

Run update & validate (update → resolve → doctor → build)

Sign (optional)

Back / M) Main Menu

Optionally: after returning from “Edit flows”, auto-run update & validate (opinionated mode). If you want that, do it now.

Acceptance

User can edit flows (via flow wizard) then run pipeline.

Pipeline always includes build.

Tests

update_app_pack_pipeline_order (mock calls or capture order).

PR-PACK-05: Extension catalog loader (OCI JSON) + type selection with explanations
Goal

Implement the catalog-driven extension pack wizard foundation.

UX

Prompt for catalog ref with default:

[oci://ghcr.io/greenticai/catalogs/extensions:latest]

Load catalog

Show extension types list with explanations, including:

Messaging / Events / OAuth / Secrets / State / Telemetry / Control / Observer / Capability offer / Custom (scaffold only)

Implementation

Add a small catalog client that:

fetches the JSON artifact from OCI (reuse existing distribution/OCI tooling if greentic-pack already has it; don’t invent another downloader)

parses into a typed struct

Strong preference: if there is already a catalog/descriptor type in greentic-types or greentic-interfaces, reuse it. Otherwise:

add a minimal typed struct in greentic-pack for now, and later lift it into types (but keep PR small).

Acceptance

Wizard loads catalog and displays types with descriptions.

“Custom extension” is present and selectable.

Tests

catalog_load_fixture_json: support fixture://extensions.json resolver for tests.

PR-PACK-06: Create extension pack from template (including Custom scaffold)
Goal

After choosing type, choose template and scaffold the extension pack.

UX

Template selection list (catalog-driven)

QA form (catalog-driven schema) for required fields

Apply scaffold plan

Run pipeline: update → resolve → doctor → build

Optional sign prompt

Implementation detail (important)

Catalog must include:

templates per type

a QA schema (or reference) for required answers

an apply plan (steps like: create dirs, write pack.yaml fragments, call add-extension, add stub flows/components)

For Custom extension:

provide at least one “Custom scaffold template” that creates:

minimal pack skeleton

placeholder manifest extension block (or documentation placeholder)

stub directories (components/, flows/, i18n/)

a README telling devs what to implement next

Acceptance

Can scaffold a custom extension pack and build it (even if it does “nothing” yet).

Can scaffold at least one real extension type from catalog (even if minimal).

Tests

create_extension_custom_scaffold: asserts directories/files exist.

apply_plan_is_deterministic: same inputs produce same outputs.

PR-PACK-07: Update extension pack (catalog-driven edits + pipeline + optional sign)
Goal

Implement update flow for extension packs.

UX

Ask pack dir [.]

Ask catalog ref defaulting to the ghcr URL

Menu:

Edit extension entries (catalog-driven forms)

Edit flows (go to flow wizard)

Run update & validate (update → resolve → doctor → build)

Sign (optional)

“All roads lead to update & validate” (either user selects it, or auto-run after edits if you want opinionated behavior).

Acceptance

Works for existing packs; doesn’t require re-scaffolding.

Tests

update_extension_pack_flow: mock catalog + verify pipeline runs.

PR-PACK-08: Signing integration (optional) + verify hooks
Goal

Standardize optional signing across create/update flows.

Scope

Add “Sign (optional)” step after successful build in create/update paths.

Reuse existing greentic-pack sign implementation.

Optionally offer “Verify signature” in update extension pack flow (if useful).

Acceptance

If user chooses sign, wizard asks key path and calls sign.

Tests

sign_invocation_smoke (with temp test key or fixture path).

PR-PACK-09: Guardrails — no raw print, no raw UI strings, deterministic menus
Goal

Prevent regression and keep the wizard clean.

Scope

Grep-based tests to ensure wizard modules do not use println!/eprintln! directly.

Optional: forbid raw string literals in wizard modules except i18n keys (lightweight check).

Ensure every wizard screen is QA-driven and resolved via i18n.

Acceptance

CI fails if someone bypasses the qa-lib/i18n pathway.

Notes on “go to flow wizard”

Implementation-wise, this is simplest and robust:

Spawn greentic-flow wizard <pack-dir> as a child process

Inherit stdio so it behaves naturally

Return to pack wizard afterwards

No need to mention “greentic-flow” in UI text; the menu item can just be the localized “Edit flows”.

What I need from you (only if you want the extension catalog to reuse existing types)

If you paste the existing type name(s) or file paths in greentic-types / greentic-interfaces that look like “catalog/descriptor/extension kinds”, I’ll map PR-PACK-05/06 to those exact structs so we don’t invent a parallel schema.

Otherwise, we proceed with a minimal typed struct in greentic-pack and later lift it.
