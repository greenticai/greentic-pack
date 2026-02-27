use std::collections::BTreeSet;

use regex::Regex;

#[test]
fn wizard_module_does_not_use_direct_println_calls() {
    let source = include_str!("../src/cli/wizard.rs");
    assert!(
        !source.contains("println!("),
        "wizard module must not call println! directly"
    );
    assert!(
        !source.contains("eprintln!("),
        "wizard module must not call eprintln! directly"
    );
    assert!(
        !source.contains("write!("),
        "wizard module must route output writes through wizard_ui"
    );
    assert!(
        !source.contains("writeln!("),
        "wizard module must route output lines through wizard_ui"
    );
    assert!(
        !source.contains("\"Back\""),
        "wizard module must use i18n keys for Back labels"
    );
    assert!(
        !source.contains("\"Main Menu\""),
        "wizard module must use i18n keys for Main Menu labels"
    );
}

#[test]
fn wizard_i18n_keys_exist_in_en_gb_bundle() {
    let source = include_str!("../src/cli/wizard.rs");
    let bundle_raw = include_str!("../i18n/pack_wizard/en-GB.json");
    let bundle: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(bundle_raw).expect("parse wizard en-GB bundle");

    let re = Regex::new("\"(wizard\\.[A-Za-z0-9_.-]+)\"").expect("compile regex");
    let mut keys = BTreeSet::new();
    for capture in re.captures_iter(source) {
        let key = capture.get(1).expect("capture group").as_str().to_string();
        keys.insert(key);
    }

    let mut missing = Vec::new();
    for key in keys {
        if !bundle.contains_key(&key) {
            missing.push(key);
        }
    }

    assert!(
        missing.is_empty(),
        "wizard keys missing in en-GB bundle: {}",
        missing.join(", ")
    );
}

#[test]
fn wizard_catalog_fixture_uses_i18n_keys_for_labels() {
    let fixture_raw = include_str!("./fixtures/wizard/extensions.json");
    let fixture: serde_json::Value =
        serde_json::from_str(fixture_raw).expect("parse wizard catalog fixture");
    let types = fixture
        .get("extension_types")
        .and_then(serde_json::Value::as_array)
        .expect("extension_types array");

    for item in types {
        assert!(
            item.get("name_key")
                .and_then(serde_json::Value::as_str)
                .is_some(),
            "extension type missing name_key: {}",
            item.get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?")
        );
        assert!(
            item.get("description_key")
                .and_then(serde_json::Value::as_str)
                .is_some(),
            "extension type missing description_key: {}",
            item.get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?")
        );
    }
}
