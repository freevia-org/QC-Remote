use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Classification {
    Read,
    LiveWrite,
    PersistentWrite,
    RiskyWrite,
}

#[derive(Clone, Copy, Debug)]
pub struct ActionSpec {
    pub name: &'static str,
    pub rpc: &'static str,
    pub classification: Classification,
    pub description: &'static str,
    pub input_schema: &'static str,
    pub gateway_arguments: &'static [(&'static str, &'static str)],
    pub gateway_true_arguments: &'static [&'static str],
}

include!("generated_actions.rs");

impl ActionSpec {
    /// The generated input schema, parsed once per process.
    ///
    /// Every tool call used to re-parse it two or three times (null
    /// restoration, validation, the tool listing), and every `tools/list`
    /// re-parsed all of them.
    ///
    /// Keyed by the schema text itself, which is `'static`, so the cache holds
    /// at most one entry per distinct schema string compiled into the binary.
    pub fn parsed_input_schema(&self) -> &'static Map<String, Value> {
        static PARSED: OnceLock<Mutex<HashMap<&'static str, &'static Map<String, Value>>>> =
            OnceLock::new();
        let mut parsed = PARSED
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        parsed.entry(self.input_schema).or_insert_with(|| {
            Box::leak(Box::new(
                serde_json::from_str(self.input_schema).expect("generated action input schema"),
            ))
        })
    }

    pub fn tool(&self) -> Tool {
        let mut schema = Value::Object(self.parsed_input_schema().clone());
        portable_nullable_schema(&mut schema);
        let schema = schema.as_object().expect("object input schema").clone();
        let (read_only, destructive) = match self.classification {
            Classification::Read => (true, false),
            Classification::LiveWrite => (false, false),
            Classification::PersistentWrite | Classification::RiskyWrite => (false, true),
        };
        Tool::new(self.name, self.description, Arc::<JsonObject>::new(schema)).with_annotations(
            ToolAnnotations::new()
                .read_only(read_only)
                .destructive(destructive)
                .idempotent(read_only)
                .open_world(false),
        )
    }
}

/// Some MCP clients discard type arrays. Preserve every constraint in an anyOf
/// union, with a separate null branch, while validation uses the canonical schema.
fn portable_nullable_schema(schema: &mut Value) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
        for child in properties.values_mut() {
            portable_nullable_schema(child);
        }
    }
    for key in ["items", "not"] {
        if let Some(child) = object.get_mut(key) {
            portable_nullable_schema(child);
        }
    }
    for key in ["allOf", "anyOf"] {
        if let Some(branches) = object.get_mut(key).and_then(Value::as_array_mut) {
            for branch in branches {
                portable_nullable_schema(branch);
            }
        }
    }
    if let Some(types) = object.get("type").and_then(Value::as_array).cloned() {
        let branches: Vec<_> = types
            .iter()
            .map(|kind| {
                let mut branch = object.clone();
                branch.insert("type".into(), kind.clone());
                if let Some(values) = branch.get_mut("enum").and_then(Value::as_array_mut) {
                    values.retain(|value| value.is_null() == (kind == "null"));
                }
                if kind == "null"
                    && branch.get("enum").is_none_or(|values| {
                        values
                            .as_array()
                            .is_some_and(|values| values.contains(&Value::Null))
                    })
                {
                    json!({"type":"null"})
                } else {
                    Value::Object(branch)
                }
            })
            .collect();
        *schema = json!({"anyOf": branches});
    }
}
