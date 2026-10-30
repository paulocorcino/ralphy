use super::input::{StringCap, STRING_CAP};
use super::*;
use alacritty_terminal::vte::ansi::CursorShape;

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
    out.push_str(&format!(
        "cursor {:?} wrap={} pen={:?}/{:?}/{:?} mode={:?} style={:?} region={:?} title={:?} history={}\n",
        grid.cursor.point,
        grid.cursor.input_needs_wrap,
        template.fg,
        template.bg,
        template.flags,
        *screen.term.mode(),
        screen.term.cursor_style(),
        screen.region.region(),
        screen.title.title.lock().unwrap_or_else(PoisonError::into_inner),
        grid.history_size(),
    ));
    out
}

fn dump_grid(grid: &Grid<Cell>) -> String {
    let mut out = String::new();
    for l in grid.topmost_line().0..=grid.bottommost_line().0 {
        let row = &grid[Line(l)];
        for c in 0..grid.columns() {
            let cell = &row[Column(c)];
            out.push_str(&format!(
                "{}{:?}{:?}{:x}{:?}{:?}{:?}|",
                cell.c,
                cell.fg,
                cell.bg,
                cell.flags.bits(),
                cell.underline_color(),
                cell.zerowidth(),
                cell.hyperlink().map(|link| link.uri().to_string()),
            ));
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
        stream.push_str(&format!(
            "\x1b[3{}mline {n}\x1b[0m \x1b[1;3;4mbold\x1b[0m \x1b[2;7;9mdim\x1b[0m \x1b[38;5;{n}mindexed\x1b[0m \x1b[48;2;10;20;{n}mrgb\x1b[0m\r\n",
            n % 7 + 1
        ));
    }
    a.feed(stream.as_bytes());
    a.resize(ROWS_40, 60);
    let long: String = "0123456789".repeat(15);
    a.feed(
        format!(
            "{long}\r\n\x1b[4:3m\x1b[58;2;1;2;3mcurly\x1b[0m \x1b[4:2;58;5;1mdouble\x1b[0m \u{754c}\u{754c} e\u{301}\r\n\x1b[8mhidden\x1b[0m\r\n"
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
        stream.push_str(&format!("primary {n}\r\n"));
    }
    stream.push_str("cursor here\x1b[?1049h\x1b[H");
    for n in 0..10 {
        stream.push_str(&format!("\x1b[{};5H\x1b[44malt row {n}\x1b[0m", n + 1));
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
        b"\x1b]0;first title\x07\x1b]2;the\x01 title\x07see \x1b]8;id=a;https://example.com/x\x1b\\the link\x1b]8;;\x1b\\ here\r\n",
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

/// Codex's input box: a scroll region over the top rows, and 3000 lines
/// scrolled inside it. The lines go to the scrollback, up to its cap, and a
/// rebuilt screen scrolls the next lines the same way.
#[test]
fn lines_scrolled_inside_a_top_scroll_region_reach_the_scrollback() {
    let mut stream = format!("\x1b[1;{}r\x1b[{};1H", ROWS_40 - 3, ROWS_40 - 3);
    for n in 0..3000 {
        stream.push_str(&format!("\r\nhistory {n}"));
    }
    stream.push_str(&format!("\x1b[{};1H\x1b[7m input box \x1b[0m", ROWS_40 - 1));
    let mut a = Screen::new(ROWS_40, COLS_120, SCROLLBACK_LINES);
    a.feed(stream.as_bytes());
    assert_eq!(a.term.grid().history_size(), SCROLLBACK_LINES);
    assert_eq!(a.region.region(), Some((1, usize::from(ROWS_40) - 3)));

    let mut b = rebuilt(&mut a);
    assert_same(&mut a, &mut b, "scroll region");
    let more = format!("\x1b[{};1H\r\nnext 1\r\nnext 2", ROWS_40 - 3);
    a.feed(more.as_bytes());
    b.feed(more.as_bytes());
    assert_same(&mut a, &mut b, "scrolling after the snapshot");
}

/// The tracker applies the emulator's rules: an invalid region is ignored, a
/// bottom past the screen is clamped, a full reset or a resize clears it, and
/// a resize to the same size does not.
#[test]
fn the_region_tracker_follows_the_emulator() {
    let mut screen = Screen::new(ROWS_40, COLS_120, SCROLLBACK_LINES);
    screen.feed(b"\x1b[5;10r\x1b[10;5r");
    assert_eq!(
        screen.region.region(),
        Some((5, 10)),
        "top after bottom is ignored"
    );
    screen.feed(b"\x1b[3;999r");
    assert_eq!(screen.region.region(), Some((3, 40)));
    screen.feed(b"\x1b[r");
    assert_eq!(screen.region.region(), None);
    screen.feed(b"\x1b[2;9r\x1bc");
    assert_eq!(screen.region.region(), None, "a full reset");
    screen.feed(b"\x1b[2;9r");
    screen.resize(ROWS_40, COLS_120);
    assert_eq!(
        screen.region.region(),
        Some((2, 9)),
        "a resize to the same size keeps it, as the emulator does"
    );
    screen.resize(30, 80);
    assert_eq!(screen.region.region(), None, "a resize");
}

/// A synchronized update (mode 2026) that never ends is on the screen at
/// once (ADR-0079 D2).
#[test]
fn a_synchronized_update_is_applied_at_once() {
    let mut screen = Screen::new(ROWS_40, COLS_120, SCROLLBACK_LINES);
    screen.feed(b"\x1b[?2026hinside the update");
    assert!(
        screen.text().starts_with("inside the update"),
        "{}",
        screen.text()
    );
}

#[test]
fn the_scrollback_keeps_its_line_count() {
    let mut screen = Screen::new(ROWS_40, COLS_120, SCROLLBACK_LINES);
    for n in 0..5000 {
        screen.feed(format!("line {n}\r\n").as_bytes());
    }
    assert_eq!(screen.term.grid().history_size(), SCROLLBACK_LINES);
    assert!(
        screen.text().starts_with("line 3961\n"),
        "{:.40}",
        screen.text()
    );
}

#[test]
fn sizes_are_clamped() {
    let mut screen = Screen::new(0, 0, SCROLLBACK_LINES);
    assert_eq!(screen.size(), (2, 2));
    screen.feed(b"some text that wraps many times");
    screen.resize(u16::MAX, u16::MAX);
    assert_eq!(screen.size(), (300, 500));
    assert_eq!(screen.term.screen_lines(), 300);
    assert_eq!(screen.term.columns(), 500);
    screen.resize(0, 0);
    assert_eq!(screen.size(), (2, 2));
    screen.feed(b"more text");
    let mut b = rebuilt(&mut screen);
    assert_same(&mut screen, &mut b, "2x2");
    assert_eq!(clamp_size(1, 501), (2, 500));
}

/// A string sequence is ended at the cap and its rest is dropped; the bytes
/// after it pass.
#[test]
fn a_string_past_the_cap_is_ended_and_its_rest_dropped() {
    let mut cap = StringCap::default();
    let mut out = Vec::new();
    let mut stream = b"\x1b]2;".to_vec();
    stream.extend(std::iter::repeat_n(b'y', 4 * STRING_CAP));
    cap.filter(&stream, &mut out);
    // `ESC ]`, the string body up to the cap, CAN.
    assert_eq!(out.len(), 2 + STRING_CAP + 1);
    assert_eq!(out.last(), Some(&0x18), "ended with CAN");
    out.clear();
    cap.filter(b"yyy\x1b]2;short\x07after", &mut out);
    assert_eq!(out, b"\x1b]2;short\x07after".to_vec());

    let mut screen = Screen::new(ROWS_40, COLS_120, SCROLLBACK_LINES);
    let mut title = b"\x1b]2;".to_vec();
    title.extend(std::iter::repeat_n(b't', 2 * STRING_CAP));
    title.extend_from_slice(b"\x07visible");
    for chunk in title.chunks(4096) {
        screen.feed(chunk);
    }
    let kept = screen
        .title
        .title
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map_or(0, String::len);
    assert!(kept <= STRING_CAP, "title of {kept} bytes");
    assert!(
        screen.text().starts_with("visible"),
        "{:.40}",
        screen.text()
    );
}

/// A small, fixed pseudo-random stream: the same bytes on every run.
fn random_bytes(len: usize) -> Vec<u8> {
    let mut state: u64 = 689;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect()
}

/// The hostile streams of the spike of #689, generated here: no panic, and
/// the scrollback stays at its cap.
#[test]
fn hostile_output_does_not_panic() {
    let mut never_ended = b"\x1b]2;".to_vec();
    never_ended.extend(std::iter::repeat_n(b'y', 4 << 20));
    let cases: [(&str, Vec<u8>); 4] = [
        ("random bytes", random_bytes(4 << 20)),
        (
            "CSI with 10 000 parameters",
            format!("\x1b[{}m", vec!["1"; 10_000].join(";")).into_bytes(),
        ),
        ("OSC that never ends", never_ended),
        (
            "a DCS, an APC and a cursor far away",
            b"\x1bP1;2|abc\x1b\\\x1b_apc\x1b\\\x1b[65535;65535H\x1b[65535@\x1b[65535L\x1b[65535S\xff\xfe".to_vec(),
        ),
    ];
    for (name, stream) in cases {
        let mut screen = Screen::new(ROWS_40, COLS_120, SCROLLBACK_LINES);
        for chunk in stream.chunks(8192) {
            screen.feed(chunk);
        }
        screen.resize(0, 0);
        screen.feed(&stream[..stream.len().min(65536)]);
        let snapshot = screen.snapshot();
        let mut b = Screen::new(2, 2, SCROLLBACK_LINES);
        b.feed(&snapshot);
        assert!(
            screen.term.grid().history_size() <= SCROLLBACK_LINES,
            "{name}: history {}",
            screen.term.grid().history_size()
        );
        assert!(screen.text().len() < 1 << 20, "{name}");
    }
}

/// The manager keeps a screen per session under a lock that tokio tasks
/// share.
#[test]
fn a_screen_can_move_between_threads() {
    fn is_send<T: Send>() {}
    is_send::<Screen>();
}
