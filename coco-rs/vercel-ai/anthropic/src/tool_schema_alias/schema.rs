use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use regex::Regex;
use serde_json::{Map, Value};
use sha2::Digest as _;

use super::{AliasNode, DependentSchema, PatternProperty, PropertyAlias, SchemaProjection};

const MAX_PROPERTY_NAME_CHARS: usize = 64;

/// Project every declared property name through one schema-wide namespace.
///
/// A global namespace is deliberately stricter than a per-branch namespace:
/// the same original key always receives the same wire key across `allOf`,
/// `anyOf`, `oneOf`, conditionals, and `$ref` targets. Valid names are reserved
/// before aliases are allocated, so an alias can never shadow a valid name in
/// another applicable branch.
pub(crate) fn project_schema(schema: &Value) -> (Value, SchemaProjection) {
    let mut valid_names = HashSet::new();
    let mut invalid_names = HashSet::new();
    collect_property_names(schema, &mut valid_names, &mut invalid_names);

    let mut invalid_names: Vec<String> = invalid_names.into_iter().collect();
    invalid_names.sort();
    let mut reserved = valid_names;
    let aliases: HashMap<String, String> = invalid_names
        .into_iter()
        .map(|original| {
            let wire = allocate_wire_name(&original, &mut reserved);
            (original, wire)
        })
        .collect();

    let mut projected = schema.clone();
    let mut nodes_by_pointer = HashMap::new();
    let root = project_node(&mut projected, "#", &aliases, &mut nodes_by_pointer);
    (
        projected,
        SchemaProjection {
            root,
            nodes_by_pointer,
        },
    )
}

fn collect_property_names(
    schema: &Value,
    valid_names: &mut HashSet<String>,
    invalid_names: &mut HashSet<String>,
) {
    let Some(object) = schema.as_object() else {
        return;
    };
    for name in local_property_names(object) {
        if is_valid_property_name(&name) {
            valid_names.insert(name);
        } else {
            invalid_names.insert(name);
        }
    }

    for keyword in ["$defs", "definitions", "properties", "patternProperties"] {
        if let Some(children) = object.get(keyword).and_then(Value::as_object) {
            for child in children.values() {
                collect_property_names(child, valid_names, invalid_names);
            }
        }
    }
    for keyword in ["dependentSchemas", "dependencies"] {
        if let Some(children) = object.get(keyword).and_then(Value::as_object) {
            for child in children.values().filter(|child| !child.is_array()) {
                collect_property_names(child, valid_names, invalid_names);
            }
        }
    }
    for keyword in [
        "additionalProperties",
        "unevaluatedProperties",
        "additionalItems",
        "unevaluatedItems",
        "contains",
        "if",
        "then",
        "else",
        "not",
    ] {
        if let Some(child) = object.get(keyword) {
            collect_property_names(child, valid_names, invalid_names);
        }
    }
    if let Some(items) = object.get("items").filter(|items| items.is_object()) {
        collect_property_names(items, valid_names, invalid_names);
    }
    for keyword in ["items", "prefixItems", "allOf", "anyOf", "oneOf"] {
        if let Some(children) = object.get(keyword).and_then(Value::as_array) {
            for child in children {
                collect_property_names(child, valid_names, invalid_names);
            }
        }
    }
}

fn local_property_names(object: &Map<String, Value>) -> HashSet<String> {
    let mut names = HashSet::new();
    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        names.extend(properties.keys().cloned());
    }
    if let Some(required) = object.get("required").and_then(Value::as_array) {
        names.extend(
            required
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string),
        );
    }
    if let Some(dependencies) = object.get("dependentRequired").and_then(Value::as_object) {
        names.extend(dependencies.keys().cloned());
        names.extend(
            dependencies
                .values()
                .filter_map(Value::as_array)
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string),
        );
    }
    for keyword in ["dependentSchemas", "dependencies"] {
        if let Some(dependencies) = object.get(keyword).and_then(Value::as_object) {
            names.extend(dependencies.keys().cloned());
            names.extend(
                dependencies
                    .values()
                    .filter_map(Value::as_array)
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string),
            );
        }
    }
    names
}

fn project_node(
    schema: &mut Value,
    pointer: &str,
    aliases: &HashMap<String, String>,
    nodes_by_pointer: &mut HashMap<String, Arc<AliasNode>>,
) -> Arc<AliasNode> {
    let Some(object) = schema.as_object_mut() else {
        let node = Arc::new(AliasNode {
            pointer: pointer.to_string(),
            ..AliasNode::default()
        });
        nodes_by_pointer.insert(pointer.to_string(), node.clone());
        return node;
    };
    let mut local_property_names = local_property_names(object);

    let reference = object
        .get("$ref")
        .and_then(Value::as_str)
        .filter(|reference| reference.starts_with('#'))
        .map(str::to_string);
    if let Some(Value::String(reference)) = object.get_mut("$ref") {
        *reference = project_reference(reference, aliases);
    }
    let mut node = AliasNode {
        pointer: pointer.to_string(),
        reference,
        ..AliasNode::default()
    };

    for definitions_key in ["$defs", "definitions"] {
        if let Some(definitions) = object
            .get_mut(definitions_key)
            .and_then(Value::as_object_mut)
        {
            for (name, definition) in definitions {
                let child_pointer = child_pointer(pointer, definitions_key, name);
                project_node(definition, &child_pointer, aliases, nodes_by_pointer);
            }
        }
    }

    if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
        let original_properties = std::mem::take(properties);
        let mut entries: Vec<(String, Value)> = original_properties.into_iter().collect();
        entries.sort_by(|(left, _), (right, _)| left.cmp(right));
        let mut projected_properties = Map::new();

        for (original_name, mut property_schema) in entries {
            let wire_name = aliases
                .get(&original_name)
                .cloned()
                .unwrap_or_else(|| original_name.clone());
            let child = project_node(
                &mut property_schema,
                &child_pointer(pointer, "properties", &original_name),
                aliases,
                nodes_by_pointer,
            );
            projected_properties.insert(wire_name.clone(), property_schema);
            node.properties.push(PropertyAlias {
                original_name,
                wire_name,
                child,
            });
        }
        *properties = projected_properties;
    }
    rename_string_array(object.get_mut("required"), aliases);
    rename_dependency_arrays(object.get_mut("dependentRequired"), aliases);

    if let Some(patterns) = object
        .get_mut("patternProperties")
        .and_then(Value::as_object_mut)
    {
        let original_patterns = std::mem::take(patterns);
        let mut entries: Vec<(String, Value)> = original_patterns.into_iter().collect();
        entries.sort_by(|(left, _), (right, _)| left.cmp(right));
        let mut used_patterns: HashSet<String> =
            entries.iter().map(|(pattern, _)| pattern.clone()).collect();
        let mut supplemental = Vec::new();

        for (pattern, mut child_schema) in entries {
            let child = project_node(
                &mut child_schema,
                &child_pointer(pointer, "patternProperties", &pattern),
                aliases,
                nodes_by_pointer,
            );
            if let Ok(compiled) = Regex::new(&pattern) {
                local_property_names.extend(aliases.iter().filter_map(|(original, wire)| {
                    (compiled.is_match(original) && !compiled.is_match(wire))
                        .then_some(original.clone())
                }));
                node.pattern_properties.push(PatternProperty {
                    pattern: compiled.clone(),
                    child: child.clone(),
                });
                if let Some(mut alias_pattern) = alias_pattern(&compiled, aliases) {
                    while !used_patterns.insert(alias_pattern.clone()) {
                        alias_pattern = format!("(?:{alias_pattern})");
                    }
                    if let Ok(compiled_alias) = Regex::new(&alias_pattern) {
                        node.pattern_properties.push(PatternProperty {
                            pattern: compiled_alias,
                            child,
                        });
                        supplemental.push((alias_pattern, child_schema.clone()));
                    }
                }
            }
            patterns.insert(pattern, child_schema);
        }
        for (pattern, child_schema) in supplemental {
            patterns.insert(pattern, child_schema);
        }
    }

    if let Some(dependencies) = object
        .get_mut("dependentSchemas")
        .and_then(Value::as_object_mut)
    {
        let original_dependencies = std::mem::take(dependencies);
        let mut projected_dependencies = Map::new();
        for (original_name, mut child_schema) in original_dependencies {
            let wire_name = aliases
                .get(&original_name)
                .cloned()
                .unwrap_or_else(|| original_name.clone());
            let child = project_node(
                &mut child_schema,
                &child_pointer(pointer, "dependentSchemas", &original_name),
                aliases,
                nodes_by_pointer,
            );
            projected_dependencies.insert(wire_name.clone(), child_schema);
            node.dependent_schemas.push(DependentSchema {
                original_name,
                wire_name,
                child,
            });
        }
        *dependencies = projected_dependencies;
    }

    if let Some(dependencies) = object
        .get_mut("dependencies")
        .and_then(Value::as_object_mut)
    {
        let original_dependencies = std::mem::take(dependencies);
        let mut projected_dependencies = Map::new();
        for (original_name, mut dependency) in original_dependencies {
            let wire_name = aliases
                .get(&original_name)
                .cloned()
                .unwrap_or_else(|| original_name.clone());
            if dependency.is_array() {
                rename_string_array(Some(&mut dependency), aliases);
            } else {
                let child = project_node(
                    &mut dependency,
                    &child_pointer(pointer, "dependencies", &original_name),
                    aliases,
                    nodes_by_pointer,
                );
                node.dependent_schemas.push(DependentSchema {
                    original_name: original_name.clone(),
                    wire_name: wire_name.clone(),
                    child,
                });
            }
            projected_dependencies.insert(wire_name, dependency);
        }
        *dependencies = projected_dependencies;
    }

    // `required` and dependency keywords may name properties without a local
    // `properties` entry. They still need an object-key projection rule at
    // this schema node even though there is no child schema to traverse.
    let existing_names: HashSet<&str> = node
        .properties
        .iter()
        .map(|property| property.original_name.as_str())
        .collect();
    let mut synthetic_names: Vec<String> = local_property_names
        .into_iter()
        .filter(|name| aliases.contains_key(name) && !existing_names.contains(name.as_str()))
        .collect();
    synthetic_names.sort();
    for original_name in synthetic_names {
        let wire_name = aliases
            .get(&original_name)
            .cloned()
            .unwrap_or_else(|| original_name.clone());
        node.properties.push(PropertyAlias {
            child: AliasNode {
                pointer: format!(
                    "{pointer}/__propertyNames/{}",
                    escape_json_pointer(&original_name)
                ),
                ..AliasNode::default()
            }
            .into(),
            original_name,
            wire_name,
        });
    }

    node.additional_properties = project_optional_schema_keyword(
        object,
        pointer,
        "additionalProperties",
        aliases,
        nodes_by_pointer,
    );
    node.unevaluated_properties = project_optional_schema_keyword(
        object,
        pointer,
        "unevaluatedProperties",
        aliases,
        nodes_by_pointer,
    );
    node.contains =
        project_optional_schema_keyword(object, pointer, "contains", aliases, nodes_by_pointer);

    let mut has_legacy_tuple_items = false;
    if let Some(items) = object.get_mut("items") {
        if items.is_object() {
            node.items = Some(project_node(
                items,
                &single_pointer(pointer, "items"),
                aliases,
                nodes_by_pointer,
            ));
        } else if let Some(tuple_items) = items.as_array_mut() {
            has_legacy_tuple_items = true;
            node.prefix_items = tuple_items
                .iter_mut()
                .enumerate()
                .map(|(index, item)| {
                    project_node(
                        item,
                        &indexed_pointer(pointer, "items", index),
                        aliases,
                        nodes_by_pointer,
                    )
                })
                .collect();
        }
    }
    if has_legacy_tuple_items {
        node.items = project_optional_schema_keyword(
            object,
            pointer,
            "additionalItems",
            aliases,
            nodes_by_pointer,
        );
    }

    if let Some(prefix_items) = object.get_mut("prefixItems").and_then(Value::as_array_mut) {
        node.prefix_items = prefix_items
            .iter_mut()
            .enumerate()
            .map(|(index, item)| {
                project_node(
                    item,
                    &indexed_pointer(pointer, "prefixItems", index),
                    aliases,
                    nodes_by_pointer,
                )
            })
            .collect();
    }
    if let Some(unevaluated_items) = project_optional_schema_keyword(
        object,
        pointer,
        "unevaluatedItems",
        aliases,
        nodes_by_pointer,
    ) {
        node.array_item_applicators.push(unevaluated_items);
    }

    for keyword in ["allOf", "anyOf", "oneOf"] {
        if let Some(applicators) = object.get_mut(keyword).and_then(Value::as_array_mut) {
            for (index, applicator) in applicators.iter_mut().enumerate() {
                node.applicators.push(project_node(
                    applicator,
                    &indexed_pointer(pointer, keyword, index),
                    aliases,
                    nodes_by_pointer,
                ));
            }
        }
    }
    // Projection is syntactic rather than validation: every branch must apply
    // the same schema-wide key aliases, including the negated branch, so the
    // provider evaluates the projected schema against projected keys.
    for keyword in ["if", "then", "else", "not"] {
        if let Some(applicator) = object.get_mut(keyword) {
            node.applicators.push(project_node(
                applicator,
                &single_pointer(pointer, keyword),
                aliases,
                nodes_by_pointer,
            ));
        }
    }

    let node = Arc::new(node);
    nodes_by_pointer.insert(pointer.to_string(), node.clone());
    node
}

fn project_optional_schema_keyword(
    object: &mut Map<String, Value>,
    pointer: &str,
    keyword: &str,
    aliases: &HashMap<String, String>,
    nodes_by_pointer: &mut HashMap<String, Arc<AliasNode>>,
) -> Option<Arc<AliasNode>> {
    object.get_mut(keyword).and_then(|schema| {
        schema.is_object().then(|| {
            project_node(
                schema,
                &single_pointer(pointer, keyword),
                aliases,
                nodes_by_pointer,
            )
        })
    })
}

fn rename_string_array(value: Option<&mut Value>, aliases: &HashMap<String, String>) {
    let Some(values) = value.and_then(Value::as_array_mut) else {
        return;
    };
    for name in values {
        if let Some(original_name) = name.as_str()
            && let Some(wire_name) = aliases.get(original_name)
        {
            *name = Value::String(wire_name.clone());
        }
    }
}

fn rename_dependency_arrays(value: Option<&mut Value>, aliases: &HashMap<String, String>) {
    let Some(dependencies) = value.and_then(Value::as_object_mut) else {
        return;
    };
    let original = std::mem::take(dependencies);
    for (name, mut dependent) in original {
        rename_string_array(Some(&mut dependent), aliases);
        dependencies.insert(aliases.get(&name).cloned().unwrap_or(name), dependent);
    }
}

fn alias_pattern(pattern: &Regex, aliases: &HashMap<String, String>) -> Option<String> {
    let mut wire_names: Vec<&str> = aliases
        .iter()
        .filter_map(|(original, wire)| {
            (pattern.is_match(original) && !pattern.is_match(wire)).then_some(wire.as_str())
        })
        .collect();
    wire_names.sort_unstable();
    wire_names.dedup();
    (!wire_names.is_empty()).then(|| {
        let alternatives = wire_names
            .into_iter()
            .map(regex::escape)
            .collect::<Vec<_>>()
            .join("|");
        format!("^(?:{alternatives})$")
    })
}

pub(super) fn is_valid_property_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_PROPERTY_NAME_CHARS
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn allocate_wire_name(original: &str, reserved: &mut HashSet<String>) -> String {
    let mut base: String = original
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if base.is_empty() {
        base.push_str("property");
    }

    let digest = format!("{:x}", sha2::Sha256::digest(original.as_bytes()));
    for digest_chars in (8..=56).step_by(4) {
        let base_chars = MAX_PROPERTY_NAME_CHARS - digest_chars - 1;
        let prefix: String = base.chars().take(base_chars).collect();
        let candidate = format!("{prefix}_{}", &digest[..digest_chars]);
        if reserved.insert(candidate.clone()) {
            return candidate;
        }
    }
    let mut ordinal = 2_i64;
    loop {
        let suffix = format!("_{}_{ordinal}", &digest[..48]);
        let prefix: String = base
            .chars()
            .take(MAX_PROPERTY_NAME_CHARS.saturating_sub(suffix.len()))
            .collect();
        let candidate = format!("{prefix}{suffix}");
        if reserved.insert(candidate.clone()) {
            return candidate;
        }
        ordinal += 1;
    }
}

pub(super) fn escape_json_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn unescape_json_pointer(value: &str) -> String {
    value.replace("~1", "/").replace("~0", "~")
}

fn project_reference(reference: &str, aliases: &HashMap<String, String>) -> String {
    let Some(pointer) = reference.strip_prefix("#/") else {
        return reference.to_string();
    };
    let mut previous = None;
    let projected: Vec<String> = pointer
        .split('/')
        .map(|segment| {
            let decoded = unescape_json_pointer(segment);
            let projected = if matches!(
                previous.as_deref(),
                Some("properties" | "dependentSchemas" | "dependentRequired" | "dependencies")
            ) {
                aliases.get(&decoded).cloned().unwrap_or(decoded)
            } else {
                decoded
            };
            previous = Some(projected.clone());
            escape_json_pointer(&projected)
        })
        .collect();
    format!("#/{}", projected.join("/"))
}

fn single_pointer(parent: &str, keyword: &str) -> String {
    format!("{parent}/{}", escape_json_pointer(keyword))
}

fn child_pointer(parent: &str, keyword: &str, child: &str) -> String {
    format!(
        "{parent}/{}/{}",
        escape_json_pointer(keyword),
        escape_json_pointer(child)
    )
}

fn indexed_pointer(parent: &str, keyword: &str, index: usize) -> String {
    format!("{parent}/{}/{index}", escape_json_pointer(keyword))
}

pub(super) fn value_child_pointer(parent: &str, child: &str) -> String {
    format!("{parent}/{}", escape_json_pointer(child))
}
