//! JSON value types.
//!
//! These are type aliases for working with JSON data in the SDK.

use serde_json::Value;
use std::collections::HashMap;

/// A JSON value.
pub type JSONValue = Value;

/// A JSON object (map of string to JSON value).
pub type JSONObject = HashMap<String, JSONValue>;

/// A JSON array.
pub type JSONArray = Vec<JSONValue>;
