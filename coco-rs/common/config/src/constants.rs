//! Key constants for the coco configuration system.
//!
//! Ports relevant values from TS `constants/` directory including context
//! windows, token limits, timeouts, and API URLs.

// ---------------------------------------------------------------------------
// Tool concurrency
// ---------------------------------------------------------------------------

/// Maximum number of concurrent tool executions.
pub const DEFAULT_MAX_TOOL_CONCURRENCY: i32 = 8;

// ---------------------------------------------------------------------------
// Context windows (from TS `utils/context.ts`)
// ---------------------------------------------------------------------------

/// Default context window size for all models (200k tokens).
pub const DEFAULT_CONTEXT_WINDOW: i64 = 200_000;

/// Context window for 1M-capable models (Sonnet 4, Opus 4.6).
pub const CONTEXT_WINDOW_1M: i64 = 1_000_000;

// ---------------------------------------------------------------------------
// Output token limits (from TS `utils/context.ts`)
// ---------------------------------------------------------------------------

// Per-model output-token caps live on `ModelInfo` — see
// `model::ModelInfo::max_output_tokens` (baseline) and
// `model::ModelInfo::max_output_tokens_escalate` (opt-in Phase-1
// recovery ceiling). The previous global constants
// (`DEFAULT_MAX_OUTPUT_TOKENS` / `MAX_OUTPUT_TOKENS_DEFAULT` /
// `MAX_OUTPUT_TOKENS_UPPER_LIMIT` / `COMPACT_MAX_OUTPUT_TOKENS` /
// `CAPPED_DEFAULT_MAX_TOKENS` / `ESCALATED_MAX_TOKENS`) were TS
// `claude-code`-era Anthropic-Opus-specific magic numbers that
// would mis-cap on smaller models (GPT-4 4k, Haiku 8k, …). Deleted
// outright — production paths read per-model values via
// `ModelInfo`, and recovery escalate is now per-model opt-in.

// ---------------------------------------------------------------------------
// Timeouts
// ---------------------------------------------------------------------------

/// Default API timeout in seconds.
pub const DEFAULT_API_TIMEOUT_SECS: i64 = 600;

/// Settings watcher debounce in milliseconds.
pub const SETTINGS_WATCHER_DEBOUNCE_MS: u64 = 1000;

/// Default idle notification threshold in milliseconds (1 minute).
pub const DEFAULT_IDLE_NOTIF_THRESHOLD_MS: i64 = 60_000;

// ---------------------------------------------------------------------------
// Auto-compact
// ---------------------------------------------------------------------------

/// Auto-compact threshold (percentage of context window).
pub const DEFAULT_AUTO_COMPACT_PCT: i32 = 90;

// ---------------------------------------------------------------------------
// Tool result limits (from TS `constants/toolLimits.ts`)
// ---------------------------------------------------------------------------

/// Default maximum size in characters for tool results before persisting to
/// disk.
pub const DEFAULT_MAX_RESULT_SIZE_CHARS: i64 = 50_000;

/// Maximum size for tool results in tokens.
pub const MAX_TOOL_RESULT_TOKENS: i64 = 100_000;

/// Bytes per token estimate for calculating token count from byte size.
pub const BYTES_PER_TOKEN: i64 = 4;

/// Maximum size for tool results in bytes (derived from token limit).
pub const MAX_TOOL_RESULT_BYTES: i64 = MAX_TOOL_RESULT_TOKENS * BYTES_PER_TOKEN;

/// Maximum aggregate tool result characters per single user message.
pub const MAX_TOOL_RESULTS_PER_MESSAGE_CHARS: i64 = 200_000;

/// Maximum character length for tool summary strings in compact views.
pub const TOOL_SUMMARY_MAX_LENGTH: i64 = 50;

// ---------------------------------------------------------------------------
// API image/PDF limits (from TS `constants/apiLimits.ts`)
// ---------------------------------------------------------------------------

/// Maximum base64-encoded image size (API enforced, 5 MB).
pub const API_IMAGE_MAX_BASE64_SIZE: i64 = 5 * 1024 * 1024;

/// Target raw image size to stay under base64 limit after encoding (~3.75 MB).
pub const IMAGE_TARGET_RAW_SIZE: i64 = (API_IMAGE_MAX_BASE64_SIZE * 3) / 4;

/// Client-side maximum image dimension for resizing.
pub const IMAGE_MAX_WIDTH: i32 = 2000;

/// Client-side maximum image height for resizing.
pub const IMAGE_MAX_HEIGHT: i32 = 2000;

/// Maximum raw PDF file size for base64 encoding (20 MB).
pub const PDF_TARGET_RAW_SIZE: i64 = 20 * 1024 * 1024;

/// Maximum number of pages in a PDF accepted by the API.
pub const API_PDF_MAX_PAGES: i32 = 100;

/// Size threshold above which PDFs are extracted into page images (3 MB).
pub const PDF_EXTRACT_SIZE_THRESHOLD: i64 = 3 * 1024 * 1024;

/// Maximum PDF file size for the page extraction path (100 MB).
pub const PDF_MAX_EXTRACT_SIZE: i64 = 100 * 1024 * 1024;

/// Max pages the Read tool will extract in a single call.
pub const PDF_MAX_PAGES_PER_READ: i32 = 20;

/// PDFs with more pages than this get reference treatment on @ mention.
pub const PDF_AT_MENTION_INLINE_THRESHOLD: i32 = 10;

/// Maximum number of media items per API request.
pub const API_MAX_MEDIA_PER_REQUEST: i32 = 100;

// ---------------------------------------------------------------------------
// Session tracking
// ---------------------------------------------------------------------------

/// Maximum session cost tracking entries.
pub const MAX_SESSION_COST_ENTRIES: usize = 100;

// ---------------------------------------------------------------------------
// API URLs (from TS `constants/product.ts` and `constants/oauth.ts`)
// ---------------------------------------------------------------------------

/// Product website URL.
pub const PRODUCT_URL: &str = "https://claude.com/claude-code";

/// Anthropic API base URL (production).
pub const ANTHROPIC_API_BASE_URL: &str = "https://api.anthropic.com";

/// Claude.ai base URL (production).
pub const CLAUDE_AI_BASE_URL: &str = "https://claude.ai";

/// OAuth token endpoint (production).
pub const OAUTH_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";

/// OAuth authorize URL (production, console).
pub const OAUTH_AUTHORIZE_URL: &str = "https://platform.claude.com/oauth/authorize";

// ---------------------------------------------------------------------------
// Product / paths
// ---------------------------------------------------------------------------

/// Product name for paths and display.
pub const PRODUCT_NAME: &str = "coco";

/// Config directory name.
pub const CONFIG_DIR_NAME: &str = ".coco";

/// Project config directory name.
pub const PROJECT_CONFIG_DIR: &str = ".coco";

/// No-content message placeholder.
pub const NO_CONTENT_MESSAGE: &str = "(no content)";

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "constants.test.rs"]
mod tests;
