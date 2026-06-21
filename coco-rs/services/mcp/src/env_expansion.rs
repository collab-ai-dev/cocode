//! Expand `${VAR}` / `${VAR:-default}` references in MCP server config.
//!
//! Applied at parse time over stdio `command` / `args` / `env` and remote
//! `url` / `headers`, before the [`crate::types::McpServerConfig`] is
//! constructed, so the launch / transport layer only ever sees resolved values.
//! Unknown variables with no default are left as the literal `${VAR}` token
//! (aids debugging) and collected so the loader can emit a single warning.
//!
//! These are arbitrary *user-supplied* variable names from a `.mcp.json`
//! (`${HOME}`, `${MY_TOKEN}`, …), not coco-owned config knobs, so they are read
//! through an injected lookup rather than `coco_config::EnvKey` (which is a
//! closed set of coco keys). The lookup is a closure so production reads
//! `std::env` at the single config-parse boundary while tests stay pure.

use std::collections::HashMap;

use crate::types::McpServerConfig;

/// Resolves a variable name to its value. Returns `None` when unset.
pub(crate) trait EnvLookup {
    fn get(&self, name: &str) -> Option<String>;
}

/// Production lookup: reads process environment for arbitrary user var names.
pub(crate) struct ProcessEnv;

impl EnvLookup for ProcessEnv {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

impl EnvLookup for HashMap<String, String> {
    fn get(&self, name: &str) -> Option<String> {
        HashMap::get(self, name).cloned()
    }
}

/// Expand `${VAR}` / `${VAR:-default}` in `value`. A placeholder must hold at
/// least one non-`}` char (so `${}` is left literal). Unknown vars with no
/// default are left verbatim and pushed onto `missing`.
pub(crate) fn expand_str(
    value: &str,
    lookup: &impl EnvLookup,
    missing: &mut Vec<String>,
) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        // `[^}]+`: find the closing `}` and require a non-empty body.
        match after.find('}') {
            Some(end) if end > 0 => {
                let content = &after[..end];
                // Split on the first `:-` so a default may itself contain `:-`.
                // `split_once` finds the first occurrence only.
                let (name, default) = match content.split_once(":-") {
                    Some((n, d)) => (n, Some(d)),
                    None => (content, None),
                };
                if let Some(v) = lookup.get(name) {
                    out.push_str(&v);
                } else if let Some(d) = default {
                    out.push_str(d);
                } else {
                    missing.push(name.to_string());
                    out.push_str("${");
                    out.push_str(content);
                    out.push('}');
                }
                rest = &after[end + 1..];
            }
            // No closing `}` or empty body — not a placeholder; emit the
            // literal `${` and resume scanning after it.
            _ => {
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Expand every value of a string map in place.
pub(crate) fn expand_map(
    map: &HashMap<String, String>,
    lookup: &impl EnvLookup,
    missing: &mut Vec<String>,
) -> HashMap<String, String> {
    map.iter()
        .map(|(k, v)| (k.clone(), expand_str(v, lookup, missing)))
        .collect()
}

/// Expand env references across a parsed config: stdio `command`/`args`/`env`
/// and remote `url`/`headers`. `sdk` and `claudeai_proxy` are left untouched.
/// Returns the deduplicated list of referenced-but-unset variables (no default
/// supplied).
pub(crate) fn expand_config(config: &mut McpServerConfig, lookup: &impl EnvLookup) -> Vec<String> {
    let mut missing = Vec::new();
    match config {
        McpServerConfig::Stdio(c) => {
            c.command = expand_str(&c.command, lookup, &mut missing);
            c.args = c
                .args
                .iter()
                .map(|a| expand_str(a, lookup, &mut missing))
                .collect();
            c.env = expand_map(&c.env, lookup, &mut missing);
        }
        McpServerConfig::Sse(c) => {
            c.url = expand_str(&c.url, lookup, &mut missing);
            c.headers = expand_map(&c.headers, lookup, &mut missing);
            expand_oauth(&mut c.oauth, lookup, &mut missing);
        }
        McpServerConfig::Http(c) => {
            c.url = expand_str(&c.url, lookup, &mut missing);
            c.headers = expand_map(&c.headers, lookup, &mut missing);
            expand_oauth(&mut c.oauth, lookup, &mut missing);
        }
        McpServerConfig::WebSocket(c) => {
            c.url = expand_str(&c.url, lookup, &mut missing);
            c.headers = expand_map(&c.headers, lookup, &mut missing);
        }
        McpServerConfig::Sdk(_) | McpServerConfig::ClaudeAiProxy(_) => {}
    }
    missing.sort();
    missing.dedup();
    missing
}

fn expand_oauth(
    oauth: &mut Option<crate::types::McpOAuthConfig>,
    lookup: &impl EnvLookup,
    missing: &mut Vec<String>,
) {
    let Some(oauth) = oauth else {
        return;
    };
    expand_optional(&mut oauth.client_id, lookup, missing);
    let Some(xaa) = &mut oauth.xaa else {
        return;
    };
    expand_optional(&mut xaa.client_id, lookup, missing);
    expand_optional(&mut xaa.client_secret, lookup, missing);
    expand_optional(&mut xaa.idp_client_id, lookup, missing);
    expand_optional(&mut xaa.idp_client_secret, lookup, missing);
    expand_optional(&mut xaa.idp_id_token, lookup, missing);
    expand_optional(&mut xaa.idp_token_endpoint, lookup, missing);
    expand_optional(&mut xaa.scope, lookup, missing);
}

fn expand_optional(value: &mut Option<String>, lookup: &impl EnvLookup, missing: &mut Vec<String>) {
    if let Some(raw) = value {
        *raw = expand_str(raw, lookup, missing);
    }
}

#[cfg(test)]
#[path = "env_expansion.test.rs"]
mod tests;
