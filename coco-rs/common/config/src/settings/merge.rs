/// Deep merge two JSON values. `overlay` fields override `base` fields.
/// Arrays are concatenated (no dedup in this implementation — caller can dedup).
/// Objects are recursively merged.
pub fn deep_merge(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let entry = base_map
                    .entry(key.clone())
                    .or_insert(serde_json::Value::Null);
                deep_merge(entry, overlay_val);
            }
        }
        (serde_json::Value::Array(base_arr), serde_json::Value::Array(overlay_arr)) => {
            base_arr.extend(overlay_arr.iter().cloned());
        }
        (base, overlay) => {
            *base = overlay.clone();
        }
    }
}

#[cfg(test)]
#[path = "merge.test.rs"]
mod tests;
