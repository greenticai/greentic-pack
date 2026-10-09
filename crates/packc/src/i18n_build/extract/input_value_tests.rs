// An Adaptive Card input's `value` is data, not display text.
//
// It is what the card submits when the user does not touch the control, so a
// translated default is an invalid submission: an `Input.ChoiceSet` whose
// default `"Retail"` became `"Comercio minorista"` matches none of its own
// choices, and the downstream tool refuses it. Consumers rewrite every
// extracted path into an `{{i18n:KEY}}` marker, so anything extracted here is
// something a locale can replace — these tests pin what must never be.

use std::path::Path;

use super::{ExtractedString, extract_from_value};
use serde_json::{Value, json};

fn extract(card: &Value) -> Vec<ExtractedString> {
    extract_from_value(card, "card.test", "", Path::new("test.json"), true)
}

fn paths(strings: &[ExtractedString]) -> Vec<&str> {
    let mut out: Vec<&str> = strings.iter().map(|s| s.json_path.as_str()).collect();
    out.sort_unstable();
    out
}

fn assert_no_value_extracted(strings: &[ExtractedString]) {
    let leaked: Vec<&str> = strings
        .iter()
        .filter(|s| {
            s.json_path.ends_with(".value")
                || s.json_path.ends_with(".valueOn")
                || s.json_path.ends_with(".valueOff")
        })
        .map(|s| s.json_path.as_str())
        .collect();
    assert!(leaked.is_empty(), "input values were extracted: {leaked:?}");
}

#[test]
fn choice_set_default_value_and_choice_values_are_never_extracted() {
    let card = json!({
        "type": "AdaptiveCard",
        "body": [{
            "type": "Input.ChoiceSet",
            "id": "business_activity",
            "label": "Business activity",
            "value": "Retail",
            "choices": [
                {"title": "Retail", "value": "Retail"},
                {"title": "Hospitality", "value": "Hospitality"}
            ]
        }]
    });

    let strings = extract(&card);

    assert_no_value_extracted(&strings);
    // Display text on the same element is still translated.
    assert_eq!(
        paths(&strings),
        vec![
            "body[0].choices[0].title",
            "body[0].choices[1].title",
            "body[0].label",
        ]
    );
}

#[test]
fn text_input_default_value_is_never_extracted() {
    let card = json!({
        "type": "AdaptiveCard",
        "body": [{
            "type": "Input.Text",
            "id": "city",
            "label": "City",
            "placeholder": "Where is the business?",
            "value": "London"
        }]
    });

    let strings = extract(&card);

    assert_no_value_extracted(&strings);
    assert_eq!(
        paths(&strings),
        vec!["body[0].label", "body[0].placeholder"]
    );
}

#[test]
fn every_input_type_keeps_its_value() {
    let card = json!({
        "type": "AdaptiveCard",
        "body": [
            {"type": "Input.Text", "id": "a", "value": "London"},
            {"type": "Input.Number", "id": "b", "value": "42"},
            {"type": "Input.Date", "id": "c", "value": "2026-09-20"},
            {"type": "Input.Time", "id": "d", "value": "09:30"},
            {"type": "Input.Toggle", "id": "e", "title": "Subscribe",
             "value": "true", "valueOn": "true", "valueOff": "false"},
            {"type": "Input.ChoiceSet", "id": "f", "value": "Retail",
             "choices": [{"title": "Retail", "value": "Retail"}]},
            {"type": "Input.Rating", "id": "g", "value": "Good"}
        ]
    });

    let strings = extract(&card);

    assert_no_value_extracted(&strings);
    // The toggle's title is its visible label and stays translatable.
    assert_eq!(
        paths(&strings),
        vec!["body[4].title", "body[5].choices[0].title"]
    );
}

#[test]
fn inputs_nested_in_containers_and_show_cards_keep_their_value() {
    let card = json!({
        "type": "AdaptiveCard",
        "body": [{
            "type": "Container",
            "items": [{
                "type": "ColumnSet",
                "columns": [{
                    "type": "Column",
                    "items": [{"type": "Input.Text", "id": "city", "value": "London"}]
                }]
            }]
        }],
        "actions": [{
            "type": "Action.ShowCard",
            "title": "More",
            "card": {
                "type": "AdaptiveCard",
                "body": [{"type": "Input.Text", "id": "town", "value": "Paris"}]
            }
        }]
    });

    let strings = extract(&card);

    assert_no_value_extracted(&strings);
    assert_eq!(paths(&strings), vec!["actions[0].title"]);
}

#[test]
fn fact_values_are_display_text_and_stay_translatable() {
    let card = json!({
        "type": "AdaptiveCard",
        "body": [{
            "type": "FactSet",
            "facts": [{"title": "Status", "value": "Approved"}]
        }]
    });

    let strings = extract(&card);

    assert_eq!(
        paths(&strings),
        vec!["body[0].facts[0].title", "body[0].facts[0].value"]
    );
}

#[test]
fn a_non_input_element_value_is_still_extracted() {
    // Unchanged behaviour for anything that is not an input: the field list
    // has always carried `value`, and only inputs are known to be data.
    let card = json!({
        "type": "AdaptiveCard",
        "body": [{"type": "TextBlock", "text": "Hello", "value": "Shown"}]
    });

    let strings = extract(&card);

    assert_eq!(paths(&strings), vec!["body[0].text", "body[0].value"]);
}
