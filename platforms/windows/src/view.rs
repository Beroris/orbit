//! What the launcher is showing right now.

use crate::grid::{Grid, Line};
use crate::integrations::Tool;
use crate::rows::*;
use orbit_core::Builtin;
use orbit_core::text::fold;
use orbit_core::vim::{KeyboardMode, VimMode};

#[derive(Clone, Debug, PartialEq)]
pub enum Scope {
    Root,
    Notes,
    Snippets,
    Links,
    Emoji,
    Clipboard,
    Vault(Tool),
    Help,
    Diagnostics,
    Actions { of: String },
    Confirm(Builtin),
    SetAlias { key: String, of: String },
    SetHotkey { of: String },
    /// Filling in a quicklink's blanks, in boxes in the search bar.
    LinkArgs { of: String, fields: usize },
    /// Making or changing a quicklink or snippet, in boxes under the search bar. The fields are
    /// in the app's `form`. `multiline` is whether the field being typed in takes several lines.
    Form { title: String, link: bool, multiline: bool, choice: bool },
    /// A place's weather, drawn in place of the list. Typing looks up another place.
    Weather,
    /// An internet speed test, drawn in place of the list.
    SpeedTest,
    /// The timer, drawn in place of the list. Typing a time sets it.
    Timer,
    Stopwatch,
    /// A colour picked from the screen.
    Colour(orbit_core::colour::Colour),
    /// Every file on the PC, found by Everything as you type.
    Files,
    /// A list an extension fills as you type.
    Extension { ext: String, command: String, title: String, placeholder: String },
}

impl Scope {
    pub fn input_glyph(&self) -> char {
        match self {
            Scope::Root => GLYPH_SEARCH,
            Scope::Notes => GLYPH_NOTE,
            Scope::Snippets => GLYPH_SNIPPET,
            Scope::Links => GLYPH_LINK,
            Scope::Emoji => GLYPH_EMOJI,
            Scope::Clipboard => GLYPH_CLIPBOARD,
            Scope::Vault(tool) => tool.glyph(),
            Scope::Help => GLYPH_INFO,
            Scope::Diagnostics => GLYPH_INFO,
            Scope::Actions { .. } => GLYPH_PLAY,
            Scope::Confirm(_) => GLYPH_WARNING,
            Scope::SetAlias { .. } => GLYPH_TAG,
            Scope::SetHotkey { .. } => GLYPH_KEYBOARD,
            Scope::LinkArgs { .. } => GLYPH_LINK,
            Scope::Form { link: true, .. } => GLYPH_LINK,
            Scope::Form { .. } => GLYPH_SNIPPET,
            Scope::Weather => GLYPH_WEATHER,
            Scope::SpeedTest => GLYPH_SPEED,
            Scope::Timer => GLYPH_TIMER,
            Scope::Stopwatch => GLYPH_STOPWATCH,
            Scope::Colour(_) => GLYPH_PICKER,
            Scope::Files => GLYPH_FILE,
            Scope::Extension { ext, .. } => orbit_core::builtin_ext::BuiltinExt::from_id(ext).map_or(GLYPH_EXTENSION, crate::rows::extension_glyph),
        }
    }

    pub fn placeholder(&self) -> String {
        match self {
            Scope::Root => "Search apps, commands, notes…".into(),
            Scope::Notes => "Search notes…".into(),
            Scope::Snippets => "Search snippets…".into(),
            Scope::Links => "Search quicklinks…".into(),
            Scope::Emoji => "Search Emoji & Symbols…".into(),
            Scope::Clipboard => "Search clipboard history…".into(),
            Scope::Vault(tool) => format!("Search {}…", tool.name()),
            Scope::Help => "Filter help…".into(),
            Scope::Diagnostics => "Filter diagnostics…".into(),
            Scope::Actions { of } => format!("Actions for {of}"),
            Scope::Confirm(_) => "Are you sure?".into(),
            Scope::SetAlias { of, .. } => format!("Type an alias for {of}"),
            Scope::SetHotkey { of } => format!("Press a hotkey for {of}"),
            Scope::LinkArgs { .. } => String::new(),
            Scope::Form { .. } => String::new(),
            Scope::Weather => "Look up another place\u{2026}".into(),
            Scope::SpeedTest => "Speed Test".into(),
            Scope::Timer => "Type a time, such as 5m or 1h 30m\u{2026}".into(),
            Scope::Stopwatch => "Stopwatch".into(),
            Scope::Colour(_) => "Colour".into(),
            Scope::Files => "Search files\u{2026}".into(),
            Scope::Extension { title, placeholder, .. } if placeholder.is_empty() => format!("Search {title}\u{2026}"),
            Scope::Extension { placeholder, .. } => placeholder.clone(),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Scope::Root => "Orbit",
            Scope::Notes => "Notes",
            Scope::Snippets => "Snippets",
            Scope::Links => "Quicklinks",
            Scope::Emoji => "Search Emoji & Symbols",
            Scope::Clipboard => "Clipboard history",
            Scope::Vault(tool) => tool.name(),
            Scope::Help => "Help",
            Scope::Diagnostics => "Diagnostics",
            Scope::Actions { .. } => "Actions",
            Scope::Confirm(_) => "Confirm",
            Scope::SetAlias { .. } => "Alias",
            Scope::SetHotkey { .. } => "Hotkey",
            Scope::LinkArgs { of, .. } => of,
            Scope::Form { link: true, .. } => "Quicklink",
            Scope::Form { .. } => "Snippet",
            // Open-Meteo's data asks to be credited where it's shown.
            Scope::Weather => "Weather by Open-Meteo.com",
            Scope::SpeedTest => "Speedtest by Ookla",
            Scope::Timer => "Timer",
            Scope::Stopwatch => "Stopwatch",
            Scope::Colour(_) => "Colour",
            // Everything asks to be named where its results are shown.
            Scope::Files => "Files, found by Everything",
            Scope::Extension { title, .. } => title,
        }
    }

    pub fn empty_message(&self) -> Option<&'static str> {
        match self {
            Scope::Root => Some("No results"),
            Scope::Notes => Some("No notes yet"),
            Scope::Snippets => Some("No snippets yet"),
            Scope::Links => Some("No quicklinks match"),
            Scope::Emoji => Some("No emoji match"),
            Scope::Clipboard => Some("Nothing here yet. Copy something and it will appear."),
            Scope::Vault(_) => Some("Nothing matches"),
            Scope::Files => Some("No files match"),
            Scope::Extension { .. } => Some("Nothing found"),
            _ => None,
        }
    }

    /// Scopes where typing narrows the list.
    pub fn can_filter(&self) -> bool {
        matches!(self, Scope::Root | Scope::Notes | Scope::Snippets | Scope::Links | Scope::Emoji | Scope::Clipboard | Scope::Vault(_) | Scope::Help | Scope::Diagnostics | Scope::Files | Scope::Extension { .. })
    }

    /// Scopes whose rows come from the data thread.
    pub fn is_data(&self) -> bool {
        matches!(self, Scope::Notes | Scope::Snippets | Scope::Clipboard)
    }
}

/// Where a small menu comes from: either end of the footer, or the drop-down at the right of the
/// search box, which it hangs from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuSide {
    Left,
    Right,
    Drop,
}

/// A small menu over the list, as the footer tells them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    /// What Ctrl+K opened, which the same key closes.
    Actions,
    /// Orbit's own menu, a filter drop-down or an app picker.
    Other,
}

/// A small menu on top of a list. The menu's rows are the view's for as long as it's open, so keys
/// and running them work as they do anywhere. What it covers waits here, to be drawn behind it.
pub struct Menu {
    pub side: MenuSide,
    /// How many rows tall it is, which stays put as typing narrows it down.
    pub shown: usize,
    pub behind: Behind,
}

pub struct Behind {
    pub scope: Scope,
    pub query: String,
    pub rows: Vec<Row>,
    pub static_rows: Vec<Row>,
    pub scroll: usize,
    pub selected: usize,
    pub grid: Option<Grid>,
}

#[derive(Clone)]
pub struct SavedView {
    pub scope: Scope,
    pub query: String,
    pub selected: usize,
}

pub struct View {
    pub scope: Scope,
    pub rows: Vec<Row>,
    /// The full list behind a static scope. `rows` holds what passes the filter.
    pub static_rows: Vec<Row>,
    pub selected: usize,
    pub scroll: usize,
    /// Where results from the data thread go in `rows`, so they never shift rows above them.
    pub async_insert_at: usize,
    pub secondary_label: Option<&'static str>,
    pub visible_rows: usize,
    pub stack: Vec<SavedView>,
    pub last_query: Option<String>,
    /// Set when the list is drawn as tiles. `scroll` then counts the grid's lines, not rows.
    pub grid: Option<Grid>,
    /// The launcher's DPI, which rows are measured in to know which are on screen.
    pub dpi: u32,
}

impl View {
    pub fn new(visible_rows: usize) -> Self {
        View {
            scope: Scope::Root,
            rows: Vec::new(),
            static_rows: Vec::new(),
            selected: 0,
            scroll: 0,
            async_insert_at: 0,
            secondary_label: None,
            visible_rows,
            stack: Vec::new(),
            last_query: None,
            grid: None,
            dpi: 96,
        }
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    pub fn select(&mut self, index: usize) {
        if self.rows.is_empty() {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        self.selected = self.selectable_near(index.min(self.rows.len() - 1));
        self.ensure_visible();
    }

    /// The row at `index`, or the next one that can be selected: section titles can't be.
    fn selectable_near(&self, index: usize) -> usize {
        let forward = (index..self.rows.len()).find(|&i| !self.rows[i].is_header());
        let back = (0..index).rev().find(|&i| !self.rows[i].is_header());
        forward.or(back).unwrap_or(index)
    }

    /// Left and right in a grid: along the rows, stepping over section titles.
    pub fn move_along(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len() as i32;
        let mut next = self.selected as i32;
        for _ in 0..len {
            next = (next + delta).rem_euclid(len);
            if !self.rows[next as usize].is_header() {
                break;
            }
        }
        self.select(next as usize);
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        if let Some(grid) = &self.grid {
            // Up and down go by lines. A page is a page's worth of them.
            let lines = if delta.abs() == 1 { delta } else { delta.signum() * (grid.room / crate::grid::CELL_DIP).max(1) };
            let row = grid.row_vertically(self.selected, lines);
            self.select(row);
            return;
        }
        let len = self.rows.len() as i32;
        // One-step moves wrap and step over section titles; page moves stop at the ends.
        if delta.abs() == 1 {
            let mut next = self.selected as i32;
            for _ in 0..len {
                next = (next + delta).rem_euclid(len);
                if !self.rows[next as usize].is_header() {
                    break;
                }
            }
            self.select(next as usize);
        } else {
            self.select((self.selected as i32 + delta).clamp(0, len - 1) as usize);
        }
    }

    pub fn scroll_by(&mut self, delta: i32) {
        if let Some(grid) = &self.grid {
            self.scroll = (self.scroll as i32 + delta).clamp(0, grid.last_first() as i32) as usize;
            // The selection stays where it is unless it would be off the screen.
            let shown = grid.layout(self.scroll);
            let on_screen = shown.iter().any(|&(line, _, _)| matches!(line, Line::Cells { start, count } if self.selected >= start && self.selected < start + count));
            if !on_screen {
                let first = shown.iter().find_map(|&(line, _, _)| match line {
                    Line::Cells { start, .. } => Some(start),
                    Line::Title(_) => None,
                });
                if let Some(start) = first {
                    self.selected = start;
                }
            }
            return;
        }
        let max = self.last_scroll() as i32;
        self.scroll = (self.scroll as i32 + delta).clamp(0, max) as usize;
        if self.selected < self.scroll {
            self.selected = self.scroll;
        } else if !self.fits(self.scroll, self.selected) {
            // The last row that's drawn whole.
            self.selected = (self.scroll..self.rows.len()).take_while(|&at| self.fits(self.scroll, at)).last().unwrap_or(self.scroll);
        }
        if self.selected < self.rows.len() {
            self.selected = self.selectable_near(self.selected);
        }
    }

    fn ensure_visible(&mut self) {
        if let Some(grid) = &self.grid {
            self.scroll = grid.scroll_to_show(self.selected, self.scroll.min(grid.last_first()));
            return;
        }
        // A shorter list, after something in it was deleted, moves down to fill the space.
        self.scroll = self.scroll.min(self.last_scroll());
        // Rows are measured, not counted: section titles are shorter than rows, so more rows fit
        // than `visible_rows`, and counting would scroll before the selection reached the bottom.
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else {
            while self.scroll < self.selected && !self.fits(self.scroll, self.selected) {
                self.scroll += 1;
            }
        }
        // Keep a section's title in view with its first row.
        if self.scroll == self.selected && self.scroll > 0 && self.rows[self.scroll - 1].is_header() {
            self.scroll -= 1;
        }
    }

    /// Whether row `index` is drawn whole with the list scrolled to `scroll`.
    fn fits(&self, scroll: usize, index: usize) -> bool {
        crate::ui::row_fits(&self.rows, scroll, index, self.dpi, self.visible_rows)
    }

    /// The furthest down the list scrolls: just far enough to show its last row whole.
    fn last_scroll(&self) -> usize {
        let Some(last) = self.rows.len().checked_sub(1) else { return 0 };
        let mut scroll = last;
        while scroll > 0 && self.fits(scroll - 1, last) {
            scroll -= 1;
        }
        scroll
    }

    pub fn reset_position(&mut self) {
        self.selected = 0;
        self.scroll = 0;
    }

    /// Applies a local filter to a static scope.
    pub fn filter_static(&mut self, query: &str) {
        let needle = fold(query.trim());
        self.rows.clear();
        for row in &self.static_rows {
            if needle.is_empty() || fold(&row.title).contains(&needle) || fold(&row.subtitle).contains(&needle) {
                self.rows.push(row.clone());
            }
        }
        self.select(self.selected);
    }

    /// What the keys do right now, as (keys, what they do), in reading order. `menu` is the small
    /// menu open over the list, if one is.
    pub fn footer_hints(&self, vim: Option<VimMode>, menu: Option<MenuKind>) -> Vec<(&'static str, &'static str)> {
        let mut out = Vec::with_capacity(5);
        if let Scope::Form { multiline, choice, .. } = self.scope {
            out.push((if multiline { "Ctrl+Enter" } else { "Enter" }, "Save"));
            if choice {
                out.push(("Space", "Choose app"));
            }
            out.push(("Tab", "Next field"));
            out.push(("Esc", "Cancel"));
            return out;
        }
        if vim == Some(VimMode::Normal) && menu.is_none() {
            out.push(("i", "Type"));
        }
        if let Some(op) = self.selected_row().and_then(|row| row.primary.as_ref()) {
            out.push(("Enter", primary_label(op)));
        }
        // Ctrl+K closes the actions it opened, so with them open it stays, lit. Orbit's menu and a
        // drop-down aren't its, so they leave it out.
        let can_menu = self.selected_row().is_some_and(|row| row.source != RowSource::Plain) && !matches!(self.scope, Scope::Actions { .. });
        if menu == Some(MenuKind::Actions) || (menu.is_none() && can_menu) {
            out.push(("Ctrl+K", "Actions"));
        }
        if matches!(self.scope, Scope::LinkArgs { fields, .. } if fields > 1) {
            out.push(("Tab", "Next box"));
        }
        let typed = self.last_query.as_deref().is_some_and(|query| !query.is_empty());
        let esc = if vim == Some(VimMode::Insert) {
            "Navigate"
        } else if typed {
            "Clear"
        } else if self.stack.is_empty() {
            "Close"
        } else {
            "Back"
        };
        // Everyone knows what Esc does when it only closes the window.
        if esc != "Close" {
            out.push(("Esc", esc));
        }
        out
    }
}

pub fn help_rows(launcher_hint: &str, keyboard: KeyboardMode) -> Vec<Row> {
    let row = |keys: &str, what: &str| Row::info(keys.to_string(), what.to_string(), GLYPH_INFO);
    let mut vim_rows = Vec::new();
    if keyboard == KeyboardMode::Vim {
        vim_rows.extend([
            row("i a I A", "Start typing: at the caret, after it, at the start, at the end"),
            row("/", "Start a new search"),
            row("Esc", "Stop typing and navigate. Esc again clears the search, then goes back or closes Orbit"),
            row("j k, 5j", "Move through results, optionally several at once"),
            row("h l", "Go back, and run the selected result"),
            row("gg G, 5G", "First result, last result, or a result by number"),
            row("Ctrl+D Ctrl+U", "Half a page down and up. Ctrl+F and Ctrl+B move a page"),
            row("q", "Close Orbit"),
            row("x s dd cc dw cw D C", "Delete in the search box, as in Vim"),
            row("0 $ w b u p", "Caret to start, end, next and previous word. Undo. Paste"),
            row(":", "Start a search for commands only"),
        ]);
    }
    if keyboard != KeyboardMode::Standard {
        vim_rows.extend([
            row("Alt+J Alt+K", "Move through results"),
            row("Alt+H Alt+L", "Go back, and run the selected result"),
            row("Alt+D Alt+U", "Half a page down and up"),
            row("Alt+G Alt+Shift+G", "First and last result"),
        ]);
    }
    let mut rows = vim_rows;
    rows.extend([
        row(launcher_hint, "Open or close Orbit"),
        row("↑ ↓, Page Up/Down", "Move through results"),
        row("Enter", "Run the selected result"),
        row("Ctrl+Enter", "Second action on the result, such as Run as administrator"),
        row("Ctrl+K or Tab", "Actions for the result, in a small menu. Press it again to close"),
        row("Ctrl+,", "Open Orbit Settings"),
        row("Esc", "Clear what's typed, then go back, then close Orbit"),
        row("Backspace", "In an empty box, go back"),
        row("> or :", "Start your search with this to see commands only"),
        row("g something", "Search with a quicklink. Alias \"g\" is Google"),
        row("Tab", "In a quicklink's boxes, go to the next one. Shift+Tab goes back"),
        row("emoji", "Find Search Emoji & Symbols, to paste one where you were typing"),
        row("quicklink, snippet", "Find Create Quicklink and Create Snippet, to make one from here"),
        row("2*21 or 10 km to mi", "Calculate and convert. Enter copies the result"),
        row("github.com", "Open an address directly"),
        row("?", "Show this help"),
    ]);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(n: usize) -> Vec<Row> {
        (0..n).map(|i| Row::info(format!("row {i}"), "", GLYPH_INFO)).collect()
    }

    fn view(n: usize) -> View {
        let mut v = View::new(8);
        v.rows = rows(n);
        v
    }

    #[test]
    fn selection_wraps_and_stays_visible() {
        let mut v = view(20);
        v.move_selection(-1);
        assert_eq!(v.selected, 19);
        assert_eq!(v.scroll, 12, "last row is visible");
        v.move_selection(1);
        assert_eq!(v.selected, 0);
        assert_eq!(v.scroll, 0);
    }

    #[test]
    fn paging_stops_at_the_ends() {
        let mut v = view(20);
        v.move_selection(-8);
        assert_eq!(v.selected, 0);
        v.move_selection(100);
        assert_eq!(v.selected, 19);
    }

    #[test]
    fn scrolling_keeps_selection_in_view() {
        let mut v = view(20);
        v.scroll_by(5);
        assert_eq!(v.scroll, 5);
        assert_eq!(v.selected, 5);
        v.select(12);
        v.scroll_by(-5);
        assert_eq!(v.scroll, 0);
        assert_eq!(v.selected, 7, "selection is pulled back into view");
        v.scroll_by(-100);
        assert_eq!(v.scroll, 0);
        assert_eq!(v.selected, 7);
        v.scroll_by(1000);
        assert_eq!(v.scroll, 12);
    }

    /// The home page: a short first title, five favourites, a title, then suggestions. The titles
    /// take less room than rows, so more than `visible_rows` rows are on screen.
    fn home(visible: usize) -> View {
        let mut v = View::new(visible);
        v.rows.push(Row::header("Favourites"));
        v.rows.extend(rows(5));
        v.rows.push(Row::header("Suggestions"));
        v.rows.extend(rows(6));
        v
    }

    #[test]
    fn a_row_on_screen_below_the_titles_doesnt_scroll() {
        let mut v = home(7);
        // 8 + 5 rows + 30 + 1 row is 6 rows and 38 DIP, inside 7 rows.
        v.select(7);
        assert_eq!(v.scroll, 0, "the first suggestion is on screen");
        // The next one doesn't fit, so going to it scrolls just enough: past the short title and
        // the first favourite.
        v.select(8);
        assert_eq!(v.scroll, 2);
        v.select(0);
        assert_eq!((v.selected, v.scroll), (1, 0), "back at the top, its title shows again");
        v.scroll_by(100);
        assert!(crate::ui::row_fits(&v.rows, v.scroll, v.rows.len() - 1, 96, 7), "the last row shows at the furthest scroll");
        assert!(!crate::ui::row_fits(&v.rows, v.scroll - 1, v.rows.len() - 1, 96, 7), "and it goes no further than that");
        assert!(crate::ui::row_fits(&v.rows, v.scroll, v.selected, 96, 7), "with the selection on screen");
    }

    #[test]
    fn empty_lists_are_safe() {
        let mut v = view(0);
        v.move_selection(1);
        v.move_selection(-5);
        v.scroll_by(3);
        v.select(4);
        assert_eq!(v.selected, 0);
        assert!(v.selected_row().is_none());
    }

    #[test]
    fn static_filter_keeps_a_valid_selection() {
        let mut v = View::new(8);
        v.static_rows = vec![Row::info("Alpha", "first", GLYPH_INFO), Row::info("Beta", "second", GLYPH_INFO)];
        v.filter_static("");
        assert_eq!(v.rows.len(), 2);
        v.select(1);
        v.filter_static("alp");
        assert_eq!(v.rows.len(), 1);
        assert_eq!(v.selected, 0);
        v.filter_static("second");
        assert_eq!(v.rows[0].title, "Beta");
    }

    #[test]
    fn footer_describes_the_selection() {
        let mut v = view(0);
        assert!(v.footer_hints(None, None).is_empty(), "closing goes without saying");
        v.stack.push(SavedView { scope: Scope::Root, query: String::new(), selected: 0 });
        assert_eq!(v.footer_hints(None, None), [("Esc", "Back")]);
        v.stack.clear();
        v.rows.push(Row {
            source: RowSource::Note { id: 1, pinned: false },
            primary: Some(Op::NoteOpen(1)),
            ..Row::info("n", "", GLYPH_NOTE)
        });
        v.secondary_label = Some("Copy");
        assert_eq!(v.footer_hints(None, None), [("Enter", "Open note"), ("Ctrl+K", "Actions")]);
    }

    #[test]
    fn a_menu_keeps_back_in_the_footer() {
        let mut v = view(0);
        v.rows.push(Row { primary: Some(Op::NoteOpen(1)), ..Row::info("n", "", GLYPH_NOTE) });
        // A menu stands on the list it covers, so Esc goes back to it.
        v.stack.push(SavedView { scope: Scope::Root, query: String::new(), selected: 0 });
        v.scope = Scope::Actions { of: "n".into() };
        assert_eq!(v.footer_hints(None, Some(MenuKind::Actions)), [("Enter", "Open note"), ("Ctrl+K", "Actions"), ("Esc", "Back")]);
        assert_eq!(v.footer_hints(None, Some(MenuKind::Other)), [("Enter", "Open note"), ("Esc", "Back")], "Ctrl+K isn't Orbit's menu or a drop-down");
        v.last_query = Some("cop".into());
        assert_eq!(v.footer_hints(None, Some(MenuKind::Other)), [("Enter", "Open note"), ("Esc", "Clear")], "Esc clears the menu's search first");
    }

    #[test]
    fn a_shorter_list_moves_down_to_fill_the_space() {
        let mut v = View::new(4);
        v.rows = (0..10).map(|i| Row::info(format!("{i}"), "", GLYPH_INFO)).collect();
        v.select(9);
        assert_eq!(v.scroll, 6, "the last four rows showing");
        // The last row was deleted and the list came back one shorter.
        v.rows.pop();
        v.select(v.selected);
        assert_eq!((v.selected, v.scroll), (8, 5), "no empty space under the end");
    }

    fn with_sections() -> View {
        let mut v = View::new(4);
        v.rows = vec![
            Row::header("Favourites"),
            Row::info("a", "", GLYPH_INFO),
            Row::info("b", "", GLYPH_INFO),
            Row::header("Suggestions"),
            Row::info("c", "", GLYPH_INFO),
            Row::info("d", "", GLYPH_INFO),
        ];
        v
    }

    #[test]
    fn section_titles_are_never_selected() {
        let mut v = with_sections();
        v.select(0);
        assert_eq!(v.selected, 1, "the first row, not the title");
        assert_eq!(v.scroll, 0);
        v.move_selection(1);
        assert_eq!(v.selected, 2);
        v.move_selection(1);
        assert_eq!(v.selected, 4, "steps over the title");
        v.move_selection(-1);
        assert_eq!(v.selected, 2);
        v.select(1);
        v.move_selection(-1);
        assert_eq!(v.selected, 5, "wraps past the first title");
        v.move_selection(1);
        assert_eq!(v.selected, 1);
        assert_eq!(v.scroll, 0, "with the title back in view");
        v.select(3);
        assert_eq!(v.selected, 4);
    }

    #[test]
    fn a_section_title_scrolls_in_with_its_first_row() {
        let mut v = with_sections();
        v.select(5);
        assert_eq!(v.scroll, 2);
        v.select(4);
        v.select(2);
        assert_eq!(v.scroll, 2);
        v.select(1);
        assert_eq!(v.scroll, 0, "row 1 is first in its section");
    }

    #[test]
    fn a_list_of_only_titles_is_safe() {
        let mut v = View::new(4);
        v.rows = vec![Row::header("Favourites")];
        v.select(0);
        v.move_selection(1);
        v.move_selection(-1);
        v.scroll_by(1);
        assert_eq!(v.selected, 0);
    }

    #[test]
    fn esc_says_clear_while_something_is_typed() {
        let mut v = View::new(8);
        v.last_query = Some("test".to_string());
        assert_eq!(v.footer_hints(None, None), [("Esc", "Clear")]);
        v.last_query = Some(String::new());
        assert!(v.footer_hints(None, None).is_empty());
        v.last_query = Some("test".to_string());
        assert_eq!(v.footer_hints(Some(VimMode::Normal), None), [("i", "Type"), ("Esc", "Clear")]);
        assert_eq!(v.footer_hints(Some(VimMode::Insert), None), [("Esc", "Navigate")]);
    }

    #[test]
    fn footer_shows_the_vim_mode_keys() {
        let v = view(0);
        assert_eq!(v.footer_hints(Some(VimMode::Normal), None), [("i", "Type")]);
        assert_eq!(v.footer_hints(Some(VimMode::Insert), None), [("Esc", "Navigate")]);
    }

    #[test]
    fn help_lists_the_keys_for_the_setting_in_use() {
        let has = |rows: &[Row], title: &str| rows.iter().any(|row| row.title == title);
        let standard = help_rows("Alt+Space", KeyboardMode::Standard);
        assert!(!has(&standard, "Alt+J Alt+K") && !has(&standard, "gg G, 5G"));
        let alt = help_rows("Alt+Space", KeyboardMode::VimKeys);
        assert!(has(&alt, "Alt+J Alt+K") && !has(&alt, "gg G, 5G"));
        let vim = help_rows("Alt+Space", KeyboardMode::Vim);
        assert!(has(&vim, "Alt+J Alt+K") && has(&vim, "gg G, 5G"));
    }
}
