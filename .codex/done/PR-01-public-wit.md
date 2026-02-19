0) Global rule for all repos (tell Codex this every time)

Use this paragraph at the top of every prompt:

Global policy: greentic:component@0.6.0 WIT must have a single source of truth in greentic-interfaces. No other repo should define or vendor package greentic:component@0.6.0 or world component-v0-v6-v0 in its own wit/ directory. Repos may keep tiny repo-specific worlds (e.g. messaging-provider-teams) but must depend on the canonical greentic component WIT via deps/ pointing at greentic-interfaces or via a published crate path, never by copying the WIT file contents.

C) greentic-pack repo prompt (remove vendored WIT; consume canonical)
You are working in the greentic-pack repository.

Goal
- Ensure greentic-pack does not vendor/copy canonical greentic component WIT.
- All references to `greentic:component@0.6.0` WIT must come from greentic-interfaces.
- Remove or stop using `crates/vendor/greentic-flow/wit/*` if it is only there to provide canonical WIT; replace with dependency on greentic-flow crate or greentic-interfaces WIT.
- Ensure tests/fixtures that need a v0.6 component use greentic-interfaces-guest wrapper macro (or canonical WIT deps), not copied WIT.

Work
1) Inventory:
- Find all `.wit` files in this repo declaring `package greentic:component@0.6.0;`.
- Find vendored WIT directories under `crates/vendor/`.
- Classify:
  a) canonical greentic component WIT (must be removed/replaced)
  b) pack-specific worlds (can remain)

2) Replace:
- For v0.6 component fixtures in tests (e.g. noop-component-v06-src):
  - Remove local WIT copies of greentic:component if present.
  - Depend on canonical WIT from greentic-interfaces (path dep in workspace or crate-provided wit dir).
  - If a Rust fixture component exists, switch to `greentic_interfaces_guest::export_component_v060!`.

3) Add a guard test:
- Add a test that fails if any committed `.wit` file in this repo contains `package greentic:component@0.6.0;` (excluding `target/` and excluding explicitly allowed fixture directories if you must keep one).
- This prevents future copying.

Deliverables
- No committed canonical greentic component WIT duplicates in greentic-pack
- Tests updated to use canonical WIT / guest wrapper macro
- Guard test in place

Now implement it.