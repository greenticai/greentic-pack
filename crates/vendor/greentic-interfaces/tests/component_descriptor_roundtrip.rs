use serde_json::Value;

#[test]
fn descriptor_fixture_contains_ops_and_setup() -> serde_json::Result<()> {
    let text = include_str!("fixtures/component-descriptor-example.json");
    let descriptor: Value = serde_json::from_str(text)?;

    let ops = descriptor
        .get("ops")
        .and_then(Value::as_array)
        .expect("fixture must define an ops array");
    assert!(ops.len() >= 2, "expect at least two ops, got {}", ops.len());

    let setup = descriptor
        .get("setup")
        .expect("fixture must include a setup contract");
    assert!(
        setup.get("qa_spec").is_some(),
        "setup contract needs qa_spec"
    );
    assert!(
        setup
            .get("examples")
            .and_then(Value::as_array)
            .map(|examples| !examples.is_empty())
            .unwrap_or(false),
        "setup contract needs at least one example answers blob"
    );

    Ok(())
}
