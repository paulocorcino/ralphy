//! The snapshot writer: the screen as escape sequences that rebuild it in a
//! reset terminal of the same size (ADR-0079 D5). `alacritty_terminal` has no
//! writer of its own.

use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags, Hyperlink};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};

use super::{with_primary, Listener};

/// The input modes the snapshot keeps, each with the sequence that sets it.
/// No auto-wrap, origin mode, insert mode and new-line mode are written
/// apart, because they change how the cursor and its last cell are written.
const MODES: [(TermMode, &str); 10] = [
    (TermMode::APP_CURSOR, "\x1b[?1h"),
    (TermMode::APP_KEYPAD, "\x1b="),
    (TermMode::MOUSE_REPORT_CLICK, "\x1b[?1000h"),
    (TermMode::MOUSE_DRAG, "\x1b[?1002h"),
    (TermMode::MOUSE_MOTION, "\x1b[?1003h"),
    (TermMode::SGR_MOUSE, "\x1b[?1006h"),
    (TermMode::UTF8_MOUSE, "\x1b[?1005h"),
    (TermMode::BRACKETED_PASTE, "\x1b[?2004h"),
    (TermMode::FOCUS_IN_OUT, "\x1b[?1004h"),
    (TermMode::ALTERNATE_SCROLL, "\x1b[?1007h"),
];

/// The kitty keyboard flags, in the bit order of `CSI > flags u`.
const KITTY_FLAGS: [TermMode; 5] = [
    TermMode::DISAMBIGUATE_ESC_CODES,
    TermMode::REPORT_EVENT_TYPES,
    TermMode::REPORT_ALTERNATE_KEYS,
    TermMode::REPORT_ALL_KEYS_AS_ESC,
    TermMode::REPORT_ASSOCIATED_TEXT,
];

/// The cell flags that are attributes (SGR), not layout.
const ATTRIBUTES: Flags = Flags::from_bits_truncate(
    Flags::INVERSE.bits()
        | Flags::BOLD.bits()
        | Flags::ITALIC.bits()
        | Flags::DIM.bits()
        | Flags::HIDDEN.bits()
        | Flags::STRIKEOUT.bits()
        | Flags::ALL_UNDERLINES.bits(),
);

const SGR_FLAGS: [(Flags, &str); 11] = [
    (Flags::BOLD, "1"),
    (Flags::DIM, "2"),
    (Flags::ITALIC, "3"),
    (Flags::UNDERLINE, "4"),
    (Flags::DOUBLE_UNDERLINE, "4:2"),
    (Flags::UNDERCURL, "4:3"),
    (Flags::DOTTED_UNDERLINE, "4:4"),
    (Flags::DASHED_UNDERLINE, "4:5"),
    (Flags::INVERSE, "7"),
    (Flags::HIDDEN, "8"),
    (Flags::STRIKEOUT, "9"),
];

/// Writes the snapshot of `term`. `region` is the scroll region as DECSTBM
/// writes it; `title` is the window title.
pub(super) fn write(
    term: &mut Term<Listener>,
    region: Option<(usize, usize)>,
    title: Option<&str>,
) -> Vec<u8> {
    let mut out = String::from("\x1b[0m");
    let mut pen = Pen::default();
    let alternate = term.mode().contains(TermMode::ALT_SCREEN);

    // 1. The primary screen, its scrollback first.
    with_primary(term, |grid| write_rows(grid, true, &mut out, &mut pen));

    // 2. The alternate screen over it, with the primary cursor saved by 1049.
    if alternate {
        let primary_cursor = with_primary(term, |grid| grid.cursor.point);
        out.push_str(&format!(
            "\x1b[0m\x1b[{};{}H\x1b[?1049h",
            primary_cursor.line.0.saturating_add(1),
            primary_cursor.column.0.saturating_add(1)
        ));
        pen = Pen::default();
        write_rows(term.grid(), false, &mut out, &mut pen);
    }
    if pen.link.is_some() {
        out.push_str("\x1b]8;;\x1b\\");
        pen.link = None;
    }

    let mode = *term.mode();

    // 3. The scroll region. DECSTBM moves the cursor home, so it comes
    // before the cursor.
    let region = region.filter(|(top, bottom)| top < bottom);
    if let Some((top, bottom)) = region {
        out.push_str(&format!("\x1b[{top};{bottom}r"));
    }

    // 4. The input modes.
    for (flag, sequence) in MODES {
        if mode.contains(flag) {
            out.push_str(sequence);
        }
    }
    let kitty = KITTY_FLAGS
        .iter()
        .enumerate()
        .filter(|(_, flag)| mode.contains(**flag))
        .fold(0u32, |flags, (bit, _)| flags | (1 << bit));
    if kitty != 0 {
        out.push_str(&format!("\x1b[>{kitty}u"));
    }
    let origin = mode.contains(TermMode::ORIGIN);
    if origin {
        out.push_str("\x1b[?6h");
    }

    // 5. The cursor, then the modes that change how a cell is written.
    let origin_top = match (origin, region) {
        (true, Some((top, _))) => top.saturating_sub(1),
        _ => 0,
    };
    write_cursor(term.grid(), origin_top, &mut out, &mut pen);
    if mode.contains(TermMode::INSERT) {
        out.push_str("\x1b[4h");
    }
    if mode.contains(TermMode::LINE_FEED_NEW_LINE) {
        out.push_str("\x1b[20h");
    }
    if !mode.contains(TermMode::LINE_WRAP) {
        out.push_str("\x1b[?7l");
    }
    let template = term.grid().cursor.template.clone();
    pen.apply(&template, &mut out);
    if !mode.contains(TermMode::SHOW_CURSOR) {
        out.push_str("\x1b[?25l");
    }
    let style = term.cursor_style();
    let shape = match (style.shape, style.blinking) {
        (CursorShape::Block, true) => Some(1),
        (CursorShape::Underline, true) => Some(3),
        (CursorShape::Underline, false) => Some(4),
        (CursorShape::Beam, true) => Some(5),
        (CursorShape::Beam, false) => Some(6),
        // A steady block is the default; a hidden or hollow shape has no
        // DECSCUSR number.
        (CursorShape::Block, false) | (CursorShape::HollowBlock | CursorShape::Hidden, _) => None,
    };
    if let Some(shape) = shape {
        out.push_str(&format!("\x1b[{shape} q"));
    }

    // 6. The title, without control characters.
    if let Some(title) = title {
        let clean: String = title.chars().filter(|c| !c.is_control()).collect();
        out.push_str(&format!("\x1b]2;{clean}\x07"));
    }
    out.into_bytes()
}

/// Moves the cursor to its place. A pending wrap (the cursor past the last
/// column, waiting for the next character) has no sequence of its own: it is
/// rebuilt by writing the last cell again.
fn write_cursor(grid: &Grid<Cell>, origin_top: usize, out: &mut String, pen: &mut Pen) {
    let point = grid.cursor.point;
    let row = usize::try_from(point.line.0)
        .unwrap_or(0)
        .saturating_sub(origin_top);
    let mut column = point.column.0;
    let mut cell = None;
    if grid.cursor.input_needs_wrap {
        // The pending wrap of a wide character sits on its spacer; write the
        // wide character from its first column.
        if column > 0
            && grid[point.line][Column(column)]
                .flags
                .contains(Flags::WIDE_CHAR_SPACER)
        {
            column -= 1;
        }
        cell = Some(&grid[point.line][Column(column)]);
    }
    out.push_str(&format!("\x1b[{};{}H", row + 1, column + 1));
    if let Some(cell) = cell {
        pen.apply(cell, out);
        write_cell(cell, out);
    }
}

/// A cell with nothing to write: a space with default colors and no
/// attribute.
fn is_blank(cell: &Cell) -> bool {
    cell.c == ' '
        && cell.fg == Color::Named(NamedColor::Foreground)
        && cell.bg == Color::Named(NamedColor::Background)
        && (cell.flags - Flags::WRAPLINE).is_empty()
        && cell.extra.is_none()
}

fn write_cell(cell: &Cell, out: &mut String) {
    out.push(cell.c);
    if let Some(zero_width) = cell.zerowidth() {
        out.extend(zero_width.iter());
    }
}

/// Every row from the oldest scrollback line down (or from the top of the
/// screen when `with_history` is false). A wrapped row is joined to the next,
/// so the receiving terminal wraps it at its own width; every other row ends
/// with CR LF.
fn write_rows(grid: &Grid<Cell>, with_history: bool, out: &mut String, pen: &mut Pen) {
    let columns = grid.columns();
    let last = columns - 1;
    let top = if with_history {
        grid.topmost_line().0
    } else {
        out.push_str("\x1b[H");
        0
    };
    let bottom = grid.bottommost_line().0;
    let mut after_wrapped = false;
    for l in top..=bottom {
        let row = &grid[Line(l)];
        let wrapped = row[Column(last)].flags.contains(Flags::WRAPLINE);
        let end = if wrapped {
            columns
        } else {
            let end = (0..columns)
                .rev()
                .find(|c| !is_blank(&row[Column(*c)]))
                .map_or(0, |c| c + 1);
            // The row before left a pending wrap; only a written cell makes
            // it fire, or the receiving terminal loses this row.
            if after_wrapped {
                end.max(1)
            } else {
                end
            }
        };
        after_wrapped = wrapped;
        for c in 0..end {
            let cell = &row[Column(c)];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            pen.apply(cell, out);
            write_cell(cell, out);
        }
        if !wrapped && l != bottom {
            // A colored background would paint the new row when the
            // receiving terminal scrolls.
            if pen.set && pen.bg != Some(Color::Named(NamedColor::Background)) {
                out.push_str("\x1b[0m");
                pen.set = false;
            }
            out.push_str("\r\n");
        }
    }
}

/// The attributes last written, so a run of equal cells gets one SGR.
struct Pen {
    fg: Option<Color>,
    bg: Option<Color>,
    flags: Flags,
    underline: Option<Color>,
    /// False when the receiving terminal's attributes are not known.
    set: bool,
    link: Option<Hyperlink>,
}

impl Default for Pen {
    fn default() -> Self {
        Pen {
            fg: None,
            bg: None,
            flags: Flags::empty(),
            underline: None,
            set: false,
            link: None,
        }
    }
}

impl Pen {
    /// Writes what it takes to give the next cell the attributes and the
    /// hyperlink of `cell`.
    fn apply(&mut self, cell: &Cell, out: &mut String) {
        let flags = cell.flags & ATTRIBUTES;
        let underline = cell.underline_color();
        let same = self.set
            && self.fg == Some(cell.fg)
            && self.bg == Some(cell.bg)
            && self.flags == flags
            && self.underline == underline;
        if !same {
            out.push_str("\x1b[0");
            for (flag, code) in SGR_FLAGS {
                if flags.contains(flag) {
                    out.push(';');
                    out.push_str(code);
                }
            }
            if cell.fg != Color::Named(NamedColor::Foreground) {
                write_color(cell.fg, 30, out);
            }
            if cell.bg != Color::Named(NamedColor::Background) {
                write_color(cell.bg, 40, out);
            }
            if let Some(color) = underline {
                write_underline_color(color, out);
            }
            out.push('m');
            self.fg = Some(cell.fg);
            self.bg = Some(cell.bg);
            self.flags = flags;
            self.underline = underline;
            self.set = true;
        }
        let link = cell.hyperlink();
        if link != self.link {
            match &link {
                Some(link) => {
                    out.push_str(&format!("\x1b]8;id={};{}\x1b\\", link.id(), link.uri()))
                }
                None => out.push_str("\x1b]8;;\x1b\\"),
            }
            self.link = link;
        }
    }
}

/// A foreground (`base` 30) or background (`base` 40) color as SGR
/// parameters.
fn write_color(color: Color, base: u32, out: &mut String) {
    match color {
        Color::Named(name) => {
            let index = name as u32;
            let code = match index {
                0..=7 => base + index,
                8..=15 => base + 60 + index - 8,
                // The default color, and the colors only a renderer uses.
                _ => base + 9,
            };
            out.push_str(&format!(";{code}"));
        }
        Color::Indexed(index) => out.push_str(&format!(";{};5;{index}", base + 8)),
        Color::Spec(rgb) => out.push_str(&format!(";{};2;{};{};{}", base + 8, rgb.r, rgb.g, rgb.b)),
    }
}

/// The underline color (SGR 58). It has no short form for the 16 named
/// colors, so they are written by index.
fn write_underline_color(color: Color, out: &mut String) {
    match color {
        Color::Named(name) if (name as u32) < 16 => {
            out.push_str(&format!(";58;5;{}", name as u32));
        }
        Color::Named(_) => {}
        Color::Indexed(index) => out.push_str(&format!(";58;5;{index}")),
        Color::Spec(rgb) => out.push_str(&format!(";58;2;{};{};{}", rgb.r, rgb.g, rgb.b)),
    }
}
