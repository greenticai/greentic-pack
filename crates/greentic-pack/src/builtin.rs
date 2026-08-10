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
//! The runner also dispatches `state.get`, `state.set` and `telco-x.call`,
//! none of which are below. Packs using them cannot be built today. They are
//! deliberately left out rather than fixed blind — each needs the same hazard
//! check `mcp` got (see [`BUILTIN_EXACT_KINDS`]) before it is trusted to the
//! prefix rule.
//!
//! `var.set` is a further case: the designer emits it, and the engine has a
//! `NodeKind::VarSet`, but it is absent from the runner's own `NATIVE_OP_KEYS`
//! — so the runner must be fixed before this list can mirror it.
//!
//! Whoever closes that gap: read the runner, and check every addition against the
//! `mcp.exec` hazard documented on [`BUILTIN_EXACT_KINDS`] before trusting the
//! prefix rule.

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
const BUILTIN_EXACT_KINDS: &[&str] = &["mcp"];

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
