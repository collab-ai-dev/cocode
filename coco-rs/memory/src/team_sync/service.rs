//! HTTP push/pull pipeline for team memory sync.
//!
//! Core contract:
//!
//! - `pull(state, repo, etag)` — `GET /api/claude_code/team_memory?repo=...`,
//!   server wins per-key. Updates `state.last_known_checksum` from the
//!   response/`ETag`. Handles 200 / 304 / 404 / auth failures.
//! - `push(state, repo, entries)` — delta-only PUT. Drops entries
//!   whose local SHA matches `state.server_checksums[key]`. Splits
//!   batches over `MAX_PUT_BODY_BYTES`. Pre-scans each entry for
//!   secrets (skipped + reported on `skipped_secrets`).
//! - `compute_content_hash(s)` — `sha256:<hex>` matching the server's
//!   `entryChecksums` format (anthropic/anthropic#283027).
//!
//! Auth: caller-provided `Authorization` value (typically a Bearer
//! token from Claude.ai OAuth). The HTTP layer keeps no token state
//! itself — the caller refreshes via `coco_inference::auth` (or the
//! keyring store) and passes the live token per-call.
//!
//! Watcher integration is the next-step port (debounced file events
//! → push). The HTTP surface here is callable directly from a CLI
//! command or REPL slash-command for one-shot sync.

use std::collections::HashMap;
use std::time::Duration;

use sha2::Digest;
use sha2::Sha256;

use super::secret_scanner::scan_for_secrets;
use super::types::MAX_CONFLICT_RETRIES;
use super::types::MAX_FILE_SIZE_BYTES;
use super::types::MAX_PUT_BODY_BYTES;
use super::types::SYNC_TIMEOUT_MS;
use super::types::SkippedSecretFile;
use super::types::SyncState;
use super::types::TeamMemoryContent;
use super::types::TeamMemoryData;
use super::types::TeamMemoryHashesResult;
use super::types::TeamMemorySyncFetchResult;
use super::types::TeamMemorySyncPushResult;
use super::types::TeamMemoryTooManyEntries;

/// Compute `sha256:<hex>` over the UTF-8 bytes of `content`. Format
/// matches the server's `entryChecksums`.
pub fn compute_content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(7 + digest.len() * 2);
    out.push_str("sha256:");
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Build the team-memory sync endpoint URL. `base_url` is the
/// Anthropic API base (e.g. `https://api.anthropic.com`).
pub fn endpoint(base_url: &str, repo_slug: &str) -> String {
    let encoded = urlencoding_encode(repo_slug);
    format!("{base_url}/api/claude_code/team_memory?repo={encoded}")
}

/// Minimal RFC-3986 query-component encoder for the `repo` slug.
/// Avoids pulling `urlencoding` for one call site.
fn urlencoding_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Pull team memory data from the server. Updates
/// `state.last_known_checksum` on
/// success; resets it to `None` on 404. The caller is responsible for
/// retry policy on `success: false` results.
pub async fn pull(
    state: &mut SyncState,
    base_url: &str,
    repo_slug: &str,
    bearer_token: &str,
    if_none_match: Option<&str>,
) -> TeamMemorySyncFetchResult {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_millis(SYNC_TIMEOUT_MS))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return TeamMemorySyncFetchResult {
                success: false,
                error: Some(format!("http client: {e}")),
                ..Default::default()
            };
        }
    };

    let mut req = client
        .get(endpoint(base_url, repo_slug))
        .header("Authorization", format!("Bearer {bearer_token}"));
    if let Some(etag) = if_none_match {
        req = req.header("If-None-Match", format!("\"{}\"", etag.trim_matches('"')));
    }

    let response = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "team-memory-sync: pull request failed");
            return TeamMemorySyncFetchResult {
                success: false,
                error: Some(format!("network: {e}")),
                ..Default::default()
            };
        }
    };

    let status = response.status();
    if status.as_u16() == 404 {
        state.last_known_checksum = None;
        return TeamMemorySyncFetchResult {
            success: true,
            is_empty: true,
            ..Default::default()
        };
    }
    if status.as_u16() == 304 {
        return TeamMemorySyncFetchResult {
            success: true,
            ..Default::default()
        };
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return TeamMemorySyncFetchResult {
            success: false,
            error: Some(format!("http {status}: {body}")),
            ..Default::default()
        };
    }

    let etag_header = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_matches('"').to_string());

    let data: TeamMemoryData = match response.json().await {
        Ok(d) => d,
        Err(e) => {
            return TeamMemorySyncFetchResult {
                success: false,
                error: Some(format!("invalid response: {e}")),
                ..Default::default()
            };
        }
    };

    if !data.checksum.is_empty() {
        state.last_known_checksum = Some(data.checksum.clone());
    } else if let Some(etag) = etag_header {
        state.last_known_checksum = Some(etag);
    }

    state.server_checksums = data.content.entry_checksums.clone();

    TeamMemorySyncFetchResult {
        success: true,
        data: Some(data),
        is_empty: false,
        ..Default::default()
    }
}

/// One entry to consider for push. The caller (typically the watcher
/// or a CLI command) reads files off disk + provides them here; the
/// service stays unaware of filesystem layout to keep tests
/// hermetic.
#[derive(Debug, Clone)]
pub struct PushEntry {
    /// Path relative to the team memory dir (e.g. `"MEMORY.md"`).
    pub path: String,
    /// UTF-8 entry content.
    pub content: String,
}

/// Push delta entries. Core path:
///
/// 1. Pre-scan each entry for secrets (skipped → `result.skipped_secrets`,
///    never sent) and per-entry size cap.
/// 2. Truncate alphabetically when `state.server_max_entries` is set —
///    the server cap is per-org and only learned from a structured 413
///    on a prior push. Records the dropped count for telemetry.
/// 3. Conflict-retry loop (up to [`MAX_CONFLICT_RETRIES`]). Each
///    iteration:
///    - (a) Recompute the delta from `entries.hash` vs `state.server_checksums`.
///    - (b) Split into PUT-sized batches under [`MAX_PUT_BODY_BYTES`].
///    - (c) PUT each batch with `If-Match` against
///      `state.last_known_checksum`.
///    - (d) On 200: update `state.server_checksums` for the batch keys,
///      advance `state.last_known_checksum` from the response, and move
///      to the next batch.
///    - (e) On 412: break out of the batch loop, fetch `?view=hashes`
///      to refresh `state.server_checksums`, and start the next
///      conflict iteration. The delta recomputation naturally drops
///      keys that a teammate just pushed with identical content.
///    - (f) On 413 with structured body: cache
///      `state.server_max_entries` and fail-out — the truncation
///      lands on the NEXT push (re-truncating mid-push would require
///      re-hashing).
///    - (g) On other failure: fail-out, partial-upload state preserved.
///
/// Local-wins-on-conflict semantics: a teammate's same-key edit gets
/// overwritten by the local push — silently discarding a just-typed
/// user edit is worse than losing a remote change the user can
/// re-pull. The hash refresh in (e) drops only matching content.
pub async fn push(
    state: &mut SyncState,
    base_url: &str,
    repo_slug: &str,
    bearer_token: &str,
    entries: &[PushEntry],
) -> TeamMemorySyncPushResult {
    let mut result = TeamMemorySyncPushResult::default();

    // Step 1: secret pre-scan + per-entry size cap. Identical to the
    // pre-refactor flow but lifted out of the inner loop.
    let mut clean: Vec<&PushEntry> = Vec::new();
    for entry in entries {
        if entry.content.len() > MAX_FILE_SIZE_BYTES {
            tracing::warn!(
                path = %entry.path,
                size = entry.content.len(),
                cap = MAX_FILE_SIZE_BYTES,
                "team-memory-sync: dropping oversized entry from push batch"
            );
            continue;
        }
        if let Some(skipped) = scan_for_secrets(&entry.path, &entry.content) {
            result.skipped_secrets.push(skipped);
            continue;
        }
        clean.push(entry);
    }

    // Step 2: server-cap truncation. Sort by path so the same N-of-M
    // subset is selected across pushes — without this, the dropped set
    // could oscillate and serverChecksums would never converge.
    if let Some(cap) = state.server_max_entries
        && (clean.len() as i32) > cap
    {
        clean.sort_by(|a, b| a.path.cmp(&b.path));
        let dropped = clean.len() as i32 - cap;
        let dropped_names: Vec<String> = clean[cap as usize..]
            .iter()
            .map(|e| e.path.clone())
            .collect();
        tracing::warn!(
            total = clean.len(),
            cap,
            dropped,
            dropped_names = %dropped_names.join(", "),
            "team-memory-sync: local entries exceed server cap; truncating"
        );
        result.truncated_count = dropped;
        clean.truncate(cap as usize);
    }

    // Pre-hash every clean entry once. Hashes are stable across
    // conflict-retry iterations even though `server_checksums` rotates.
    let mut local_hashes: HashMap<String, String> = HashMap::with_capacity(clean.len());
    for entry in &clean {
        local_hashes.insert(entry.path.clone(), compute_content_hash(&entry.content));
    }

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_millis(SYNC_TIMEOUT_MS))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            result.error = Some(format!("http client: {e}"));
            return result;
        }
    };

    let mut total_uploaded = 0i32;
    // Step 3: conflict-retry loop. `attempt == 0` is the first push;
    // 1..=MAX_CONFLICT_RETRIES are 412-driven retries.
    for attempt in 0..=MAX_CONFLICT_RETRIES {
        // (a) Recompute delta against current server_checksums.
        let mut to_push: Vec<&PushEntry> = clean
            .iter()
            .copied()
            .filter(|entry| {
                let local_hash = &local_hashes[&entry.path];
                state
                    .server_checksums
                    .get(&entry.path)
                    .is_none_or(|h| h != local_hash)
            })
            .collect();
        if to_push.is_empty() {
            // Convergence: nothing left to push (initial empty delta
            // or teammate's push was a strict superset of ours).
            result.success = true;
            result.uploaded_count = total_uploaded;
            return result;
        }
        // Sort for deterministic batching (helps tests + log readability).
        to_push.sort_by(|a, b| a.path.cmp(&b.path));

        // (b) Batch by serialized-body byte budget.
        let batches = batch_by_bytes(&to_push);

        // (c) PUT each batch sequentially.
        let outcome = push_batches(
            state,
            &client,
            base_url,
            repo_slug,
            bearer_token,
            &batches,
            &local_hashes,
        )
        .await;
        total_uploaded += outcome.uploaded;

        match outcome.terminal {
            BatchTerminal::Success => {
                result.success = true;
                result.uploaded_count = total_uploaded;
                return result;
            }
            BatchTerminal::Conflict => {
                result.conflict = true;
                result.conflict_retries = attempt + 1;
                if attempt >= MAX_CONFLICT_RETRIES {
                    tracing::warn!(
                        retries = result.conflict_retries,
                        "team-memory-sync: giving up after repeated 412 conflicts"
                    );
                    result.uploaded_count = total_uploaded;
                    result.error = Some("conflict resolution failed after retries".into());
                    return result;
                }
                // (e) Refresh per-key hashes via `?view=hashes`.
                let probe =
                    fetch_team_memory_hashes(state, base_url, repo_slug, bearer_token).await;
                match probe {
                    Ok(hashes) => {
                        // Replace wholesale — keys missing from the
                        // probe disappeared server-side and we want
                        // those re-uploaded on the next iteration.
                        state.server_checksums = hashes.entry_checksums;
                    }
                    Err(probe_err) => {
                        result.uploaded_count = total_uploaded;
                        result.error = Some(format!("conflict probe failed: {probe_err}"));
                        return result;
                    }
                }
                continue;
            }
            BatchTerminal::TooManyEntries { max_entries } => {
                // (f) Cache the cap; surface to caller so the next
                // push truncates. The watcher / CLI command warns the
                // user; this layer just records.
                state.server_max_entries = Some(max_entries);
                result.server_max_entries = Some(max_entries);
                result.uploaded_count = total_uploaded;
                result.error = Some(format!("server cap exceeded (max_entries={max_entries})"));
                return result;
            }
            BatchTerminal::OtherError(msg) => {
                result.uploaded_count = total_uploaded;
                result.error = Some(msg);
                return result;
            }
        }
    }

    // Unreachable per the loop's match arms — every branch returns.
    // Keep a defensive fall-through for forward-compat with future
    // BatchTerminal variants.
    result.uploaded_count = total_uploaded;
    result.error = Some("unexpected end of conflict resolution loop".into());
    result
}

/// Split a delta into PUT-sized batches under [`MAX_PUT_BODY_BYTES`].
/// Each batch carries a `HashMap<path, content>` ready to JSON-encode
/// as `{ "entries": ... }`. Server upsert-merge across batches is safe.
fn batch_by_bytes(to_push: &[&PushEntry]) -> Vec<HashMap<String, String>> {
    let mut batches: Vec<HashMap<String, String>> = Vec::new();
    let mut current: HashMap<String, String> = HashMap::new();
    let mut current_bytes = 2; // `{}` overhead
    for entry in to_push {
        // Rough estimate: key + value + JSON quoting + ":,".
        let entry_bytes = entry.path.len() + entry.content.len() + 8;
        if current_bytes + entry_bytes > MAX_PUT_BODY_BYTES && !current.is_empty() {
            batches.push(std::mem::take(&mut current));
            current_bytes = 2;
        }
        current.insert(entry.path.clone(), entry.content.clone());
        current_bytes += entry_bytes;
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

/// Terminal state of one push-batches pass. Drives the conflict-retry
/// outer loop in [`push`].
enum BatchTerminal {
    /// All batches succeeded (200).
    Success,
    /// At least one batch returned 412 PreconditionFailed. Outer
    /// loop should refresh hashes and retry.
    Conflict,
    /// A batch returned a structured 413 with effective
    /// `max_entries`. Cache + bail.
    TooManyEntries { max_entries: i32 },
    /// Network error, parse error, or any non-{200, 412, 413} HTTP
    /// status. Carries the error message for `result.error`.
    OtherError(String),
}

struct BatchOutcome {
    uploaded: i32,
    terminal: BatchTerminal,
}

/// Inner loop: PUT every batch sequentially, advancing
/// `state.server_checksums` + `state.last_known_checksum` on each
/// success. Returns the count actually pushed in this pass plus the
/// terminal state. Extracted from [`push`] so the conflict-retry
/// outer loop stays narrow.
async fn push_batches(
    state: &mut SyncState,
    client: &reqwest::Client,
    base_url: &str,
    repo_slug: &str,
    bearer_token: &str,
    batches: &[HashMap<String, String>],
    local_hashes: &HashMap<String, String>,
) -> BatchOutcome {
    let mut uploaded = 0i32;
    for batch in batches {
        let body = serde_json::json!({ "entries": batch });
        let mut req = client
            .put(endpoint(base_url, repo_slug))
            .header("Authorization", format!("Bearer {bearer_token}"))
            .header("Content-Type", "application/json")
            .json(&body);
        if let Some(etag) = &state.last_known_checksum {
            req = req.header("If-Match", format!("\"{}\"", etag.trim_matches('"')));
        }
        let put = req.send().await;
        match put {
            Err(e) => {
                tracing::warn!(error = %e, "team-memory-sync: push request failed");
                return BatchOutcome {
                    uploaded,
                    terminal: BatchTerminal::OtherError(format!("network: {e}")),
                };
            }
            Ok(resp) => {
                let status = resp.status();
                if status.as_u16() == 412 {
                    tracing::info!(
                        "team-memory-sync: 412 PreconditionFailed (ETag mismatch); will retry"
                    );
                    return BatchOutcome {
                        uploaded,
                        terminal: BatchTerminal::Conflict,
                    };
                }
                if status.as_u16() == 413 {
                    // Structured-413 parse — fail to OtherError if it
                    // looks like a gateway HTML page rather than the
                    // app's typed response.
                    let body_text = resp.text().await.unwrap_or_default();
                    if let Ok(parsed) = serde_json::from_str::<TeamMemoryTooManyEntries>(&body_text)
                        && parsed.error.details.error_code == "team_memory_too_many_entries"
                    {
                        return BatchOutcome {
                            uploaded,
                            terminal: BatchTerminal::TooManyEntries {
                                max_entries: parsed.error.details.max_entries,
                            },
                        };
                    }
                    return BatchOutcome {
                        uploaded,
                        terminal: BatchTerminal::OtherError(format!(
                            "http 413 (unstructured gateway): {body_text}"
                        )),
                    };
                }
                if !status.is_success() {
                    let body_text = resp.text().await.unwrap_or_default();
                    return BatchOutcome {
                        uploaded,
                        terminal: BatchTerminal::OtherError(format!("http {status}: {body_text}")),
                    };
                }
                // 200 — advance ETag chain + server_checksums.
                let etag_header = resp
                    .headers()
                    .get("etag")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.trim_matches('"').to_string());
                // Body may carry a `checksum` field that takes priority
                // over the ETag header.
                let resp_checksum = resp.json::<serde_json::Value>().await.ok().and_then(|v| {
                    v.get("checksum")
                        .and_then(|c| c.as_str().map(str::to_string))
                });
                if let Some(c) = resp_checksum.or(etag_header) {
                    state.last_known_checksum = Some(c);
                }
                for path in batch.keys() {
                    if let Some(hash) = local_hashes.get(path) {
                        state.server_checksums.insert(path.clone(), hash.clone());
                    }
                }
                uploaded += batch.len() as i32;
            }
        }
    }
    BatchOutcome {
        uploaded,
        terminal: BatchTerminal::Success,
    }
}

/// `GET <endpoint>&view=hashes` — fetch per-key checksums + metadata
/// without entry bodies. Used during 412 conflict resolution to
/// cheaply refresh `state.server_checksums`.
///
/// Requires anthropic/anthropic#283027 (server-side `?view=hashes`
/// support). When the server returns 200 without
/// `entryChecksums`, we treat it as a probe failure and surface the
/// error so the outer loop fails the push — the watcher retries on
/// the next file edit.
pub async fn fetch_team_memory_hashes(
    state: &mut SyncState,
    base_url: &str,
    repo_slug: &str,
    bearer_token: &str,
) -> Result<TeamMemoryHashesResult, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(SYNC_TIMEOUT_MS))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let url = format!("{}&view=hashes", endpoint(base_url, repo_slug));
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {bearer_token}"))
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;
    let status = resp.status();
    if status.as_u16() == 404 {
        state.last_known_checksum = None;
        return Ok(TeamMemoryHashesResult {
            organization_id: String::new(),
            repo: repo_slug.to_string(),
            version: 0,
            last_modified: String::new(),
            checksum: String::new(),
            entry_checksums: HashMap::new(),
        });
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("http {status}: {body}"));
    }
    let etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_matches('"').to_string());
    let parsed: TeamMemoryHashesResult = resp.json().await.map_err(|e| format!("parse: {e}"))?;
    if parsed.entry_checksums.is_empty() && parsed.checksum.is_empty() {
        return Err("server did not return entryChecksums (?view=hashes unsupported)".into());
    }
    if !parsed.checksum.is_empty() {
        state.last_known_checksum = Some(parsed.checksum.clone());
    } else if let Some(e) = etag {
        state.last_known_checksum = Some(e);
    }
    Ok(parsed)
}

/// Convenience: filter `entries` through the secret scanner only,
/// returning the clean set + skipped report. Useful for callers that
/// want to inspect what would be pushed before actually sending.
pub fn scan_only(entries: &[PushEntry]) -> (Vec<&PushEntry>, Vec<SkippedSecretFile>) {
    let mut clean = Vec::new();
    let mut skipped = Vec::new();
    for e in entries {
        if let Some(s) = scan_for_secrets(&e.path, &e.content) {
            skipped.push(s);
        } else {
            clean.push(e);
        }
    }
    (clean, skipped)
}

/// Apply pulled `TeamMemoryContent` to the local file tree under
/// `dir`. Server wins per-key — every entry gets written verbatim.
/// Existing local-only files are NOT removed (deletions don't
/// propagate). Errors during individual file writes are logged but
/// don't abort the operation.
///
/// Three guarantees per entry:
///
/// 1. **Path validation** via [`crate::path::team::validate_team_mem_key`]
///    — null bytes, UNC `\\` / `//`, drive-root, unexpanded tilde,
///    URL-encoded `%2e%2e`, fullwidth-NFKC traversal, planted symlinks
///    pointing outside `dir`. Defense-in-depth against a malicious or
///    compromised server.
/// 2. **Per-entry size cap** — refuse any entry larger than
///    [`MAX_FILE_SIZE_BYTES`] (250 KB). The server has the same cap
///    but a bug or rogue server could deliver an oversized blob.
/// 3. **Skip-if-equal** — read the existing file first and skip the
///    write when the byte content already matches. Preserves mtime so
///    a wired-up file-watcher doesn't trigger a spurious push-back
///    (ping-pong: pull → watcher → push → 412 conflict loop).
pub async fn apply_pulled_content(dir: &std::path::Path, content: &TeamMemoryContent) {
    if let Err(e) = tokio::fs::create_dir_all(dir).await {
        tracing::warn!(error = %e, dir = %dir.display(), "team-memory-sync: mkdir failed");
        return;
    }
    for (rel_path, body) in &content.entries {
        // (1) Path validation — fails closed on any taxonomy hit.
        let target = match crate::path::validate_team_mem_key(rel_path, dir) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    path = %rel_path,
                    error = ?e,
                    "team-memory-sync: rejected key (path validation)"
                );
                continue;
            }
        };
        // (2) Size cap.
        if body.len() > MAX_FILE_SIZE_BYTES {
            tracing::warn!(
                path = %rel_path,
                size = body.len(),
                cap = MAX_FILE_SIZE_BYTES,
                "team-memory-sync: skipping oversized pulled entry"
            );
            continue;
        }
        // (3) Skip-if-equal — read current bytes, compare, skip write
        // if matching. Missing file = no skip. Errors fall through to
        // the write attempt.
        if let Ok(existing) = tokio::fs::read(&target).await
            && existing == body.as_bytes()
        {
            continue;
        }
        if let Some(parent) = target.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if let Err(e) = tokio::fs::write(&target, body).await {
            tracing::warn!(error = %e, path = %target.display(), "team-memory-sync: write failed");
        }
    }
}

#[cfg(test)]
#[path = "service.test.rs"]
mod tests;
