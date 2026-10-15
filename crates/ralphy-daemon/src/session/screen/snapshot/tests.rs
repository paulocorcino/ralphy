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
