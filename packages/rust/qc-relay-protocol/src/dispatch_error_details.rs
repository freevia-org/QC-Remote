//! Bounded optional dispatch facts. This never derives delivery from prose.
use serde_json::{Map, Value};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Copy only typed, non-identifying facts into an existing error projection.
/// Unknown fields and free text are deliberately omitted, not truncated.
pub fn project_dispatch_error_details(
    code: Option<&str>,
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
) {
    for key in ["requestSent", "writesQuarantined"] {
        if let Some(value) = source.get(key).and_then(Value::as_bool) {
            target.insert(key.into(), Value::Bool(value));
        }
    }
    // A named predispatch refusal cannot support the opposite facts. Drop
    // contradictions rather than inventing replacement values.
    if code == Some("WRITE_QUARANTINED") {
        if target.get("requestSent").and_then(Value::as_bool) == Some(true) {
            target.remove("requestSent");
        }
        if target.get("writesQuarantined").and_then(Value::as_bool) == Some(false) {
            target.remove("writesQuarantined");
        }
    }
    let Some(quarantine) = source.get("quarantine").and_then(Value::as_object) else {
        return;
    };
    let Some(scope @ ("presetGrid" | "device")) = quarantine.get("scope").and_then(Value::as_str)
    else {
        return;
    };
    let mut safe = Map::new();
    safe.insert("scope".into(), Value::String(scope.into()));
    for key in ["sinceUnixMs", "sessionEpoch"] {
        let Some(value) = quarantine
            .get(key)
            .and_then(Value::as_u64)
            .filter(|value| *value <= MAX_SAFE_INTEGER)
        else {
            return;
        };
        safe.insert(key.into(), Value::from(value));
    }
    target.insert("quarantine".into(), Value::Object(safe));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project(value: Value) -> Value {
        let mut result = Map::new();
        project_dispatch_error_details(
            value.get("code").and_then(Value::as_str),
            value.as_object().unwrap(),
            &mut result,
        );
        Value::Object(result)
    }

    #[test]
    fn optional_facts_keep_false_and_drop_free_text() {
        for scope in ["presetGrid", "device"] {
            assert_eq!(
                project(json!({"requestSent":false,"writesQuarantined":true,
                "quarantine":{"scope":scope,"sinceUnixMs":1,"sessionEpoch":0,
                    "reason":"secret","method":"secret","clearsBy":"secret","credential":"secret"},
                "payload":"secret","userActionRequired":true})),
                json!({"requestSent":false,"writesQuarantined":true,
                    "quarantine":{"scope":scope,"sinceUnixMs":1,"sessionEpoch":0}})
            );
        }
        assert_eq!(project(json!({})), json!({}));
    }

    #[test]
    fn malformed_values_are_not_coerced_or_echoed() {
        for invalid in [json!(null), json!("false"), json!(0), json!({"secret":"x"})] {
            assert_eq!(
                project(json!({"requestSent":invalid,"writesQuarantined":invalid})),
                json!({})
            );
        }
        for invalid in [
            json!(-1),
            json!(1.5),
            json!("1"),
            json!(9_007_199_254_740_992u64),
            json!(null),
        ] {
            assert_eq!(
                project(
                    json!({"quarantine":{"scope":"secret","sinceUnixMs":invalid,"sessionEpoch":invalid}})
                ),
                json!({})
            );
        }
    }

    #[test]
    fn quarantine_requires_every_valid_field_but_drops_extra_keys() {
        let valid = json!({"scope":"device","sinceUnixMs":MAX_SAFE_INTEGER,"sessionEpoch":0});
        for key in ["scope", "sinceUnixMs", "sessionEpoch"] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert_eq!(
                project(json!({"requestSent":false,"quarantine":missing})),
                json!({"requestSent":false})
            );
            let mut malformed = valid.clone();
            malformed[key] = Value::Null;
            assert_eq!(project(json!({"quarantine":malformed})), json!({}));
        }
        let mut extra = valid.clone();
        extra["secret"] = json!("must not escape");
        assert_eq!(
            project(json!({"quarantine":extra})),
            json!({"quarantine":valid})
        );
    }

    #[test]
    fn only_exact_quarantine_drops_contradictory_facts_without_flipping() {
        assert_eq!(
            project(
                json!({"code":"WRITE_QUARANTINED","requestSent":true,"writesQuarantined":false})
            ),
            json!({})
        );
        assert_eq!(
            project(
                json!({"code":"WRITE_QUARANTINED","requestSent":false,"writesQuarantined":true})
            ),
            json!({"requestSent":false,"writesQuarantined":true})
        );
        for code in [
            "WRITE_OUTCOME_AMBIGUOUS",
            "DEVICE_OUTCOME_UNKNOWN",
            "DEVICE_ERROR",
            "OTHER",
        ] {
            assert_eq!(
                project(json!({"code":code,"requestSent":true,"writesQuarantined":false})),
                json!({"requestSent":true,"writesQuarantined":false})
            );
        }
    }
}
