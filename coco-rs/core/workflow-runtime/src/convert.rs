//! `serde_json::Value` ↔ JS value conversion across the host↔VM boundary.
//!
//! Bridges through QuickJS's own JSON codec (`ctx.json_parse` /
//! `ctx.json_stringify`) — robust, matches `JSON.stringify` semantics (the
//! workflow DSL's "data in/out is JSON-serializable" contract), and can't
//! desync from JS number/unicode handling. `undefined`/functions/symbols
//! stringify to nothing → mapped to `null`.

use rquickjs::Ctx;
use rquickjs::Value;

/// Convert a `serde_json::Value` into a JS value in `ctx`.
pub fn json_to_js<'js>(ctx: &Ctx<'js>, value: &serde_json::Value) -> rquickjs::Result<Value<'js>> {
    // serde_json never fails to serialize a Value; QuickJS JSON.parse rebuilds it.
    ctx.json_parse(value.to_string())
}

/// Convert a JS value into a `serde_json::Value`. Non-JSON shapes
/// (undefined/functions/symbols) become `null`, matching `JSON.stringify`.
pub fn js_to_json<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<serde_json::Value> {
    match ctx.json_stringify(value)? {
        Some(text) => {
            let text = text.to_string()?;
            Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
        }
        None => Ok(serde_json::Value::Null),
    }
}

#[cfg(test)]
#[path = "convert.test.rs"]
mod tests;
