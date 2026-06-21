use coco_messages::Message;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;

pub(crate) struct SlashCommandRenderOptions<'a> {
    pub(crate) styles: UiStyles<'a>,
    pub(crate) width: u16,
    pub(crate) syntax_highlighting: SyntaxHighlighting,
    pub(crate) apply_user_background: bool,
}

pub(crate) fn render_slash_command_user_text(
    source: &Message,
    text: &str,
    opts: SlashCommandRenderOptions<'_>,
) -> Option<Vec<Line<'static>>> {
    if !is_slash_command_origin(source) {
        return None;
    }
    if coco_messages::is_command_input(text) {
        return Some(render_command_echo(text, opts));
    }
    if coco_messages::is_local_command_output(text) {
        return Some(render_command_output(text, opts));
    }
    None
}

fn is_slash_command_origin(source: &Message) -> bool {
    matches!(
        source,
        Message::User(u) if u.origin == Some(coco_messages::MessageOrigin::SlashCommand)
    )
}

fn render_command_echo(text: &str, opts: SlashCommandRenderOptions<'_>) -> Vec<Line<'static>> {
    let name = coco_messages::extract_tag(text, coco_messages::COMMAND_NAME_TAG).unwrap_or("");
    let args = coco_messages::extract_tag(text, coco_messages::COMMAND_ARGS_TAG).unwrap_or("");
    let echo = if args.is_empty() {
        name.to_string()
    } else {
        format!("{name} {args}")
    };
    let span = Span::raw(format!("❯ {echo}")).fg(opts.styles.user_message());
    let mut line = Line::from(span);
    if opts.apply_user_background
        && let Some(bg) = opts.styles.user_message_bg()
    {
        line = line.style(ratatui::style::Style::default().bg(bg));
    }
    vec![line]
}

fn render_command_output(text: &str, opts: SlashCommandRenderOptions<'_>) -> Vec<Line<'static>> {
    // A `<local-command-stderr>` body means the command failed — render it in
    // the error color as plain `⎿ error` rows (the slash analogue of a tool's
    // error result). `<local-command-stdout>` keeps the markdown + dim path.
    if let Some(err) = coco_messages::extract_tag(text, coco_messages::LOCAL_COMMAND_STDERR_TAG) {
        return render_error_output(err, opts.styles.error());
    }
    let body =
        coco_messages::extract_tag(text, coco_messages::LOCAL_COMMAND_STDOUT_TAG).unwrap_or("");
    if body.is_empty() {
        return Vec::new();
    }
    let md_opts = coco_tui_markdown::MarkdownOptions::new(
        opts.styles,
        opts.width.saturating_sub(4),
        opts.syntax_highlighting,
    );
    coco_tui_markdown::render_markdown(body, md_opts, None)
        .into_iter()
        .enumerate()
        .map(|(index, mut line)| {
            let prefix = if index == 0 { "  └ " } else { "    " };
            line.spans
                .insert(0, Span::raw(prefix).fg(opts.styles.dim()));
            line
        })
        .collect()
}

/// Render a stderr body as plain `⎿ error` rows in the given color. Kept
/// markdown-free so error text (paths, reasons) shows verbatim like a tool
/// result's error row.
fn render_error_output(body: &str, color: ratatui::style::Color) -> Vec<Line<'static>> {
    if body.is_empty() {
        return Vec::new();
    }
    body.lines()
        .enumerate()
        .map(|(index, line)| {
            let prefix = if index == 0 { "  └ " } else { "    " };
            Line::from(vec![
                Span::raw(prefix).fg(color),
                Span::raw(line.to_string()).fg(color),
            ])
        })
        .collect()
}
