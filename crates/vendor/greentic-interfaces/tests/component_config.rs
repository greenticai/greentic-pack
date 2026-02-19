use std::path::PathBuf;

#[test]
fn component_config_shape_is_annotated() {
    let wit_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wit/greentic/component@0.5.0/package.wit");
    let contents = std::fs::read_to_string(&wit_path)
        .unwrap_or_else(|_| panic!("missing {}", wit_path.display()));

    assert!(
        contents.contains("record config"),
        "config record should be declared in {}",
        wit_path.display()
    );
    assert!(
        contents.contains("@config"),
        "config record must be tagged so tooling can discover it"
    );
    assert!(
        contents.contains("@default(10)"),
        "config record should carry an explicit default value"
    );
    assert!(
        contents.contains("@flow:hidden"),
        "config record should showcase flow-hidden metadata"
    );
    assert!(
        contents.contains("interface component-config-schema"),
        "optional schema export interface should be present"
    );
}
