//! Mermaid diagram rendering for the coco TUI, as Unicode box-drawing cells.
//!
//! Reuses `mermaid-rs-renderer`'s pure-Rust parse + geometric layout (the
//! `parse_mermaid` → `compute_layout` half of its pipeline) and projects the
//! resulting `Layout` onto a character grid — **no rasterization and no
//! terminal graphics protocol**, so output is ordinary styled `Line`s that flow
//! through coco's native-scrollback engine like any other text.
//!
//! Scope: box-and-arrow graphs (`flowchart` / `classDiagram` / `stateDiagram` /
//! `erDiagram`, all `DiagramData::Graph`). Continuous-geometry diagrams (pie,
//! gantt, sankey, sequence, …) and any layout too dense to read at the given
//! width return `None`, signalling the caller to fall back to the verbatim code
//! fence — never worse than today's behavior.

use coco_tui_ui::style::UiStyles;
use mermaid_rs_renderer::EdgeLayout;
use mermaid_rs_renderer::Layout;
use mermaid_rs_renderer::RenderOptions;
use mermaid_rs_renderer::compute_layout;
use mermaid_rs_renderer::layout::DiagramData;
use mermaid_rs_renderer::parse_mermaid;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;

mod grid;
use grid::CellGrid;
use grid::truncate_to_width;

/// Hard cap on diagram height (rows); taller layouts fall back to verbatim.
const MAX_ROWS: usize = 60;
/// A node must quantize to at least this many cells or the diagram is too dense.
const MIN_NODE_W: usize = 3;
const MIN_NODE_H: usize = 3;
/// Terminal cell width/height ratio — used to preserve the diagram's aspect
/// when mapping pixel geometry to the (taller-than-wide) character grid.
const CELL_ASPECT: f32 = 0.5;

/// Render a `mermaid` fence body to terminal cells, or `None` to request the
/// verbatim-fence fallback (unsupported diagram type, parse error, empty graph,
/// or a layout that will not fit `width` legibly). Never panics.
pub fn mermaid_to_lines(src: &str, styles: UiStyles<'_>, width: u16) -> Option<Vec<Line<'static>>> {
    let cols = (width as usize).saturating_sub(4);
    if cols < 12 {
        return None;
    }
    // The upstream parse/layout is infallible-by-trust but reaches third-party
    // ranking/routing code with internal unwrap/index sites. Contain any panic
    // so a pathological diagram degrades to the verbatim fence rather than
    // taking down the render — honoring the "never panics" contract. The guard
    // tells the app's global panic hook this panic is expected and recoverable,
    // so it does NOT restore the terminal or dump a backtrace mid-render.
    let _restore_guard = coco_tui_ui::panic_guard::PanicRestoreGuard::new();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let parsed = parse_mermaid(src).ok()?;
        let opts = RenderOptions::default();
        let layout = compute_layout(&parsed.graph, &opts.theme, &opts.layout);
        match &layout.diagram {
            DiagramData::Graph { .. } => render_graph(&layout, styles, cols),
            // Continuous-geometry / error diagrams: keep the source verbatim.
            _ => None,
        }
    }))
    .ok()
    .flatten()
}

fn quantize(px: f32, scale: f32) -> usize {
    // Panic-safety of the float→usize crossing rests on two saturating
    // behaviors: Rust's `as` cast saturates (+inf → usize::MAX, later bounded by
    // the grid_w/grid_h caps and CellGrid::set's bounds check), and
    // `f32::max(NaN, 0.0)` returns 0.0 (so non-finite geometry collapses to 0
    // instead of producing an out-of-range index). Keep this form — a "cleaner"
    // rewrite could reintroduce a panic on pathological layout coordinates.
    (px * scale).round().max(0.0) as usize
}

fn render_graph(layout: &Layout, styles: UiStyles<'_>, cols: usize) -> Option<Vec<Line<'static>>> {
    if layout.nodes.is_empty() {
        return None;
    }
    let lw = layout.width.max(1.0);
    let lh = layout.height.max(1.0);

    // Fit to `cols`, preserving aspect; clamp height to MAX_ROWS, re-deriving the
    // horizontal scale so the diagram stays proportional rather than squashed.
    let mut sx = cols as f32 / lw;
    let mut sy = sx * CELL_ASPECT;
    if lh * sy > MAX_ROWS as f32 {
        sy = MAX_ROWS as f32 / lh;
        sx = sy / CELL_ASPECT;
    }

    let grid_w = ((lw * sx).ceil() as usize + 1).min(cols);
    let grid_h = ((lh * sy).ceil() as usize + 1).min(MAX_ROWS + 1);
    if grid_w < 4 || grid_h < 3 {
        return None;
    }

    // Density guard: if any visible node collapses below a legible box, bail to
    // the verbatim fence rather than paint an unreadable tangle.
    for node in layout.nodes.values() {
        if node.hidden {
            continue;
        }
        let bw = quantize(node.x + node.width, sx).saturating_sub(quantize(node.x, sx));
        let bh = quantize(node.y + node.height, sy).saturating_sub(quantize(node.y, sy));
        if bw < MIN_NODE_W || bh < MIN_NODE_H {
            return None;
        }
    }

    let mut grid = CellGrid::new(grid_w, grid_h);
    let border = Style::default().fg(styles.border());
    let label = Style::default().fg(styles.text());
    let edge = Style::default().fg(styles.dim());
    let arrow = Style::default().fg(styles.primary());
    let sub = Style::default()
        .fg(styles.panel_border())
        .add_modifier(Modifier::DIM);

    // Subgraph containers first, so node boxes paint on top.
    for sg in &layout.subgraphs {
        let x = quantize(sg.x, sx);
        let y = quantize(sg.y, sy);
        let w = quantize(sg.x + sg.width, sx).saturating_sub(x).max(2);
        let h = quantize(sg.y + sg.height, sy).saturating_sub(y).max(2);
        grid.rect(x, y, w, h, sub);
        if !sg.label.is_empty() && w > 4 {
            grid.text(x + 1, y, &truncate_to_width(&sg.label, w - 2), sub);
        }
    }

    // Nodes.
    for node in layout.nodes.values() {
        if node.hidden {
            continue;
        }
        let x = quantize(node.x, sx);
        let y = quantize(node.y, sy);
        let w = quantize(node.x + node.width, sx)
            .saturating_sub(x)
            .max(MIN_NODE_W);
        let h = quantize(node.y + node.height, sy)
            .saturating_sub(y)
            .max(MIN_NODE_H);
        grid.rect(x, y, w, h, border);

        let inner_w = w.saturating_sub(2);
        let inner_h = h.saturating_sub(2);
        let shown = node.label.lines.len().min(inner_h);
        let start = y + 1 + inner_h.saturating_sub(shown) / 2;
        for (i, text) in node.label.lines.iter().take(inner_h).enumerate() {
            grid.text_centered(x + 1, start + i, inner_w, text, label);
        }
    }

    // Edges (routed polylines + arrowheads) last.
    for e in &layout.edges {
        draw_edge(&mut grid, e, sx, sy, edge, arrow);
        if let (Some(text), Some((ax, ay))) = (e.label.as_ref(), e.label_anchor)
            && let Some(first) = text.lines.first()
        {
            // Center the (truncated) label on the anchor and clamp to the grid
            // so it straddles the edge instead of starting at the anchor column.
            let gw = grid.width();
            let truncated = truncate_to_width(first, gw);
            let tw = coco_tui_ui::truncate::display_width(&truncated);
            let cx = quantize(ax, sx);
            let start = cx.saturating_sub(tw / 2).min(gw.saturating_sub(tw));
            // Only stamp onto a blank run so the label never overwrites a node
            // border or a box-interior label. Try the anchor row, then one row
            // above/below; if no clear run exists, drop the label — preserving
            // the diagram beats rendering an unreadable overlap.
            let ay0 = quantize(ay, sy);
            if let Some(row) = [ay0, ay0.saturating_sub(1), ay0 + 1]
                .into_iter()
                .find(|&r| grid.run_is_clear(start, r, tw))
            {
                grid.text(start, row, &truncated, label);
            }
        }
    }

    Some(grid.into_lines())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// Direction from `a` toward `b` (assumes `a != b`).
fn dir(a: (usize, usize), b: (usize, usize)) -> Dir {
    if b.0 > a.0 {
        Dir::Right
    } else if b.0 < a.0 {
        Dir::Left
    } else if b.1 > a.1 {
        Dir::Down
    } else {
        Dir::Up
    }
}

/// Rounded corner glyph for an interior vertex connecting toward `prev` and
/// `next`. `None` for a collinear pass-through (leave the straight stroke).
fn corner_glyph(prev: (usize, usize), cur: (usize, usize), next: (usize, usize)) -> Option<char> {
    use Dir::*;
    Some(match (dir(cur, prev), dir(cur, next)) {
        (Left, Down) | (Down, Left) => '╮',
        (Right, Down) | (Down, Right) => '╭',
        (Left, Up) | (Up, Left) => '╯',
        (Right, Up) | (Up, Right) => '╰',
        _ => return None,
    })
}

fn draw_edge(grid: &mut CellGrid, e: &EdgeLayout, sx: f32, sy: f32, line: Style, arrow: Style) {
    let mut raw: Vec<(usize, usize)> = e
        .points
        .iter()
        .map(|&(px, py)| (quantize(px, sx), quantize(py, sy)))
        .collect();
    raw.dedup();
    if raw.len() < 2 {
        return;
    }
    // Orthogonalize: a diagonal hop becomes an L by inserting its bend vertex,
    // so every segment is axis-aligned and every turn is an explicit vertex.
    let mut pts: Vec<(usize, usize)> = Vec::with_capacity(raw.len() * 2);
    pts.push(raw[0]);
    for w in raw.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        if x0 != x1 && y0 != y1 {
            pts.push((x1, y0));
        }
        pts.push((x1, y1));
    }
    pts.dedup();
    if pts.len() < 2 {
        return;
    }

    let n = pts.len();
    // Interior turn vertices that get a corner glyph. Exclude them from the
    // straight segments below so this edge's own hline+vline don't pre-fill the
    // bend cell with a `┼` — then the corner is *merged* (not overwritten), so a
    // bend shared with another edge's stroke connects (├┬┼…) instead of erasing
    // it. A lone bend merges onto a blank cell and stays a plain ╭╮╰╯.
    let turns: std::collections::HashSet<(usize, usize)> = (1..n.saturating_sub(1))
        .filter(|&i| corner_glyph(pts[i - 1], pts[i], pts[i + 1]).is_some())
        .map(|i| pts[i])
        .collect();
    for w in pts.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        if y0 == y1 {
            let lo = x0.min(x1);
            let hi = x0.max(x1);
            let lo = if turns.contains(&(lo, y0)) {
                lo + 1
            } else {
                lo
            };
            let hi = if turns.contains(&(hi, y0)) {
                hi.saturating_sub(1)
            } else {
                hi
            };
            if lo <= hi {
                grid.hline(lo, hi, y0, line);
            }
        } else {
            let lo = y0.min(y1);
            let hi = y0.max(y1);
            let lo = if turns.contains(&(x0, lo)) {
                lo + 1
            } else {
                lo
            };
            let hi = if turns.contains(&(x0, hi)) {
                hi.saturating_sub(1)
            } else {
                hi
            };
            if lo <= hi {
                grid.vline(x0, lo, hi, line);
            }
        }
    }
    for i in 1..n.saturating_sub(1) {
        if let Some(c) = corner_glyph(pts[i - 1], pts[i], pts[i + 1]) {
            grid.stroke(pts[i].0, pts[i].1, c, line);
        }
    }
    if e.arrow_end {
        let (tx, ty) = pts[n - 1];
        let (px, py) = pts[n - 2];
        grid.put(tx, ty, arrow_char(px, py, tx, ty), arrow);
    }
    if e.arrow_start {
        let (tx, ty) = pts[0];
        let (px, py) = pts[1];
        grid.put(tx, ty, arrow_char(px, py, tx, ty), arrow);
    }
}

/// Arrowhead glyph for a stroke arriving at `(tx,ty)` from `(fx,fy)`.
fn arrow_char(fx: usize, fy: usize, tx: usize, ty: usize) -> char {
    if tx > fx {
        '→'
    } else if tx < fx {
        '←'
    } else if ty > fy {
        '↓'
    } else {
        '↑'
    }
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
