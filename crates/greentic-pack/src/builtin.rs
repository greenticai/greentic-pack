//! Classification of flow nodes that the runner engine executes itself
//! ("builtins") rather than resolving to a pack component.
//!
//! This is the single source of truth shared by pack validation
//! (`validate::ComponentReferencesExistValidator`) and the packc resolve/build
//! pipeline, so they never drift. It MUST mirror the runner's `NodeKind`
//! dispatch (`greentic-runner-host` engine) — canonically its `NATIVE_OP_KEYS`
//! in `runner/flow_adapter.rs`. A key the runner dispatches but this list omits
//! makes `resolve`/`build` demand a component that can never exist, so the pack
//! cannot be built at all.
//!
//! # Known drift — this list is NOT yet a full mirror
//!
//! The runner also dispatches `state.get`, `state.set`, `telco-x.call`, and
//! `mcp`, none of which are below. Packs using them cannot be built today. They
//! are deliberately left out of this change rather than fixed blind:
//!
//! - `mcp` in particular is NOT a safe addition as written.
//!   [`is_builtin_component_id`] treats a listed kind as a prefix whenever a `.`
//!   follows, so a bare `"mcp"` entry would also swallow **`mcp.exec`** — a real
//!   component, shipped with its own wasm and manifest in `examples/weather-demo`
//!   and `examples/adaptive-mcp-oauth-demo`. It would be silently reclassified as
//!   engine-dispatched, skip resolution, and vanish from the built pack. Adding
//!   `mcp` therefore needs exact-match semantics, not a list entry.
//! - `var.set` is a third case: the designer emits it, and the engine has a
//!   `NodeKind::VarSet`, but it is absent from the runner's own `NATIVE_OP_KEYS`
//!   — so the runner must be fixed before this list can mirror it.
//!
//! Whoever closes that gap: read the runner, and check every addition against the
//! `mcp.exec` hazard above before trusting the prefix rule.

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

/// Whether a component-id string names a runner builtin (engine-handled, with
/// no pack component to resolve). Accepts both the bare kind (`dw.agent`) and
/// the dotted form (`dw.agent.support`); `emit.*` is always builtin.
pub fn is_builtin_component_id(id: &str) -> bool {
    id.starts_with("emit.")
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
}
