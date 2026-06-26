//! Locked-schema drift tripwire for every default-registered built-in tool.
//!
//! Ported from opencode's `tool-edit.test.ts` / `tool-bash.test.ts` source
//! guards, which freeze each tool's JSON input-schema property set so a silent
//! schema change fails CI. coco-rs is mid-port of Claude Code parity, so a
//! tool gaining/losing/renaming an input field — the model-facing wire
//! contract — must be an intentional, reviewed change, never an accident.
//!
//! This snapshots the `{properties, required}` of all 40 default tools in ONE
//! golden file. A diff names exactly which tool's schema moved. Regenerate an
//! intentional change with:
//!   INSTA_UPDATE=always cargo test -p coco-tools --test tool_schemas_frozen
//! then review the snapshot diff before committing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use coco_tool_runtime::Tool;

/// Reduce a tool's full JSON Schema to the drift-sensitive surface: the sorted
/// property names and the sorted `required` set. We deliberately do NOT pin
/// descriptions or types here — those churn often and have their own coverage;
/// the property/required *set* is the wire contract the model binds to.
fn schema_summary(schema: &serde_json::Value) -> serde_json::Value {
    let mut properties: Vec<String> = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    properties.sort();

    let mut required: Vec<String> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    required.sort();

    serde_json::json!({ "properties": properties, "required": required })
}

fn insert<T: Tool>(map: &mut BTreeMap<String, serde_json::Value>, tool: T) {
    map.insert(
        tool.name().to_string(),
        schema_summary(tool.runtime_validation_schema().as_value()),
    );
}

#[test]
fn default_tool_input_schemas_are_frozen() {
    use coco_tools::*;

    let mut map: BTreeMap<String, serde_json::Value> = BTreeMap::new();

    // File I/O
    insert(&mut map, BashTool);
    insert(&mut map, ReadTool);
    insert(&mut map, WriteTool);
    insert(&mut map, EditTool);
    insert(&mut map, GlobTool);
    insert(&mut map, GrepTool);
    insert(&mut map, NotebookEditTool);
    insert(&mut map, ApplyPatchTool);

    // Web
    insert(&mut map, WebFetchTool);
    insert(&mut map, WebSearchTool);

    // Agent, Workflow & Team
    insert(&mut map, AgentTool);
    insert(&mut map, WorkflowTool);
    insert(&mut map, SkillTool);
    insert(&mut map, SendMessageTool);

    // Task Management
    insert(&mut map, TaskCreateTool);
    insert(&mut map, TaskGetTool);
    insert(&mut map, TaskListTool);
    insert(&mut map, TaskUpdateTool);
    insert(&mut map, TaskStopTool);
    insert(&mut map, TaskOutputTool);
    insert(&mut map, TodoWriteTool);

    // Plan & Worktree
    insert(&mut map, EnterPlanModeTool);
    insert(&mut map, ExitPlanModeTool);
    insert(&mut map, EnterWorktreeTool);
    insert(&mut map, ExitWorktreeTool);

    // Utility
    insert(&mut map, AskUserQuestionTool);
    insert(&mut map, ToolSearchTool);
    insert(&mut map, ConfigTool);
    insert(&mut map, SendUserMessageTool);
    insert(&mut map, LspTool);

    // MCP management
    insert(&mut map, McpAuthTool);
    insert(&mut map, ListMcpResourcesTool);
    insert(&mut map, ReadMcpResourceTool);

    // Scheduling
    insert(&mut map, CronCreateTool);
    insert(&mut map, CronDeleteTool);
    insert(&mut map, CronListTool);
    insert(&mut map, ScheduleWakeupTool);
    insert(&mut map, MonitorTool);
    insert(&mut map, RemoteTriggerTool);

    // Shell
    insert(&mut map, PowerShellTool);
    insert(&mut map, ReplTool);
    insert(&mut map, SleepTool);

    insta::assert_json_snapshot!("default_tool_input_schemas", map);
}
