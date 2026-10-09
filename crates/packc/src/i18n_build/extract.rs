#![forbid(unsafe_code)]
//! Extract translatable strings from Adaptive Cards.
//!
//! This module provides the recursive extraction logic for translatable text fields
//! and generates i18n key-value pairs for translation bundles.
//!
//! Ported from `greentic-cards2pack/src/i18n_extract/extractor.rs`
//! so that `greentic-pack` has no cross-crate dependency on `greentic-cards2pack`.
//!
//! # Extractable Fields
//!
//! - `text` - TextBlock, RichTextBlock text content
//! - `title` - Action titles, card titles
//! - `placeholder` - Input placeholders
//! - `label` - Input labels
//! - `altText` - Image alt text
//! - `errorMessage` - Validation error messages
//! - `inlineAction.title` - Inline action titles
//!
//! # Generated Key Format
//!
//! Keys follow the pattern: `{card_id}.{json_path}.{field}`
//!
//! Examples:
//! - `incident.body_0.text`
//! - `incident.actions_0.title`
//! - `greeting.body_1_items_0.text`

use std::path::Path;

use serde_json::Value;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// An extracted translatable string.
#[derive(Debug, Clone)]
pub struct ExtractedString {
    /// Generated i18n key.
    pub key: String,
    /// Original text value.
    pub value: String,
    /// Source file path.
    pub source_file: std::path::PathBuf,
    /// JSON path to the field (e.g., "body[0].text").
    pub json_path: String,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Text fields that should be extracted for translation.
const TRANSLATABLE_FIELDS: &[&str] = &[
    "text",
    "title",
    "placeholder",
    "label",
    "altText",
    "errorMessage",
    // Display text on non-input elements only. An `Input.*` element's `value`
    // is its default SUBMISSION, so it is never extracted — see
    // `is_input_element`.
    "value",
    "fallbackText",
    "speak",
];

/// Fields that contain nested elements with translatable content.
/// Note: "facts" and "choices" are excluded here because they have
/// dedicated extraction logic below (FactSet, ChoiceSet).
const CONTAINER_FIELDS: &[&str] = &[
    "body",
    "actions",
    "items",
    "columns",
    "inlines",
    "card", // For Action.ShowCard
    "inlineAction",
];

// ---------------------------------------------------------------------------
// Recursive extractor (from extractor.rs)
// ---------------------------------------------------------------------------

/// Extract strings from a JSON value recursively.
pub fn extract_from_value(
    value: &Value,
    prefix: &str,
    path: &str,
    source_file: &Path,
    skip_i18n_patterns: bool,
) -> Vec<ExtractedString> {
    let mut strings = Vec::new();

    match value {
        Value::Object(obj) => {
            extract_translatable_fields(
                obj,
                prefix,
                path,
                source_file,
                skip_i18n_patterns,
                &mut strings,
            );
            extract_container_fields(
                obj,
                prefix,
                path,
                source_file,
                skip_i18n_patterns,
                &mut strings,
            );
            extract_factset(
                obj,
                prefix,
                path,
                source_file,
                skip_i18n_patterns,
                &mut strings,
            );
            extract_choiceset(
                obj,
                prefix,
                path,
                source_file,
                skip_i18n_patterns,
                &mut strings,
            );
        }
        Value::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                let item_path = format!("{}_{}", path, i);
                strings.extend(extract_from_value(
                    item,
                    prefix,
                    &item_path,
                    source_file,
                    skip_i18n_patterns,
                ));
            }
        }
        _ => {}
    }

    strings
}

fn extract_translatable_fields(
    obj: &serde_json::Map<String, Value>,
    prefix: &str,
    path: &str,
    source_file: &Path,
    skip_i18n_patterns: bool,
    strings: &mut Vec<ExtractedString>,
) {
    let is_input = is_input_element(obj);
    for field in TRANSLATABLE_FIELDS {
        if *field == "value" && is_input {
            continue;
        }
        if let Some(Value::String(text)) = obj.get(*field)
            && should_extract(text, skip_i18n_patterns)
        {
            strings.push(ExtractedString {
                key: build_key(prefix, path, field),
                value: text.clone(),
                source_file: source_file.to_path_buf(),
                json_path: build_json_path(path, field),
            });
        }
    }
}

/// Whether `obj` is an Adaptive Card input (`Input.Text`, `Input.ChoiceSet`,
/// `Input.Toggle`, ...).
///
/// An input's `value` is DATA: it is what the card submits when the user
/// leaves the control untouched. Consumers rewrite every extracted path into
/// an `{{i18n:KEY}}` marker, so extracting it lets a locale replace the
/// default submission — an `Input.ChoiceSet` defaulting to `"Retail"` renders
/// in Spanish as `"Comercio minorista"`, which matches none of its own
/// choices, and a downstream tool rejects the submission. The same holds for
/// a choice's `value` (never extracted, see `extract_choiceset`) and for
/// `Input.Toggle`'s `valueOn` / `valueOff` (not in `TRANSLATABLE_FIELDS`).
/// The input's display text — `label`, `placeholder`, `errorMessage`, a
/// toggle's `title`, each choice's `title` — is still translated.
fn is_input_element(obj: &serde_json::Map<String, Value>) -> bool {
    obj.get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.starts_with("Input."))
}

fn extract_container_fields(
    obj: &serde_json::Map<String, Value>,
    prefix: &str,
    path: &str,
    source_file: &Path,
    skip_i18n_patterns: bool,
    strings: &mut Vec<ExtractedString>,
) {
    for field in CONTAINER_FIELDS {
        if let Some(child) = obj.get(*field) {
            let child_path = if path.is_empty() {
                field.to_string()
            } else {
                format!("{}_{}", path, field)
            };
            strings.extend(extract_from_value(
                child,
                prefix,
                &child_path,
                source_file,
                skip_i18n_patterns,
            ));
        }
    }
}

fn extract_factset(
    obj: &serde_json::Map<String, Value>,
    prefix: &str,
    path: &str,
    source_file: &Path,
    skip_i18n_patterns: bool,
    strings: &mut Vec<ExtractedString>,
) {
    let Some(facts) = obj.get("facts").and_then(|v| v.as_array()) else {
        return;
    };
    for (i, fact) in facts.iter().enumerate() {
        let fact_path = format!("{}_facts_{}", path, i);
        if let Some(fact_obj) = fact.as_object() {
            for field in ["title", "value"] {
                if let Some(Value::String(text)) = fact_obj.get(field)
                    && should_extract(text, skip_i18n_patterns)
                {
                    strings.push(ExtractedString {
                        key: build_key(prefix, &fact_path, field),
                        value: text.clone(),
                        source_file: source_file.to_path_buf(),
                        json_path: build_json_path(&fact_path, field),
                    });
                }
            }
        }
    }
}

fn extract_choiceset(
    obj: &serde_json::Map<String, Value>,
    prefix: &str,
    path: &str,
    source_file: &Path,
    skip_i18n_patterns: bool,
    strings: &mut Vec<ExtractedString>,
) {
    let Some(choices) = obj.get("choices").and_then(|v| v.as_array()) else {
        return;
    };
    // Only a choice's `title` is display text; its `value` is what the input
    // submits and must never be extracted.
    for (i, choice) in choices.iter().enumerate() {
        let choice_path = format!("{}_choices_{}", path, i);
        if let Some(choice_obj) = choice.as_object()
            && let Some(Value::String(title)) = choice_obj.get("title")
            && should_extract(title, skip_i18n_patterns)
        {
            strings.push(ExtractedString {
                key: build_key(prefix, &choice_path, "title"),
                value: title.clone(),
                source_file: source_file.to_path_buf(),
                json_path: build_json_path(&choice_path, "title"),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Key / path helpers (private)
// ---------------------------------------------------------------------------

/// Check if a string should be extracted.
pub fn should_extract(text: &str, skip_i18n_patterns: bool) -> bool {
    let trimmed = text.trim();

    if trimmed.is_empty() {
        return false;
    }

    // Skip existing i18n patterns
    if skip_i18n_patterns && (trimmed.contains("$t(") || trimmed.contains("$tp(")) {
        return false;
    }

    // Skip pure template expressions (Handlebars)
    if trimmed.starts_with("{{") && trimmed.ends_with("}}") {
        return false;
    }

    // Skip variable references
    if trimmed.starts_with("${") && trimmed.ends_with('}') {
        return false;
    }

    true
}

/// Build an i18n key from prefix, path, and field.
pub fn build_key(prefix: &str, path: &str, field: &str) -> String {
    if path.is_empty() {
        format!("{}.{}", prefix, field)
    } else {
        format!("{}.{}.{}", prefix, path, field)
    }
}

/// Build a JSON path string for documentation.
pub fn build_json_path(path: &str, field: &str) -> String {
    if path.is_empty() {
        return field.to_string();
    }

    let parts: Vec<&str> = path.split('_').collect();
    let mut result = String::new();
    for (i, part) in parts.iter().enumerate() {
        if part.parse::<usize>().is_ok() {
            result.push_str(&format!("[{}]", part));
        } else {
            if i > 0 {
                result.push('.');
            }
            result.push_str(part);
        }
    }
    format!("{}.{}", result, field)
}

// ---------------------------------------------------------------------------
// Tests — from extractor.rs
// ---------------------------------------------------------------------------

#[cfg(test)]
mod input_value_tests;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_should_extract() {
        assert!(should_extract("Hello World", false));
        assert!(!should_extract("", false));
        assert!(!should_extract("   ", false));
        assert!(!should_extract("$t(key)", true));
        assert!(!should_extract("{{variable}}", false));
        assert!(!should_extract("${var}", false));
        assert!(should_extract("$t(key)", false));
    }

    #[test]
    fn test_build_key() {
        assert_eq!(build_key("card", "", "text"), "card.text");
        assert_eq!(build_key("card", "body_0", "text"), "card.body_0.text");
    }

    #[test]
    fn test_build_json_path() {
        assert_eq!(build_json_path("", "text"), "text");
        assert_eq!(build_json_path("body_0", "text"), "body[0].text");
        assert_eq!(
            build_json_path("body_1_items_0", "text"),
            "body[1].items[0].text"
        );
    }

    #[test]
    fn test_extract_from_simple_card() {
        let card = json!({
            "type": "AdaptiveCard",
            "body": [
                { "type": "TextBlock", "text": "Hello World" }
            ],
            "actions": [
                { "type": "Action.Submit", "title": "Submit" }
            ]
        });

        let strings = extract_from_value(&card, "test", "", Path::new("test.json"), true);

        assert_eq!(strings.len(), 2);
        assert!(
            strings
                .iter()
                .any(|s| s.key == "test.body_0.text" && s.value == "Hello World")
        );
        assert!(
            strings
                .iter()
                .any(|s| s.key == "test.actions_0.title" && s.value == "Submit")
        );
    }

    #[test]
    fn test_extract_skips_i18n_patterns() {
        let card = json!({
            "type": "AdaptiveCard",
            "body": [
                { "type": "TextBlock", "text": "$t(card.greeting)" },
                { "type": "TextBlock", "text": "Regular text" }
            ]
        });

        let strings = extract_from_value(&card, "test", "", Path::new("test.json"), true);
        assert_eq!(strings.len(), 1);
        assert_eq!(strings[0].value, "Regular text");
    }

    #[test]
    fn test_extract_input_fields() {
        let card = json!({
            "type": "AdaptiveCard",
            "body": [{
                "type": "Input.Text",
                "id": "name",
                "label": "Your Name",
                "placeholder": "Enter your name",
                "errorMessage": "Name is required"
            }]
        });

        let strings = extract_from_value(&card, "test", "", Path::new("test.json"), true);
        assert_eq!(strings.len(), 3);
        assert!(strings.iter().any(|s| s.key.ends_with(".label")));
        assert!(strings.iter().any(|s| s.key.ends_with(".placeholder")));
        assert!(strings.iter().any(|s| s.key.ends_with(".errorMessage")));
    }

    #[test]
    fn test_extract_factset() {
        let card = json!({
            "type": "AdaptiveCard",
            "body": [{
                "type": "FactSet",
                "facts": [
                    {"title": "Name", "value": "John Doe"},
                    {"title": "Email", "value": "john@example.com"}
                ]
            }]
        });

        let strings = extract_from_value(&card, "test", "", Path::new("test.json"), true);
        assert_eq!(strings.len(), 4);
    }

    #[test]
    fn test_extract_choice_set() {
        let card = json!({
            "type": "AdaptiveCard",
            "body": [{
                "type": "Input.ChoiceSet",
                "id": "choice",
                "label": "Select an option",
                "choices": [
                    {"title": "Option A", "value": "a"},
                    {"title": "Option B", "value": "b"}
                ]
            }]
        });

        let strings = extract_from_value(&card, "test", "", Path::new("test.json"), true);
        assert_eq!(strings.len(), 3); // label + 2 choice titles
    }

    #[test]
    fn test_extract_nested_column_items() {
        let card = json!({
            "type": "AdaptiveCard",
            "body": [{
                "type": "ColumnSet",
                "columns": [
                    { "type": "Column", "items": [{ "type": "TextBlock", "text": "Left column" }] },
                    { "type": "Column", "items": [{ "type": "TextBlock", "text": "Right column" }] }
                ]
            }]
        });

        let strings = extract_from_value(&card, "test", "", Path::new("test.json"), true);
        assert!(strings.iter().any(|s| s.value == "Left column"));
        assert!(strings.iter().any(|s| s.value == "Right column"));
    }

    #[test]
    fn test_extract_show_card_action() {
        let card = json!({
            "type": "AdaptiveCard",
            "actions": [{
                "type": "Action.ShowCard",
                "title": "Show Details",
                "card": {
                    "type": "AdaptiveCard",
                    "body": [{ "type": "TextBlock", "text": "Hidden detail" }]
                }
            }]
        });

        let strings = extract_from_value(&card, "test", "", Path::new("test.json"), true);
        assert!(strings.iter().any(|s| s.value == "Show Details"));
        assert!(strings.iter().any(|s| s.value == "Hidden detail"));
    }

    #[test]
    fn test_extract_skips_pure_handlebars() {
        let card = json!({
            "type": "AdaptiveCard",
            "body": [
                { "type": "TextBlock", "text": "{{variable}}" },
                { "type": "TextBlock", "text": "Hello {{name}}" }
            ]
        });

        let strings = extract_from_value(&card, "test", "", Path::new("test.json"), true);
        assert!(!strings.iter().any(|s| s.value == "{{variable}}"));
        assert!(strings.iter().any(|s| s.value == "Hello {{name}}"));
    }
}
