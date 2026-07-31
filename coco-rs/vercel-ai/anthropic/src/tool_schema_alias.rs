use std::collections::HashMap;
use std::collections::HashSet;

use serde_json::Map;
use serde_json::Value;
use sha2::Digest as _;
use thiserror::Error;

const MAX_PROPERTY_NAME_CHARS: usize = 64;

#[derive(Debug, Error)]
pub(crate) enum ToolSchemaAliasError {
    #[error(
        "tool input contains both projected property {wire_name:?} and original property {original_name:?}"
    )]
    PropertyCollision {
        wire_name: String,
        original_name: String,
    },
}

#[derive(Clone, Default)]
pub(crate) struct ToolSchemaAliases {
    by_tool: HashMap<String, SchemaProjection>,
}

#[derive(Clone, Default)]
pub(crate) struct SchemaProjection {
    root: AliasNode,
    definitions: HashMap<String, AliasNode>,
}

#[derive(Clone, Default)]
struct AliasNode {
    properties: Vec<PropertyAlias>,
    items: Option<Box<AliasNode>>,
    prefix_items: Vec<AliasNode>,
    alternatives: Vec<AliasNode>,
    reference: Option<String>,
}

#[derive(Clone)]
struct PropertyAlias {
    original_name: String,
    wire_name: String,
    child: AliasNode,
}

#[derive(Clone, Copy)]
enum ProjectionDirection {
    ToWire,
    FromWire,
}

impl ToolSchemaAliases {
    pub(crate) fn insert(&mut self, tool_name: String, projection: SchemaProjection) {
        self.by_tool.insert(tool_name, projection);
    }

    pub(crate) fn project_input(
        &self,
        tool_name: &str,
        input: &mut Value,
    ) -> Result<(), ToolSchemaAliasError> {
        self.transform(tool_name, input, ProjectionDirection::ToWire)
    }

    pub(crate) fn restore_input(
        &self,
        tool_name: &str,
        input: &mut Value,
    ) -> Result<(), ToolSchemaAliasError> {
        self.transform(tool_name, input, ProjectionDirection::FromWire)
    }

    pub(crate) fn requires_projection(&self, tool_name: &str) -> bool {
        self.by_tool
            .get(tool_name)
            .is_some_and(SchemaProjection::has_aliases)
    }

    pub(crate) fn project_message_tool_inputs(
        &self,
        messages: &mut [Value],
    ) -> Result<(), ToolSchemaAliasError> {
        for message in messages {
            let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
                continue;
            };
            for block in content {
                if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                    continue;
                }
                let Some(tool_name) = block
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                else {
                    continue;
                };
                if let Some(input) = block.get_mut("input") {
                    self.project_input(&tool_name, input)?;
                }
            }
        }
        Ok(())
    }

    fn transform(
        &self,
        tool_name: &str,
        input: &mut Value,
        direction: ProjectionDirection,
    ) -> Result<(), ToolSchemaAliasError> {
        let Some(projection) = self.by_tool.get(tool_name) else {
            return Ok(());
        };
        transform_value(&projection.root, &projection.definitions, input, direction)
    }
}

impl SchemaProjection {
    fn has_aliases(&self) -> bool {
        self.root.has_aliases() || self.definitions.values().any(AliasNode::has_aliases)
    }
}

impl AliasNode {
    fn has_aliases(&self) -> bool {
        self.properties.iter().any(|property| {
            property.original_name != property.wire_name || property.child.has_aliases()
        }) || self.items.as_deref().is_some_and(AliasNode::has_aliases)
            || self.prefix_items.iter().any(AliasNode::has_aliases)
            || self.alternatives.iter().any(AliasNode::has_aliases)
    }
}

pub(crate) fn project_schema(schema: &Value) -> (Value, SchemaProjection) {
    let mut projected = schema.clone();
    let mut definitions = HashMap::new();
    let root = project_node(&mut projected, &mut definitions);
    (projected, SchemaProjection { root, definitions })
}

fn project_node(schema: &mut Value, definitions: &mut HashMap<String, AliasNode>) -> AliasNode {
    let Some(object) = schema.as_object_mut() else {
        return AliasNode::default();
    };

    let mut node = AliasNode {
        reference: object
            .get("$ref")
            .and_then(Value::as_str)
            .map(str::to_string),
        ..AliasNode::default()
    };

    for definitions_key in ["$defs", "definitions"] {
        if let Some(definitions_object) = object
            .get_mut(definitions_key)
            .and_then(Value::as_object_mut)
        {
            let mut names: Vec<String> = definitions_object.keys().cloned().collect();
            names.sort();
            for name in names {
                if let Some(definition) = definitions_object.get_mut(&name) {
                    let alias_node = project_node(definition, definitions);
                    definitions.insert(
                        format!("#/{definitions_key}/{}", escape_json_pointer(&name)),
                        alias_node,
                    );
                }
            }
        }
    }

    if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
        let original_properties = std::mem::take(properties);
        let mut entries: Vec<(String, Value)> = original_properties.into_iter().collect();
        entries.sort_by(|(left, _), (right, _)| left.cmp(right));

        let mut reserved: HashSet<String> = entries
            .iter()
            .filter(|(name, _)| is_valid_property_name(name))
            .map(|(name, _)| name.clone())
            .collect();
        let mut original_to_wire = HashMap::new();
        let mut projected_properties = Map::new();

        for (original_name, mut property_schema) in entries {
            let wire_name = if is_valid_property_name(&original_name) {
                original_name.clone()
            } else {
                allocate_wire_name(&original_name, &mut reserved)
            };
            original_to_wire.insert(original_name.clone(), wire_name.clone());
            let child = project_node(&mut property_schema, definitions);
            projected_properties.insert(wire_name.clone(), property_schema);
            node.properties.push(PropertyAlias {
                original_name,
                wire_name,
                child,
            });
        }
        *properties = projected_properties;

        if let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) {
            for name in required {
                if let Some(original_name) = name.as_str()
                    && let Some(wire_name) = original_to_wire.get(original_name)
                {
                    *name = Value::String(wire_name.clone());
                }
            }
        }
    }

    if let Some(items) = object.get_mut("items") {
        node.items = Some(Box::new(project_node(items, definitions)));
    }
    if let Some(prefix_items) = object.get_mut("prefixItems").and_then(Value::as_array_mut) {
        node.prefix_items = prefix_items
            .iter_mut()
            .map(|item| project_node(item, definitions))
            .collect();
    }
    for alternative_key in ["allOf", "anyOf", "oneOf"] {
        if let Some(alternatives) = object
            .get_mut(alternative_key)
            .and_then(Value::as_array_mut)
        {
            node.alternatives.extend(
                alternatives
                    .iter_mut()
                    .map(|alternative| project_node(alternative, definitions)),
            );
        }
    }

    node
}

fn transform_value(
    node: &AliasNode,
    definitions: &HashMap<String, AliasNode>,
    value: &mut Value,
    direction: ProjectionDirection,
) -> Result<(), ToolSchemaAliasError> {
    if let Some(reference) = &node.reference
        && let Some(referenced) = definitions.get(reference)
    {
        transform_value(referenced, definitions, value, direction)?;
    }

    if let Some(object) = value.as_object_mut() {
        for property in &node.properties {
            let (source, destination) = match direction {
                ProjectionDirection::ToWire => (&property.original_name, &property.wire_name),
                ProjectionDirection::FromWire => (&property.wire_name, &property.original_name),
            };

            if source == destination {
                if let Some(child_value) = object.get_mut(source) {
                    transform_value(&property.child, definitions, child_value, direction)?;
                }
                continue;
            }
            if !object.contains_key(source) {
                continue;
            }
            if object.contains_key(destination) {
                return Err(ToolSchemaAliasError::PropertyCollision {
                    wire_name: property.wire_name.clone(),
                    original_name: property.original_name.clone(),
                });
            }
            if let Some(mut child_value) = object.remove(source) {
                transform_value(&property.child, definitions, &mut child_value, direction)?;
                object.insert(destination.clone(), child_value);
            }
        }
    }

    if let Some(array) = value.as_array_mut() {
        for (index, item) in array.iter_mut().enumerate() {
            if let Some(prefix_node) = node.prefix_items.get(index) {
                transform_value(prefix_node, definitions, item, direction)?;
            } else if let Some(items) = &node.items {
                transform_value(items, definitions, item, direction)?;
            }
        }
    }

    for alternative in &node.alternatives {
        transform_value(alternative, definitions, value, direction)?;
    }
    Ok(())
}

fn is_valid_property_name(name: &str) -> bool {
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

fn escape_json_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
#[path = "tool_schema_alias.test.rs"]
mod tests;
