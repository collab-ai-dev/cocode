//! Bounded expansion queue for tool output already frozen in scrollback.

use std::collections::VecDeque;

use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use crate::transcript::render::HistoryLineRenderOptions;
use crate::transcript::render::render_committed_tool_pair;
use crate::transcript::render::render_committed_tool_pair_lines;
use coco_tui_ui::engine::history_insert::HistoryRows;
use coco_tui_ui::engine::history_insert::render_history_rows_with_base_dir;
use coco_tui_ui::engine::terminal::SurfaceBackend;
use coco_tui_ui::engine::terminal::SurfaceTerminal;

const RING_CAPACITY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommittedToolKey {
    message_uuid: uuid::Uuid,
    call_id: String,
}

#[derive(Debug)]
pub(crate) struct PreparedCommittedToolReprint {
    key: CommittedToolKey,
    pub(crate) rows: HistoryRows,
}

impl PreparedCommittedToolReprint {
    pub(crate) fn expected_rows(&self) -> u16 {
        self.rows.height()
    }
}

#[derive(Debug, Default)]
pub(crate) struct CommittedToolExpand {
    ring: VecDeque<CommittedToolKey>,
    pending: VecDeque<CommittedToolKey>,
}

impl CommittedToolExpand {
    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(crate) fn request(&mut self, cells: &[RenderedCell]) -> bool {
        while let Some(key) = self.ring.pop_back() {
            if find_tool_pair(cells, &key).is_some() {
                self.pending.push_back(key);
                return true;
            }
        }
        false
    }

    pub(crate) fn prepare(
        &mut self,
        cells: &[RenderedCell],
        options: HistoryLineRenderOptions<'_>,
    ) -> Option<PreparedCommittedToolReprint> {
        loop {
            let key = self.pending.front()?.clone();
            let Some((invocation, result)) = find_tool_pair(cells, &key) else {
                self.pending.pop_front();
                continue;
            };
            let turn = cells[..=invocation]
                .iter()
                .filter(|cell| matches!(cell.kind, CellKind::UserText { .. }))
                .count()
                .max(1);
            let mut lines = vec![Line::from(vec![
                Span::raw("  ↳ ").fg(options.styles.secondary()),
                Span::raw(crate::i18n::t!("chat.reprinted_from_turn", turn = turn).to_string())
                    .style(options.styles.dim_style()),
            ])];
            lines.extend(render_committed_tool_pair_lines(
                cells, invocation, result, options, true,
            ));
            return Some(PreparedCommittedToolReprint {
                key,
                rows: render_history_rows_with_base_dir(
                    lines,
                    options.width,
                    options.cwd.map(std::path::Path::new),
                ),
            });
        }
    }

    pub(crate) fn commit<B>(
        &mut self,
        terminal: &mut SurfaceTerminal<B>,
        prepared: &PreparedCommittedToolReprint,
    ) -> Result<u16, B::Error>
    where
        B: SurfaceBackend,
    {
        self.commit_with(prepared, |rows| terminal.insert_history_rows(rows))
    }

    fn commit_with<E>(
        &mut self,
        prepared: &PreparedCommittedToolReprint,
        insert: impl FnOnce(&HistoryRows) -> Result<u16, E>,
    ) -> Result<u16, E> {
        let rows = insert(&prepared.rows)?;
        if self.pending.front() == Some(&prepared.key) {
            self.pending.pop_front();
        }
        tracing::debug!(
            target: "tui::surface::insert",
            kind = "committed_tool_reprint",
            message_uuid = %prepared.key.message_uuid,
            call_id = %prepared.key.call_id,
            rows,
            "scrollback insert: expanded committed tool re-print",
        );
        Ok(rows)
    }

    pub(crate) fn record(&mut self, keys: impl IntoIterator<Item = CommittedToolKey>) {
        for key in keys {
            if self.pending.contains(&key) {
                continue;
            }
            self.ring.retain(|existing| existing != &key);
            self.ring.push_back(key);
            while self.ring.len() > RING_CAPACITY {
                self.ring.pop_front();
            }
        }
    }

    pub(crate) fn replace_ring(&mut self, keys: impl IntoIterator<Item = CommittedToolKey>) {
        self.ring.clear();
        self.record(keys);
    }

    pub(crate) fn reset(&mut self) {
        self.ring.clear();
        self.pending.clear();
    }
}

pub(crate) fn collapsed_tool_keys(
    cells: &[RenderedCell],
    start: usize,
    end: usize,
    options: HistoryLineRenderOptions<'_>,
) -> Vec<CommittedToolKey> {
    let mut keys = Vec::new();
    let mut results = std::collections::HashMap::<&str, VecDeque<usize>>::new();
    for (index, cell) in cells.iter().enumerate().take(end).skip(start) {
        if let CellKind::ToolResult { call_id } = &cell.kind {
            results.entry(call_id).or_default().push_back(index);
        }
    }
    for invocation in start..end {
        let CellKind::ToolUse { call_id, .. } = &cells[invocation].kind else {
            continue;
        };
        let Some(candidates) = results.get_mut(call_id.as_str()) else {
            continue;
        };
        while candidates
            .front()
            .is_some_and(|result| *result <= invocation)
        {
            candidates.pop_front();
        }
        let Some(result) = candidates.pop_front() else {
            continue;
        };
        if render_committed_tool_pair(cells, invocation, result, options, false).truncated {
            keys.push(CommittedToolKey {
                message_uuid: cells[invocation].message_uuid,
                call_id: call_id.clone(),
            });
        }
    }
    keys
}

fn find_tool_pair(cells: &[RenderedCell], key: &CommittedToolKey) -> Option<(usize, usize)> {
    let invocation = cells.iter().position(|cell| {
        cell.message_uuid == key.message_uuid
            && matches!(
                &cell.kind,
                CellKind::ToolUse { call_id, .. } if call_id == &key.call_id
            )
    })?;
    let result = cells
        .iter()
        .enumerate()
        .skip(invocation + 1)
        .find_map(|(index, cell)| {
            matches!(
                &cell.kind,
                CellKind::ToolResult { call_id } if call_id == &key.call_id
            )
            .then_some(index)
        })?;
    Some((invocation, result))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(index: usize) -> CommittedToolKey {
        CommittedToolKey {
            message_uuid: uuid::Uuid::from_u128(index as u128 + 1),
            call_id: format!("call-{index}"),
        }
    }

    #[test]
    fn ring_evicts_the_oldest_entry_at_capacity() {
        let mut expand = CommittedToolExpand::default();
        expand.record((0..=RING_CAPACITY).map(key));

        assert_eq!(expand.ring.len(), RING_CAPACITY);
        assert_eq!(expand.ring.front(), Some(&key(1)));
        assert_eq!(expand.ring.back(), Some(&key(RING_CAPACITY)));
    }

    #[test]
    fn replay_replacement_rebuilds_ring_and_clears_old_entries() {
        let mut expand = CommittedToolExpand::default();
        expand.record([key(1), key(2)]);

        expand.replace_ring([key(9)]);

        assert_eq!(expand.ring, VecDeque::from([key(9)]));
        assert!(expand.pending.is_empty());
    }

    #[test]
    fn failed_insert_keeps_pending_request_retryable() {
        let pending = key(3);
        let prepared = PreparedCommittedToolReprint {
            key: pending.clone(),
            rows: coco_tui_ui::engine::history_insert::render_history_rows(
                vec![Line::from("expanded")],
                20,
            ),
        };
        let mut expand = CommittedToolExpand::default();
        expand.pending.push_back(pending);

        let error = expand
            .commit_with(&prepared, |_| Err::<u16, _>("insert failed"))
            .expect_err("first insert fails");

        assert_eq!(error, "insert failed");
        assert_eq!(expand.pending.front(), Some(&prepared.key));
        assert_eq!(
            expand
                .commit_with(&prepared, |_| Ok::<u16, &str>(1))
                .expect("retry succeeds"),
            1
        );
        assert!(expand.pending.is_empty());
    }

    #[test]
    fn stale_pending_entry_is_discarded_during_prepare() {
        let mut expand = CommittedToolExpand::default();
        expand.pending.push_back(key(4));
        let theme = crate::theme::Theme::default();
        let options = HistoryLineRenderOptions {
            styles: coco_tui_ui::style::UiStyles::new(&theme),
            width: 40,
            syntax_highlighting: coco_tui_ui::display::SyntaxHighlighting::Off,
            show_system_reminders: false,
            show_thinking: false,
            hyperlinks_enabled: false,
            cwd: None,
            kb_handle: None,
            replay_cache_policy: Default::default(),
            reasoning_metadata: None,
            subagent_summaries: None,
        };

        assert!(expand.prepare(&[], options).is_none());
        assert!(expand.pending.is_empty());
    }

    #[test]
    fn successful_commit_leaves_second_request_for_a_follow_up_frame() {
        let first = key(5);
        let second = key(6);
        let prepared = PreparedCommittedToolReprint {
            key: first.clone(),
            rows: coco_tui_ui::engine::history_insert::render_history_rows(
                vec![Line::from("first expanded")],
                20,
            ),
        };
        let mut expand = CommittedToolExpand::default();
        expand.pending.extend([first, second.clone()]);

        expand
            .commit_with(&prepared, |_| Ok::<u16, &str>(1))
            .expect("first coalesced request commits");

        assert!(expand.has_pending());
        assert_eq!(expand.pending.front(), Some(&second));
    }
}
