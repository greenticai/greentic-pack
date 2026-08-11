//! Classification of flow nodes that the runner engine executes itself
//! ("builtins") rather than resolving to a pack component.
//!
//! This is the single source of truth shared by pack validation
//! (`validate::ComponentReferencesExistValidator`) and the packc resolve/build
//! pipeline, so they never drift. It MUST mirror the runner's `NodeKind`
//! dispatch (`greentic-runner-host` engine). A key the runner dispatches but
//! this list omits makes `resolve`/`build` demand a component that can never
//! exist, so the pack cannot be built at all.
//!
//! # Read the right runner list
//!
//! The runner keeps TWO lists and they are not the same. `NATIVE_OP_KEYS`
//! (`runner/flow_adapter.rs`) governs loading a RAW `.ygtc`; a BUILT pack
//! reaches the engine through the compiled manifest, where `HostNode::from`
//! applies its own `is_builtin` prefix allowlist (`runner/engine.rs`). Only the
//! second one describes what a pack this crate produces will actually do.
//! Reasoning from `NATIVE_OP_KEYS` alone yields a confidently wrong answer —
//! it lists keys the compiled path still cannot dispatch.
//!
//! What makes the difference is that `is_builtin` decides whether a dotted
//! `component.id` is kept intact or split into `<component>.<operation>`. A key
//! that is split never reaches its own match arm, and the resulting prefix
//! (`"state"`, `"approval"`) is by construction never a key in the pack's
//! component map. That is the bug `approval.` and `var.` were added to
//! `is_builtin` to fix.
//!
//! # `state.get` / `state.set` are deliberately NOT here
//!
//! The runner has a native arm for both (`NodeKind::BuiltinStateGet` /
//! `BuiltinStateSet`, run by `execute_state_get`/`execute_state_set`) and lists
//! both in `NATIVE_OP_KEYS`. They are still absent below, for two independent
//! reasons — either one alone is disqualifying.
//!
//! **`state.get` is a real pack component in this repo.** `examples/qa-demo`
//! resolves it from `repo://io.3bridges.components.state@1.0.0`: digest-pinned in
//! `pack.lock.json`, declared in `pack.yaml` as `required_capabilities:
//! [state:get]`, and materialized with a wasm artifact by
//! `packc/tests/run_examples_manifest.rs`. Listing it here would make
//! `collect_flow_component_ids` skip it and silently drop that component from the
//! built pack. This is the `mcp.exec` hazard landing on the op-key ITSELF rather
//! than on a `.suffix`, so no placement is safe — `state`, a `state.` prefix and
//! an exact `state.get` all swallow it. `state.set` shares that component family
//! and its `state:` capability namespace, so it stays out alongside `state.get`
//! rather than being split off on the accident that no `state.set` component
//! exists yet.
//!
//! **The runner cannot dispatch either from a built pack today.**
//! `BuiltinStateGet`/`BuiltinStateSet` are unit variants: they read key/value out
//! of the payload and take nothing from `component.operation`, so a packc-emitted
//! state node always carries `operation: None`. The engine's `is_builtin` has no
//! `state.` arm, so that id is dot-split into component `"state"` + operation
//! `"get"` and dispatch fails with `component 'state' not found in pack`. Adding
//! these keys here would trade a loud build failure for a loud dispatch failure
//! without making one flow work.
//!
//! Closing this needs a `state.` arm in the runner's `is_builtin` FIRST, and then
//! a ruling on a genuine cross-repo contradiction: greentic-designer's
//! `catalog.baseline.yaml` calls `state.get` native (`component_ref: null`) while
//! `examples/qa-demo` resolves it as a component. One of the two is wrong, and
//! whichever way it goes, qa-demo's `pack.yaml`, `pack.lock.json`, both resolve
//! sidecars and `run_examples_manifest.rs` move in the same change.
//!
//! `var.set` used to be listed as blocked on the runner. It was not, for the
//! reason in "Read the right runner list" above: the compiled path already
//! carries a `"var."` prefix in `is_builtin` and a `"var.set"` arm that writes
//! `state.vars`. It is in [`BUILTIN_EXACT_KINDS`] below.
//!
//! Whoever extends this list: read the runner's `is_builtin`, not just
//! `NATIVE_OP_KEYS`, and check every addition against the `mcp.exec` hazard
//! documented on [`BUILTIN_EXACT_KINDS`] before trusting the prefix rule.

use greentic_types::Node;

/// Builtin/dispatch flow-node kinds. Agentic and dispatch kinds appear in flows
/// as `dw.agent.<agent_id>`, `dw.agent_graph.<graph_id>`, etc., so callers must
/// match both the bare kind and the dotted `<kind>.<suffix>` form.
const BUILTIN_KINDS: &[&str] = &[
    "session.wait",
    "flow.call",
    "provider.invoke",
    "dw.agent",
    "dw.agent_graph",
    "sorla.call",
    "operala.call",
    "agentic.call",
    "approval.call",
];

/// Builtin kinds that match ONLY on equality — never as a `<kind>.<suffix>`
/// prefix.
///
/// `mcp` is here rather than in [`BUILTIN_KINDS`] because the prefix rule would
/// also swallow **`mcp.exec`**, a real component shipped with its own wasm and
/// manifest in `examples/weather-demo` and `examples/adaptive-mcp-oauth-demo`.
/// Under the prefix rule `mcp.exec` would be silently reclassified as
/// engine-dispatched, skip resolution, and vanish from the built pack — a
/// working pack quietly losing a component, which is worse than the build
/// failure this entry exists to fix.
///
/// The runner dispatches the bare `mcp` op-key natively (`NATIVE_OP_KEYS` in
/// `runner/flow_adapter.rs`, executed by `runner/mcp_node.rs`), and
/// `greentic-flow` lowers such a node to `component.id == "mcp"` with no
/// operation. There is therefore no pack component to resolve, and demanding a
/// resolve/summary entry for one made every flow containing an MCP node
/// unbuildable.
///
/// `var.set` is here for the same reason and with the same hazard: the engine
/// classifies it in `is_builtin` (a `"var."` prefix) and executes it through a
/// `"var.set"` arm that writes `state.vars`, so it has no pack component
/// either — while a prefix entry would capture any future `var.set.*`
/// component. greentic-designer's Set Variable palette node emits exactly this
/// op-key, and every flow using it was unbuildable.
///
/// `telco-x.call` is the runner's remote-dispatch node for the `"telco-x"`
/// runtime (`NodeKind::TelcoXCall` → `execute_telco_x_call` →
/// `execute_remote_dispatch`), the same family as `sorla.call` / `operala.call` /
/// `agentic.call` in [`BUILTIN_KINDS`]. Unlike the state kinds it takes its
/// `target` FROM `component.operation`, so its nodes carry an operation, which is
/// what keeps the engine from dot-splitting the id — its match arm is reachable
/// from a compiled manifest even though `is_builtin` has no `telco-x.` prefix.
/// Nothing in greentic-pack, greentic-designer or greentic-runner ships a
/// component id starting with `telco-x`; the designer's own
/// `catalog.baseline.yaml` marks the capability `dispatch: true` with no
/// `component_ref` at all. It is exact-match anyway, because greentic-flow's
/// `classify_node_type` reads a 2-segment key as a builtin but a 3+-segment one
/// as an adapter component — so a `telco-x.call.*` id is precisely the shape a
/// prefix entry would misclassify and drop.
const BUILTIN_EXACT_KINDS: &[&str] = &["mcp", "var.set", "telco-x.call"];

/// Whether a component-id string names a runner builtin (engine-handled, with
/// no pack component to resolve). Accepts both the bare kind (`dw.agent`) and
/// the dotted form (`dw.agent.support`); `emit.*` is always builtin.
///
/// [`BUILTIN_EXACT_KINDS`] entries are matched by equality only — see that
/// constant for why `mcp` must not participate in the dotted-prefix rule.
pub fn is_builtin_component_id(id: &str) -> bool {
    id.starts_with("emit.")
        || BUILTIN_EXACT_KINDS.contains(&id)
        || BUILTIN_KINDS.iter().any(|kind| {
            id == *kind
                || id
                    .strip_prefix(kind)
                    .is_some_and(|rest| rest.starts_with('.'))
        })
}

/// The kind-bearing id of a flow node. A `dw.agent.<id>` (and other dispatch)
/// node compiles to the generic `component.exec` placeholder with the real kind
/// carried in `operation`, so the operation is authoritative when the component
/// id is empty/`component.exec`; otherwise the component id is.
pub fn node_effective_id(node: &Node) -> &str {
    let id = node.component.id.as_str();
    if id.is_empty() || id == "component.exec" {
        node.component.operation.as_deref().unwrap_or(id)
    } else {
        id
    }
}

/// Whether a flow node is a runner builtin (so it needs no resolve mapping,
/// summary entry, or manifest component reference).
pub fn node_is_builtin(node: &Node) -> bool {
    is_builtin_component_id(node_effective_id(node))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_ids_cover_runner_node_kinds() {
        for id in [
            "session.wait",
            "flow.call",
            "provider.invoke",
            "emit.response",
        ] {
            assert!(is_builtin_component_id(id), "{id} should be builtin");
        }
        for id in [
            "dw.agent",
            "dw.agent.smoke-agent",
            "dw.agent_graph",
            "dw.agent_graph.triage",
            "sorla.call",
            "operala.call",
            "agentic.call",
            "approval.call",
        ] {
            assert!(is_builtin_component_id(id), "{id} should be builtin");
        }
        for id in [
            "qa.process",
            "templating.handlebars",
            "dw.agentish",
            "agentic",
        ] {
            assert!(!is_builtin_component_id(id), "{id} must NOT be builtin");
        }
    }

    /// The bare `mcp` op-key is engine-dispatched, so it needs no resolve or
    /// summary entry. Without this, every flow carrying an MCP node failed
    /// `build` with "missing resolve summary entries for nodes <id>".
    #[test]
    fn bare_mcp_is_builtin() {
        assert!(is_builtin_component_id("mcp"));
    }

    /// `mcp.exec` is a REAL component (`examples/weather-demo`,
    /// `examples/adaptive-mcp-oauth-demo` ship its wasm and manifest). Treating
    /// `mcp` as a dotted prefix would reclassify it as engine-dispatched, skip
    /// its resolution, and drop it from the built pack — silently. This is the
    /// hazard that puts `mcp` in `BUILTIN_EXACT_KINDS` instead of
    /// `BUILTIN_KINDS`; do not "simplify" the two lists into one.
    /// Set Variable is a first-class greentic-designer palette node; every flow
    /// using it failed `build` with "missing resolve summary entries" until this
    /// entry existed.
    #[test]
    fn bare_var_set_is_builtin() {
        assert!(is_builtin_component_id("var.set"));
    }

    /// Exact-match only, for the same reason as `mcp`: a prefix entry would
    /// capture a `var.set.*` component and silently drop it from the pack.
    #[test]
    fn var_set_is_exact_match_only() {
        for id in ["var.set.custom", "var.setter", "var"] {
            assert!(!is_builtin_component_id(id), "{id} must NOT be builtin");
        }
    }

    /// `telco-x.call` is engine-dispatched to the `"telco-x"` remote runtime, so
    /// it needs no resolve or summary entry. Without this, every flow carrying a
    /// telco-x node failed `build` with "missing resolve summary entries".
    #[test]
    fn bare_telco_x_call_is_builtin() {
        assert!(is_builtin_component_id("telco-x.call"));
    }

    /// Exact-match only, for the same reason as `mcp` and `var.set`.
    /// greentic-flow's `classify_node_type` reads a 3+-segment key as an adapter
    /// component, so `telco-x.call.*` is exactly the shape a prefix entry would
    /// reclassify as engine-dispatched and drop from the built pack. `telco-x`
    /// alone is not a builtin either — the dispatch verb is the whole key.
    #[test]
    fn telco_x_call_is_exact_match_only() {
        for id in ["telco-x.call.sms", "telco-x.caller", "telco-x", "telco"] {
            assert!(!is_builtin_component_id(id), "{id} must NOT be builtin");
        }
    }

    /// `state.get`/`state.set` must stay NON-builtin even though the runner has a
    /// native arm for each and lists both in `NATIVE_OP_KEYS`.
    ///
    /// `state.get` is a REAL component here: `examples/qa-demo` resolves it from
    /// `repo://io.3bridges.components.state@1.0.0`, digest-pinned in
    /// `pack.lock.json` and declared in `pack.yaml` under
    /// `required_capabilities: [state:get]`. Classifying it as a builtin makes
    /// `collect_flow_component_ids` skip it and drops that component from the
    /// built pack — silently, which is worse than the build failure someone will
    /// be tempted to fix by adding it here. Unlike `mcp`/`mcp.exec` the collision
    /// is on the op-key itself, so an exact entry is no safer than a prefix one.
    /// `state.set` shares the same component family and capability namespace.
    ///
    /// The engine also cannot dispatch either from a compiled manifest today:
    /// both `NodeKind`s are unit variants, so the node carries `operation: None`,
    /// and with no `state.` arm in the engine's `is_builtin` the id is dot-split
    /// into `"state"` — never a key in the pack's component map. See the module
    /// doc; this needs a runner change first, not a change here.
    #[test]
    fn state_get_and_set_are_components_not_builtins() {
        for id in ["state.get", "state.set", "state", "state.getter"] {
            assert!(
                !is_builtin_component_id(id),
                "{id} must NOT be builtin — `state.*` is a resolvable component \
                 family (examples/qa-demo), and classifying it here drops the \
                 component from the built pack"
            );
        }
    }

    #[test]
    fn mcp_is_exact_match_only_and_never_swallows_mcp_exec() {
        for id in ["mcp.exec", "mcp.anything", "mcp.exec.v2"] {
            assert!(
                !is_builtin_component_id(id),
                "{id} is a pack component, not a builtin"
            );
        }
    }
}
