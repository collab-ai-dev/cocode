use std::collections::HashMap;

use vercel_ai_provider::InputTokens;
use vercel_ai_provider::OutputTokens;
use vercel_ai_provider::Usage;

use super::anthropic_messages_api::AnthropicUsage;

/// Convert Anthropic usage to unified `Usage`.
///
/// Anthropic reports `input_tokens` exclusive of cache reads and cache writes,
/// so the normalized provider `InputTokens::total` adds all three buckets.
///
/// When `iterations` is present (compaction occurred), sums across all iterations
/// to get the true total tokens consumed/billed.
pub fn convert_anthropic_usage(usage: Option<&AnthropicUsage>) -> Usage {
    let Some(usage) = usage else {
        return Usage {
            input_tokens: InputTokens::default(),
            output_tokens: OutputTokens {
                total: None,
                text: None,
                reasoning: None,
            },
            raw: None,
        };
    };

    let cache_creation_tokens = usage.cache_creation_input_tokens.unwrap_or(0);
    let cache_read_tokens = usage.cache_read_input_tokens.unwrap_or(0);

    // When iterations is present (compaction occurred), sum across all iterations
    let (input_tokens, output_tokens) = if let Some(ref iterations) = usage.iterations
        && !iterations.is_empty()
    {
        let (total_in, total_out) =
            iterations
                .iter()
                .fold((0u64, 0u64), |(acc_in, acc_out), iter| {
                    (
                        acc_in.saturating_add(iter.input_tokens),
                        acc_out.saturating_add(iter.output_tokens),
                    )
                });
        (total_in, total_out)
    } else {
        (usage.input_tokens, usage.output_tokens)
    };

    // Build raw usage map
    let mut raw_map: HashMap<String, serde_json::Value> = HashMap::new();
    raw_map.insert(
        "input_tokens".into(),
        serde_json::Value::Number(input_tokens.into()),
    );
    raw_map.insert(
        "output_tokens".into(),
        serde_json::Value::Number(output_tokens.into()),
    );
    if let Some(cc) = usage.cache_creation_input_tokens {
        raw_map.insert(
            "cache_creation_input_tokens".into(),
            serde_json::Value::Number(cc.into()),
        );
    }
    if let Some(cr) = usage.cache_read_input_tokens {
        raw_map.insert(
            "cache_read_input_tokens".into(),
            serde_json::Value::Number(cr.into()),
        );
    }

    Usage {
        input_tokens: InputTokens::from_exclusive_buckets(
            Some(input_tokens),
            Some(cache_read_tokens),
            Some(cache_creation_tokens),
        ),
        output_tokens: OutputTokens {
            total: Some(output_tokens),
            text: None,
            reasoning: None,
        },
        raw: Some(raw_map),
    }
}

#[cfg(test)]
#[path = "convert_anthropic_usage.test.rs"]
mod tests;
