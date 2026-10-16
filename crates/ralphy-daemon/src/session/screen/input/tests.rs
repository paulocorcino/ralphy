use super::*;

/// A cap whose string just passed [`STRING_CAP`], so it drops bytes.
fn dropping() -> StringCap {
    let mut cap = StringCap::default();
    let mut stream = b"\x1b]2;".to_vec();
    stream.extend(std::iter::repeat_n(b'y', STRING_CAP + 1));
    let mut out = Vec::new();
    cap.filter(&stream, &mut out);
    assert_eq!(out.last(), Some(&CAN), "the string was cut");
    cap
}

fn filtered(cap: &mut StringCap, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    cap.filter(bytes, &mut out);
    out
}

#[test]
fn a_dropped_string_ends_at_bel_can_and_sub() {
    for end in [BEL, CAN, SUB] {
        let mut cap = dropping();
        let mut stream = b"yy".to_vec();
        stream.push(end);
        stream.extend_from_slice(b"after");
        assert_eq!(filtered(&mut cap, &stream), b"after", "ended by {end:#04x}");
    }
}

#[test]
fn a_dropped_string_ends_at_esc_backslash() {
    let mut cap = dropping();
    assert_eq!(filtered(&mut cap, b"yy\x1b\\after"), b"after");
}

/// `ESC [` inside a string ends the string and starts a CSI, so the bytes
/// after it are not part of a string and are never cut.
#[test]
fn another_escape_inside_a_string_ends_the_string() {
    let mut cap = StringCap::default();
    let mut stream = b"\x1b]2;ab\x1b[31m".to_vec();
    stream.extend(std::iter::repeat_n(b'y', STRING_CAP + 1));
    assert_eq!(filtered(&mut cap, &stream), stream);
}
