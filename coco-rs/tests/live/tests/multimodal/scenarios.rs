//! End-to-end multimodal scenarios — drive a real `QueryEngine`
//! against `coco_tools::ReadTool` / `coco_tools::BashTool` with a
//! capturing scripted provider, then inspect the engine-assembled
//! prompt the (mocked) provider received on the post-tool turn.
//!
//! These exercise the **full** upstream pipeline:
//!
//! ```text
//!  user prompt
//!     ↓
//!  Turn 1: model emits Read tool_call
//!     ↓
//!  ToolExecutor → coco_tools::ReadTool::execute (real disk read)
//!     ↓
//!  Tool::render_for_model — projects data into Vec<ToolResultContentPart>
//!     ↓
//!  create_tool_result_message_with_parts (multi-part path)
//!     ↓
//!  MessageHistory + normalize_messages_for_api
//!     ↓
//!  Turn 2: model receives normalized prompt — captured here
//! ```
//!
//! Provider-specific wire conversion (Anthropic image block / OpenAI
//! Chat degradation / OpenAI Responses native input_image) is covered
//! by per-provider unit tests inside each `vercel-ai-*` crate. The
//! seam check forbids `tests/live` from depending on `vercel-ai-*`,
//! so we stop one layer up — at the normalized prompt — and trust
//! those tests to catch wire-shape drift.

use anyhow::Context;
use anyhow::Result;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::LlmMessage;
use coco_llm_types::ToolContentPart;
use coco_llm_types::ToolResultContent as InnerToolResultContent;
use coco_llm_types::ToolResultContentPart;
use serde_json::json;

use crate::multimodal::fixtures;
use crate::multimodal::harness::fresh_workdir;
use crate::multimodal::harness::run_multimodal_scenario;
use crate::multimodal::scripted_model::Reply;

// ── Image flow ────────────────────────────────────────────────────

/// Real PNG round-trip. The scripted model issues a Read on a 1×1 PNG
/// in turn 1, the production ReadTool decodes + re-encodes via the D1
/// pipeline, and the engine builds the post-tool prompt for turn 2.
/// The captured prompt MUST carry an `LlmMessage::Tool` whose
/// `ToolResultContent::Content` value is `[FileData(image/*)]`.
pub async fn read_png_round_trip_via_real_loop() -> Result<()> {
    assert_image_round_trip("photo.png", &fixtures::png_1x1_red()?, "image/").await
}

/// JPEG path. The decoded media type stays `image/*`; downstream
/// providers see real bytes.
pub async fn read_jpeg_round_trip_via_real_loop() -> Result<()> {
    assert_image_round_trip("photo.jpg", &fixtures::jpeg_1x1_red()?, "image/").await
}

/// WebP-with-alpha. The D1 image pipeline downgrades WebP-α to PNG
/// before re-encoding, so we only pin the `image/` prefix — not the
/// exact subtype.
pub async fn read_webp_round_trip_via_real_loop() -> Result<()> {
    assert_image_round_trip("photo.webp", &fixtures::webp_1x1_red()?, "image/").await
}

async fn assert_image_round_trip(
    file_name: &str,
    bytes: &[u8],
    expected_media_prefix: &str,
) -> Result<()> {
    // Pre-mint the workdir so we can bake the absolute path into the
    // scripted Read tool call. ReadTool requires absolute paths —
    // relative paths would resolve against the worker's process cwd,
    // not the engine's `cwd_override`. Mirrors `tui/suite/tool_chain.rs`.
    let workdir = fresh_workdir()?;
    let target = workdir.path().join(file_name);
    std::fs::write(&target, bytes).with_context(|| format!("write {file_name} fixture"))?;
    let abs_path = target.to_string_lossy().into_owned();

    let prompt = format!("Use the Read tool to read \"{abs_path}\". After it returns, reply DONE.");

    let outcome = run_multimodal_scenario(
        workdir,
        vec![
            // Turn 1: emit the Read tool call. The agent loop runs
            // ReadTool against the real image on disk — no mock.
            Reply::tool_call("call_read_1", "Read", json!({ "file_path": abs_path })),
            // Turn 2: see the tool result, finish the turn.
            Reply::text("DONE"),
        ],
        &prompt,
    )
    .await?;

    assert_eq!(
        outcome.model.call_count(),
        2,
        "expected 2 LLM calls (pre + post tool), got {}",
        outcome.model.call_count()
    );

    let prompts = outcome.model.captured_prompts();
    assert_eq!(
        prompts.len(),
        2,
        "expected 2 captured prompts, got {prompts:?}"
    );

    // Turn-1 prompt has only the user message — no tool result yet.
    assert!(
        !contains_tool_result_with_filedata(&prompts[0]),
        "turn-1 prompt should NOT yet contain a tool result; \
         got {} messages",
        prompts[0].len()
    );

    // Turn-2 prompt MUST carry the multimodal tool result.
    let (media_type, data_len) = find_image_filedata(&prompts[1])
        .context("turn-2 prompt missing FileData(image/*) tool result")?;
    assert!(
        media_type.starts_with(expected_media_prefix),
        "turn-2 FileData media_type {media_type:?} must start with {expected_media_prefix:?}",
    );
    assert!(
        data_len > 0,
        "turn-2 FileData base64 must be non-empty (got {data_len} bytes)",
    );

    // Sanity: the assistant response that came out of turn 2 carries
    // the literal "DONE" (proves the model's second reply landed).
    assert!(
        outcome.result.response_text.contains("DONE"),
        "expected final reply to contain 'DONE', got: {}",
        outcome.result.response_text
    );
    Ok(())
}

// ── Text fast path: singleton-Text engages Level-1 persistence ────

/// A text-only Read MUST take the singleton-Text fast path so
/// `tool_outcome_builder` keeps using `create_tool_result_message`
/// (with Level-1 persistence) instead of the multi-part sibling.
/// Captured-prompt assertion: the post-tool LlmMessage::Tool's output
/// is `ToolResultContent::Text { value: <cat output> }`, NOT
/// `ToolResultContent::Content`.
pub async fn read_text_uses_text_fast_path_in_real_loop() -> Result<()> {
    let workdir = fresh_workdir()?;
    let target = workdir.path().join("hello.txt");
    std::fs::write(&target, "line one\nline two\n").context("write hello.txt fixture")?;
    let abs_path = target.to_string_lossy().into_owned();

    let outcome = run_multimodal_scenario(
        workdir,
        vec![
            Reply::tool_call(
                "call_read_text",
                "Read",
                json!({ "file_path": abs_path.clone() }),
            ),
            Reply::text("DONE"),
        ],
        &format!("Use Read on {abs_path}. Then reply DONE."),
    )
    .await?;

    let prompts = outcome.model.captured_prompts();
    let turn2 = prompts.get(1).context("missing turn-2 prompt")?;
    let tool_result = find_tool_result(turn2).context("turn-2 missing tool result")?;

    match &tool_result.output {
        InnerToolResultContent::Text { value, .. } => {
            assert!(
                value.contains("1\tline one") && value.contains("2\tline two"),
                "fast-path Text must carry cat-style output, got: {value}"
            );
        }
        other => panic!("text Read MUST take fast path (Text variant), got: {other:?}",),
    }
    Ok(())
}

// ── Bash variants: structured envelope still rides through ────────

/// `Bash` with `cat` of an image file produces a structured-content
/// envelope. The renderer converts it into a `FileData` part; the
/// engine threads that into `ToolResultContent::Content` for turn 2.
/// This catches regressions where Bash's `render_for_model` stops
/// honoring `structuredContent` — which would silently drop the image
/// payload before any provider conversion happened.
pub async fn bash_cat_image_round_trip_via_real_loop() -> Result<()> {
    // Tiny PNG fixture — cat will read raw bytes which Bash MAY
    // promote into `structuredContent` via the image-output detector.
    let workdir = fresh_workdir()?;
    let target = workdir.path().join("photo.png");
    std::fs::write(&target, fixtures::png_1x1_red()?).context("write photo.png fixture")?;
    let abs_path = target.to_string_lossy().into_owned();

    let outcome = run_multimodal_scenario(
        workdir,
        vec![
            Reply::tool_call(
                "call_bash_cat",
                "Bash",
                json!({
                    "command": format!("cat {abs_path}"),
                    "description": "show png bytes",
                }),
            ),
            Reply::text("DONE"),
        ],
        &format!("Cat {abs_path}, then reply DONE."),
    )
    .await?;

    let prompts = outcome.model.captured_prompts();
    let turn2 = prompts.get(1).context("missing turn-2 prompt")?;

    // The Bash structured-image path MAY engage depending on how
    // shell stdout reaches the tool — assert a tool result landed
    // and fail loud if it's neither a FileData (the multimodal path)
    // nor a non-empty Text (the fallback when structuredContent
    // wasn't built).
    let tr = find_tool_result(turn2).context("turn-2 missing Bash tool result")?;
    match &tr.output {
        InnerToolResultContent::Content { value, .. } => {
            let has_filedata = value
                .iter()
                .any(|p| matches!(p, ToolResultContentPart::FileData { .. }));
            assert!(
                has_filedata,
                "Bash multimodal Content variant must carry FileData, got {value:?}",
            );
        }
        InnerToolResultContent::Text { value, .. } => {
            // Acceptable fallback path: Bash didn't promote stdout to
            // structuredContent (e.g. when shell did the encoding).
            // Pin only that we didn't lose the bytes entirely.
            assert!(
                !value.is_empty(),
                "Bash text fallback must carry stdout, got empty",
            );
        }
        other => {
            panic!("Bash output should be Content (multimodal) or Text (fallback), got {other:?}",)
        }
    }
    Ok(())
}

// ── helpers ────────────────────────────────────────────────────────

/// True when any `LlmMessage::Tool` in the prompt holds a
/// `ToolResultContent::Content` whose value contains a `FileData`
/// part with an `image/*` media type. Used to verify the post-tool
/// turn carries the multimodal payload.
fn contains_tool_result_with_filedata(prompt: &[LlmMessage]) -> bool {
    find_image_filedata(prompt).is_some()
}

/// Find the first `FileData(image/*)` part inside a tool-result
/// `Content` variant. Returns `(media_type, data_len)`.
fn find_image_filedata(prompt: &[LlmMessage]) -> Option<(String, usize)> {
    for msg in prompt {
        let LlmMessage::Tool { content, .. } = msg else {
            continue;
        };
        for c in content {
            let ToolContentPart::ToolResult(tr) = c else {
                continue;
            };
            let InnerToolResultContent::Content { value, .. } = &tr.output else {
                continue;
            };
            for part in value {
                if let ToolResultContentPart::FileData {
                    data, media_type, ..
                } = part
                    && media_type.starts_with("image/")
                {
                    return Some((media_type.clone(), data.len()));
                }
            }
        }
    }
    None
}

/// Find the first `ToolResultPart` in any `LlmMessage::Tool`
/// in the prompt. Tests use this to inspect the output variant
/// (Text vs Content) the engine produced.
fn find_tool_result(prompt: &[LlmMessage]) -> Option<&coco_llm_types::ToolResultPart> {
    for msg in prompt {
        let LlmMessage::Tool { content, .. } = msg else {
            continue;
        };
        for c in content {
            if let ToolContentPart::ToolResult(tr) = c {
                return Some(tr);
            }
        }
    }
    None
}

/// Sanity helper — every captured prompt must have at least one
/// assistant ToolCall once the agent loop has run a tool round-trip.
/// Currently unused by the active scenarios; kept as a guarded helper
/// for follow-on tests that assert ToolCall plumbing.
#[allow(dead_code)]
fn assistant_has_tool_call(prompt: &[LlmMessage]) -> bool {
    prompt.iter().any(|msg| {
        let LlmMessage::Assistant { content, .. } = msg else {
            return false;
        };
        content
            .iter()
            .any(|p| matches!(p, AssistantContentPart::ToolCall(_)))
    })
}
