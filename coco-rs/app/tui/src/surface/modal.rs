//! Full-screen modal placement and rendering for native-scrollback surfaces.

use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::Padding;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

use crate::presentation::layout;
use crate::presentation::layout::text_width;
use crate::state::AppState;
use crate::state::ModalState;
use crate::widgets::TranscriptLayoutIndex;
use coco_tui_ui::engine::compatibility::TerminalCompatibility;
use coco_tui_ui::engine::terminal::SurfaceFrame;
use coco_tui_ui::style::UiStyles;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModalSurfacePlacement {
    AltScreen,
}

const DEFAULT_OVERLAY_MIN_WIDTH: u16 = 40;
const DEFAULT_OVERLAY_MAX_WIDTH: u16 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ModalBoxPolicy {
    width_percent: u16,
    min_width: u16,
    max_width: u16,
    min_height: u16,
    max_height: u16,
}

const DEFAULT_MODAL_POLICY: ModalBoxPolicy = ModalBoxPolicy {
    width_percent: 70,
    min_width: DEFAULT_OVERLAY_MIN_WIDTH,
    max_width: DEFAULT_OVERLAY_MAX_WIDTH,
    min_height: 1,
    max_height: u16::MAX,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistorySurfaceMode {
    NativeScrollback,
    Viewport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SurfaceFramePlan {
    pub(crate) modal_placement: Option<ModalSurfacePlacement>,
    pub(crate) history_surface: HistorySurfaceMode,
    pub(crate) attention_requested: bool,
}

impl SurfaceFramePlan {
    pub(crate) fn for_compatibility(compatibility: TerminalCompatibility) -> Self {
        Self {
            modal_placement: None,
            history_surface: history_surface_mode(compatibility),
            attention_requested: false,
        }
    }

    pub(crate) fn native_history_enabled(self) -> bool {
        self.history_surface == HistorySurfaceMode::NativeScrollback
            && !self.history_emission_deferred()
    }

    pub(crate) fn finalized_history_in_viewport(self) -> bool {
        self.history_surface == HistorySurfaceMode::Viewport
    }

    pub(crate) fn history_emission_deferred(self) -> bool {
        self.modal_placement
            .is_some_and(|placement| matches!(placement, ModalSurfacePlacement::AltScreen))
    }
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ModalSurfaceState {
    latched: Option<LatchedModalSurface>,
}

#[derive(Debug, Clone)]
struct LatchedModalSurface {
    generation: u64,
    placement: ModalSurfacePlacement,
}

impl ModalSurfaceState {
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn plan(
        &mut self,
        state: &AppState,
        compatibility: TerminalCompatibility,
        now: Instant,
    ) -> SurfaceFramePlan {
        self.plan_inner(state, compatibility, now, None)
    }

    pub(crate) fn plan_for_native_viewport(
        &mut self,
        state: &AppState,
        compatibility: TerminalCompatibility,
        now: Instant,
        width: u16,
        max_height: u16,
    ) -> SurfaceFramePlan {
        self.plan_inner(state, compatibility, now, Some((width, max_height)))
    }

    fn plan_inner(
        &mut self,
        state: &AppState,
        compatibility: TerminalCompatibility,
        _now: Instant,
        _native_viewport_bounds: Option<(u16, u16)>,
    ) -> SurfaceFramePlan {
        let Some(modal) = state.ui.modal.as_ref() else {
            self.latched = None;
            return SurfaceFramePlan::for_compatibility(compatibility);
        };

        let generation = state.ui.surface_generation();
        let previous = self
            .latched
            .as_ref()
            .filter(|latched| latched.generation == generation);
        if let Some(latched) = previous {
            return SurfaceFramePlan {
                modal_placement: Some(latched.placement),
                history_surface: history_surface_mode(compatibility),
                attention_requested: false,
            };
        }

        let Some(static_placement) = modal_surface_placement(Some(modal)) else {
            self.latched = None;
            return SurfaceFramePlan::for_compatibility(compatibility);
        };
        let placement = static_placement;
        self.latched = Some(LatchedModalSurface {
            generation,
            placement,
        });

        SurfaceFramePlan {
            modal_placement: Some(placement),
            history_surface: history_surface_mode(compatibility),
            attention_requested: false,
        }
    }
}

pub(crate) fn modal_surface_placement(modal: Option<&ModalState>) -> Option<ModalSurfacePlacement> {
    modal.map(|_| ModalSurfacePlacement::AltScreen)
}

#[cfg(test)]
pub(crate) fn modal_surface_placement_for_state(
    state: &AppState,
    now: Instant,
) -> Option<ModalSurfacePlacement> {
    let mut modal_state = ModalSurfaceState::default();
    modal_state
        .plan(state, TerminalCompatibility::NativeScrollback, now)
        .modal_placement
}

fn history_surface_mode(compatibility: TerminalCompatibility) -> HistorySurfaceMode {
    if compatibility.native_scrollback_enabled() {
        HistorySurfaceMode::NativeScrollback
    } else {
        HistorySurfaceMode::Viewport
    }
}

#[cfg(test)]
pub(crate) fn history_emission_deferred(modal: Option<&ModalState>) -> bool {
    modal_surface_placement(modal)
        .is_some_and(|placement| matches!(placement, ModalSurfacePlacement::AltScreen))
}

#[cfg(test)]
pub(crate) fn history_emission_deferred_for_state(state: &AppState, now: Instant) -> bool {
    modal_surface_placement_for_state(state, now)
        .is_some_and(|placement| matches!(placement, ModalSurfacePlacement::AltScreen))
}

pub(crate) fn render_modal_surface(
    frame: &mut SurfaceFrame<'_>,
    area: Rect,
    input_area: Option<Rect>,
    modal: &ModalState,
    state: &AppState,
    transcript_layout: &mut TranscriptLayoutIndex,
    styles: UiStyles<'_>,
) {
    if modal_surface_placement(Some(modal)).is_none() {
        return;
    }

    if let ModalState::Transcript(transcript) = modal {
        frame.render_widget(Clear, area);
        frame.render_widget(
            crate::widgets::TranscriptStateWidget::new(
                state,
                transcript,
                transcript_layout,
                styles,
            ),
            area,
        );
        return;
    }

    if let ModalState::ThemePicker(picker) = modal {
        let placement_area = modal_placement_area(area, input_area, modal);
        if placement_area.height == 0 || placement_area.width == 0 {
            return;
        }
        let box_width = placement_area.width.clamp(40, 80);
        // Inner content width = box_width − 2 border − 2 horizontal padding.
        let inner_width = box_width.saturating_sub(4).max(1);
        // Reserve rows for the fixed chrome (title/subtitle/blanks/diff box +
        // rules/syntax/footer) + 2 border rows; the theme list takes whatever
        // is left, so the footer always renders even on a short terminal.
        const CHROME_ROWS: u16 = 16;
        let list_visible = placement_area.height.saturating_sub(CHROME_ROWS).max(3) as usize;
        let lines = crate::presentation::theme_picker::theme_picker_lines(
            picker,
            state.ui.display_settings.syntax_highlighting,
            styles,
            inner_width,
            list_visible,
        );
        let box_height = (lines.len() as u16 + 2).min(placement_area.height);
        let modal_area = layout::centered_fixed_area(placement_area, box_width, box_height);
        frame.render_widget(Clear, modal_area);
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .padding(Padding::horizontal(1))
                    .border_style(Style::default().fg(styles.accent())),
            ),
            modal_area,
        );
        return;
    }

    // List dialogs migrated onto `render_select_list` render styled `Line`s
    // (colored cursor / markers) instead of a monochrome text body.
    {
        let placement_area = modal_placement_area(area, input_area, modal);
        // Vertical budget for the scrolling list: terminal height minus the
        // dialog chrome (filter/query/intro + hint) and borders. The widget
        // windows the *full* list to this many rows so the focused row stays
        // visible (no pre-truncation) and the footer always renders.
        let list_budget = placement_area.height.saturating_sub(8).max(3) as usize;
        if let Some((title, lines, border)) =
            crate::surface_content::modal_styled_surface(modal, state, styles, list_budget)
        {
            render_styled_modal_box(frame, placement_area, &title, lines, border);
            return;
        }
    }

    let Some(text_surface) = crate::surface_content::modal_text_surface(modal) else {
        return;
    };
    let (title, body, border_color) =
        crate::surface_content::surface_content(text_surface, state, styles);
    let policy = modal_box_policy(modal);
    let placement_area = modal_placement_area(area, input_area, modal);
    if placement_area.height == 0 {
        return;
    }
    let (width, height) = modal_box_size(placement_area, &body, policy);
    let modal_area = layout::centered_fixed_area(placement_area, width, height);

    frame.render_widget(Clear, modal_area);
    frame.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(border_color)),
        ),
        modal_area,
    );
}

/// Render a pre-built styled body inside a bordered, centered box. Shared by
/// the list dialogs migrated onto `render_select_list`; the box sizes to the
/// content (capped to the placement area).
fn render_styled_modal_box(
    frame: &mut SurfaceFrame<'_>,
    placement_area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    border: Color,
) {
    if placement_area.height == 0 || placement_area.width == 0 {
        return;
    }
    let content_w = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let title_w = text_width(title) as u16;
    // +4 = 2 border + 2 horizontal padding.
    let box_width = (content_w.max(title_w) + 4).clamp(40, placement_area.width.min(100));
    let box_height = (lines.len() as u16 + 2).min(placement_area.height);
    let modal_area = layout::centered_fixed_area(placement_area, box_width, box_height);

    frame.render_widget(Clear, modal_area);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
        .border_style(Style::default().fg(border));
    if !title.is_empty() {
        // The dialog title i18n values already carry their own surrounding
        // spaces (e.g. " Export Transcript "), matching the text-surface path.
        block = block.title(title.to_string());
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block),
        modal_area,
    );
}

fn modal_placement_area(area: Rect, input_area: Option<Rect>, modal: &ModalState) -> Rect {
    let _ = (input_area, modal);
    area
}

/// Required height to render `text_surface` inside a `Borders::ALL` box of
/// exactly `box_width` columns (no modal width-policy reapplied). Used by
/// the inline interaction-prompt pane where the box width is already chosen
/// by the caller.
pub(crate) fn required_text_surface_height_for_box(
    text_surface: crate::surface_content::TextSurfaceContent<'_>,
    state: &AppState,
    styles: UiStyles<'_>,
    box_width: u16,
    max_height: u16,
) -> u16 {
    if box_width == 0 || max_height == 0 {
        return 0;
    }
    let (_, body, _) = crate::surface_content::surface_content(text_surface, state, styles);
    let inner_width = box_width.saturating_sub(2).max(1) as usize;
    let wrapped_body_rows = body
        .lines()
        .map(|line| {
            let line_width = text_width(line);
            line_width.saturating_add(inner_width - 1) / inner_width
        })
        .map(|rows| rows.max(1))
        .sum::<usize>();
    // +2 for top/bottom border. Title sits on the top border, so no extra
    // row is needed for it.
    wrapped_body_rows.saturating_add(2).min(max_height as usize) as u16
}

fn modal_box_policy(modal: &ModalState) -> ModalBoxPolicy {
    match modal_surface_placement(Some(modal)) {
        Some(ModalSurfacePlacement::AltScreen) | None => DEFAULT_MODAL_POLICY,
    }
}

fn modal_box_size(area: Rect, body: &str, policy: ModalBoxPolicy) -> (u16, u16) {
    let available_width = area.width.saturating_sub(2).max(1);
    let width = ((area.width as u32 * u32::from(policy.width_percent) / 100) as u16)
        .clamp(policy.min_width.min(available_width), policy.max_width)
        .min(available_width);
    let inner_width = width.saturating_sub(2).max(1) as usize;
    let wrapped_body_rows = body
        .lines()
        .map(|line| {
            let line_width = text_width(line);
            line_width.saturating_add(inner_width - 1) / inner_width
        })
        .map(|rows| rows.max(1))
        .sum::<usize>();
    let content_height = wrapped_body_rows.saturating_add(4).min(u16::MAX as usize) as u16;
    let available_height = area.height.saturating_sub(2).max(1);
    let height = content_height
        .clamp(
            policy.min_height.min(available_height),
            policy.max_height.min(available_height),
        )
        .min(available_height);
    (width, height)
}

#[cfg(test)]
#[path = "modal.test.rs"]
mod tests;
