//! Per-run token-usage aggregator.
//!
//! Every live test calls [`record`] with the `TokenUsage` returned by
//! the API. A `zzz_emit_*` test in each runner (alphabetically last so
//! it always runs after data-collection tests) calls [`flush`] which
//! writes a JSON + Markdown report under `tests/live/last-run/`.
//!
//! The output directory is `.gitignore`d. Cost numbers use a static
//! pricing table; unknown models report `null` cost.
//!
//! ## Field provenance — what "0" means by protocol
//!
//! - **`cache_read_tokens`** — both protocols expose this. OpenAI-compat
//!   reads `prompt_tokens_details.cached_tokens`; Anthropic reads
//!   `cache_read_input_tokens`. Reliable across the board.
//! - **`cache_write_tokens`** — only the **Anthropic** wire shape
//!   carries `cache_creation_input_tokens`. The OpenAI Chat
//!   Completions wire shape (used by `deepseek-openai`, xAI, Groq,
//!   etc.) has no equivalent field, so `cache_write` is **always 0**
//!   for those rows — even when the provider IS writing to its
//!   prompt cache server-side. Matches `@ai-sdk/openai-compatible`
//!   `convert-openai-compatible-chat-usage.ts`. Not a parsing bug.
//! - **`input_tokens` / `output_tokens`** — universal.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

use coco_types::TokenUsage;

/// One key per (provider, model, scenario). `BTreeMap` so the report
/// is byte-stable across runs, which keeps diffs reviewable.
type ReportKey = (String, String, String);

#[derive(Debug, Default, Clone)]
pub struct ScenarioUsage {
    /// Number of `record()` calls — i.e. test-side aggregation events.
    /// For SDK tests this equals the number of underlying LLM HTTP
    /// calls; for CLI tests one `record()` carries the engine's full
    /// per-session `total_usage` which itself summed N HTTP calls,
    /// so `record_calls` *under-counts* HTTP traffic. See
    /// `llm_calls` for the real number when the caller passes it in.
    pub record_calls: u64,
    /// Real underlying LLM HTTP-call count, when known. Engine paths
    /// can pass `cost_tracker.total_api_calls` here; SDK paths
    /// default to 1 per `record()` (each `client.query` is one call).
    /// `0` when never populated.
    pub llm_calls: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    /// Subset of `output_tokens` spent on reasoning. Already counted
    /// in `output_tokens` (billed at the output rate). For DeepSeek's
    /// OpenAI-compat protocol this is sourced from
    /// `completion_tokens_details.reasoning_tokens`. Was silently
    /// dropped before — see `coco_types::TokenUsage` doc.
    pub reasoning_output_tokens: i64,
    pub text_output_tokens: i64,
}

impl ScenarioUsage {
    fn add(&mut self, usage: &TokenUsage, llm_calls_delta: u64) {
        self.record_calls += 1;
        self.llm_calls += llm_calls_delta;
        self.input_tokens += usage.input_tokens.total;
        self.output_tokens += usage.output_tokens.total;
        self.cache_read_tokens += usage.input_tokens.cache_read;
        self.cache_creation_tokens += usage.input_tokens.cache_write;
        self.reasoning_output_tokens += usage.output_tokens.reasoning;
        self.text_output_tokens += usage.output_tokens.text;
    }

    fn estimated_cost_usd(&self, model: &str) -> Option<f64> {
        let p = pricing(model)?;
        // Anthropic-shape pricing: cache reads are typically 10% of
        // input price, cache writes 125%. DeepSeek doesn't bill cache
        // separately for V4 yet; we use the same approximation.
        let input = self.input_tokens.max(0) as f64;
        let output = self.output_tokens.max(0) as f64;
        let read = self.cache_read_tokens.max(0) as f64;
        let write = self.cache_creation_tokens.max(0) as f64;
        let cost = (input * p.input_per_mtok / 1_000_000.0)
            + (output * p.output_per_mtok / 1_000_000.0)
            + (read * p.input_per_mtok * 0.10 / 1_000_000.0)
            + (write * p.input_per_mtok * 1.25 / 1_000_000.0);
        Some(cost)
    }
}

struct Pricing {
    input_per_mtok: f64,
    output_per_mtok: f64,
}

/// Per-million-token USD pricing, sourced from each provider's public
/// docs at the time of writing. Update when prices change.
fn pricing(model: &str) -> Option<Pricing> {
    Some(match model {
        // DeepSeek V4 (https://api-docs.deepseek.com/)
        "deepseek-v4-flash" => Pricing {
            input_per_mtok: 0.27,
            output_per_mtok: 1.10,
        },
        "deepseek-v4-pro" => Pricing {
            input_per_mtok: 0.55,
            output_per_mtok: 2.20,
        },
        _ => return None,
    })
}

static REPORT: OnceLock<Mutex<BTreeMap<ReportKey, ScenarioUsage>>> = OnceLock::new();

fn report() -> &'static Mutex<BTreeMap<ReportKey, ScenarioUsage>> {
    REPORT.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Record one API call's usage against a (provider, model, scenario)
/// triple. Safe to call from any test in any thread.
///
/// Use this for SDK-layer tests where one `record()` corresponds to
/// exactly one underlying LLM HTTP call.
pub fn record(provider: &str, model: &str, scenario: &str, usage: &TokenUsage) {
    record_with_llm_calls(provider, model, scenario, usage, 1);
}

/// Same as `record`, but pass the explicit underlying LLM HTTP call
/// count. Use this for engine-layer aggregations where one `record()`
/// carries the sum of N model invocations (e.g. `QueryResult.total_usage`
/// rolled up from `cost_tracker.total_api_calls`). Without this, the
/// `record_calls` column under-counts the real HTTP traffic.
pub fn record_with_llm_calls(
    provider: &str,
    model: &str,
    scenario: &str,
    usage: &TokenUsage,
    llm_calls: u64,
) {
    let mut guard = report().lock().expect("usage_report mutex poisoned");
    let entry = guard
        .entry((
            provider.to_string(),
            model.to_string(),
            scenario.to_string(),
        ))
        .or_default();
    entry.add(usage, llm_calls);
}

fn output_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("last-run")
}

/// Snapshot the current report and write `token-usage.json` +
/// `token-usage.md` under `tests/live/last-run/<runner>/`. `runner` is
/// the test-binary name (e.g. `sdk_deepseek`, `cli_deepseek`).
pub fn flush(runner: &str) -> std::io::Result<()> {
    let snapshot: BTreeMap<ReportKey, ScenarioUsage> = {
        let guard = report().lock().expect("usage_report mutex poisoned");
        guard.clone()
    };
    let dir = output_dir().join(runner);
    std::fs::create_dir_all(&dir)?;

    write_json(&dir.join("token-usage.json"), &snapshot)?;
    write_markdown(&dir.join("token-usage.md"), runner, &snapshot)?;
    eprintln!(
        "[coco-tests-live] wrote token-usage report to {}",
        dir.display()
    );
    Ok(())
}

fn write_json(
    path: &std::path::Path,
    snapshot: &BTreeMap<ReportKey, ScenarioUsage>,
) -> std::io::Result<()> {
    let entries: Vec<serde_json::Value> = snapshot
        .iter()
        .map(|((provider, model, scenario), usage)| {
            serde_json::json!({
                "provider": provider,
                "model": model,
                "scenario": scenario,
                "record_calls": usage.record_calls,
                "llm_calls": usage.llm_calls,
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
                "reasoning_output_tokens": usage.reasoning_output_tokens,
                "text_output_tokens": usage.text_output_tokens,
                "cache_read_tokens": usage.cache_read_tokens,
                "cache_creation_tokens": usage.cache_creation_tokens,
                "estimated_cost_usd": usage.estimated_cost_usd(model),
            })
        })
        .collect();
    let totals = totals(snapshot);
    let body = serde_json::json!({
        "generated_at": chrono_iso8601(),
        "entries": entries,
        "totals": {
            "record_calls": totals.record_calls,
            "llm_calls": totals.llm_calls,
            "input_tokens": totals.input_tokens,
            "output_tokens": totals.output_tokens,
            "reasoning_output_tokens": totals.reasoning_output_tokens,
            "text_output_tokens": totals.text_output_tokens,
            "cache_read_tokens": totals.cache_read_tokens,
            "cache_creation_tokens": totals.cache_creation_tokens,
        },
    });
    std::fs::write(path, serde_json::to_string_pretty(&body)?.as_bytes())?;
    Ok(())
}

fn write_markdown(
    path: &std::path::Path,
    runner: &str,
    snapshot: &BTreeMap<ReportKey, ScenarioUsage>,
) -> std::io::Result<()> {
    let mut out = String::new();
    out.push_str(&format!("# Token Usage Report — `{runner}`\n\n"));
    out.push_str(&format!("Generated at {}.\n\n", chrono_iso8601()));
    out.push_str(
        "> **Note** — `cache_write` is **only observable on the Anthropic wire shape** \
         (`cache_creation_input_tokens`). Rows from OpenAI-compatible providers \
         (e.g. `deepseek-openai`) always show `0` because the OpenAI Chat \
         Completions response has no cache-write field. This matches the upstream \
         `@ai-sdk/openai-compatible` reference and is not a parsing bug. See \
         `tests/live/tests/common/usage_report.rs` for details.\n\n",
    );
    if snapshot.is_empty() {
        out.push_str("No usage recorded — every test was skipped (likely missing API key).\n");
        std::fs::write(path, out.as_bytes())?;
        return Ok(());
    }
    out.push_str(
        "| provider | model | scenario | record / llm calls | input | output (text + reasoning) | cache_read | cache_write | cost (USD) |\n",
    );
    out.push_str(
        "| -------- | ----- | -------- | ----: | ----: | -----: | ---------: | ----------: | ---------: |\n",
    );
    for ((provider, model, scenario), usage) in snapshot {
        let cost = usage
            .estimated_cost_usd(model)
            .map(|c| format!("{c:.4}"))
            .unwrap_or_else(|| "n/a".into());
        let output_breakdown = format!(
            "{} ({} text + {} reasoning)",
            usage.output_tokens, usage.text_output_tokens, usage.reasoning_output_tokens,
        );
        out.push_str(&format!(
            "| {provider} | {model} | {scenario} | {} / {} | {} | {output_breakdown} | {} | {} | {cost} |\n",
            usage.record_calls,
            usage.llm_calls,
            usage.input_tokens,
            usage.cache_read_tokens,
            usage.cache_creation_tokens,
        ));
    }
    let totals = totals(snapshot);
    out.push_str(&format!(
        "| **TOTAL** |  |  | **{} / {}** | **{}** | **{} ({} text + {} reasoning)** | **{}** | **{}** |  |\n",
        totals.record_calls,
        totals.llm_calls,
        totals.input_tokens,
        totals.output_tokens,
        totals.text_output_tokens,
        totals.reasoning_output_tokens,
        totals.cache_read_tokens,
        totals.cache_creation_tokens,
    ));
    std::fs::write(path, out.as_bytes())?;
    Ok(())
}

fn totals(snapshot: &BTreeMap<ReportKey, ScenarioUsage>) -> ScenarioUsage {
    let mut total = ScenarioUsage::default();
    for usage in snapshot.values() {
        total.record_calls += usage.record_calls;
        total.llm_calls += usage.llm_calls;
        total.input_tokens += usage.input_tokens;
        total.output_tokens += usage.output_tokens;
        total.reasoning_output_tokens += usage.reasoning_output_tokens;
        total.text_output_tokens += usage.text_output_tokens;
        total.cache_read_tokens += usage.cache_read_tokens;
        total.cache_creation_tokens += usage.cache_creation_tokens;
    }
    total
}

/// Approximate ISO-8601 timestamp without pulling in `chrono`. Format:
/// `2026-05-05T14:30:00Z` style; sub-second precision is irrelevant for
/// a per-run report.
fn chrono_iso8601() -> String {
    // Use the local clock; SystemTime since UNIX_EPOCH gives seconds.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Convert epoch seconds → UTC date string. Avoid pulling chrono in;
    // the breakdown below is naive but accurate for post-1970 dates.
    let days = secs / 86_400;
    let secs_in_day = secs % 86_400;
    let h = secs_in_day / 3_600;
    let m = (secs_in_day % 3_600) / 60;
    let s = secs_in_day % 60;
    let (y, mo, d) = days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Convert epoch days to (year, month 1-12, day 1-31). Algorithm from
/// Howard Hinnant's `civil_from_days`.
fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 {
        z / 146_097
    } else {
        (z - 146_096) / 146_097
    };
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp.wrapping_sub(9) };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}
