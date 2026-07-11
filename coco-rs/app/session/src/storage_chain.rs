use super::*;
pub(super) fn selected_agent_chain_indices(entries: &[TranscriptEntry]) -> Vec<usize> {
    if entries.is_empty() {
        return Vec::new();
    }
    let mut by_uuid: HashMap<String, usize> = HashMap::new();
    let mut parent_uuids: HashSet<String> = HashSet::new();
    for (idx, entry) in entries.iter().enumerate() {
        if !entry.uuid.is_empty() {
            by_uuid.insert(entry.uuid.clone(), idx);
        }
        if let Some(parent_uuid) = entry.parent_uuid.as_deref()
            && !parent_uuid.is_empty()
        {
            parent_uuids.insert(parent_uuid.to_string());
        }
    }
    if parent_uuids.is_empty() {
        return (0..entries.len()).collect();
    }

    let mut leaf_indices = Vec::new();
    let mut leaf_seen = HashSet::new();
    for (idx, terminal) in entries.iter().enumerate() {
        if terminal.uuid.is_empty() || parent_uuids.contains(&terminal.uuid) {
            continue;
        }
        let mut visited = HashSet::new();
        let mut cursor = Some(idx);
        while let Some(cursor_idx) = cursor {
            let entry = &entries[cursor_idx];
            if !entry.uuid.is_empty() && !visited.insert(entry.uuid.clone()) {
                break;
            }
            if entry.entry_type == entry_kind::USER || entry.entry_type == entry_kind::ASSISTANT {
                if leaf_seen.insert(cursor_idx) {
                    leaf_indices.push(cursor_idx);
                }
                break;
            }
            cursor = entry
                .parent_uuid
                .as_deref()
                .filter(|parent_uuid| !parent_uuid.is_empty())
                .and_then(|parent_uuid| by_uuid.get(parent_uuid).copied());
        }
    }
    let Some(leaf_idx) = leaf_indices
        .into_iter()
        .fold(None::<usize>, |best, idx| match best {
            Some(best_idx) if entries[idx].timestamp > entries[best_idx].timestamp => Some(idx),
            Some(best_idx) => Some(best_idx),
            None => Some(idx),
        })
    else {
        return (0..entries.len()).collect();
    };
    recover_agent_parallel_tool_branch_indices(
        entries,
        walk_agent_parent_chain_indices(entries, &by_uuid, leaf_idx),
    )
}

pub(super) fn walk_agent_parent_chain_indices(
    entries: &[TranscriptEntry],
    by_uuid: &HashMap<String, usize>,
    leaf_idx: usize,
) -> Vec<usize> {
    let mut walked = Vec::new();
    let mut visited = HashSet::new();
    let mut cursor = Some(leaf_idx);
    while let Some(idx) = cursor {
        let entry = &entries[idx];
        if !entry.uuid.is_empty() && !visited.insert(entry.uuid.clone()) {
            break;
        }
        walked.push(idx);
        cursor = entry
            .parent_uuid
            .as_deref()
            .filter(|parent_uuid| !parent_uuid.is_empty())
            .and_then(|parent_uuid| by_uuid.get(parent_uuid).copied());
    }
    walked.reverse();
    walked
}

pub(super) fn recover_agent_parallel_tool_branch_indices(
    entries: &[TranscriptEntry],
    chain_indices: Vec<usize>,
) -> Vec<usize> {
    if chain_indices.is_empty() {
        return chain_indices;
    }

    let chain_set: HashSet<usize> = chain_indices.iter().copied().collect();
    let mut assistant_groups: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut tool_result_children: HashMap<&str, Vec<usize>> = HashMap::new();
    for (idx, entry) in entries.iter().enumerate() {
        if entry.entry_type == entry_kind::ASSISTANT
            && let Some(request_id) = entry.request_id.as_deref()
            && !request_id.is_empty()
        {
            assistant_groups.entry(request_id).or_default().push(idx);
        }
        if entry.entry_type == entry_kind::USER
            && is_tool_result_entry(entry)
            && let Some(parent_uuid) = entry.parent_uuid.as_deref()
            && !parent_uuid.is_empty()
        {
            tool_result_children
                .entry(parent_uuid)
                .or_default()
                .push(idx);
        }
    }

    let mut last_group_position: HashMap<&str, usize> = HashMap::new();
    for (pos, idx) in chain_indices.iter().copied().enumerate() {
        let entry = &entries[idx];
        if entry.entry_type == entry_kind::ASSISTANT
            && let Some(request_id) = entry.request_id.as_deref()
            && assistant_groups.contains_key(request_id)
        {
            last_group_position.insert(request_id, pos);
        }
    }

    let mut out = Vec::with_capacity(chain_indices.len());
    let mut emitted: HashSet<usize> = HashSet::new();
    let mut expanded_request_ids: HashSet<&str> = HashSet::new();
    for (pos, idx) in chain_indices.into_iter().enumerate() {
        if emitted.insert(idx) {
            out.push(idx);
        }

        let entry = &entries[idx];
        let Some(request_id) = (entry.entry_type == entry_kind::ASSISTANT)
            .then_some(entry.request_id.as_deref())
            .flatten()
        else {
            continue;
        };
        if last_group_position.get(request_id) != Some(&pos)
            || !expanded_request_ids.insert(request_id)
        {
            continue;
        }

        let mut recovered = Vec::new();
        if let Some(group) = assistant_groups.get(request_id) {
            for assistant_idx in group {
                if !chain_set.contains(assistant_idx) {
                    recovered.push(*assistant_idx);
                }
                if let Some(children) =
                    tool_result_children.get(entries[*assistant_idx].uuid.as_str())
                {
                    recovered.extend(children.iter().copied().filter(|i| !chain_set.contains(i)));
                }
            }
        }
        recovered.sort_by(|a, b| {
            entries[*a]
                .timestamp
                .cmp(&entries[*b].timestamp)
                .then_with(|| a.cmp(b))
        });
        for recovered_idx in recovered {
            if emitted.insert(recovered_idx) {
                out.push(recovered_idx);
            }
        }
    }

    out
}

pub(super) fn is_tool_result_entry(entry: &TranscriptEntry) -> bool {
    entry
        .message
        .as_ref()
        .and_then(|message| message.get("content"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|content| {
            content.iter().any(|block| {
                block
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|kind| kind == "tool_result" || kind == "tool-result")
            })
        })
}

/// Recursively collect `agent-*.jsonl` transcript entries under a subagents
/// directory into `out`, in path order (deterministic across reads).
/// `recurse` descends boundedly for nested workflow/run layouts; `.meta.json`
/// sidecars and unparseable lines are skipped.
pub(crate) fn collect_agent_transcript_entries(dir: &Path, recurse: bool, out: &mut Vec<Entry>) {
    let max_depth = if recurse { 8 } else { 0 };
    collect_agent_transcript_entries_at_depth(dir, max_depth, out);
}

pub(super) fn collect_agent_transcript_entries_at_depth(
    dir: &Path,
    depth_remaining: usize,
    out: &mut Vec<Entry>,
) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = read_dir.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if depth_remaining > 0 {
                collect_agent_transcript_entries_at_depth(&path, depth_remaining - 1, out);
            }
            continue;
        }
        let is_agent_transcript = path.extension().is_some_and(|ext| ext == "jsonl")
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("agent-"));
        if !is_agent_transcript {
            continue;
        }
        let Ok(entries) = load_entries_from_file(&path) else {
            continue;
        };
        for entry in entries {
            match entry {
                Entry::Unknown(_) => {}
                entry => out.push(entry),
            }
        }
    }
}

/// Parse a single JSONL line into an [`Entry`].
/// Dispatch order: parse once into `serde_json::Value`, then route by
/// the `type` field. Transcript types
/// (`user`/`assistant`/`system`/`attachment`) go to `TranscriptEntry`;
/// every other `type` value is attempted as a `MetadataEntry`; anything
/// that fails to deserialize lands as `Entry::Unknown` with a
/// `tracing::debug!` so the failure shows up in logs instead of being
/// silently swallowed.
pub(super) fn parse_entry(line: &str) -> Entry {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        tracing::debug!(line = %line, "transcript line was not valid json");
        return Entry::Unknown(serde_json::Value::String(line.to_string()));
    };
    let entry_type = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let looks_like_transcript = matches!(
        entry_type,
        entry_kind::USER | entry_kind::ASSISTANT | entry_kind::SYSTEM | entry_kind::ATTACHMENT
    );
    if looks_like_transcript {
        if let Ok(transcript) = serde_json::from_value::<TranscriptEntry>(value.clone()) {
            return Entry::Transcript(Box::new(transcript));
        }
    } else if let Ok(meta) = serde_json::from_value::<MetadataEntry>(value.clone()) {
        return Entry::Metadata(meta);
    }
    tracing::debug!(
        entry_type = %entry_type,
        "transcript line did not match any known Entry shape — preserving as Unknown",
    );
    Entry::Unknown(value)
}
