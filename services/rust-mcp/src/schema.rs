//! Validator for the closed JSON Schema subset emitted by the action generator.
//! Unknown keywords and patterns fail closed; the catalog test checks all schemas.
use serde_json::Value;

const PATTERNS: &[&str] = &[
    r"^#[0-9a-fA-F]{6}$",
    r"^(?!\s)(?!.*\s$)(?!.*\\)(?!.*//)(?!.*(?:^|/)\.{1,2}(?:/|$)).+$",
    r"^(?!\s)(?!.*\s$)(?!\.{1,2}$)[^/\\\x00-\x1F\x7F]+$",
    r"^(?!\s)(?!.*\s$)[^\x00-\x1F\x7F]+$",
    r"^[^\x00-\x1F\x7F]*$",
];
const KEYWORDS: &[&str] = &[
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "allOf",
    "anyOf",
    "not",
    "const",
    "enum",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "pattern",
    "minItems",
    "maxItems",
    "uniqueItems",
    "description",
    "default",
    "title",
];

fn type_matches(value: &Value, kind: &Value) -> bool {
    match kind.as_str() {
        Some("null") => value.is_null(),
        Some("boolean") => value.is_boolean(),
        Some("integer") => value.is_i64() || value.is_u64(),
        Some("number") => value.is_number(),
        Some("string") => value.is_string(),
        Some("array") => value.is_array(),
        Some("object") => value.is_object(),
        _ => false,
    }
}
fn equal(left: &Value, right: &Value) -> bool {
    if left.is_number() && right.is_number() {
        left.as_f64() == right.as_f64()
    } else {
        left == right
    }
}
fn bounded(count: usize, schema: &Value, min: &str, max: &str) -> bool {
    schema
        .get(min)
        .is_none_or(|n| n.as_u64().is_some_and(|n| count as u64 >= n))
        && schema
            .get(max)
            .is_none_or(|n| n.as_u64().is_some_and(|n| count as u64 <= n))
}
// ECMAScript \s differs from Rust's Unicode White_Space (notably U+0085/U+FEFF).
fn js_space(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | ' ' | '\u{00a0}' | '\u{1680}' |
        '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
        '\u{205f}' | '\u{3000}' | '\u{feff}')
}
fn pattern_matches(text: &str, pattern: &str) -> bool {
    let visible = || !text.chars().any(|c| c <= '\u{001f}' || c == '\u{007f}');
    let trimmed = || !text.is_empty() && !text.starts_with(js_space) && !text.ends_with(js_space);
    match PATTERNS.iter().position(|p| *p == pattern) {
        Some(0) => {
            text.len() == 7
                && text.starts_with('#')
                && text.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
        }
        Some(1) => {
            trimmed()
                && !text.contains(['\\', '\r', '\n', '\u{2028}', '\u{2029}'])
                && !text.contains("//")
                && !text.split('/').any(|part| part == "." || part == "..")
        }
        Some(2) => {
            trimmed() && visible() && !text.contains(['/', '\\']) && text != "." && text != ".."
        }
        Some(3) => trimmed() && visible(),
        Some(4) => visible(),
        _ => false,
    }
}

/// Whether every keyword and pattern anywhere in `schema` is one this
/// validator enforces.
fn fully_supported(schema: &Value) -> bool {
    let Some(object) = schema.as_object() else {
        return false;
    };
    object.iter().all(|(key, child)| match key.as_str() {
        "pattern" => child
            .as_str()
            .is_some_and(|pattern| PATTERNS.contains(&pattern)),
        "properties" => child
            .as_object()
            .is_some_and(|properties| properties.values().all(fully_supported)),
        "items" | "not" => fully_supported(child),
        "allOf" | "anyOf" => child
            .as_array()
            .is_some_and(|branches| branches.iter().all(fully_supported)),
        key => KEYWORDS.contains(&key),
    })
}

pub(crate) fn matches(value: &Value, schema: &Value) -> bool {
    let Some(object) = schema.as_object() else {
        return false;
    };
    if object.keys().any(|key| !KEYWORDS.contains(&key.as_str())) {
        return false;
    }
    if let Some(kind) = object.get("type") {
        let valid = if let Some(types) = kind.as_array() {
            types.iter().any(|kind| type_matches(value, kind))
        } else {
            type_matches(value, kind)
        };
        if !valid {
            return false;
        }
    }
    if object
        .get("const")
        .is_some_and(|expected| !equal(value, expected))
    {
        return false;
    }
    if object.get("enum").is_some_and(|values| {
        !values
            .as_array()
            .is_some_and(|values| values.iter().any(|expected| equal(value, expected)))
    }) {
        return false;
    }
    for (key, all) in [("allOf", true), ("anyOf", false)] {
        if let Some(branches) = object.get(key) {
            let Some(branches) = branches.as_array() else {
                return false;
            };
            let valid = if all {
                branches.iter().all(|branch| matches(value, branch))
            } else {
                branches.iter().any(|branch| matches(value, branch))
            };
            if !valid {
                return false;
            }
        }
    }
    // A `not` inverts its branch, so an unenforceable keyword inside it would
    // turn "cannot check" into "passes". Refuse that before inverting.
    if object
        .get("not")
        .is_some_and(|branch| !fully_supported(branch) || matches(value, branch))
    {
        return false;
    }
    if let Some(number) = value.as_f64()
        && (object
            .get("minimum")
            .is_some_and(|min| !min.as_f64().is_some_and(|min| number >= min))
            || object
                .get("maximum")
                .is_some_and(|max| !max.as_f64().is_some_and(|max| number <= max)))
    {
        return false;
    }
    if let Some(text) = value.as_str()
        && (!bounded(text.chars().count(), schema, "minLength", "maxLength")
            || object
                .get("pattern")
                .is_some_and(|pattern| !pattern.as_str().is_some_and(|p| pattern_matches(text, p))))
    {
        return false;
    }
    if let Some(items) = value.as_array() {
        if !bounded(items.len(), schema, "minItems", "maxItems") {
            return false;
        }
        if object
            .get("items")
            .is_some_and(|child| !items.iter().all(|item| matches(item, child)))
        {
            return false;
        }
        if object.get("uniqueItems") == Some(&Value::Bool(true))
            && items
                .iter()
                .enumerate()
                .any(|(index, item)| items[..index].iter().any(|previous| equal(item, previous)))
        {
            return false;
        }
    }
    if let Some(values) = value.as_object() {
        if object.get("required").is_some_and(|names| {
            !names.as_array().is_some_and(|names| {
                names
                    .iter()
                    .all(|name| name.as_str().is_some_and(|name| values.contains_key(name)))
            })
        }) {
            return false;
        }
        let properties = object.get("properties").and_then(Value::as_object);
        for (name, value) in values {
            if let Some(child) = properties.and_then(|properties| properties.get(name)) {
                if !matches(value, child) {
                    return false;
                }
            } else if object.get("additionalProperties") == Some(&Value::Bool(false)) {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assert_supported(schema: &Value) {
        let object = schema.as_object().expect("schema object");
        // Every generated node must declare at least one constraint.
        assert!(!object.is_empty());
        for key in object.keys() {
            assert!(KEYWORDS.contains(&key.as_str()), "unsupported {key}");
        }
        if let Some(pattern) = object.get("pattern") {
            assert!(
                PATTERNS.contains(&pattern.as_str().unwrap()),
                "unsupported {pattern}"
            );
        }
        if let Some(properties) = object.get("properties") {
            for child in properties.as_object().unwrap().values() {
                assert_supported(child);
            }
        }
        for key in ["items", "not"] {
            if let Some(child) = object.get(key) {
                assert_supported(child);
            }
        }
        for key in ["allOf", "anyOf"] {
            if let Some(branches) = object.get(key) {
                for child in branches.as_array().unwrap() {
                    assert_supported(child);
                }
            }
        }
    }
    #[test]
    fn entire_generated_catalog_uses_supported_constraints() {
        for action in crate::ACTIONS {
            assert_supported(&Value::Object(action.parsed_input_schema().clone()));
        }
    }
    #[test]
    fn nested_and_dependent_constraints_are_enforced() {
        let schema = json!({"type":"object", "required":["items"], "additionalProperties":false,
            "properties":{"items":{"type":["array","null"],"minItems":1,"maxItems":2,"uniqueItems":true,"items":{"type":"integer","minimum":1,"maximum":3}}},
            "allOf":[{"properties":{"items":{"maxItems":1}}}]});
        for good in [json!({"items":[1]}), json!({"items":null})] {
            assert!(matches(&good, &schema));
        }
        for bad in [
            json!({}),
            json!({"items":[]}),
            json!({"items":[0]}),
            json!({"items":[1,2]}),
            json!({"items":[1,1]}),
            json!({"items":[1],"extra":true}),
        ] {
            assert!(!matches(&bad, &schema), "accepted {bad}");
        }
        assert!(!matches(
            &json!("value"),
            &json!({"unknownConstraint":true})
        ));
    }
    /// The same file is run by the cloud Worker's validator
    /// (tests/remote-action-schema.test.ts in the maintainers' suite), so the
    /// two surfaces reach one verdict on every case.
    #[test]
    fn shared_validator_cases_reach_their_stated_verdict() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/schema-validator-cases.json"
        ))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert!(cases.len() >= 40, "only {} shared cases", cases.len());
        for case in cases {
            assert_eq!(
                matches(&case["value"], &case["schema"]),
                case["valid"].as_bool().unwrap(),
                "{}",
                case["name"]
            );
        }
    }
    #[test]
    fn patterns_preserve_ascii_control_and_javascript_whitespace_semantics() {
        assert!(pattern_matches("", PATTERNS[4]));
        assert!(pattern_matches("\u{0085}", PATTERNS[3]));
        assert!(!pattern_matches("\u{feff}name", PATTERNS[3]));
        assert!(!pattern_matches("name\u{001f}", PATTERNS[4]));
        assert!(!pattern_matches("a\u{2028}b", PATTERNS[1]));
        assert!(pattern_matches("/media/My Presets/", PATTERNS[1]));
    }
}
