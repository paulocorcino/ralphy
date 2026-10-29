use crate::session::screen::{Screen, SCROLLBACK_LINES};

/// A row with a colored background ends with a reset before CR LF, so the
/// receiving terminal does not paint the next row when it scrolls; the next
/// row sets its color again.
#[test]
fn a_colored_background_is_reset_before_the_line_break() {
    let mut screen = Screen::new(5, 20, SCROLLBACK_LINES);
    screen.feed(b"\x1b[41mred\r\nred2");
    let snapshot = String::from_utf8(screen.snapshot()).expect("a snapshot is text");
    assert!(
        snapshot.contains("red\x1b[0m\r\n\x1b[0;41mred2"),
        "{snapshot:?}"
    );
}

/// A blinking underline cursor is written with its own DECSCUSR number.
#[test]
fn a_blinking_underline_cursor_survives_a_snapshot() {
    let mut screen = Screen::new(5, 20, SCROLLBACK_LINES);
    screen.feed(b"\x1b[3 q");
    let snapshot = String::from_utf8(screen.snapshot()).expect("a snapshot is text");
    assert!(snapshot.contains("\x1b[3 q"), "{snapshot:?}");
}

/// A hyperlink still open after the last cell is closed before the scroll
/// region and the input modes are written.
#[test]
fn a_link_open_at_the_end_of_the_screen_is_closed_before_the_region() {
    let mut screen = Screen::new(5, 20, SCROLLBACK_LINES);
    screen.feed(b"\x1b]8;id=a;https://example.com/x\x1b\\link\x1b[2;4r\x1b[?2004h");
    let snapshot = String::from_utf8(screen.snapshot()).expect("a snapshot is text");
    assert!(
        snapshot.contains("\x1b]8;;\x1b\\\x1b[2;4r\x1b[?2004h"),
        "{snapshot:?}"
    );
}
