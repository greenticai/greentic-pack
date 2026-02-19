#![cfg(feature = "gui-fragment")]

#[test]
fn gui_fragment_types_are_available() {
    use greentic_interfaces::bindings::greentic_gui_1_0_0_gui_fragment::exports::greentic::gui::fragment_api as api;

    let ctx = api::FragmentContext {
        tenant_ctx: "tenant-json".to_string(),
        user_ctx: "user-json".to_string(),
        route: "/invoices".to_string(),
        session_id: "session-123".to_string(),
    };

    assert_eq!(ctx.route, "/invoices");
    assert_eq!(ctx.session_id, "session-123");
}
