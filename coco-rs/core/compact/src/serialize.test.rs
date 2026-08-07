use super::*;
use crate::types::ClearToolInputs;
use crate::types::ContextEditStrategy;
use crate::types::ThinkingKeep;
use crate::types::ToolUseKeep;
use coco_types::ToolName;

#[test]
fn empty_strategies_return_none() {
    assert!(encode_anthropic_context_management(&[]).is_none());
}

#[test]
fn clear_thinking_all_emits_string_keep() {
    let strategies = vec![ContextEditStrategy::ClearThinking {
        keep: ThinkingKeep::All,
    }];
    let v = encode_anthropic_context_management(&strategies).unwrap();
    let edit = &v["edits"][0];
    assert_eq!(edit["type"], "clear_thinking_20251015");
    assert_eq!(edit["keep"], "all");
}

#[test]
fn clear_thinking_recent_emits_object_keep() {
    let strategies = vec![ContextEditStrategy::ClearThinking {
        keep: ThinkingKeep::Recent { turns: 3 },
    }];
    let v = encode_anthropic_context_management(&strategies).unwrap();
    let keep = &v["edits"][0]["keep"];
    assert_eq!(keep["type"], "thinking_turns");
    assert_eq!(keep["value"], 3);
}

#[test]
fn clear_tool_uses_with_specific_tools_emits_camelcase() {
    let strategies = vec![ContextEditStrategy::ClearToolUses {
        trigger: Some(150_000),
        keep_recent: Some(ToolUseKeep { value: 5 }),
        clear_at_least: None,
        clear_inputs: ClearToolInputs::SpecificTools(vec![ToolName::Read, ToolName::Bash]),
        exclude_tools: vec![],
        exclude_tool_strs: vec![],
    }];
    let v = encode_anthropic_context_management(&strategies).unwrap();
    let edit = &v["edits"][0];
    assert_eq!(edit["type"], "clear_tool_uses_20250919");
    assert_eq!(edit["trigger"]["type"], "input_tokens");
    assert_eq!(edit["trigger"]["value"], 150_000);
    assert_eq!(edit["keep"]["type"], "tool_uses");
    assert_eq!(edit["keep"]["value"], 5);
    let inputs = edit["clearToolInputs"].as_array().unwrap();
    assert!(inputs.iter().any(|v| v == "Read"));
    assert!(inputs.iter().any(|v| v == "Bash"));
}

#[test]
fn clear_tool_uses_with_excludes_emits_camelcase_excludes() {
    let strategies = vec![ContextEditStrategy::ClearToolUses {
        trigger: None,
        keep_recent: None,
        clear_at_least: None,
        clear_inputs: ClearToolInputs::None,
        exclude_tools: vec![ToolName::Edit, ToolName::Write],
        exclude_tool_strs: vec!["mcp__custom".to_string()],
    }];
    let v = encode_anthropic_context_management(&strategies).unwrap();
    let edit = &v["edits"][0];
    let excluded = edit["excludeTools"].as_array().unwrap();
    assert_eq!(excluded.len(), 3);
    assert!(excluded.iter().any(|v| v == "Edit"));
    assert!(excluded.iter().any(|v| v == "Write"));
    assert!(excluded.iter().any(|v| v == "mcp__custom"));
    assert_eq!(edit["clearToolInputs"], false);
}

#[test]
fn clear_inputs_all_emits_true() {
    let strategies = vec![ContextEditStrategy::ClearToolUses {
        trigger: None,
        keep_recent: None,
        clear_at_least: None,
        clear_inputs: ClearToolInputs::All,
        exclude_tools: vec![],
        exclude_tool_strs: vec![],
    }];
    let v = encode_anthropic_context_management(&strategies).unwrap();
    assert_eq!(v["edits"][0]["clearToolInputs"], true);
}

#[test]
fn clear_at_least_emits_camelcase_input_tokens() {
    let strategies = vec![ContextEditStrategy::ClearToolUses {
        trigger: Some(180_000),
        keep_recent: None,
        clear_at_least: Some(140_000),
        clear_inputs: ClearToolInputs::None,
        exclude_tools: vec![],
        exclude_tool_strs: vec![],
    }];
    let v = encode_anthropic_context_management(&strategies).unwrap();
    let edit = &v["edits"][0];
    assert_eq!(edit["clearAtLeast"]["type"], "input_tokens");
    assert_eq!(edit["clearAtLeast"]["value"], 140_000);
}

#[test]
fn combined_strategies_preserve_order() {
    let strategies = vec![
        ContextEditStrategy::ClearThinking {
            keep: ThinkingKeep::All,
        },
        ContextEditStrategy::ClearToolUses {
            trigger: Some(180_000),
            keep_recent: None,
            clear_at_least: None,
            clear_inputs: ClearToolInputs::None,
            exclude_tools: vec![],
            exclude_tool_strs: vec![],
        },
    ];
    let v = encode_anthropic_context_management(&strategies).unwrap();
    let edits = v["edits"].as_array().unwrap();
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0]["type"], "clear_thinking_20251015");
    assert_eq!(edits[1]["type"], "clear_tool_uses_20250919");
}
