# coco-messages — Crate Plan

TS source: `utils/messages.ts` (5512 LOC, 114 exports), `utils/messages/mappers.ts`, `history.ts`, `cost-tracker.ts`

## Dependencies

```
coco-messages depends on:
  - coco-types    (Message wraps LanguageModelV4Message — vercel-ai types come via coco-types)
  - coco-error
  - serde, serde_json, uuid, chrono

coco-messages does NOT depend on:
  - vercel-ai-provider directly (gets vercel-ai types transitively through coco-types)
  - coco-config   (no model/settings knowledge)
  - coco-inference (no LLM calls)
  - coco-tool     (no tool awareness)
  - any app/ crate
```

## Type Ownership

All message types (Message, UserMessage, AssistantMessage, SystemMessage, StreamingToolUse,
StreamingThinking, TaskBudget) are **defined in coco-types**. This crate provides 114 functions
to create, normalize, filter, and inspect those types. ToolUseGroup is the only type defined locally.

## Function Categories (114 exports from `messages.ts`)

### Category 1: Message Creation (13 functions)

```rust
pub fn create_user_message(params: CreateUserMessageParams) -> UserMessage;
pub fn create_assistant_message(content: Vec<AssistantContentBlock>, usage: Option<TokenUsage>) -> AssistantMessage;
pub fn create_assistant_error_message(content: &str, error: Option<ApiError>) -> AssistantMessage;
pub fn create_user_interruption_message(interrupt_type: InterruptType) -> UserMessage;
pub fn create_synthetic_user_caveat_message(caveat: &str) -> UserMessage;
pub fn create_permission_retry_message(tool_name: &str, reason: &str) -> UserMessage;
pub fn create_bridge_status_message(status: BridgeStatus) -> UserMessage;
pub fn create_tool_result_message(tool_use_id: &str, content: ToolResultContent) -> UserMessage;
pub fn create_compact_boundary_message(info: &CompactBoundaryInfo) -> SystemMessage;
pub fn create_system_reminder_message(content: &str, tier: ReminderTier) -> UserMessage;
pub fn create_progress_message(tool_name: &str, progress: &ToolProgress) -> UserMessage;
pub fn create_cancellation_message(reason: &str) -> UserMessage;
pub fn create_rejection_message(tool_name: &str, reason: &str) -> UserMessage;

pub struct CreateUserMessageParams {
    pub content: MessageContent,  // String or Vec<ContentBlock>
    pub is_meta: bool,
    pub is_virtual: bool,
    pub is_compact_summary: bool,
    pub permission_mode: Option<PermissionMode>,
    pub origin: Option<MessageOrigin>,
}
```

### Category 2: Normalization (10 functions)

```rust
/// 10-step pipeline for API submission:
/// 1. Filter virtual messages (is_virtual = true)
/// 2. Filter progress messages
/// 3. Filter meta-only system messages
/// 4. Strip tool refs for disabled tools
/// 5. Ensure tool result pairing
/// 6. Merge consecutive same-role messages
/// 7. Reorder attachments
/// 8. Strip signature blocks
/// 9. Apply content replacement budgets
/// 10. Final validation
pub fn normalize_messages_for_api(messages: &[Message]) -> Vec<Message>;

pub fn merge_user_messages(messages: &[UserMessage]) -> UserMessage;
pub fn merge_assistant_messages(messages: &[AssistantMessage]) -> AssistantMessage;
pub fn reorder_attachments_for_api(messages: &mut [Message]);
pub fn split_multi_block_messages(messages: &[Message]) -> Vec<Message>;
pub fn strip_images_from_messages(messages: &[Message]) -> Vec<Message>;
pub fn strip_signature_blocks(content: &str) -> String;

/// Since Message already wraps LlmMessage, this is trivial:
///   messages.iter().map(|m| m.message().clone()).collect()
/// Plus prepending SystemPrompt as LlmMessage::System.
/// Uses coco_types re-exports — never raw vercel-ai types.
pub fn to_llm_prompt(
    normalized: &[Message],
    system: &SystemPrompt,
) -> LlmPrompt;  // = Vec<LlmMessage> via coco_types alias
```

### Category 3: Filtering & Cleanup (11 functions)

```rust
pub fn filter_whitespace_only_messages(messages: &[Message]) -> Vec<Message>;
pub fn filter_orphaned_thinking_messages(messages: &[Message]) -> Vec<Message>;
pub fn filter_unresolved_tool_uses(messages: &[Message]) -> Vec<Message>;
pub fn ensure_tool_result_pairing(messages: &[Message]) -> Vec<Message>;
pub fn strip_tool_reference_blocks(messages: &[Message], disabled_tools: &HashSet<String>) -> Vec<Message>;
pub fn remove_empty_content_blocks(message: &mut Message);
pub fn filter_by_kind(messages: &[Message], kind: MessageKind) -> Vec<&Message>;
pub fn filter_meta_messages(messages: &[Message]) -> Vec<Message>;
pub fn filter_virtual_messages(messages: &[Message]) -> Vec<Message>;
pub fn filter_progress_messages(messages: &[Message]) -> Vec<Message>;
pub fn filter_compact_summary_messages(messages: &[Message]) -> Vec<Message>;
```

### Category 4: Reordering (3 functions)

```rust
/// Groups tool_use with pre/post hooks and results for UI display.
/// Tool lifecycle grouping: PreToolUse hooks → tool_use block → tool_result → PostToolUse hooks
pub fn reorder_messages_in_ui(messages: &[Message]) -> Vec<Message>;
pub fn reorder_hooks_with_tool_uses(messages: &[Message]) -> Vec<Message>;
pub fn group_tool_uses_with_results(messages: &[Message]) -> Vec<ToolUseGroup>;
```

### Category 5: Predicates & Inspection (19 functions)

```rust
pub fn is_user_message(msg: &Message) -> bool;
pub fn is_assistant_message(msg: &Message) -> bool;
pub fn is_tool_use_message(msg: &Message) -> bool;
pub fn is_tool_result_message(msg: &Message) -> bool;
pub fn is_meta_message(msg: &Message) -> bool;
pub fn is_virtual_message(msg: &Message) -> bool;
pub fn is_compact_boundary_message(msg: &Message) -> bool;
pub fn is_compact_summary_message(msg: &Message) -> bool;
pub fn is_system_reminder_message(msg: &Message) -> bool;
pub fn is_progress_message(msg: &Message) -> bool;
pub fn is_api_error_message(msg: &Message) -> bool;
pub fn has_tool_calls(msg: &AssistantMessage) -> bool;
pub fn has_thinking_content(msg: &AssistantMessage) -> bool;
pub fn has_text_content(msg: &AssistantMessage) -> bool;
pub fn get_tool_use_ids(msg: &AssistantMessage) -> Vec<String>;
pub fn get_stop_reason(msg: &AssistantMessage) -> Option<StopReason>;
pub fn message_has_content(msg: &Message) -> bool;
pub fn is_interruption_message(msg: &Message) -> bool;
pub fn is_cancellation_message(msg: &Message) -> bool;
```

### Category 6: Merging (4 functions)

```rust
/// Merge adjacent same-role messages (required for Bedrock API compatibility)
pub fn merge_consecutive_messages(messages: &[Message]) -> Vec<Message>;
pub fn merge_content_blocks(blocks: &[ContentBlock]) -> Vec<ContentBlock>;
pub fn can_merge_messages(a: &Message, b: &Message) -> bool;
pub fn merge_two_messages(a: &Message, b: &Message) -> Message;
```

### Category 7: Lookups & Performance (8 functions)

```rust
/// Pre-computed lookup maps for O(1) access to message relationships.
/// Avoids O(n^2) behavior per render cycle.
pub struct MessageLookups {
    pub sibling_tool_use_ids: HashMap<String, Vec<String>>,
    pub tool_result_ids: HashMap<String, String>,
    pub progress_by_tool_use: HashMap<String, Vec<String>>,
    pub hook_by_tool_use: HashMap<String, Vec<String>>,
    pub message_by_uuid: HashMap<String, usize>,
}

pub fn build_message_lookups(messages: &[Message]) -> MessageLookups;
pub fn get_sibling_tool_use_ids(lookups: &MessageLookups, tool_use_id: &str) -> &[String];
pub fn get_tool_result_ids(lookups: &MessageLookups, tool_use_id: &str) -> Option<&str>;
pub fn get_last_assistant_message(messages: &[Message]) -> Option<&AssistantMessage>;
pub fn has_tool_calls_in_last_turn(messages: &[Message]) -> bool;
pub fn find_message_index_by_uuid(messages: &[Message], uuid: &str) -> Option<usize>;
pub fn find_last_user_message(messages: &[Message]) -> Option<&UserMessage>;
pub fn count_tool_uses_in_messages(messages: &[Message]) -> usize;
```

### Category 8: Stream Processing (1 function)

```rust
/// Process streaming message deltas and update state via callbacks.
/// Handles: text deltas, tool call deltas, thinking deltas, usage updates.
pub fn handle_message_from_stream(
    delta: &StreamDelta,
    state: &mut StreamingMessageState,
    callbacks: &StreamCallbacks,
);

pub struct StreamingMessageState {
    pub current_tool_uses: Vec<StreamingToolUse>,
    pub current_thinking: Option<StreamingThinking>,
    pub accumulated_text: String,
}
```

### Category 9: Compact Boundaries (3 functions)

```rust
/// Context-preservation markers that enable efficient history slicing.
pub fn is_compact_boundary_message(msg: &Message) -> bool;
pub fn find_last_compact_boundary_index(messages: &[Message]) -> Option<usize>;
pub fn create_compact_boundary_info(
    pre_tokens: i64,
    post_tokens: i64,
    summary_model: &str,
) -> CompactBoundaryInfo;
```

### Category 10: Context & Wrapping (3 functions)

```rust
/// Wrap content in <system-reminder> XML tags for model injection
pub fn wrap_in_system_reminder(content: &str) -> String;
pub fn wrap_command_text(text: &str, width: usize) -> String;
pub fn extract_text_from_message(msg: &Message) -> String;
```

### Category 11: Permission Messages (7 constants/functions)

```rust
pub const PERMISSION_DENIED_PREFIX: &str = "Permission denied";
pub const PERMISSION_DENIED_SUFFIX: &str = "The user has denied this action.";
pub fn create_permission_denied_message(tool_name: &str, reason: &str) -> String;
pub fn create_tool_use_error_message(tool_name: &str, error: &str) -> String;
pub fn format_denial_message(tool_name: &str, input: &Value, reason: &str) -> String;
```

### Category 12: Synthetic Messages (3 constants/functions)

```rust
/// Internally-generated messages that never reach the API.
pub const SYNTHETIC_INTERRUPT_MARKER: &str = "[interrupted]";
pub const SYNTHETIC_CANCEL_MARKER: &str = "[cancelled]";
pub fn is_synthetic_message(msg: &Message) -> bool;
```

### Category 13: Utilities (6 functions)

```rust
pub fn derive_short_message_id(uuid: &str) -> String;  // 6-char base36
pub fn extract_text_content(content: &[ContentBlock]) -> String;
pub fn get_message_text(msg: &Message) -> Option<String>;
pub fn truncate_content(content: &str, max_chars: usize) -> String;
pub fn count_content_blocks(msg: &Message) -> usize;
pub fn estimate_message_tokens(msg: &Message) -> i64;
```

### Category 14: Types (re-exported from coco-types)

```rust
// StreamingToolUse and StreamingThinking are defined in coco-types
// (shared across coco-messages, coco-inference, coco-query).
// Re-exported here for convenience.
pub use coco_types::{StreamingToolUse, StreamingThinking};

/// Local to coco-messages — grouping of tool_use + hooks + result for UI display.
pub struct ToolUseGroup {
    pub tool_use_id: String,
    pub pre_hooks: Vec<Message>,
    pub tool_use: Message,
    pub tool_result: Option<Message>,
    pub post_hooks: Vec<Message>,
}
```

### Category 15: SDK Message Conversion (from `messages/mappers.ts`, 6 functions)

```rust
/// Convert internal messages to SDK format for external consumers.
pub fn to_sdk_message(msg: &Message) -> SdkMessage;
pub fn from_sdk_message(sdk_msg: &SdkMessage) -> Message;
pub fn normalize_sdk_content(content: &SdkContent) -> Vec<ContentBlock>;
pub fn to_sdk_text_block(block: &TextContent) -> SdkTextBlock;
pub fn to_sdk_tool_use_block(block: &ToolCallContent) -> SdkToolUseBlock;
pub fn to_sdk_tool_result_block(block: &ToolResultContent) -> SdkToolResultBlock;
```

## History (from `history.ts`)

```rust
pub struct HistoryEntry {
    pub display: String,
    pub timestamp: i64,
    pub project: String,
    pub session_id: Option<String>,
    pub pasted_contents: HashMap<i32, StoredPastedContent>,
}

pub struct StoredPastedContent {
    pub id: i32,
    pub content_type: PasteType,  // Text, Image
    pub content: Option<String>,        // inline for small
    pub content_hash: Option<String>,   // hash ref for large
    pub media_type: Option<String>,
    pub filename: Option<String>,
}

pub fn read_history(project: &str) -> impl Iterator<Item = HistoryEntry>;
pub fn add_to_history(entry: &HistoryEntry);
pub fn expand_pasted_refs(input: &str, pastes: &HashMap<i32, StoredPastedContent>) -> String;
```

## Cost Tracking (from `cost-tracker.ts`)

```rust
pub struct SessionCostState {
    pub total_cost_usd: f64,
    pub total_api_duration_ms: i64,
    pub total_tool_duration_ms: i64,
    pub total_lines_added: i64,
    pub total_lines_removed: i64,
    pub model_usage: HashMap<String, ModelUsage>,  // keyed by canonical model name
}

pub fn get_total_cost() -> f64;
pub fn add_session_cost(cost: f64, usage: &TokenUsage, model: &str);
pub fn format_total_cost() -> String;              // "$0.42 | 12.3K in / 4.5K out"
pub fn save_session_costs(session_id: &str);
pub fn restore_session_costs(session_id: &str) -> Option<SessionCostState>;
```

## Message Pipeline Architecture

```
Creation (13 fns)
    ↓
Normalization (10 fns) — normalize_messages_for_api()
    ↓
UI Reordering (3 fns) — reorder_messages_in_ui()
    ↓
API Preparation — to_language_model_prompt()
    ↓
Validation (via predicates, 19 fns)
```

Key design: `MessageLookups` struct pre-computes relationships once per render cycle,
avoiding O(n^2) scans. Used by TUI for sibling/progress/hook display.
