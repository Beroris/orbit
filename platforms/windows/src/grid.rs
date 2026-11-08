//! Laying out a list as a grid of tiles, for the emoji list. It still uses the view's rows, so
//! selecting and running one works as it does everywhere else. This only says where each one goes.

use crate::rows::Row;

pub const COLUMNS: usize = 10;
pub const CELL_DIP: i32 = 64;
pub const HEADER_DIP: i32 = 30;

/// One line down the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    /// A section's title, by its row.
    Title(usize),
    /// Tiles, from this row, this many across.
    Cells { start: usize, count: usize },
}

impl Line {
    fn height(self) -> i32 {
        match self {
            Line::Title(_) => HEADER_DIP,
            Line::Cells { .. } => CELL_DIP,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Grid {
    pub lines: Vec<Line>,
    /// How much height there is to show lines in, in DIPs.
    pub room: i32,
}

impl Grid {
    pub fn new(rows: &[Row], room: i32) -> Grid {
        let mut lines = Vec::with_capacity(rows.len() / COLUMNS + 8);
        let mut at = 0;
        while at < rows.len() {
            if rows[at].is_header() {
                lines.push(Line::Title(at));
                at += 1;
                continue;
            }
            let count = rows[at..].iter().take(COLUMNS).take_while(|row| !row.is_header()).count();
            lines.push(Line::Cells { start: at, count });
            at += count;
        }
        Grid { lines, room }
    }

    fn line_of(&self, row: usize) -> Option<usize> {
        self.lines.iter().position(|line| match *line {
            Line::Title(at) => at == row,
            Line::Cells { start, count } => row >= start && row < start + count,
        })
    }

    /// The lines that fit starting at `first`, as (line, top in DIPs from the list's top, height).
    pub fn layout(&self, first: usize) -> Vec<(Line, i32, i32)> {
        let mut out = Vec::with_capacity(12);
        let mut top = 0;
        for line in self.lines.iter().skip(first) {
            let height = line.height();
            if top + height > self.room {
                break;
            }
            out.push((*line, top, height));
            top += height;
        }
        out
    }

    /// The row a line's title or tile stands for, from a position in DIPs in the list.
    pub fn row_at(&self, first: usize, x: i32, y: i32, width: i32) -> Option<usize> {
        if x < 0 || x >= width || y < 0 {
            return None;
        }
        let (line, top, height) = self.layout(first).into_iter().find(|&(_, top, height)| y >= top && y < top + height)?;
        let _ = (top, height);
        let Line::Cells { start, count } = line else { return None };
        let column = (x * COLUMNS as i32 / width) as usize;
        (column < count).then_some(start + column)
    }

    /// The most lines can start at, so that the end of the list sits at the bottom.
    pub fn last_first(&self) -> usize {
        let mut used = 0;
        for (at, line) in self.lines.iter().enumerate().rev() {
            used += line.height();
            if used > self.room {
                return at + 1;
            }
        }
        0
    }

    /// The row one line up or down, in the same column or as near as that line allows.
    pub fn row_vertically(&self, row: usize, delta: i32) -> usize {
        let Some(mut line) = self.line_of(row) else { return row };
        let Line::Cells { start, .. } = self.lines[line] else { return row };
        let column = row - start;
        let mut left = delta.unsigned_abs();
        let mut answer = row;
        while left > 0 {
            let next = if delta < 0 { line.checked_sub(1) } else { (line + 1 < self.lines.len()).then_some(line + 1) };
            let Some(next) = next else { break };
            line = next;
            if let Line::Cells { start, count } = self.lines[line] {
                answer = start + column.min(count - 1);
                left -= 1;
            }
        }
        answer
    }

    /// The first line to show so that `row` is on screen, moving as little as possible from `first`.
    pub fn scroll_to_show(&self, row: usize, first: usize) -> usize {
        let Some(line) = self.line_of(row) else { return first };
        let mut first = first.min(self.last_first());
        if line < first {
            first = line;
        } else {
            while first < line && !self.layout(first).iter().any(|&(shown, _, _)| shown == self.lines[line]) {
                first += 1;
            }
        }
        // Keep a section's title in view with its first line.
        if first == line && first > 0 && matches!(self.lines[first - 1], Line::Title(_)) {
            first -= 1;
        }
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rows::GLYPH_INFO;

    /// Sections of these many tiles.
    fn rows(sections: &[usize]) -> Vec<Row> {
        let mut out = Vec::new();
        for (at, &count) in sections.iter().enumerate() {
            out.push(Row::header(format!("Section {at}")));
            out.extend((0..count).map(|i| Row::info(format!("{i}"), "", GLYPH_INFO)));
        }
        out
    }

    #[test]
    fn lines_break_at_titles_and_at_the_edge() {
        let grid = Grid::new(&rows(&[12, 3]), 400);
        assert_eq!(
            grid.lines,
            [
                Line::Title(0),
                Line::Cells { start: 1, count: 10 },
                Line::Cells { start: 11, count: 2 },
                Line::Title(13),
                Line::Cells { start: 14, count: 3 },
            ]
        );
    }

    #[test]
    fn moving_down_keeps_the_column_and_skips_titles() {
        let grid = Grid::new(&rows(&[12, 3]), 400);
        assert_eq!(grid.row_vertically(5, 1), 12, "a short line: the nearest tile on it");
        assert_eq!(grid.row_vertically(11, 1), 14, "over the title");
        assert_eq!(grid.row_vertically(14, -1), 11);
        assert_eq!(grid.row_vertically(16, 1), 16, "nothing below");
        assert_eq!(grid.row_vertically(3, -1), 3, "only a title above");
        assert_eq!(grid.row_vertically(1, 2), 14, "two lines down");
    }

    #[test]
    fn a_click_finds_its_tile_and_titles_do_nothing() {
        let grid = Grid::new(&rows(&[12]), 400);
        assert_eq!(grid.row_at(0, 5, HEADER_DIP + 2, 700), Some(1));
        assert_eq!(grid.row_at(0, 699, HEADER_DIP + 2, 700), Some(10));
        assert_eq!(grid.row_at(0, 5, 5, 700), None, "on a title");
        assert_eq!(grid.row_at(0, 200, HEADER_DIP + CELL_DIP + 2, 700), None, "past the end of a short line");
        assert_eq!(grid.row_at(1, 5, 2, 700), Some(1), "scrolled by one line");
        assert_eq!(grid.row_at(0, -1, HEADER_DIP + 2, 700), None);
    }

    #[test]
    fn only_lines_that_fit_are_shown() {
        let grid = Grid::new(&rows(&[100]), HEADER_DIP + CELL_DIP * 2 + 5);
        assert_eq!(grid.layout(0).len(), 3);
        assert!(grid.layout(0).iter().all(|&(_, top, height)| top + height <= grid.room));
    }

    #[test]
    fn scrolling_follows_the_selection_with_its_title() {
        let grid = Grid::new(&rows(&[100, 10]), CELL_DIP * 3);
        assert_eq!(grid.scroll_to_show(1, 0), 0);
        let bottom = grid.scroll_to_show(100, 0);
        assert!(grid.layout(bottom).iter().any(|&(line, _, _)| line == Line::Cells { start: 91, count: 10 }));
        assert_eq!(grid.scroll_to_show(1, 5), 0, "back up, and the title comes too");
        let second = grid.scroll_to_show(102, 0);
        assert!(grid.layout(second).iter().any(|&(line, _, _)| matches!(line, Line::Cells { start: 102, .. })));
    }

    #[test]
    fn the_end_of_the_list_reaches_the_bottom_and_no_further() {
        let grid = Grid::new(&rows(&[100]), CELL_DIP * 3);
        assert_eq!(grid.last_first(), grid.lines.len() - 3);
        assert_eq!(Grid::new(&rows(&[3]), 400).last_first(), 0);
    }
}
