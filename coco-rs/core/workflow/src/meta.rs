use serde_json::Map;
use serde_json::Value;
use tree_sitter::Node;
use tree_sitter::Parser;

use crate::InvalidMetaSnafu;
use crate::MissingMetaSnafu;
use crate::NondeterministicApiSnafu;
use crate::Result;
use crate::SyntaxSnafu;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowPhaseMeta {
    pub title: String,
    pub detail: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowMeta {
    pub name: String,
    pub description: String,
    pub title: Option<String>,
    pub when_to_use: Option<String>,
    pub phases: Vec<WorkflowPhaseMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowScript {
    pub meta: WorkflowMeta,
    pub script_body: String,
}

pub fn parse_workflow_meta(source: &str) -> Result<WorkflowMeta> {
    parse_workflow_script(source, true).map(|script| script.meta)
}

pub fn parse_workflow_script(source: &str, check_determinism: bool) -> Result<WorkflowScript> {
    let tree = parse_typescript(source)?;
    let root = tree.root_node();
    if root.has_error() || has_typescript_only_syntax(root) {
        return SyntaxSnafu.fail();
    }
    if check_determinism {
        reject_nondeterministic_apis(root, source)?;
    }

    let first = first_named_child(root).ok_or_else(|| MissingMetaSnafu.build())?;
    let object = meta_object_node(first, source)?;
    let value = eval_literal(object, source)?;
    let meta = workflow_meta_from_value(value)?;
    let script_body = format!(
        "{}{}",
        source.get(..first.start_byte()).unwrap_or_default(),
        source.get(first.end_byte()..).unwrap_or_default()
    );
    Ok(WorkflowScript { meta, script_body })
}

fn parse_typescript(source: &str) -> Result<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .map_err(|_| SyntaxSnafu.build())?;
    parser
        .parse(source, None)
        .ok_or_else(|| SyntaxSnafu.build())
}

fn has_typescript_only_syntax(root: Node<'_>) -> bool {
    let mut found = false;
    visit(root, &mut |node| {
        if found {
            return;
        }
        found = matches!(
            node.kind(),
            "type_annotation"
                | "type_alias_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "ambient_declaration"
                | "internal_module"
                | "module"
                | "namespace_export_declaration"
                | "as_expression"
                | "satisfies_expression"
                | "instantiation_expression"
                | "abstract_class_declaration"
                | "accessibility_modifier"
                | "override_modifier"
                | "decorator"
        );
    });
    found
}

fn first_named_child(root: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = root.walk();
    root.named_children(&mut cursor).next()
}

/// Resolve `export const meta = <object>` to its object-literal node, requiring
/// the exact TS `isMetaExport` shape: a `const` lexical declaration with a
/// single declarator named `meta` whose initializer is an object literal.
/// `export let/var meta` and multi-declarator forms are rejected.
fn meta_object_node<'a>(statement: Node<'a>, source: &str) -> Result<Node<'a>> {
    if statement.kind() != "export_statement" {
        return MissingMetaSnafu.fail();
    }
    let Some(declaration) = statement.child_by_field_name("declaration") else {
        return MissingMetaSnafu.fail();
    };
    if declaration.kind() != "lexical_declaration"
        || !lexical_declaration_is_const(declaration, source)
    {
        return MissingMetaSnafu.fail();
    }
    let mut cursor = declaration.walk();
    let mut declarators = declaration
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "variable_declarator");
    let (Some(declarator), None) = (declarators.next(), declarators.next()) else {
        return MissingMetaSnafu.fail();
    };
    let Some(name) = declarator.child_by_field_name("name") else {
        return MissingMetaSnafu.fail();
    };
    if name.kind() != "identifier" || node_text(name, source) != "meta" {
        return MissingMetaSnafu.fail();
    }
    let Some(value) = declarator.child_by_field_name("value") else {
        return MissingMetaSnafu.fail();
    };
    if value.kind() != "object" {
        return InvalidMetaSnafu {
            message: "meta must be an object literal",
        }
        .fail();
    }
    Ok(value)
}

fn lexical_declaration_is_const(declaration: Node<'_>, source: &str) -> bool {
    let mut cursor = declaration.walk();
    declaration
        .children(&mut cursor)
        .next()
        .is_some_and(|first| node_text(first, source) == "const")
}

fn eval_literal(node: Node<'_>, source: &str) -> Result<Value> {
    match node.kind() {
        "object" => eval_object(node, source),
        "array" => eval_array(node, source),
        "string" => Ok(Value::String(unquote_string(node_text(node, source))?)),
        "template_string" => eval_template_string(node, source),
        "number" => eval_number(node_text(node, source)),
        "true" => Ok(Value::Bool(true)),
        "false" => Ok(Value::Bool(false)),
        "null" => Ok(Value::Null),
        other => InvalidMetaSnafu {
            message: format!("non-literal value `{other}`"),
        }
        .fail(),
    }
}

fn eval_object(node: Node<'_>, source: &str) -> Result<Value> {
    let mut map = Map::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "pair" {
            return InvalidMetaSnafu {
                message: format!("unsupported object member `{}`", child.kind()),
            }
            .fail();
        }
        let Some(key_node) = child.child_by_field_name("key") else {
            return InvalidMetaSnafu {
                message: "object member is missing a key",
            }
            .fail();
        };
        let key = eval_key(key_node, source)?;
        if matches!(key.as_str(), "__proto__" | "constructor" | "prototype") {
            return InvalidMetaSnafu {
                message: format!("reserved key `{key}`"),
            }
            .fail();
        }
        let Some(value_node) = child.child_by_field_name("value") else {
            return InvalidMetaSnafu {
                message: format!("object member `{key}` is missing a value"),
            }
            .fail();
        };
        map.insert(key, eval_literal(value_node, source)?);
    }
    Ok(Value::Object(map))
}

fn eval_array(node: Node<'_>, source: &str) -> Result<Value> {
    let mut values = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        values.push(eval_literal(child, source)?);
    }
    Ok(Value::Array(values))
}

fn eval_key(node: Node<'_>, source: &str) -> Result<String> {
    match node.kind() {
        "property_identifier" | "identifier" => Ok(node_text(node, source).to_string()),
        "string" => unquote_string(node_text(node, source)),
        other => InvalidMetaSnafu {
            message: format!("unsupported key `{other}`"),
        }
        .fail(),
    }
}

fn eval_template_string(node: Node<'_>, source: &str) -> Result<Value> {
    // Reject only `${...}` interpolations — escape sequences are allowed (and
    // cooked below). The named children of a template string include
    // `escape_sequence` nodes, so checking for any named child would wrongly
    // reject a backtick literal that merely contains `\n`.
    let mut cursor = node.walk();
    if node
        .named_children(&mut cursor)
        .any(|child| child.kind() == "template_substitution")
    {
        return InvalidMetaSnafu {
            message: "template strings with substitutions are not literal",
        }
        .fail();
    }
    let raw = node_text(node, source);
    let body = raw
        .strip_prefix('`')
        .and_then(|s| s.strip_suffix('`'))
        .unwrap_or(raw);
    Ok(Value::String(cook_js_string(body)?))
}

fn eval_number(raw: &str) -> Result<Value> {
    let number = raw.parse::<serde_json::Number>().map_err(|_| {
        InvalidMetaSnafu {
            message: format!("invalid number `{raw}`"),
        }
        .build()
    })?;
    Ok(Value::Number(number))
}

fn workflow_meta_from_value(value: Value) -> Result<WorkflowMeta> {
    let Value::Object(mut object) = value else {
        return InvalidMetaSnafu {
            message: "meta must be an object",
        }
        .fail();
    };
    let name = required_string(&mut object, "name")?;
    let description = required_string(&mut object, "description")?;
    let title = optional_nonempty_string(&mut object, "title");
    let when_to_use = optional_any_string(&mut object, "whenToUse");
    let phases = normalize_phases(object.remove("phases"))?;
    Ok(WorkflowMeta {
        name,
        description,
        title,
        when_to_use,
        phases,
    })
}

/// Required string field (`name` / `description`): must be a string with
/// length > 0. Mirrors TS `validateMetaFields`, which rejects only `length ===
/// 0` — a whitespace-only value is accepted.
fn required_string(object: &mut Map<String, Value>, key: &str) -> Result<String> {
    let Some(value) = object.remove(key) else {
        return InvalidMetaSnafu {
            message: format!("meta.{key} is required"),
        }
        .fail();
    };
    let Value::String(value) = value else {
        return InvalidMetaSnafu {
            message: format!("meta.{key} must be a string"),
        }
        .fail();
    };
    if value.is_empty() {
        return InvalidMetaSnafu {
            message: format!("meta.{key} must be non-empty"),
        }
        .fail();
    }
    Ok(value)
}

/// Optional `title`: kept only when a non-empty string, else dropped. A
/// non-string value is silently dropped (TS does not error), matching
/// `typeof title === 'string' && title.length > 0 ? title : undefined`.
fn optional_nonempty_string(object: &mut Map<String, Value>, key: &str) -> Option<String> {
    match object.remove(key) {
        Some(Value::String(value)) if !value.is_empty() => Some(value),
        _ => None,
    }
}

/// Optional `whenToUse`: kept when any string (empty allowed), else dropped —
/// matching `typeof whenToUse === 'string' ? whenToUse : undefined`.
fn optional_any_string(object: &mut Map<String, Value>, key: &str) -> Option<String> {
    match object.remove(key) {
        Some(Value::String(value)) => Some(value),
        _ => None,
    }
}

/// Normalize the optional `phases` field, mirroring TS `normalizePhases` (W0p):
/// anything that isn't an array drops to empty (never an error); each element
/// is kept only when it is an object with a string `title`; string entries and
/// missing/non-string titles are silently skipped; `detail`/`model` are kept
/// only when strings.
fn normalize_phases(value: Option<Value>) -> Result<Vec<WorkflowPhaseMeta>> {
    let Some(Value::Array(items)) = value else {
        return Ok(Vec::new());
    };
    let mut phases = Vec::new();
    for item in items {
        let Value::Object(object) = item else {
            continue;
        };
        let Some(Value::String(title)) = object.get("title") else {
            continue;
        };
        phases.push(WorkflowPhaseMeta {
            title: title.clone(),
            detail: optional_str_field(&object, "detail"),
            model: optional_str_field(&object, "model"),
        });
    }
    Ok(phases)
}

fn optional_str_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    match object.get(key) {
        Some(Value::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn reject_nondeterministic_apis(root: Node<'_>, source: &str) -> Result<()> {
    let mut rejected: Option<String> = None;
    visit(root, &mut |node| {
        if rejected.is_some() {
            return;
        }
        let kind = node.kind();
        if kind == "member_expression" {
            // Match `Identifier.Identifier` by NAME (like acorn), not by raw
            // node text: the object must be a plain identifier and the property
            // a plain property identifier. This flags `Date?.now`, `Date . now`
            // and `Date./*c*/now`, while ignoring `Date["now"]` (a subscript
            // expression) and `x.Date.now` (object is itself a member access).
            let (Some(object), Some(property)) = (
                node.child_by_field_name("object"),
                node.child_by_field_name("property"),
            ) else {
                return;
            };
            if object.kind() != "identifier" || property.kind() != "property_identifier" {
                return;
            }
            let object = node_text(object, source);
            let property = node_text(property, source);
            if (object == "Date" && property == "now") || (object == "Math" && property == "random")
            {
                rejected = Some(format!("{object}.{property}"));
            }
            return;
        }
        if kind == "new_expression"
            && let Some(constructor) = node.child_by_field_name("constructor")
            && constructor.kind() == "identifier"
            && node_text(constructor, source) == "Date"
            && new_expression_argument_count(node) == 0
        {
            rejected = Some("new Date".to_string());
        }
    });
    if let Some(api) = rejected {
        return NondeterministicApiSnafu { api }.fail();
    }
    Ok(())
}

fn new_expression_argument_count(node: Node<'_>) -> usize {
    let mut count = 0;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "arguments" {
            count = child.named_child_count();
        }
    }
    count
}

fn visit<F>(node: Node<'_>, f: &mut F)
where
    F: FnMut(Node<'_>),
{
    f(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit(child, f);
    }
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    source.get(node.byte_range()).unwrap_or_default()
}

fn unquote_string(raw: &str) -> Result<String> {
    let quote = raw.chars().next().ok_or_else(|| {
        InvalidMetaSnafu {
            message: "empty string literal",
        }
        .build()
    })?;
    if quote != '"' && quote != '\'' {
        return InvalidMetaSnafu {
            message: format!("unsupported string literal `{raw}`"),
        }
        .fail();
    }
    if !raw.ends_with(quote) || raw.len() < 2 {
        return InvalidMetaSnafu {
            message: format!("unterminated string literal `{raw}`"),
        }
        .fail();
    }
    let body = &raw[quote.len_utf8()..raw.len() - quote.len_utf8()];
    cook_js_string(body)
}

/// Decode a JS string/template body (the text between the quotes/backticks)
/// with JavaScript escape semantics, matching acorn's `cooked` value — so
/// idiomatic JS escapes valid in claude-code (`\'`, `\xNN`, `\v`, line
/// continuations, `\<other>` → `<other>`) cook correctly instead of being
/// rejected by JSON escape rules.
fn cook_js_string(body: &str) -> Result<String> {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(esc) = chars.next() else {
            return InvalidMetaSnafu {
                message: "dangling escape in string literal",
            }
            .fail();
        };
        match esc {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'b' => out.push('\u{0008}'),
            'f' => out.push('\u{000C}'),
            'v' => out.push('\u{000B}'),
            '0' => out.push('\0'),
            // Line continuation: `\<LF>` and `\<CRLF>` produce nothing.
            '\n' => {}
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            'x' => out.push(read_hex_escape(&mut chars, 2)?),
            'u' => out.push(read_unicode_escape(&mut chars)?),
            // `\'`, `\"`, `` \` ``, `\\`, `\/`, and any other escaped char are
            // the literal character.
            other => out.push(other),
        }
    }
    Ok(out)
}

fn read_hex_escape(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    len: usize,
) -> Result<char> {
    let mut value = 0u32;
    for _ in 0..len {
        let Some(digit) = chars.next().and_then(|c| c.to_digit(16)) else {
            return InvalidMetaSnafu {
                message: "invalid hex escape in string literal",
            }
            .fail();
        };
        value = value * 16 + digit;
    }
    char::from_u32(value).ok_or_else(|| {
        InvalidMetaSnafu {
            message: "invalid code point in string literal",
        }
        .build()
    })
}

fn read_unicode_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Result<char> {
    if chars.peek() != Some(&'{') {
        return read_hex_escape(chars, 4);
    }
    chars.next(); // consume '{'
    let mut value = 0u32;
    let mut digits = 0;
    loop {
        match chars.next() {
            Some('}') => break,
            Some(c) => {
                let Some(digit) = c.to_digit(16) else {
                    return InvalidMetaSnafu {
                        message: "invalid \\u{...} escape in string literal",
                    }
                    .fail();
                };
                value = value * 16 + digit;
                digits += 1;
                if value > 0x10_FFFF {
                    return InvalidMetaSnafu {
                        message: "\\u{...} code point out of range",
                    }
                    .fail();
                }
            }
            None => {
                return InvalidMetaSnafu {
                    message: "unterminated \\u{...} escape in string literal",
                }
                .fail();
            }
        }
    }
    if digits == 0 {
        return InvalidMetaSnafu {
            message: "empty \\u{} escape in string literal",
        }
        .fail();
    }
    char::from_u32(value).ok_or_else(|| {
        InvalidMetaSnafu {
            message: "invalid code point in string literal",
        }
        .build()
    })
}
