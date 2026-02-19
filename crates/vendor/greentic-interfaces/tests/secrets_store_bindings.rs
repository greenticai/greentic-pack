use greentic_interfaces::bindings::greentic_secrets_store_1_0_0_store::greentic::secrets_store::secrets_store::SecretsError;

#[test]
fn secrets_store_bindings_expose_error_variants() {
    let _ = SecretsError::NotFound;
    let _ = SecretsError::Denied;
    let _ = SecretsError::InvalidKey;
    let _ = SecretsError::Internal;
}
