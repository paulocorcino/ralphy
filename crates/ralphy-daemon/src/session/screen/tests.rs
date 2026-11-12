use super::*;
use alacritty_terminal::vte::ansi::CursorShape;
use std::fmt::Write as _;

mod emulation;

const ROWS_40: u16 = 40;
const COLS_120: u16 = 120;

/// Text, colors, attributes, zero-width characters and hyperlink of every
/// cell; the cursor, its pending wrap and its pen; the modes, the scroll
/// region, the title and the scrollback length. Two screens with the same
/// dump look and behave the same.
fn dump(screen: &mut Screen) -> String {
    let mut out = String::new();
    if screen.term.mode().contains(TermMode::ALT_SCREEN) {
        out.push_str("PRIMARY\n");
        out.push_str(&with_primary(&mut screen.term, dump_grid));
        out.push_str("ALTERNATE\n");
    }
    out.push_str(&dump_grid(screen.term.grid()));
    let grid = screen.term.grid();
    let template = &grid.cursor.template;
    writeln!(
        out,
        "cursor {:?} wrap={} pen={:?}/{:?}/{:?} mode={:?} style={:?} region={:?} \
         title={:?} history={}",
        grid.cursor.point,
        grid.cursor.input_needs_wrap,
        template.fg,
        template.bg,
        template.flags,
        *screen.term.mode(),
        screen.term.cursor_style(),
        screen.region.region(),
        screen
            .title
            .title
            .lock()
            .unwrap_or_else(PoisonError::into_inner),
        grid.history_size(),
    )
    .expect(crate::WRITE_TO_STRING);
    out
}

fn dump_grid(grid: &Grid<Cell>) -> String {
    let mut out = String::new();
    for l in grid.topmost_line().0..=grid.bottommost_line().0 {
        let row = &grid[Line(l)];
        for c in 0..grid.columns() {
            let cell = &row[Column(c)];
            write!(
                out,
                "{}{:?}{:?}{:x}{:?}{:?}{:?}|",
                cell.c,
                cell.fg,
                cell.bg,
                cell.flags.bits(),
                cell.underline_color(),
                cell.zerowidth(),
                cell.hyperlink().map(|link| link.uri().to_owned()),
            )
            .expect(crate::WRITE_TO_STRING);
        }
        out.push('\n');
    }
    out
}

/// Feeds the snapshot of `a` to a fresh screen of the same size and returns
/// it.
fn rebuilt(a: &mut Screen) -> Screen {
    let (rows, cols) = a.size();
    let mut b = Screen::new(rows, cols, SCROLLBACK_LINES);
    b.feed(&a.snapshot());
    b
}

/// The first line where the dumps differ, so a failure names the cell.
fn assert_same(a: &mut Screen, b: &mut Screen, case: &str) {
    let (left, right) = (dump(a), dump(b));
    if left != right {
        let first = left
            .lines()
            .zip(right.lines())
            .position(|(x, y)| x != y)
            .map_or_else(
                || {
                    format!(
                        "line count {} vs {}",
                        left.lines().count(),
                        right.lines().count()
                    )
                },
                |i| {
                    format!(
                        "line {i}:\n  {:.300}\n  {:.300}",
                        left.lines().nth(i).unwrap_or_default(),
                        right.lines().nth(i).unwrap_or_default()
                    )
                },
            );
        panic!("{case}: the rebuilt screen differs at {first}");
    }
}

fn round_trip(stream: &[u8], case: &str) {
    let mut a = Screen::new(ROWS_40, COLS_120, SCROLLBACK_LINES);
    a.feed(stream);
    let mut b = rebuilt(&mut a);
    assert_same(&mut a, &mut b, case);
}

/// Colored text, every attribute, wide and combining characters, and a
/// width change in the middle.
#[test]
fn a_snapshot_rebuilds_the_screen_after_a_width_change() {
    let mut a = Screen::new(ROWS_40, COLS_120, SCROLLBACK_LINES);
    let mut stream = String::new();
    for n in 0..80 {
        write!(
            stream,
            "\x1b[3{}mline {n}\x1b[0m \x1b[1;3;4mbold\x1b[0m \x1b[2;7;9mdim\x1b[0m \
             \x1b[38;5;{n}mindexed\x1b[0m \x1b[48;2;10;20;{n}mrgb\x1b[0m\r\n",
            n % 7 + 1
        )
        .expect(crate::WRITE_TO_STRING);
    }
    a.feed(stream.as_bytes());
    a.resize(ROWS_40, 60);
    let long: String = "0123456789".repeat(15);
    a.feed(
        format!(
            "{long}\r\n\x1b[4:3m\x1b[58;2;1;2;3mcurly\x1b[0m \x1b[4:2;58;5;1mdouble\x1b[0m \
             \u{754c}\u{754c} e\u{301}\r\n\x1b[8mhidden\x1b[0m\r\n"
        )
        .as_bytes(),
    );
    a.feed(b"\x1b[5;30r\x1b[12;7H\x1b[?25l\x1b[1;35;44m");
    let mut b = rebuilt(&mut a);
    assert_same(&mut a, &mut b, "width change");
    assert!(!b.term.mode().contains(TermMode::SHOW_CURSOR));
    assert_eq!(b.term.grid().cursor.point, a.term.grid().cursor.point);
    assert_eq!(b.region.region(), Some((5, 30)));
}

/// Every input mode ADR-0079 D5 lists, set alone, is set again by the
/// snapshot.
#[test]
fn each_input_mode_survives_a_snapshot() {
    let cases: [(&str, &str, TermMode, bool); 16] = [
        (
            "mouse 1000",
            "\x1b[?1000h",
            TermMode::MOUSE_REPORT_CLICK,
            true,
        ),
        ("mouse 1002", "\x1b[?1002h", TermMode::MOUSE_DRAG, true),
        ("mouse 1003", "\x1b[?1003h", TermMode::MOUSE_MOTION, true),
        (
            "mouse encoding 1006",
            "\x1b[?1006h",
            TermMode::SGR_MOUSE,
            true,
        ),
        (
            "mouse encoding 1005",
            "\x1b[?1005h",
            TermMode::UTF8_MOUSE,
            true,
        ),
        (
            "bracketed paste 2004",
            "\x1b[?2004h",
            TermMode::BRACKETED_PASTE,
            true,
        ),
        ("application cursor", "\x1b[?1h", TermMode::APP_CURSOR, true),
        ("application keypad", "\x1b=", TermMode::APP_KEYPAD, true),
        (
            "focus events 1004",
            "\x1b[?1004h",
            TermMode::FOCUS_IN_OUT,
            true,
        ),
        (
            "kitty keyboard 1",
            "\x1b[>1u",
            TermMode::DISAMBIGUATE_ESC_CODES,
            true,
        ),
        (
            "kitty keyboard 31",
            "\x1b[>31u",
            TermMode::REPORT_ASSOCIATED_TEXT,
            true,
        ),
        ("insert mode", "\x1b[4h", TermMode::INSERT, true),
        (
            "new-line mode",
            "\x1b[20h",
            TermMode::LINE_FEED_NEW_LINE,
            true,
        ),
        (
            "alternate scroll 1007",
            "\x1b[?1007h",
            TermMode::ALTERNATE_SCROLL,
            true,
        ),
        ("no auto-wrap", "\x1b[?7l", TermMode::LINE_WRAP, false),
        (
            "origin mode",
            "\x1b[3;20r\x1b[?6h\x1b[4;5H",
            TermMode::ORIGIN,
            true,
        ),
    ];
    for (name, sequence, flag, on) in cases {
        let mut a = Screen::new(ROWS_40, COLS_120, SCROLLBACK_LINES);
        a.feed(format!("text before\r\n{sequence}").as_bytes());
        assert_eq!(
            a.term.mode().contains(flag),
            on,
            "{name}: the stream sets it"
        );
        let mut b = rebuilt(&mut a);
        assert_eq!(
            b.term.mode().contains(flag),
            on,
            "{name}: the snapshot keeps it"
        );
        assert_same(&mut a, &mut b, name);
    }
}

#[test]
fn the_cursor_shape_and_its_visibility_survive_a_snapshot() {
    for (sequence, shape, blinking) in [
        ("\x1b[1 q", CursorShape::Block, true),
        ("\x1b[4 q", CursorShape::Underline, false),
        ("\x1b[5 q", CursorShape::Beam, true),
        ("\x1b[6 q\x1b[?25l", CursorShape::Beam, false),
    ] {
        let mut a = Screen::new(ROWS_40, COLS_120, SCROLLBACK_LINES);
        a.feed(sequence.as_bytes());
        let mut b = rebuilt(&mut a);
        let style = b.term.cursor_style();
        assert_eq!(
            (style.shape, style.blinking),
            (shape, blinking),
            "{sequence:?}"
        );
        assert_same(&mut a, &mut b, sequence);
    }
}

/// A row that wrapped, followed by a row that was erased: the snapshot must
/// still make the pending wrap of the first row fire, or the receiving
/// terminal puts the next line one row too high. Found in a recorded Claude
/// Code stream by the spike of #689.
#[test]
fn a_wrapped_row_before_an_empty_row_keeps_its_line() {
    let full = "x".repeat(usize::from(COLS_120));
    round_trip(
        format!("{full}y\r\x1b[K\r\nthe next line\r\nand one more").as_bytes(),
        "wrapped row, empty row",
    );
}

/// The cursor waiting past the last column, also after a wide character.
#[test]
fn a_pending_wrap_survives_a_snapshot() {
    let full = "x".repeat(usize::from(COLS_120));
    let mut a = Screen::new(ROWS_40, COLS_120, SCROLLBACK_LINES);
    a.feed(full.as_bytes());
    assert!(a.term.grid().cursor.input_needs_wrap);
    let mut b = rebuilt(&mut a);
    assert!(b.term.grid().cursor.input_needs_wrap);
    assert_same(&mut a, &mut b, "pending wrap");

    let wide = format!("\x1b[32m{}\u{754c}", "x".repeat(usize::from(COLS_120) - 2));
    round_trip(wide.as_bytes(), "pending wrap after a wide character");
}

/// The alternate screen is rebuilt over the primary screen, which keeps its
/// scrollback and its cursor for the moment the alternate screen ends.
#[test]
fn the_alternate_screen_keeps_the_primary_screen_under_it() {
    let mut stream = String::new();
    for n in 0..60 {
        write!(stream, "primary {n}\r\n").expect(crate::WRITE_TO_STRING);
    }
    stream.push_str("cursor here\x1b[?1049h\x1b[H");
    for n in 0..10 {
        write!(stream, "\x1b[{};5H\x1b[44malt row {n}\x1b[0m", n + 1)
            .expect(crate::WRITE_TO_STRING);
    }
    let mut a = Screen::new(ROWS_40, COLS_120, SCROLLBACK_LINES);
    a.feed(stream.as_bytes());
    let mut b = rebuilt(&mut a);
    assert!(b.term.mode().contains(TermMode::ALT_SCREEN));
    assert!(
        a.text().contains("alt row 9"),
        "reading the primary screen keeps the alternate one"
    );
    assert!(b.text().contains("alt row 9"), "{}", b.text());
    assert_same(&mut a, &mut b, "alternate screen on");

    let leave = b"\x1b[?1049lafter";
    a.feed(leave);
    b.feed(leave);
    assert_same(&mut a, &mut b, "alternate screen off again");
    assert!(b.text().contains("primary 0\n"), "{}", b.text());
}

#[test]
fn the_title_and_a_hyperlink_survive_a_snapshot() {
    round_trip(
        b"\x1b]0;first title\x07\x1b]2;the\x01 title\x07see \
          \x1b]8;id=a;https://example.com/x\x1b\\the link\x1b]8;;\x1b\\ here\r\n",
        "title and hyperlink",
    );
}

/// A wide character is one character of the text, not a character and a
/// space, and a combining mark stays on the character it marks.
#[test]
fn the_text_holds_a_wide_character_once_and_keeps_a_combining_mark() {
    let mut screen = Screen::new(ROWS_40, COLS_120, SCROLLBACK_LINES);
    screen.feed("\u{6F22}x e\u{301}\r\n".as_bytes());
    assert_eq!(screen.text().lines().next(), Some("\u{6F22}x e\u{301}"));
}

/// A line written at 120 columns is wrapped, not cut, at 60, and is whole
/// again at 120.
#[test]
fn a_resize_wraps_a_long_line_and_does_not_cut_it() {
    let long: String = (0..300usize)
        .map(|i| char::from(b'a'.saturating_add(u8::try_from(i % 26).unwrap_or(0))))
        .collect();
    let mut screen = Screen::new(ROWS_40, COLS_120, SCROLLBACK_LINES);
    screen.feed(format!("{long}\r\nshort\r\n").as_bytes());
    screen.resize(ROWS_40, 60);
    assert!(
        screen.text().lines().any(|line| line == long),
        "at 60: {}",
        screen.text()
    );
    let rows_used = screen
        .text()
        .lines()
        .take_while(|line| *line != "short")
        .count();
    assert_eq!(rows_used, 1, "one logical line");
    screen.resize(ROWS_40, COLS_120);
    assert!(
        screen.text().lines().any(|line| line == long),
        "at 120: {}",
        screen.text()
    );
}
