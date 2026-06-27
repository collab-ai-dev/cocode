//! Toast notification widget — auto-expiring messages at top-right.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Span;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

use crate::state::ui::Toast;
use crate::state::ui::ToastSeverity;
use coco_tui_ui::style::UiStyles;

/// Toast notification display.
pub struct ToastWidget<'a> {
    toasts: &'a [Toast],
    styles: UiStyles<'a>,
}

impl<'a> ToastWidget<'a> {
    pub fn new(toasts: &'a [Toast], styles: UiStyles<'a>) -> Self {
        Self { toasts, styles }
    }
}

impl Widget for ToastWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let toast_width: u16 = 40;
        let mut y = 1_u16;

        for toast in self.toasts {
            if y >= area.height.saturating_sub(2) {
                break;
            }

            let (icon, color) = match toast.severity {
                ToastSeverity::Info => ("ℹ", self.styles.dim()),
                ToastSeverity::Success => ("✓", self.styles.success()),
                ToastSeverity::Warning => ("⚠", self.styles.warning()),
                ToastSeverity::Error => ("✗", self.styles.error()),
            };

            let x = area.width.saturating_sub(toast_width + 1);
            let toast_area = Rect::new(area.x + x, area.y + y, toast_width, 1);

            let max_msg_width = (toast_width as usize).saturating_sub(5);
            let msg = truncate_width(&toast.message, max_msg_width);

            let text = format!(" {icon} {msg} ");
            Clear.render(toast_area, buf);
            Paragraph::new(Span::raw(text).fg(color)).render(toast_area, buf);

            y += 1;
        }
    }
}

fn truncate_width(message: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(message) <= max_width {
        return message.to_string();
    }
    let ellipsis = "...";
    let content_width = max_width.saturating_sub(UnicodeWidthStr::width(ellipsis));
    let mut out = String::new();
    let mut width = 0;
    for ch in message.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width > content_width {
            break;
        }
        width += ch_width;
        out.push(ch);
    }
    out.push_str(ellipsis);
    out
}
