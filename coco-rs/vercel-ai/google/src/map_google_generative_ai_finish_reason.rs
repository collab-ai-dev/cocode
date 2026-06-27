//! Map Google Generative AI finish reasons to unified finish reasons.

use vercel_ai_provider::FinishReason;
use vercel_ai_provider::UnifiedFinishReason;

/// Map a Google finish reason string to a unified finish reason.
pub fn map_finish_reason(finish_reason: Option<&str>, has_tool_calls: bool) -> FinishReason {
    match finish_reason {
        Some("STOP") => {
            if has_tool_calls {
                FinishReason::with_raw(UnifiedFinishReason::ToolUse, "STOP")
            } else {
                FinishReason::with_raw(UnifiedFinishReason::EndTurn, "STOP")
            }
        }
        Some("MAX_TOKENS") => FinishReason::with_raw(UnifiedFinishReason::MaxTokens, "MAX_TOKENS"),
        Some("SAFETY") | Some("BLOCKLIST") | Some("PROHIBITED_CONTENT") | Some("SPII") => {
            let raw = finish_reason.unwrap_or("SAFETY");
            FinishReason::with_raw(UnifiedFinishReason::ContentFilter, raw)
        }
        Some("RECITATION") => {
            FinishReason::with_raw(UnifiedFinishReason::ContentFilter, "RECITATION")
        }
        Some("MALFORMED_FUNCTION_CALL") => {
            FinishReason::with_raw(UnifiedFinishReason::Error, "MALFORMED_FUNCTION_CALL")
        }
        Some("IMAGE_SAFETY") => {
            FinishReason::with_raw(UnifiedFinishReason::ContentFilter, "IMAGE_SAFETY")
        }
        Some(other) => FinishReason::with_raw(UnifiedFinishReason::Other, other),
        None => FinishReason::other(),
    }
}

#[cfg(test)]
#[path = "map_google_generative_ai_finish_reason.test.rs"]
mod tests;
