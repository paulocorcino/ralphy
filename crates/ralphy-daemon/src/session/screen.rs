//! The screen of one console (ADR-0079): a terminal emulator that reads every
//! output byte of a session at the session size, so a reattach can get the
//! screen as it is now ([`Screen::snapshot`]) in place of the raw ring.
//!
//! This module and its `screen/` directory are the only places that name
//! `alacritty_terminal` (ADR-0079 D1). No type of that crate leaves them.
//!
//! The output of a console is untrusted data (ADR-0072 D11): every size is
//! clamped, a string sequence is capped (`input.rs`), and no method panics on
//! any input bytes.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Osc52, Term, TermMode};
use alacritty_terminal::vte::ansi::{Processor, Timeout};

mod input;
mod snapshot;
#[cfg(test)]
mod tests;

/// The size range of a session (ADR-0079 D3). `alacritty_terminal` 0.26.0
/// panics on a 0x0 resize; the upper bounds keep one hostile resize from
/// allocating a grid of 65535x65535 cells.
pub const ROWS: std::ops::RangeInclusive<u16> = 2..=300;
pub const COLS: std::ops::RangeInclusive<u16> = 2..=500;

/// Lines of scrollback each screen keeps: the page's own xterm scrollback
/// (ADR-0079 D4).
pub const SCROLLBACK_LINES: usize = 1000;

/// `(rows, cols)` clamped to [`ROWS`] and [`COLS`]. The PTY and the screen of
/// a session take the same clamped size, so the two never differ.
pub fn clamp_size(rows: u16, cols: u16) -> (u16, u16) {
    (
        rows.clamp(*ROWS.start(), *ROWS.end()),
        cols.clamp(*COLS.start(), *COLS.end()),
    )
}

/// The screen of one session: the emulator, the state it keeps private that
/// the snapshot needs (the scroll region, the title), and the string cap.
pub struct Screen {
    term: Term<Listener>,
    processor: Processor<NoSync>,
    title: Listener,
    region: input::RegionTracker,
    cap: input::StringCap,
    /// The bytes of one `feed` after the string cap; kept to reuse its memory.
    filtered: Vec<u8>,
    rows: u16,
    cols: u16,
}

/// The largest buffer `feed` keeps between calls. A PTY read is far smaller;
/// a larger feed gets a buffer that is dropped after it.
const FILTERED_KEEP: usize = 64 * 1024;

impl Screen {
    /// An empty screen of `rows` x `cols` (clamped) that keeps
    /// `scrollback_lines` lines above the visible rows.
    pub fn new(rows: u16, cols: u16, scrollback_lines: usize) -> Self {
        let (rows, cols) = clamp_size(rows, cols);
        let title = Listener::default();
        let config = Config {
            scrolling_history: scrollback_lines,
            // The page's xterm reads the kitty keyboard flags; the emulator
            // must keep them so the snapshot can write them back.
            kitty_keyboard: true,
            // A clipboard sequence never reaches the snapshot (ADR-0079 D5).
            osc52: Osc52::Disabled,
            ..Config::default()
        };
        let term = Term::new(config, &Size::new(rows, cols), title.clone());
        Screen {
            term,
            processor: Processor::new(),
            title,
            region: input::RegionTracker::new(usize::from(rows)),
            cap: input::StringCap::default(),
            filtered: Vec::new(),
            rows,
            cols,
        }
    }

    /// Applies output bytes of the session. Every byte is applied at once:
    /// a synchronized update (mode 2026) is never held back (ADR-0079 D2).
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut filtered = std::mem::take(&mut self.filtered);
        filtered.clear();
        self.cap.filter(bytes, &mut filtered);
        self.processor.advance(&mut self.term, &filtered);
        self.region.advance(&filtered);
        if filtered.capacity() > FILTERED_KEEP {
            filtered = Vec::new();
        }
        self.filtered = filtered;
    }

    /// Gives the screen a new size (clamped). Lines longer than the new width
    /// are wrapped, not cut, and the scroll region is reset, as in the
    /// emulator.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = clamp_size(rows, cols);
        self.term.resize(Size::new(rows, cols));
        self.region.resize(usize::from(rows));
        self.rows = rows;
        self.cols = cols;
    }

    /// `(rows, cols)`, after the clamp.
    // Only tests read it today.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// The screen as escape sequences: written into a reset terminal of the
    /// same size, they rebuild this screen (the content list and the order
    /// are ADR-0079 D5).
    ///
    /// It takes `&mut self` because the emulator keeps the primary screen
    /// private while the alternate screen is on; reading it swaps the two
    /// screens and swaps them back. The screen is the same after the call.
    pub fn snapshot(&mut self) -> Vec<u8> {
        let title = self
            .title
            .title
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        snapshot::write(&mut self.term, self.region.region(), title.as_deref())
    }

    /// The plain text of the active screen: its scrollback and its visible
    /// rows, a wrapped row joined to the next, trailing spaces removed, one
    /// line per `\n`. The alternate screen has no scrollback.
    // `read_screen` (#679) is its caller; until then only tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn text(&self) -> String {
        let grid = self.term.grid();
        let last = grid.columns() - 1;
        let mut lines: Vec<String> = Vec::new();
        let mut line = String::new();
        for l in grid.topmost_line().0..=grid.bottommost_line().0 {
            let row = &grid[Line(l)];
            for c in 0..=last {
                let cell = &row[Column(c)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                line.push(cell.c);
                if let Some(zero_width) = cell.zerowidth() {
                    line.extend(zero_width.iter());
                }
            }
            if !row[Column(last)].flags.contains(Flags::WRAPLINE) {
                lines.push(line.trim_end().to_string());
                line.clear();
            }
        }
        if !line.is_empty() {
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }
}

/// Runs `f` on the primary grid, also while the alternate screen is on.
///
/// `alacritty_terminal` 0.26.0 keeps the inactive grid private. Two
/// `swap_alt` calls reach it; the second one, made from the primary screen,
/// clears the alternate grid, so a clone of it is put back.
fn with_primary<R>(term: &mut Term<Listener>, f: impl FnOnce(&Grid<Cell>) -> R) -> R {
    if !term.mode().contains(TermMode::ALT_SCREEN) {
        return f(term.grid());
    }
    let alternate = term.grid().clone();
    term.swap_alt();
    let result = f(term.grid());
    term.swap_alt();
    *term.grid_mut() = alternate;
    result
}

/// Never pends, so vte's processor never buffers a synchronized update
/// (ADR-0079 D2) and never touches its 2 MiB buffer for one.
#[derive(Default)]
struct NoSync;

impl Timeout for NoSync {
    fn set_timeout(&mut self, _: Duration) {}
    fn clear_timeout(&mut self) {}
    fn pending_timeout(&self) -> bool {
        false
    }
}

/// Keeps the window title, the one emulator event the snapshot needs. Every
/// other event (a reply to a query, a bell, a clipboard request) is dropped:
/// the page's own terminal answers the child.
#[derive(Clone, Default)]
struct Listener {
    title: Arc<Mutex<Option<String>>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let title = match event {
            Event::Title(title) => Some(title),
            Event::ResetTitle => None,
            _ => return,
        };
        *self.title.lock().unwrap_or_else(PoisonError::into_inner) = title;
    }
}

/// A size in the form the emulator reads.
struct Size {
    rows: usize,
    cols: usize,
}

impl Size {
    fn new(rows: u16, cols: u16) -> Self {
        Size {
            rows: usize::from(rows),
            cols: usize::from(cols),
        }
    }
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}
