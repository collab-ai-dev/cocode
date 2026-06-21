//! Tool ID and name mapping utilities.
//!
//! This module provides utilities for mapping between tool call IDs and tool names.

use std::collections::HashMap;

/// A bidirectional mapping between tool call IDs and tool names.
#[derive(Debug, Clone, Default)]
pub struct ToolMapping {
    /// Map from tool call ID to tool name.
    id_to_name: HashMap<String, String>,
    /// Map from tool name to tool call IDs (a tool can be called multiple times).
    name_to_ids: HashMap<String, Vec<String>>,
}

impl ToolMapping {
    /// Create a new empty tool mapping.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a mapping between a tool call ID and tool name.
    pub fn add(&mut self, tool_call_id: impl Into<String>, tool_name: impl Into<String>) {
        let tool_call_id = tool_call_id.into();
        let tool_name = tool_name.into();

        self.id_to_name
            .insert(tool_call_id.clone(), tool_name.clone());
        self.name_to_ids
            .entry(tool_name)
            .or_default()
            .push(tool_call_id);
    }

    /// Get the tool name for a tool call ID.
    pub fn get_name(&self, tool_call_id: &str) -> Option<&str> {
        self.id_to_name.get(tool_call_id).map(String::as_str)
    }

    /// Get all tool call IDs for a tool name.
    pub fn get_ids(&self, tool_name: &str) -> Option<&[String]> {
        self.name_to_ids.get(tool_name).map(Vec::as_slice)
    }

    /// Remove a mapping by tool call ID.
    pub fn remove(&mut self, tool_call_id: &str) -> Option<String> {
        let tool_name = self.id_to_name.remove(tool_call_id)?;

        if let Some(ids) = self.name_to_ids.get_mut(&tool_name) {
            ids.retain(|id| id != tool_call_id);
            if ids.is_empty() {
                self.name_to_ids.remove(&tool_name);
            }
        }

        Some(tool_name)
    }

    /// Check if the mapping contains a tool call ID.
    pub fn contains_id(&self, tool_call_id: &str) -> bool {
        self.id_to_name.contains_key(tool_call_id)
    }

    /// Check if the mapping contains a tool name.
    pub fn contains_name(&self, tool_name: &str) -> bool {
        self.name_to_ids.contains_key(tool_name)
    }

    /// Get the number of tool call ID mappings.
    pub fn len(&self) -> usize {
        self.id_to_name.len()
    }

    /// Check if the mapping is empty.
    pub fn is_empty(&self) -> bool {
        self.id_to_name.is_empty()
    }

    /// Clear all mappings.
    pub fn clear(&mut self) {
        self.id_to_name.clear();
        self.name_to_ids.clear();
    }

    /// Iterate over all (tool_call_id, tool_name) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.id_to_name.iter()
    }
}

/// Generate a tool call ID from a tool name and index.
///
/// Format: `{tool_name}_{index}`
pub fn generate_tool_call_id(tool_name: &str, index: usize) -> String {
    format!("{tool_name}_{index}")
}

/// Parse a tool call ID to extract the tool name and index.
///
/// Returns `None` if the ID doesn't match the expected format.
pub fn parse_tool_call_id(tool_call_id: &str) -> Option<(&str, usize)> {
    let (name, index_str) = tool_call_id.rsplit_once('_')?;
    let index = index_str.parse().ok()?;
    Some((name, index))
}

#[cfg(test)]
#[path = "tool_mapping.test.rs"]
mod tests;
