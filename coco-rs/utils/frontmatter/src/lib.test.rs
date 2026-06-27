//! Tests for frontmatter parser.

use crate::FrontmatterValue;
use crate::parse;

#[test]
fn test_basic_frontmatter() {
    let input = "---\ntitle: Hello World\nauthor: Claude\n---\n# Body content";
    let fm = parse(input);
    assert_eq!(fm.data.get("title").unwrap().as_str(), Some("Hello World"));
    assert_eq!(fm.data.get("author").unwrap().as_str(), Some("Claude"));
    assert_eq!(fm.content.trim(), "# Body content");
}

#[test]
fn test_no_frontmatter() {
    let input = "# Just a heading\nSome text";
    let fm = parse(input);
    assert!(fm.data.is_empty());
    assert_eq!(fm.content, input);
}

#[test]
fn test_bool_values() {
    let input = "---\nenabled: true\ndisabled: false\n---\nbody";
    let fm = parse(input);
    assert_eq!(fm.data.get("enabled").unwrap().as_bool(), Some(true));
    assert_eq!(fm.data.get("disabled").unwrap().as_bool(), Some(false));
}

#[test]
fn test_null_value() {
    let input = "---\nkey:\n---\nbody";
    let fm = parse(input);
    assert_eq!(fm.data.get("key"), Some(&FrontmatterValue::Null));
}

#[test]
fn test_integer_value() {
    let input = "---\ncount: 42\n---\nbody";
    let fm = parse(input);
    assert_eq!(fm.data.get("count"), Some(&FrontmatterValue::Int(42)));
}

#[test]
fn test_quoted_string() {
    let input = "---\nname: \"quoted value\"\n---\nbody";
    let fm = parse(input);
    assert_eq!(fm.data.get("name").unwrap().as_str(), Some("quoted value"));
}

#[test]
fn test_list_values() {
    let input = "---\nallowed-tools:\n- Read\n- Write\n- Bash\n---\nbody";
    let fm = parse(input);
    let tools = fm
        .data
        .get("allowed-tools")
        .unwrap()
        .as_string_list()
        .unwrap();
    assert_eq!(tools, vec!["Read", "Write", "Bash"]);
}

#[test]
fn test_no_closing_delimiter() {
    let input = "---\nkey: val\nno closing delimiter";
    let fm = parse(input);
    assert!(fm.data.is_empty());
}

#[test]
fn test_nested_mapping() {
    // serde_yml backing — supports nested objects.
    let input = r#"---
hooks:
  PreToolUse:
    - matcher: Bash
      command: ./hook.sh
config:
  nested:
    deeper: value
---
body"#;
    let fm = parse(input);
    let hooks = fm.data.get("hooks").unwrap().as_mapping().unwrap();
    assert!(hooks.contains_key("PreToolUse"));
    let pre_tool_use = hooks.get("PreToolUse").unwrap().as_sequence().unwrap();
    assert_eq!(pre_tool_use.len(), 1);
    let entry = pre_tool_use[0].as_mapping().unwrap();
    assert_eq!(entry.get("matcher").unwrap().as_str(), Some("Bash"));
    assert_eq!(entry.get("command").unwrap().as_str(), Some("./hook.sh"));

    let config = fm.data.get("config").unwrap().as_mapping().unwrap();
    let deeper = config
        .get("nested")
        .unwrap()
        .as_mapping()
        .unwrap()
        .get("deeper")
        .unwrap();
    assert_eq!(deeper.as_str(), Some("value"));
}

#[test]
fn test_inline_mapping_in_sequence() {
    // `mcpServers` inline form: `[{name: {…}}]`.
    let input = r#"---
mcpServers:
  - github
  - slack:
      command: ./slack-mcp
      env:
        TOKEN: xyz
---
body"#;
    let fm = parse(input);
    let servers = fm.data.get("mcpServers").unwrap().as_sequence().unwrap();
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[0].as_str(), Some("github"));
    let inline = servers[1].as_mapping().unwrap();
    let slack = inline.get("slack").unwrap().as_mapping().unwrap();
    assert_eq!(slack.get("command").unwrap().as_str(), Some("./slack-mcp"));
}

#[test]
fn test_to_json_round_trip() {
    let input = r#"---
hooks:
  PreToolUse:
    - command: ./hook.sh
      timeout: 30
---
body"#;
    let fm = parse(input);
    let hooks_json = fm.data.get("hooks").unwrap().to_json();
    let expected = serde_json::json!({
        "PreToolUse": [
            {"command": "./hook.sh", "timeout": 30}
        ]
    });
    assert_eq!(hooks_json, expected);
}

#[test]
fn test_empty_body() {
    let input = "---\nkey: val\n---\n";
    let fm = parse(input);
    assert_eq!(fm.data.get("key").unwrap().as_str(), Some("val"));
    assert!(fm.content.trim().is_empty());
}

#[test]
fn test_empty_frontmatter_block() {
    // `---\n---\n` with no keys: regression for the case where the
    // closing fence sits flush against the opener and the simple
    // `find("\n---")` lookup missed it.
    let input = "---\n---\n\nbody text";
    let fm = parse(input);
    assert!(fm.data.is_empty());
    assert_eq!(fm.content.trim(), "body text");
}

#[test]
fn test_yaml_special_chars_auto_quoted() {
    // Values containing YAML-special chars like `*`, `{`, `}` parse as
    // strings via the auto-quote retry path rather than blowing up the
    // whole frontmatter.
    let input = "---\npaths: *.{ts,tsx}, src/**/*.{js,jsx}\nname: glob-skill\n---\nbody";
    let fm = parse(input);
    assert_eq!(fm.data.get("name").unwrap().as_str(), Some("glob-skill"));
    assert_eq!(
        fm.data.get("paths").unwrap().as_str(),
        Some("*.{ts,tsx}, src/**/*.{js,jsx}")
    );
}

#[test]
fn test_skill_frontmatter() {
    let input = r#"---
description: Review changed code
allowed-tools:
- Read
- Grep
- Glob
model: sonnet
user-invocable: true
---
Review the code changes and suggest improvements.
"#;
    let fm = parse(input);
    assert_eq!(
        fm.data.get("description").unwrap().as_str(),
        Some("Review changed code")
    );
    assert_eq!(fm.data.get("model").unwrap().as_str(), Some("sonnet"));
    assert_eq!(fm.data.get("user-invocable").unwrap().as_bool(), Some(true));
    let tools = fm
        .data
        .get("allowed-tools")
        .unwrap()
        .as_string_list()
        .unwrap();
    assert_eq!(tools, vec!["Read", "Grep", "Glob"]);
    assert!(fm.content.contains("Review the code changes"));
}
