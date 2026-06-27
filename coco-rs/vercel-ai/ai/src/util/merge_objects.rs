//! Deep merge two objects.

use serde_json::Value;

/// Deeply merges two JSON values together.
///
/// - Properties from `overrides` override those in `base` with the same key.
/// - For nested objects, the merge is performed recursively (deep merge).
/// - Arrays are replaced, not merged.
/// - Primitive values are replaced.
/// - If both inputs are `None`, returns `None`.
/// - If one input is `None`, returns the other.
pub fn merge_objects(base: Option<Value>, overrides: Option<Value>) -> Option<Value> {
    // If both inputs are None, return None
    if base.is_none() && overrides.is_none() {
        return None;
    }

    // If base is None, return overrides
    if base.is_none() {
        return overrides;
    }

    // If overrides is None, return base
    if overrides.is_none() {
        return base;
    }

    // At this point both are Some, so we can safely unwrap
    let (Some(base), Some(overrides)) = (base, overrides) else {
        unreachable!("Both values should be Some after the above checks");
    };

    // Both must be objects for deep merge
    match (&base, &overrides) {
        (Value::Object(base_map), Value::Object(overrides_map)) => {
            let mut result = base_map.clone();
            for (key, overrides_value) in overrides_map {
                let base_value = result.get(key);

                // Check if both values are objects that can be deeply merged
                let is_base_object = matches!(base_value, Some(Value::Object(_)));
                let is_overrides_object = overrides_value.is_object();

                if is_base_object && is_overrides_object {
                    let merged = merge_objects(base_value.cloned(), Some(overrides_value.clone()));
                    result.insert(key.clone(), merged.unwrap_or(Value::Null));
                } else {
                    result.insert(key.clone(), overrides_value.clone());
                }
            }
            Some(Value::Object(result))
        }
        // For non-objects, simply override
        _ => Some(overrides),
    }
}

#[cfg(test)]
#[path = "merge_objects.test.rs"]
mod tests;
