//! Incremental rendered-text corpus for Ctrl+O transcript search.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;

use coco_messages::Message;

use crate::presentation::thinking::format_duration_seconds;
use crate::presentation::transcript::TranscriptCell;
use crate::presentation::transcript::TranscriptSourceCell;
use crate::presentation::transcript::transcript_presentation_with_cells;
use crate::state::AppState;
use crate::state::transcript::TranscriptCellId;
use crate::state::transcript::TranscriptSearch;
use crate::state::transcript::TranscriptSearchEntry;
use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use crate::transcript::render::reader::TranscriptCellRenderer;
use coco_tui_ui::style::UiStyles;

pub(crate) struct SearchEntriesBuild {
    pub(crate) entries: Vec<TranscriptSearchEntry>,
    pub(crate) revisions: HashMap<TranscriptCellId, u64>,
    #[cfg(test)]
    pub(crate) reused_entries: usize,
}

/// Detect presentation-only state changes which do not mutate the immutable
/// transcript log. Per-cell fingerprints below still decide which entries are
/// actually re-rendered after this aggregate revision opens an index build.
pub(crate) fn side_cache_revision(state: &AppState) -> u64 {
    let mut hasher = DefaultHasher::new();

    let mut reasoning = state.session.reasoning_metadata.iter().collect::<Vec<_>>();
    reasoning.sort_unstable_by_key(|(message_id, _)| **message_id);
    for (message_id, metadata) in reasoning {
        message_id.hash(&mut hasher);
        metadata.duration_ms.hash(&mut hasher);
        metadata.reasoning_tokens.hash(&mut hasher);
    }

    for tool in &state.session.tool_executions {
        tool.call_id.hash(&mut hasher);
        tool.name.hash(&mut hasher);
        tool.status.hash(&mut hasher);
        tool.description.hash(&mut hasher);
        tool.input_preview.hash(&mut hasher);
        format_duration_seconds(tool.elapsed()).hash(&mut hasher);
    }

    let mut summaries = state.session.subagent_summaries.iter().collect::<Vec<_>>();
    summaries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    for (tool_use_id, summary) in summaries {
        tool_use_id.hash(&mut hasher);
        summary.agent_type.hash(&mut hasher);
        summary.tool_count.hash(&mut hasher);
        summary.duration_ms.hash(&mut hasher);
        summary.input_tokens.hash(&mut hasher);
        summary.output_tokens.hash(&mut hasher);
        summary.cache_read_tokens.hash(&mut hasher);
        summary.cost_usd.to_bits().hash(&mut hasher);
        summary.succeeded.hash(&mut hasher);
    }

    hasher.finish()
}

/// Build search text from the exact expanded reader projection. Entries whose
/// source/layout fingerprint is unchanged are moved from the previous index;
/// appending a message or changing the active tail therefore renders only the
/// affected cells instead of rebuilding the whole reader corpus.
pub(crate) fn build_search_entries(
    state: &AppState,
    width: u16,
    stream_revision: Option<u64>,
    previous_entries: Vec<TranscriptSearchEntry>,
    previous_revisions: HashMap<TranscriptCellId, u64>,
) -> SearchEntriesBuild {
    let cells = state.session.transcript.cells();
    let presentation = transcript_presentation_with_cells(state, cells);
    let search = TranscriptSearch::default();
    let renderer = TranscriptCellRenderer::new(
        cells,
        state,
        &search,
        UiStyles::new(&state.ui.theme),
        width.max(1),
    );
    let mut groups: Vec<(TranscriptCellId, Vec<usize>)> = Vec::new();
    for (index, cell) in presentation.cells.iter().enumerate() {
        let Some(cell_id) = cell.cell_id(cells) else {
            continue;
        };
        if let Some((last_id, indices)) = groups.last_mut()
            && last_id == &cell_id
        {
            indices.push(index);
        } else {
            groups.push((cell_id, vec![index]));
        }
    }

    let mut old_entries = previous_entries
        .into_iter()
        .filter_map(|entry| {
            let revision = previous_revisions.get(&entry.cell_id).copied()?;
            Some((entry.cell_id.clone(), (revision, entry)))
        })
        .collect::<HashMap<_, _>>();
    let mut entries = Vec::with_capacity(groups.len());
    let mut revisions = HashMap::with_capacity(groups.len());
    #[cfg(test)]
    let mut reused_entries = 0usize;

    for (cell_id, indices) in groups {
        let revision = search_entry_revision(
            state,
            cells,
            &presentation.cells,
            &indices,
            width.max(1),
            stream_revision,
        );
        revisions.insert(cell_id.clone(), revision);
        if let Some((old_revision, entry)) = old_entries.remove(&cell_id)
            && old_revision == revision
        {
            entries.push(entry);
            #[cfg(test)]
            {
                reused_entries += 1;
            }
            continue;
        }

        let lines = indices
            .into_iter()
            .flat_map(|index| {
                renderer.render_cell(
                    &presentation.cells[index],
                    /*expanded*/ true,
                    /*selected*/ false,
                )
            })
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect()
            })
            .collect::<Vec<String>>();
        entries.push(TranscriptSearchEntry {
            cell_id,
            lines: crate::transcript::search::index_lines(lines, width.max(1)),
        });
    }

    SearchEntriesBuild {
        entries,
        revisions,
        #[cfg(test)]
        reused_entries,
    }
}

fn search_entry_revision(
    state: &AppState,
    cells: &[RenderedCell],
    presentation: &[TranscriptSourceCell<'_>],
    indices: &[usize],
    width: u16,
    stream_revision: Option<u64>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    width.hash(&mut hasher);
    state.session.working_dir.hash(&mut hasher);
    for index in indices {
        match &presentation[*index] {
            TranscriptSourceCell::Committed(cell) => {
                cell.hash(&mut hasher);
                match cell {
                    TranscriptCell::MetaPreview { index } | TranscriptCell::Cell { index } => {
                        hash_rendered_cell_revision(state, cells, *index, &mut hasher);
                    }
                    TranscriptCell::ToolCall {
                        invocation, result, ..
                    } => {
                        if let Some(index) = invocation {
                            hash_rendered_cell_revision(state, cells, *index, &mut hasher);
                        }
                        if let Some(index) = result {
                            hash_rendered_cell_revision(state, cells, *index, &mut hasher);
                        }
                    }
                    TranscriptCell::ToolBatch { start, end, .. } => {
                        for index in *start..*end {
                            hash_rendered_cell_revision(state, cells, index, &mut hasher);
                        }
                    }
                }
            }
            TranscriptSourceCell::Active(_) => {
                stream_revision.hash(&mut hasher);
                for tool in &state.session.tool_executions {
                    tool.call_id.hash(&mut hasher);
                    tool.name.hash(&mut hasher);
                    tool.status.hash(&mut hasher);
                    tool.description.hash(&mut hasher);
                    tool.input_preview.hash(&mut hasher);
                    format_duration_seconds(tool.elapsed()).hash(&mut hasher);
                }
            }
        }
    }
    hasher.finish()
}

fn hash_rendered_cell_revision(
    state: &AppState,
    cells: &[RenderedCell],
    index: usize,
    hasher: &mut impl Hasher,
) {
    let Some(cell) = cells.get(index) else {
        return;
    };
    cell.message_uuid.hash(hasher);
    (Arc::as_ptr(&cell.source) as usize).hash(hasher);
    if let Some(metadata) = state.session.reasoning_metadata.get(&cell.message_uuid) {
        metadata.duration_ms.hash(hasher);
        metadata.reasoning_tokens.hash(hasher);
    }
    match &cell.kind {
        CellKind::ToolUse { call_id, .. } => {
            if let Some(tool) = state
                .session
                .tool_executions
                .iter()
                .find(|tool| &tool.call_id == call_id)
            {
                tool.status.hash(hasher);
                format_duration_seconds(tool.elapsed()).hash(hasher);
            }
        }
        CellKind::ToolResult { .. } => {
            let Message::ToolResult(result) = cell.source.as_ref() else {
                return;
            };
            if let Some(summary) = state.session.subagent_summaries.get(&result.tool_use_id) {
                summary.agent_type.hash(hasher);
                summary.tool_count.hash(hasher);
                summary.duration_ms.hash(hasher);
                summary.input_tokens.hash(hasher);
                summary.output_tokens.hash(hasher);
                summary.cache_read_tokens.hash(hasher);
                summary.cost_usd.to_bits().hash(hasher);
                summary.succeeded.hash(hasher);
            }
        }
        CellKind::UserText { .. }
        | CellKind::AssistantText { .. }
        | CellKind::AssistantThinking { .. }
        | CellKind::AssistantRedactedThinking { .. }
        | CellKind::Attachment
        | CellKind::System(_) => {}
    }
}
