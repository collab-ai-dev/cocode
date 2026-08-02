use std::collections::HashMap;
use std::sync::Arc;

use regex::Regex;
use serde_json::Value;
use thiserror::Error;

mod schema;
mod transform;

pub(crate) use schema::project_schema;
#[cfg(test)]
use schema::{escape_json_pointer, is_valid_property_name};
use transform::transform_value;

#[derive(Debug, Error)]
pub(crate) enum ToolSchemaAliasError {
    #[error(
        "tool input contains both projected property {wire_name:?} and original property {original_name:?}"
    )]
    PropertyCollision {
        wire_name: String,
        original_name: String,
    },
    #[error(
        "tool input contains unexpected property form {encountered_name:?}; expected {expected_name:?} before alias transformation"
    )]
    UnexpectedPropertyForm {
        expected_name: String,
        encountered_name: String,
    },
    #[error("tool schema contains an unresolved local reference {reference:?}")]
    UnresolvedReference { reference: String },
}

#[derive(Clone, Default)]
pub(crate) struct ToolSchemaAliases {
    by_tool: HashMap<String, SchemaProjection>,
}

#[derive(Clone, Default)]
pub(crate) struct SchemaProjection {
    root: Arc<AliasNode>,
    nodes_by_pointer: HashMap<String, Arc<AliasNode>>,
}

#[derive(Clone, Default)]
struct AliasNode {
    pointer: String,
    properties: Vec<PropertyAlias>,
    pattern_properties: Vec<PatternProperty>,
    dependent_schemas: Vec<DependentSchema>,
    additional_properties: Option<Arc<AliasNode>>,
    unevaluated_properties: Option<Arc<AliasNode>>,
    items: Option<Arc<AliasNode>>,
    prefix_items: Vec<Arc<AliasNode>>,
    contains: Option<Arc<AliasNode>>,
    array_item_applicators: Vec<Arc<AliasNode>>,
    applicators: Vec<Arc<AliasNode>>,
    reference: Option<String>,
}

#[derive(Clone)]
struct PropertyAlias {
    original_name: String,
    wire_name: String,
    child: Arc<AliasNode>,
}

#[derive(Clone)]
struct PatternProperty {
    pattern: Regex,
    child: Arc<AliasNode>,
}

#[derive(Clone)]
struct DependentSchema {
    original_name: String,
    wire_name: String,
    child: Arc<AliasNode>,
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

    fn is_empty(&self) -> bool {
        !self.by_tool.values().any(SchemaProjection::has_aliases)
    }

    /// Rewrite replayed `tool_use.input` keys to their wire aliases.
    ///
    /// Returns the tool names whose history blocks could not be projected.
    /// A failure here is per-block and never fatal: a single unprojectable
    /// input must not make request construction fail for the rest of the
    /// session (the same history is replayed every turn, so a hard error
    /// would brick the conversation until compaction dropped the message).
    /// The block is left verbatim and the caller surfaces a warning.
    pub(crate) fn project_message_tool_inputs(&self, messages: &mut [Value]) -> Vec<String> {
        let mut skipped = Vec::new();
        if self.is_empty() {
            return skipped;
        }
        for message in messages.iter_mut() {
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
                if let Some(input) = block.get_mut("input")
                    // `transform` is itself atomic per input, so a failed
                    // block is left exactly as it arrived.
                    && let Err(error) =
                        self.transform(&tool_name, input, ProjectionDirection::ToWire)
                {
                    tracing::warn!(
                        tool_name,
                        %error,
                        "replayed tool input could not be projected to wire names; sending it verbatim"
                    );
                    skipped.push(tool_name);
                }
            }
        }
        skipped
    }

    fn transform(
        &self,
        tool_name: &str,
        input: &mut Value,
        direction: ProjectionDirection,
    ) -> Result<(), ToolSchemaAliasError> {
        // No aliased property for this tool → nothing to rename, and no
        // reason to clone the input or walk the schema graph.
        let Some(projection) = self
            .by_tool
            .get(tool_name)
            .filter(|projection| projection.has_aliases())
        else {
            return Ok(());
        };
        // Tool input projection is atomic. This is intentionally a full JSON
        // clone: provider inputs are small, while partially renamed input is
        // impossible for callers to recover safely after an error.
        let mut projected = input.clone();
        let mut traversal = transform::TransformTraversal::default();
        transform_value(
            &projection.root,
            &projection.nodes_by_pointer,
            &mut projected,
            direction,
            "",
            &mut traversal,
        )?;
        *input = projected;
        Ok(())
    }
}

impl SchemaProjection {
    fn has_aliases(&self) -> bool {
        self.nodes_by_pointer.values().any(|node| {
            node.properties
                .iter()
                .any(|property| property.original_name != property.wire_name)
        })
    }
}

#[cfg(test)]
#[path = "tool_schema_alias.test.rs"]
mod tests;
