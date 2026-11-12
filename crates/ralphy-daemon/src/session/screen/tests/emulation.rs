use super::super::input::{StringCap, STRING_CAP};
use super::*;
/// Codex's input box: a scroll region over the top rows, and 3000 lines
/// scrolled inside it. The lines go to the scrollback, up to its cap, and a
/// rebuilt screen scrolls the next lines the same way.
#[test]
fn lines_scrolled_inside_a_top_scroll_region_reach_the_scrollback() {
    let mut stream = format!("\x1b[1;{}r\x1b[{};1H", ROWS_40 - 3, ROWS_40 - 3);
    for n in 0..3000 {
        write!(stream, "\r\nhistory {n}").expect(crate::WRITE_TO_STRING);
    }
    write!(stream, "\x1b[{};1H\x1b[7m input box \x1b[0m", ROWS_40 - 1)
        .expect(crate::WRITE_TO_STRING);
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
            b"\x1bP1;2|abc\x1b\\\x1b_apc\x1b\\\x1b[65535;65535H\x1b[65535@\x1b[65535L\
              \x1b[65535S\xff\xfe"
                .to_vec(),
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
