//! Incremental JSON builder for Google's streaming `partialArgs` chunks.
//!
//! During tool-call function calling,
//! Gemini emits `partialArgs` arrays where each entry is a typed leaf value
//! addressed by a JSONPath (`$.recipe.ingredients[0].name`). This accumulator
//! reconstructs both the structured `serde_json::Value` and a running JSON
//! text representation so callers can emit text deltas that, when
//! concatenated, form valid JSON.
//!
//! ```text
//! Input: [{ jsonPath:"$.location", stringValue:"Boston" }]
//! Output: '{"location":"Boston"', then finalize() → closingDelta="}"
//! ```

use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;

/// One streaming chunk of partial function-call arguments from Gemini.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PartialArg {
    pub json_path: String,
    #[serde(default)]
    pub string_value: Option<String>,
    #[serde(default)]
    pub number_value: Option<f64>,
    #[serde(default)]
    pub bool_value: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub null_value: Option<Value>,
    #[serde(default)]
    pub will_continue: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PathSegment {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone)]
struct StackEntry {
    segment: PathSegment,
    is_array: bool,
    child_count: usize,
}

/// Result of `process_partial_args` — exposed JSON snapshot + the textual
/// delta added during this call.
pub struct ProcessedPartial {
    pub current_json: Value,
    pub text_delta: String,
}

/// Result of `finalize` — final compact JSON + the closing delta needed to
/// turn the running text into valid JSON.
pub struct FinalizedJson {
    pub final_json: String,
    pub closing_delta: String,
}

/// Stateful accumulator over a sequence of `PartialArg` chunks.
#[derive(Debug, Default)]
pub struct GoogleJSONAccumulator {
    accumulated_args: Map<String, Value>,
    json_text: String,
    path_stack: Vec<StackEntry>,
    string_open: bool,
}

impl GoogleJSONAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process one batch of partial-arg chunks. Returns the running JSON
    /// snapshot and the delta added in this call.
    pub fn process_partial_args(&mut self, partial_args: &[PartialArg]) -> ProcessedPartial {
        let mut delta = String::new();

        for arg in partial_args {
            let raw_path = arg.json_path.strip_prefix("$.").unwrap_or(&arg.json_path);
            if raw_path.is_empty() {
                continue;
            }

            let segments = parse_path(raw_path);
            let prev_string = get_nested_value_in_map(&self.accumulated_args, &segments)
                .and_then(|v| v.as_str())
                .map(String::from);
            let is_string_continuation = arg.string_value.is_some() && prev_string.is_some();

            if is_string_continuation {
                let s = arg.string_value.as_deref().unwrap_or("");
                let escaped = json_string_escape_inner(s);
                let prev = prev_string.unwrap_or_default();
                set_nested_value(
                    &mut self.accumulated_args,
                    &segments,
                    Value::String(format!("{prev}{s}")),
                );
                delta.push_str(&escaped);
                continue;
            }

            let Some(resolved) = resolve_partial_arg_value(arg) else {
                continue;
            };

            set_nested_value(&mut self.accumulated_args, &segments, resolved.value);
            delta.push_str(&self.emit_navigation_to(&segments, arg, &resolved.json));
        }

        self.json_text.push_str(&delta);

        ProcessedPartial {
            current_json: Value::Object(self.accumulated_args.clone()),
            text_delta: delta,
        }
    }

    /// Emit any pending closing characters so `json_text + closing_delta` is
    /// valid serialized JSON of the accumulated object.
    ///
    /// Walks the current open-container stack and emits the matching `]` / `}`
    /// for each level — preserving insertion order, unlike re-serializing
    /// `accumulated_args` (which goes through `serde_json::Map`'s sorted
    /// representation when the `preserve_order` feature is off).
    pub fn finalize(&self) -> FinalizedJson {
        let mut closing_delta = String::new();
        if self.string_open {
            closing_delta.push('"');
        }
        // Skip the first sentinel root entry (segment="").
        for entry in self.path_stack.iter().rev() {
            closing_delta.push(if entry.is_array { ']' } else { '}' });
        }
        // If the path_stack is empty (no values were ever written), the empty
        // root object still needs to be a well-formed `{}`.
        if self.path_stack.is_empty() && self.json_text.is_empty() {
            closing_delta = "{}".to_string();
        }
        let final_json = format!("{}{}", self.json_text, closing_delta);
        FinalizedJson {
            final_json,
            closing_delta,
        }
    }

    fn ensure_root(&mut self) -> &'static str {
        if self.path_stack.is_empty() {
            self.path_stack.push(StackEntry {
                segment: PathSegment::Key(String::new()),
                is_array: false,
                child_count: 0,
            });
            "{"
        } else {
            ""
        }
    }

    fn emit_navigation_to(
        &mut self,
        target_segments: &[PathSegment],
        arg: &PartialArg,
        value_json: &str,
    ) -> String {
        let mut fragment = String::new();
        if self.string_open {
            fragment.push('"');
            self.string_open = false;
        }
        fragment.push_str(self.ensure_root());

        let Some((leaf, target_container)) = target_segments.split_last() else {
            return fragment;
        };

        let common_depth = self.find_common_stack_depth(target_container);
        fragment.push_str(&self.close_down_to(common_depth));
        fragment.push_str(&self.open_down_to(target_container, leaf));
        fragment.push_str(&self.emit_leaf(leaf, arg, value_json));
        fragment
    }

    fn find_common_stack_depth(&self, target_container: &[PathSegment]) -> usize {
        let max_depth = (self.path_stack.len() - 1).min(target_container.len());
        let common = target_container
            .iter()
            .zip(self.path_stack.iter().skip(1))
            .take(max_depth)
            .take_while(|(t, s)| s.segment == **t)
            .count();
        common + 1
    }

    fn close_down_to(&mut self, target_depth: usize) -> String {
        let mut fragment = String::new();
        while self.path_stack.len() > target_depth
            && let Some(entry) = self.path_stack.pop()
        {
            fragment.push(if entry.is_array { ']' } else { '}' });
        }
        fragment
    }

    fn open_down_to(&mut self, target_container: &[PathSegment], leaf: &PathSegment) -> String {
        let mut fragment = String::new();
        let start_idx = self.path_stack.len() - 1;
        for i in start_idx..target_container.len() {
            let seg = &target_container[i];
            let parent_idx = self.path_stack.len() - 1;
            let parent = &mut self.path_stack[parent_idx];
            if parent.child_count > 0 {
                fragment.push(',');
            }
            parent.child_count += 1;

            if let PathSegment::Key(s) = seg {
                fragment.push_str(&format!("{}:", json_string_quote(s)));
            }

            let child_seg = if i + 1 < target_container.len() {
                &target_container[i + 1]
            } else {
                leaf
            };
            let is_array = matches!(child_seg, PathSegment::Index(_));
            fragment.push(if is_array { '[' } else { '{' });

            self.path_stack.push(StackEntry {
                segment: seg.clone(),
                is_array,
                child_count: 0,
            });
        }
        fragment
    }

    fn emit_leaf(&mut self, leaf: &PathSegment, arg: &PartialArg, value_json: &str) -> String {
        let mut fragment = String::new();
        let parent_idx = self.path_stack.len() - 1;
        let parent = &mut self.path_stack[parent_idx];
        if parent.child_count > 0 {
            fragment.push(',');
        }
        parent.child_count += 1;

        if let PathSegment::Key(s) = leaf {
            fragment.push_str(&format!("{}:", json_string_quote(s)));
        }

        if arg.string_value.is_some() && arg.will_continue.unwrap_or(false) {
            // Drop the closing quote — string is "open" until next chunk.
            let trimmed = &value_json[..value_json.len().saturating_sub(1)];
            fragment.push_str(trimmed);
            self.string_open = true;
        } else {
            fragment.push_str(value_json);
        }
        fragment
    }
}

/// Splits a dotted/bracketed JSON path like `recipe.ingredients[0].name`
/// into segments.
fn parse_path(raw: &str) -> Vec<PathSegment> {
    let mut segments = Vec::new();
    for part in raw.split('.') {
        if let Some(bracket_idx) = part.find('[') {
            if bracket_idx > 0 {
                segments.push(PathSegment::Key(part[..bracket_idx].to_string()));
            }
            // Match every `[N]` group.
            let mut rest = &part[bracket_idx..];
            while let (Some(open), Some(close)) = (rest.find('['), rest.find(']')) {
                if close > open + 1
                    && let Ok(idx) = rest[open + 1..close].parse::<usize>()
                {
                    segments.push(PathSegment::Index(idx));
                }
                rest = &rest[close + 1..];
            }
        } else if !part.is_empty() {
            segments.push(PathSegment::Key(part.to_string()));
        }
    }
    segments
}

fn get_nested_value_in_map<'a>(
    map: &'a Map<String, Value>,
    segments: &[PathSegment],
) -> Option<&'a Value> {
    if segments.is_empty() {
        return None;
    }
    let head_key = match &segments[0] {
        PathSegment::Key(k) => k,
        PathSegment::Index(_) => return None,
    };
    let mut current = map.get(head_key)?;
    for seg in &segments[1..] {
        match seg {
            PathSegment::Key(k) => current = current.as_object()?.get(k)?,
            PathSegment::Index(i) => current = current.as_array()?.get(*i)?,
        }
    }
    Some(current)
}

fn set_nested_value(map: &mut Map<String, Value>, segments: &[PathSegment], value: Value) {
    if segments.is_empty() {
        return;
    }
    // First segment must be a key (root is object).
    let head_key = match &segments[0] {
        PathSegment::Key(k) => k.clone(),
        PathSegment::Index(_) => return,
    };

    if segments.len() == 1 {
        map.insert(head_key, value);
        return;
    }

    let next_is_array = matches!(segments[1], PathSegment::Index(_));
    if !map.contains_key(&head_key) || !matches_container(map.get(&head_key), next_is_array) {
        map.insert(
            head_key.clone(),
            if next_is_array {
                Value::Array(Vec::new())
            } else {
                Value::Object(Map::new())
            },
        );
    }
    if let Some(entry) = map.get_mut(&head_key) {
        set_nested_in_value(entry, &segments[1..], value);
    }
}

fn set_nested_in_value(current: &mut Value, segments: &[PathSegment], value: Value) {
    if segments.is_empty() {
        return;
    }
    if segments.len() == 1 {
        match (&segments[0], current) {
            (PathSegment::Key(k), Value::Object(o)) => {
                o.insert(k.clone(), value);
            }
            (PathSegment::Index(i), Value::Array(arr)) => {
                while arr.len() <= *i {
                    arr.push(Value::Null);
                }
                arr[*i] = value;
            }
            _ => {}
        }
        return;
    }
    let next_is_array = matches!(segments[1], PathSegment::Index(_));
    match (&segments[0], current) {
        (PathSegment::Key(k), Value::Object(o)) => {
            if !o.contains_key(k) || !matches_container(o.get(k), next_is_array) {
                o.insert(
                    k.clone(),
                    if next_is_array {
                        Value::Array(Vec::new())
                    } else {
                        Value::Object(Map::new())
                    },
                );
            }
            if let Some(entry) = o.get_mut(k) {
                set_nested_in_value(entry, &segments[1..], value);
            }
        }
        (PathSegment::Index(i), Value::Array(arr)) => {
            while arr.len() <= *i {
                arr.push(Value::Null);
            }
            if !matches_container(Some(&arr[*i]), next_is_array) {
                arr[*i] = if next_is_array {
                    Value::Array(Vec::new())
                } else {
                    Value::Object(Map::new())
                };
            }
            set_nested_in_value(&mut arr[*i], &segments[1..], value);
        }
        _ => {}
    }
}

fn matches_container(v: Option<&Value>, expect_array: bool) -> bool {
    match v {
        Some(Value::Array(_)) if expect_array => true,
        Some(Value::Object(_)) if !expect_array => true,
        _ => false,
    }
}

struct ResolvedValue {
    value: Value,
    json: String,
}

fn resolve_partial_arg_value(arg: &PartialArg) -> Option<ResolvedValue> {
    if let Some(s) = &arg.string_value {
        return Some(ResolvedValue {
            value: Value::String(s.clone()),
            json: json_string_quote(s),
        });
    }
    if let Some(n) = arg.number_value {
        let (value, json) = if n.fract() == 0.0 && n.is_finite() && n.abs() < 1e15 {
            let i = n as i64;
            (Value::Number(serde_json::Number::from(i)), i.to_string())
        } else {
            (
                serde_json::Number::from_f64(n)
                    .map(Value::Number)
                    .unwrap_or(Value::Null),
                n.to_string(),
            )
        };
        return Some(ResolvedValue { value, json });
    }
    if let Some(b) = arg.bool_value {
        return Some(ResolvedValue {
            value: Value::Bool(b),
            json: b.to_string(),
        });
    }
    if arg.null_value.is_some() {
        return Some(ResolvedValue {
            value: Value::Null,
            json: "null".into(),
        });
    }
    None
}

fn json_string_quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

fn json_string_escape_inner(s: &str) -> String {
    let quoted = json_string_quote(s);
    quoted[1..quoted.len() - 1].to_string()
}

#[cfg(test)]
#[path = "google_json_accumulator.test.rs"]
mod tests;
