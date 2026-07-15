//! Headless (`coco -p "<prompt>"`) entry point exposed as a library
//! function so live tests, embeddings, and the binary all drive the
//! same code path.
//!
//! `run_chat` returns a structured [`RunChatOutcome`] instead of
//! printing to stdout. The binary's `main()` thin-wraps this and
//! formats stdout from the outcome.
//!
//! Helpers shared by `run_chat` and noninteractive AppServer runners (`MockModel`,
//! `resolve_main_model`, `cli_runtime_overrides`,
//! `build_runtime_config_for_cli`, `build_system_prompt[_for_model]`,
//! `resolve_startup_permission_state`) live here as well, so a test
//! can drive any of them in isolation.

mod support;

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use coco_inference::{
    AISdkError, LanguageModel, LanguageModelCallOptions, LanguageModelGenerateResult,
    LanguageModelStreamResult,
};
use coco_llm_types::{AssistantContentPart, FinishReason, StopReason, TextPart, Usage};
use coco_messages::CostTracker;
use coco_query::ContinueReason;
use coco_tool_runtime::ToolRegistry;
use coco_types::TokenUsage;
use tokio_util::sync::CancellationToken;

use crate::{
    AgentHostOptions,
    shutdown::{ShutdownCoordinator, ShutdownDrainOutcome},
};
use coco_app_runtime::ProcessRuntime;
pub(crate) use support::resolve_additional_dirs;
pub use support::resolve_additional_dirs_display;
use support::{
    append_headless_goal_status, append_headless_slash_text, build_tool_filter,
    headless_goal_snapshot, headless_local_goal_text_outcome, headless_text_outcome,
    parse_headless_goal_slash, persist_headless_local_transcript_messages, summarize_tool_filter,
};

mod config;
mod mock_model;
mod model_resolution;
mod permission;
mod run;
mod system_prompt;

pub use config::*;
pub use mock_model::*;
pub use model_resolution::*;
pub use permission::*;
pub use run::*;
pub use system_prompt::*;

#[cfg(test)]
#[path = "headless.test.rs"]
mod tests;
