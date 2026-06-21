//! Slash-command suggestion ranker.
//!
//! Pure function over a `query` plus a snapshot of `SlashCommandInfo`s.
//! The implementation skips Fuse entirely: the registry holds at most ~100
//! commands and a hand-rolled ranker with an explicit priority hierarchy
//! ensures prefix matches always beat fuzzy hits.
//!
//! ## Empty query — source-aware grouping
//!
//! When the user has typed just `/` (no query yet), the top-5
//! frequently-used skills are lifted out of the alphabetical list, then
//! everything else is grouped by source in this order: builtin → user →
//! project → policy → other. Usage scores travel with the snapshot
//! ([`SlashCommandInfo::usage_score`]) — the ranker never touches the
//! filesystem on the hot popup path; the agent driver populates them
//! at snapshot time via [`coco_skills::usage::load_all`].
//!
//! ## Non-empty query — priority cascade
//!
//! Priority (lower number = better):
//! 1. Exact name match
//! 2. Exact alias match
//! 3. Name starts with `query`
//! 4. Any alias starts with `query`
//! 5. Name contains `query`
//! 6. Any alias contains `query`
//! 7. Name is a subsequence of `query` (typo-friendly catch-all, e.g.
//!    `/clr` still finds `/clear`)
//!
//! Within a priority bucket, shorter names sort first so `/help`
//! outranks `/help-extra` when the user types `/h`. Usage score
//! breaks remaining ties (usage score descending).
//!
//! ## Source suffix on description
//!
//! Every popup row's description is annotated with the command's
//! origin — `(plugin-name) text`, `text (bundled)`, `text (user)`,
//! `text (project)`, `text (policy)`. Builtin and MCP commands have
//! no suffix.

use std::collections::HashMap;
use std::collections::HashSet;

use coco_types::CommandSource;
use coco_types::CommandTypeTag;

use crate::state::SlashCommandInfo;
use crate::widgets::suggestion_popup::SuggestionItem;

/// How many "recently used" skills float to the top of the empty-query list.
const RECENT_LIMIT: usize = 5;

/// Rank a snapshot of slash commands against `query` and return popup
/// items in display order. Empty `query` uses the source-grouped
/// layout with a recently-used section on top.
pub fn rank(query: &str, commands: &[SlashCommandInfo]) -> Vec<SuggestionItem> {
    if query.is_empty() {
        rank_empty(commands)
    } else {
        rank_filtered(query, commands)
    }
}

/// Source bucket order used by the empty-query layout:
/// `[recently used] → builtin → user → project → policy → other`.
fn empty_bucket(cmd: &SlashCommandInfo) -> EmptyBucket {
    if cmd.kind != CommandTypeTag::Prompt {
        // Local / LocalOverlay commands are always in the Builtin surface
        // regardless of where the underlying handler lives. Skill-backed
        // prompt commands group by `source`.
        return EmptyBucket::Builtin;
    }
    match &cmd.source {
        Some(CommandSource::Builtin) => EmptyBucket::Builtin,
        Some(CommandSource::User) => EmptyBucket::User,
        Some(CommandSource::Project) => EmptyBucket::Project,
        Some(CommandSource::Managed) => EmptyBucket::Policy,
        _ => EmptyBucket::Other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum EmptyBucket {
    Builtin,
    User,
    Project,
    Policy,
    Other,
}

fn rank_empty(commands: &[SlashCommandInfo]) -> Vec<SuggestionItem> {
    // ---- recently used: top-5 prompt commands by decayed usage ----
    // `usage_score` is precomputed at snapshot time so the ranker
    // never touches disk on the hot popup path.
    let mut scored: Vec<(f64, &SlashCommandInfo)> = commands
        .iter()
        .filter(|c| c.kind == CommandTypeTag::Prompt)
        .filter_map(|c| (c.usage_score > 0.0).then_some((c.usage_score, c)))
        .collect();
    // Sort by score desc — partial_cmp is total for finite scores.
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let recent: Vec<&SlashCommandInfo> = scored
        .into_iter()
        .take(RECENT_LIMIT)
        .map(|(_, c)| c)
        .collect();
    let recent_ids: HashSet<&str> = recent.iter().map(|c| c.name.as_str()).collect();

    // ---- remaining: group by source, alpha within ----
    let mut by_bucket: HashMap<EmptyBucket, Vec<&SlashCommandInfo>> = HashMap::new();
    for c in commands {
        if recent_ids.contains(c.name.as_str()) {
            continue;
        }
        by_bucket.entry(empty_bucket(c)).or_default().push(c);
    }
    for v in by_bucket.values_mut() {
        v.sort_by(|a, b| a.name.cmp(&b.name));
    }

    let mut out: Vec<SuggestionItem> = Vec::with_capacity(commands.len());
    out.extend(recent.iter().map(|c| to_item(c)));
    for bucket in [
        EmptyBucket::Builtin,
        EmptyBucket::User,
        EmptyBucket::Project,
        EmptyBucket::Policy,
        EmptyBucket::Other,
    ] {
        if let Some(items) = by_bucket.get(&bucket) {
            out.extend(items.iter().map(|c| to_item(c)));
        }
    }
    out
}

fn rank_filtered(query: &str, commands: &[SlashCommandInfo]) -> Vec<SuggestionItem> {
    let needle = query.to_lowercase();
    let mut scored: Vec<(Priority, f64, &SlashCommandInfo)> = commands
        .iter()
        .filter_map(|cmd| {
            let p = classify(&needle, cmd)?;
            // Usage as final tiebreaker. Only prompt commands carry
            // usage stats — local/local-overlay never accrue score
            // and fall through to the alphabetical tiebreak.
            let u = if cmd.kind == CommandTypeTag::Prompt {
                cmd.usage_score
            } else {
                0.0
            };
            Some((p, u, cmd))
        })
        .collect();
    // Total order: priority bucket → shorter name → usage desc → name ascending.
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.2.name.len().cmp(&b.2.name.len()))
            .then_with(|| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.2.name.cmp(&b.2.name))
    });
    scored.into_iter().map(|(_, _, cmd)| to_item(cmd)).collect()
}

/// Build a popup row from a command snapshot. Description carries the
/// optional `argument_hint` in front of the prose, and a source-tag suffix
/// at the back so users can see provenance (plugin / user / project / policy
/// / bundled).
fn to_item(cmd: &SlashCommandInfo) -> SuggestionItem {
    SuggestionItem {
        label: format!("/{}", cmd.name),
        description: build_description(cmd),
        metadata: None,
    }
}

/// Compose the description shown next to the command label.
///
/// Three pieces: argument hint up front, base description in the middle,
/// source/plugin annotation at the tail.
fn build_description(cmd: &SlashCommandInfo) -> Option<String> {
    let base = source_annotated_description(cmd);
    match (cmd.argument_hint.as_deref(), base) {
        (Some(hint), Some(desc)) => Some(format!("{hint}  {desc}")),
        (Some(hint), None) => Some(hint.to_string()),
        (None, Some(desc)) => Some(desc),
        (None, None) => None,
    }
}

/// Wraps the bare description with a provenance suffix or plugin prefix:
///
/// - `plugin` with name  → `"(plugin-name) text"`
/// - `plugin` no name    → `"text (plugin)"`
/// - `bundled`           → `"text (bundled)"`
/// - `user`              → `"text (user)"`
/// - `project`           → `"text (project)"`
/// - `policy`            → `"text (policy)"`
/// - `builtin` / `mcp` / unknown → unchanged
///
/// Builtin and MCP intentionally have no suffix — those surfaces are the
/// user's expected default, so the suffix would be noise.
fn source_annotated_description(cmd: &SlashCommandInfo) -> Option<String> {
    let base = cmd.description.as_deref().unwrap_or("");
    let suffix_only_view = base.is_empty();

    let composed = match &cmd.source {
        Some(CommandSource::Plugin { name }) if !name.is_empty() => {
            if suffix_only_view {
                format!("({name})")
            } else {
                format!("({name}) {base}")
            }
        }
        Some(CommandSource::Plugin { .. }) => format_with_suffix(base, "plugin"),
        Some(CommandSource::Bundled) => format_with_suffix(base, "bundled"),
        Some(CommandSource::User) => format_with_suffix(base, "user"),
        Some(CommandSource::Project) => format_with_suffix(base, "project"),
        Some(CommandSource::Managed) => format_with_suffix(base, "policy"),
        Some(CommandSource::Skills) | Some(CommandSource::CommandsDeprecated) => {
            // Both are user-installed skills that originate from the
            // user's settings directory tree.
            format_with_suffix(base, "user")
        }
        Some(CommandSource::Builtin) | Some(CommandSource::Mcp { .. }) | None => {
            if suffix_only_view {
                return None;
            }
            base.to_string()
        }
    };
    Some(composed)
}

fn format_with_suffix(base: &str, tag: &str) -> String {
    if base.is_empty() {
        format!("({tag})")
    } else {
        format!("{base} ({tag})")
    }
}

/// Match-quality bucket. Variant order determines popup ordering — the
/// derived `Ord` walks variants top-to-bottom, so keep the highest-
/// priority match at the top of the enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Priority {
    ExactName,
    ExactAlias,
    PrefixName,
    PrefixAlias,
    ContainsName,
    ContainsAlias,
    Subsequence,
}

fn classify(needle: &str, cmd: &SlashCommandInfo) -> Option<Priority> {
    let name = cmd.name.to_lowercase();
    if name == needle {
        return Some(Priority::ExactName);
    }
    let aliases: Vec<String> = cmd.aliases.iter().map(|a| a.to_lowercase()).collect();
    if aliases.iter().any(|a| a == needle) {
        return Some(Priority::ExactAlias);
    }
    if name.starts_with(needle) {
        return Some(Priority::PrefixName);
    }
    if aliases.iter().any(|a| a.starts_with(needle)) {
        return Some(Priority::PrefixAlias);
    }
    if name.contains(needle) {
        return Some(Priority::ContainsName);
    }
    if aliases.iter().any(|a| a.contains(needle)) {
        return Some(Priority::ContainsAlias);
    }
    if is_tight_subsequence(needle, &name)
        || aliases.iter().any(|a| is_tight_subsequence(needle, a))
    {
        return Some(Priority::Subsequence);
    }
    None
}

/// Whether `needle` appears as a non-contiguous subsequence of `haystack`,
/// AND the matched range stays "tight" — the span between the first and
/// last matched character must be at most `2 × needle.len()`. The cap
/// keeps the fallback typo-friendly (`clr` → `clear`, span 5 vs needle 3)
/// while rejecting wildly-spread matches like `abc` → `build-and-clear`.
///
/// Both inputs must already be lowercased. The explicit span cap gives a
/// "don't reach too far" guarantee without pulling in a fuzzy library for
/// ~80 commands.
fn is_tight_subsequence(needle: &str, haystack: &str) -> bool {
    let needle_chars: Vec<char> = needle.chars().collect();
    if needle_chars.is_empty() {
        return true;
    }
    let max_span = needle_chars.len().saturating_mul(2);
    let mut idx = 0;
    let mut first_match: Option<usize> = None;
    for (i, h) in haystack.chars().enumerate() {
        if h == needle_chars[idx] {
            let start = *first_match.get_or_insert(i);
            idx += 1;
            if idx == needle_chars.len() {
                // Equivalent to `(i - start + 1) <= max_span` — clippy
                // prefers the rearranged form. Span is inclusive of both
                // endpoints, hence the "<" rather than "<=".
                return i - start < max_span;
            }
        }
    }
    false
}

#[cfg(test)]
#[path = "slash.test.rs"]
mod tests;
