//! `/claude-api` — reference for building applications with the Claude API or Anthropic SDK. Mirrors claude-code's claudeApi.ts.
//! Deferred: the full claude-api doc tree (~247KB of bundled .md, vendored via the files/extraction seam) is not yet ported.

pub const PROMPT: &str = r#"You are helping the user build applications with the Claude API or Anthropic SDK.

TRIGGER when: code imports `anthropic`/`@anthropic-ai/sdk`/`claude_agent_sdk`, or user asks to use Claude API, Anthropic SDKs, or Agent SDK.
DO NOT TRIGGER when: code imports `openai`/other AI SDK, general coding questions unrelated to Claude/Anthropic.

## Key Principles

1. **Use the latest API version** - Always use the most recent Claude models and API features
2. **Prefer streaming** for better UX - Use `client.messages.stream()` for real-time responses
3. **Handle errors properly** - Implement retry logic with exponential backoff for rate limits
4. **Token management** - Be mindful of context window limits and token usage

## Claude API Basics

### Authentication
- Set `ANTHROPIC_API_KEY` environment variable or pass to client constructor
- Never hardcode API keys in source code

### Messages API
The Messages API is the primary interface:
- `POST /v1/messages` - Create a message
- Required: `model`, `max_tokens`, `messages`
- Optional: `system`, `temperature`, `tools`, `stream`

### Tool Use
Claude supports tool use (function calling):
- Define tools with JSON Schema input descriptions
- Claude returns `tool_use` content blocks
- Send `tool_result` back in the next turn

### Streaming
For real-time responses:
- Use `stream: true` in the request
- Handle Server-Sent Events (SSE)
- Events: `message_start`, `content_block_start`, `content_block_delta`, `message_stop`

## Common Patterns

### Multi-turn Conversation
Maintain conversation history by accumulating messages array.

### System Prompts
Use the `system` parameter (not a system message in the messages array).

### Extended Thinking
Enable with `thinking: { type: "enabled", budget_tokens: N }` for complex reasoning.

## Common Pitfalls
- Don't put system prompts in the messages array
- Don't forget to handle rate limit errors (429)
- Don't exceed max_tokens without checking stop_reason
- Always check stop_reason to know if the response was truncated

When helping the user, read their existing code first to understand the language and framework they're using, then provide targeted guidance.
"#;
