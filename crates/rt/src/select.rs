//! Pure logic for the anchored selection mode (see
//! docs/superpowers/plans/2026-07-24-anchored-selection.md). No I/O, no winit —
//! just how the selection head moves and how its status reads, so both are
//! unit-testable without the event loop.

/// A keyboard navigation applied to the selection head while composing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    PageUp,
    PageDown,
    BufTop,
    BufBottom,
}

/// The range the head may move within, in the pane's coordinates. `cols` is the
/// grid width; `min_line`/`max_line` are the inclusive ABSOLUTE-line bounds the
/// buffer offers (oldest scrollback line .. newest live line — recall scrollback
/// lines are negative); `page` is the visible row count (for Page moves).
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub cols: usize,
    pub min_line: i32,
    pub max_line: i32,
    pub page: usize,
}

/// Move `head` one step for `nav`, clamped to `b`. Column clamps to
/// `[0, cols-1]`; line clamps to `[min_line, max_line]`. Left/Right stay on the
/// current line (use Up/Down to change line); jumps (Home/End/Page/Buf*) are
/// absolute.
pub fn move_head(head: (usize, i32), nav: Nav, b: Bounds) -> (usize, i32) {
    let (col, line) = (head.0, head.1);
    let max_col = b.cols.saturating_sub(1);
    let clamp_line = |l: i32| l.clamp(b.min_line, b.max_line);
    let (col, line) = match nav {
        Nav::Left => (col.saturating_sub(1), line),
        Nav::Right => ((col + 1).min(max_col), line),
        Nav::Up => (col, clamp_line(line - 1)),
        Nav::Down => (col, clamp_line(line + 1)),
        Nav::LineStart => (0, line),
        Nav::LineEnd => (max_col, line),
        Nav::PageUp => (col, clamp_line(line - b.page as i32)),
        Nav::PageDown => (col, clamp_line(line + b.page as i32)),
        Nav::BufTop => (0, b.min_line),
        Nav::BufBottom => (max_col, b.max_line),
    };
    (col.min(max_col), line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b() -> Bounds {
        Bounds { cols: 80, min_line: -100, max_line: 23, page: 24 }
    }

    #[test]
    fn arrows_move_one_cell_or_row_and_clamp_to_bounds() {
        // Right/Down advance; Left/Up retreat.
        assert_eq!(move_head((5, 0), Nav::Right, b()), (6, 0));
        assert_eq!(move_head((5, 0), Nav::Left, b()), (4, 0));
        assert_eq!(move_head((5, 0), Nav::Down, b()), (5, 1));
        assert_eq!(move_head((5, 0), Nav::Up, b()), (5, -1));
        // Column clamps at both ends of the row.
        assert_eq!(move_head((0, 0), Nav::Left, b()), (0, 0));
        assert_eq!(move_head((79, 0), Nav::Right, b()), (79, 0));
        // Line clamps to the buffer bounds.
        assert_eq!(move_head((5, 23), Nav::Down, b()), (5, 23));
        assert_eq!(move_head((5, -100), Nav::Up, b()), (5, -100));
    }

    #[test]
    fn home_end_hit_the_line_edges_and_page_moves_a_screenful() {
        assert_eq!(move_head((40, 5), Nav::LineStart, b()), (0, 5));
        assert_eq!(move_head((40, 5), Nav::LineEnd, b()), (79, 5));
        assert_eq!(move_head((10, 5), Nav::PageDown, b()), (10, 23)); // 5+24 clamps to 23
        assert_eq!(move_head((10, 5), Nav::PageUp, b()), (10, -19)); // 5-24
    }

    #[test]
    fn buffer_ends_jump_to_the_extremes() {
        assert_eq!(move_head((10, 5), Nav::BufTop, b()), (0, -100));
        assert_eq!(move_head((10, 5), Nav::BufBottom, b()), (79, 23));
    }

    #[test]
    fn a_zero_width_grid_keeps_the_column_at_zero() {
        let z = Bounds { cols: 0, min_line: -5, max_line: 5, page: 10 };
        assert_eq!(move_head((0, 0), Nav::Right, z), (0, 0));
        assert_eq!(move_head((0, 0), Nav::LineEnd, z), (0, 0));
    }
}
