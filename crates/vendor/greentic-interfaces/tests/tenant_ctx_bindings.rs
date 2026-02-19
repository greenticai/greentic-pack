#[cfg(feature = "bindings-rust")]
mod tenant_ctx_fields {
    use greentic_interfaces as interfaces;

    #[test]
    fn tenantctx_has_i18n_id_in_bindings() {
        #[allow(dead_code)]
        fn assert_field(ctx: interfaces::bindings::greentic::interfaces_types::types::TenantCtx) {
            let _ = ctx.i18n_id;
        }

        let _ = assert_field;
    }
}
