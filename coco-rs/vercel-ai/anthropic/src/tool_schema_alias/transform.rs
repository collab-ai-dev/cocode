use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;

use super::schema::value_child_pointer;
use super::{AliasNode, ProjectionDirection, PropertyAlias, ToolSchemaAliasError};

#[derive(Default)]
pub(super) struct TransformTraversal {
    visited: HashSet<(String, String)>,
    canonical_property_names: HashMap<(String, String), String>,
}

pub(super) fn transform_value(
    node: &AliasNode,
    nodes_by_pointer: &HashMap<String, Arc<AliasNode>>,
    value: &mut Value,
    direction: ProjectionDirection,
    value_pointer: &str,
    traversal: &mut TransformTraversal,
) -> Result<(), ToolSchemaAliasError> {
    if !traversal
        .visited
        .insert((node.pointer.clone(), value_pointer.to_string()))
    {
        return Ok(());
    }

    if let Some(reference) = &node.reference {
        let referenced = nodes_by_pointer.get(reference).ok_or_else(|| {
            ToolSchemaAliasError::UnresolvedReference {
                reference: reference.clone(),
            }
        })?;
        transform_value(
            referenced,
            nodes_by_pointer,
            value,
            direction,
            value_pointer,
            traversal,
        )?;
    }

    let mut applicable_dependencies = Vec::new();
    if let Some(object) = value.as_object_mut() {
        for property in &node.properties {
            let (source, destination) = property_names(property, direction);
            let child_pointer = value_child_pointer(value_pointer, &property.original_name);
            if source == destination {
                if let Some(child_value) = object.get_mut(source) {
                    transform_value(
                        &property.child,
                        nodes_by_pointer,
                        child_value,
                        direction,
                        &child_pointer,
                        traversal,
                    )?;
                }
                continue;
            }
            if !object.contains_key(source) {
                if let Some(child_value) = object.get_mut(destination) {
                    let rename_key = (value_pointer.to_string(), destination.to_string());
                    if traversal.canonical_property_names.get(&rename_key)
                        != Some(&property.original_name)
                    {
                        return Err(ToolSchemaAliasError::UnexpectedPropertyForm {
                            expected_name: source.to_string(),
                            encountered_name: destination.to_string(),
                        });
                    }
                    // Another applicable branch already renamed this key in
                    // the current transaction. Its child schema must still run
                    // for allOf/ref graphs.
                    transform_value(
                        &property.child,
                        nodes_by_pointer,
                        child_value,
                        direction,
                        &child_pointer,
                        traversal,
                    )?;
                }
                continue;
            }
            if object.contains_key(destination) {
                return Err(ToolSchemaAliasError::PropertyCollision {
                    wire_name: property.wire_name.clone(),
                    original_name: property.original_name.clone(),
                });
            }
            let Some(mut child_value) = object.remove(source) else {
                continue;
            };
            transform_value(
                &property.child,
                nodes_by_pointer,
                &mut child_value,
                direction,
                &child_pointer,
                traversal,
            )?;
            object.insert(destination.to_string(), child_value);
            traversal.canonical_property_names.insert(
                (value_pointer.to_string(), destination.to_string()),
                property.original_name.clone(),
            );
        }

        let declared: HashSet<&str> = node
            .properties
            .iter()
            .flat_map(|property| [property.original_name.as_str(), property.wire_name.as_str()])
            .collect();
        let keys: Vec<String> = object.keys().cloned().collect();
        for key in keys {
            let canonical_key = traversal
                .canonical_property_names
                .get(&(value_pointer.to_string(), key.clone()))
                .map_or(key.as_str(), String::as_str);
            let property_pointer = value_child_pointer(value_pointer, canonical_key);
            let mut matched_pattern = false;
            for pattern in &node.pattern_properties {
                if pattern.pattern.is_match(&key) {
                    matched_pattern = true;
                    if let Some(child_value) = object.get_mut(&key) {
                        transform_value(
                            &pattern.child,
                            nodes_by_pointer,
                            child_value,
                            direction,
                            &property_pointer,
                            traversal,
                        )?;
                    }
                }
            }
            if !declared.contains(key.as_str()) && !matched_pattern {
                if let Some(additional) = &node.additional_properties
                    && let Some(child_value) = object.get_mut(&key)
                {
                    transform_value(
                        additional,
                        nodes_by_pointer,
                        child_value,
                        direction,
                        &property_pointer,
                        traversal,
                    )?;
                }
                if let Some(unevaluated) = &node.unevaluated_properties
                    && let Some(child_value) = object.get_mut(&key)
                {
                    transform_value(
                        unevaluated,
                        nodes_by_pointer,
                        child_value,
                        direction,
                        &property_pointer,
                        traversal,
                    )?;
                }
            }
        }

        for dependency in &node.dependent_schemas {
            let trigger = match direction {
                ProjectionDirection::ToWire => &dependency.wire_name,
                ProjectionDirection::FromWire => &dependency.original_name,
            };
            if object.contains_key(trigger) {
                applicable_dependencies.push(dependency.child.clone());
            }
        }
    }
    for dependency in &applicable_dependencies {
        transform_value(
            dependency,
            nodes_by_pointer,
            value,
            direction,
            value_pointer,
            traversal,
        )?;
    }

    if let Some(array) = value.as_array_mut() {
        for (index, item) in array.iter_mut().enumerate() {
            let item_pointer = format!("{value_pointer}/{index}");
            if let Some(prefix_node) = node.prefix_items.get(index) {
                transform_value(
                    prefix_node,
                    nodes_by_pointer,
                    item,
                    direction,
                    &item_pointer,
                    traversal,
                )?;
            } else if let Some(items) = &node.items {
                transform_value(
                    items,
                    nodes_by_pointer,
                    item,
                    direction,
                    &item_pointer,
                    traversal,
                )?;
            }
            if let Some(contains) = &node.contains {
                transform_value(
                    contains,
                    nodes_by_pointer,
                    item,
                    direction,
                    &item_pointer,
                    traversal,
                )?;
            }
            for applicator in &node.array_item_applicators {
                transform_value(
                    applicator,
                    nodes_by_pointer,
                    item,
                    direction,
                    &item_pointer,
                    traversal,
                )?;
            }
        }
    }

    for applicator in &node.applicators {
        transform_value(
            applicator,
            nodes_by_pointer,
            value,
            direction,
            value_pointer,
            traversal,
        )?;
    }
    Ok(())
}

fn property_names(property: &PropertyAlias, direction: ProjectionDirection) -> (&str, &str) {
    match direction {
        ProjectionDirection::ToWire => (&property.original_name, &property.wire_name),
        ProjectionDirection::FromWire => (&property.wire_name, &property.original_name),
    }
}
