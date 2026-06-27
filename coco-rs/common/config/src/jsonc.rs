use serde::de::DeserializeOwned;
use serde_json::Map;
use serde_json::Number;
use serde_json::Value;
use std::collections::HashSet;

use crate::ConfigError;

fn parse_options() -> jsonc_parser::ParseOptions {
    jsonc_parser::ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: true,
    }
}

pub(crate) fn parse_value(contents: &str) -> crate::Result<Value> {
    jsonc_parser::parse_to_serde_value(contents, &parse_options())
        .map(|value| value.unwrap_or_else(|| Value::Object(Map::new())))
        .map_err(|source| ConfigError::Jsonc {
            message: source.to_string(),
        })
}

pub(crate) fn from_str<T>(contents: &str) -> crate::Result<T>
where
    T: DeserializeOwned,
{
    let value = parse_value(contents)?;
    serde_json::from_value(value).map_err(ConfigError::from)
}

pub(crate) fn set_dotted_value_preserving_format(
    contents: &str,
    key: &str,
    value: Value,
) -> crate::Result<String> {
    let root =
        jsonc_parser::cst::CstRootNode::parse(contents, &parse_options()).map_err(|source| {
            ConfigError::Jsonc {
                message: source.to_string(),
            }
        })?;
    let root_obj = match root.ensure_object_value() {
        Some(root_obj) => root_obj,
        None => {
            root.set_root_value(jsonc_parser::cst::CstInputValue::Object(Vec::new()));
            let Some(root_obj) = root.ensure_object_value() else {
                return Err(ConfigError::generic(
                    "failed to create root settings object",
                ));
            };
            root_obj
        }
    };
    set_dotted_cst_value(root_obj, key, value_to_cst_input(value));
    Ok(root.to_string())
}

pub(crate) fn update_value_preserving_format(
    contents: &str,
    value: Value,
) -> crate::Result<String> {
    let root =
        jsonc_parser::cst::CstRootNode::parse(contents, &parse_options()).map_err(|source| {
            ConfigError::Jsonc {
                message: source.to_string(),
            }
        })?;
    match root.root_value() {
        Some(node) => update_root_value(&root, node, value),
        None => root.set_root_value(value_to_cst_input(value)),
    }
    Ok(root.to_string())
}

fn update_root_value(
    root: &jsonc_parser::cst::CstRootNode,
    node: jsonc_parser::cst::CstNode,
    value: Value,
) {
    match (&value, node.as_object()) {
        (Value::Object(map), Some(obj)) => update_object(obj, map),
        _ => root.set_root_value(value_to_cst_input(value)),
    }
}

fn update_property_value(prop: &jsonc_parser::cst::CstObjectProp, value: Value) {
    match (&value, prop.value().and_then(|node| node.as_object())) {
        (Value::Object(map), Some(obj)) => update_object(obj, map),
        _ => prop.set_value(value_to_cst_input(value)),
    }
}

fn update_object(obj: jsonc_parser::cst::CstObject, values: &Map<String, Value>) {
    let mut retained = HashSet::new();
    for (key, value) in values {
        retained.insert(key.as_str());
        if let Some(prop) = obj.get(key) {
            update_property_value(&prop, value.clone());
        } else {
            obj.append(key, value_to_cst_input(value.clone()));
        }
    }

    for prop in obj.properties() {
        let Some(name) = prop.name().and_then(|name| name.decoded_value().ok()) else {
            continue;
        };
        if !retained.contains(name.as_str()) {
            prop.remove();
        }
    }
}

fn set_dotted_cst_value(
    root_obj: jsonc_parser::cst::CstObject,
    key: &str,
    value: jsonc_parser::cst::CstInputValue,
) {
    let mut parts = key.split('.').peekable();
    let mut current_obj = root_obj;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            if let Some(prop) = current_obj.get(part) {
                prop.set_value(value);
            } else {
                current_obj.append(part, value);
            }
            return;
        }

        current_obj = match current_obj.get(part) {
            Some(prop) => match prop.value().and_then(|node| node.as_object()) {
                Some(obj) => obj,
                None => {
                    prop.set_value(jsonc_parser::cst::CstInputValue::Object(Vec::new()));
                    let Some(obj) = prop.value().and_then(|node| node.as_object()) else {
                        return;
                    };
                    obj
                }
            },
            None => {
                current_obj.append(part, jsonc_parser::cst::CstInputValue::Object(Vec::new()));
                let Some(obj) = current_obj
                    .get(part)
                    .and_then(|prop| prop.value())
                    .and_then(|node| node.as_object())
                else {
                    return;
                };
                obj
            }
        };
    }
}

fn value_to_cst_input(value: Value) -> jsonc_parser::cst::CstInputValue {
    match value {
        Value::Null => jsonc_parser::cst::CstInputValue::Null,
        Value::Bool(value) => jsonc_parser::cst::CstInputValue::Bool(value),
        Value::Number(value) => jsonc_parser::cst::CstInputValue::Number(number_to_string(value)),
        Value::String(value) => {
            jsonc_parser::cst::CstInputValue::String(escape_string_value(value))
        }
        Value::Array(values) => jsonc_parser::cst::CstInputValue::Array(
            values.into_iter().map(value_to_cst_input).collect(),
        ),
        Value::Object(values) => jsonc_parser::cst::CstInputValue::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, value_to_cst_input(value)))
                .collect(),
        ),
    }
}

fn number_to_string(value: Number) -> String {
    value.to_string()
}

fn escape_string_value(value: String) -> String {
    value
        .chars()
        .flat_map(|ch| match ch {
            '"' => "\\u0022".chars().collect::<Vec<_>>(),
            '\\' => "\\u005c".chars().collect::<Vec<_>>(),
            ch if ch <= '\u{1f}' => format!("\\u{:04x}", ch as u32).chars().collect(),
            ch => vec![ch],
        })
        .collect()
}
