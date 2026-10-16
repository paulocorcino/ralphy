//! What the output bytes go through besides the emulator: the cap on string
//! sequences, and a second parser that tracks the scroll region.

use alacritty_terminal::vte::{Params, Parser, Perform};

/// The longest OSC, DCS, APC, PM or SOS string the emulator gets (ADR-0079
/// D3). vte keeps an OSC string in a buffer that never shrinks, so one
/// string that never ends would grow the daemon without a bound.
pub(super) const STRING_CAP: usize = 64 * 1024;

const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
const ESC: u8 = 0x1b;

/// Where the cap is in the byte stream. It follows vte's own rules for the
/// start and the end of a string: `ESC ] P X ^ _` start one; BEL, CAN, SUB
/// and `ESC \` end it; any other `ESC` ends it and starts an escape.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
enum State {
    #[default]
    Ground,
    Escape,
    InString,
    EscapeInString,
    /// Past the cap: bytes are dropped until the string ends.
    Dropping,
    EscapeWhileDropping,
}

/// Ends a string sequence with CAN after [`STRING_CAP`] bytes, and drops the
/// rest of that string.
#[derive(Default)]
pub(super) struct StringCap {
    state: State,
    len: usize,
}

impl StringCap {
    /// Appends `bytes` to `out`, with every string sequence cut at the cap.
    pub(super) fn filter(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        out.reserve(bytes.len());
        for &byte in bytes {
            match self.state {
                State::Ground => {
                    if byte == ESC {
                        self.state = State::Escape;
                    }
                    out.push(byte);
                }
                State::Escape => {
                    self.after_escape(byte);
                    out.push(byte);
                }
                State::InString => {
                    if self.in_string(byte, out) {
                        out.push(byte);
                    }
                }
                State::EscapeInString => {
                    self.escape_in_string(byte);
                    out.push(byte);
                }
                State::Dropping | State::EscapeWhileDropping => self.while_dropping(byte, out),
            }
        }
    }

    /// One byte inside a string. Past the cap it ends the string with CAN and
    /// returns false: the byte is dropped.
    fn in_string(&mut self, byte: u8, out: &mut Vec<u8>) -> bool {
        match byte {
            BEL | CAN | SUB => self.state = State::Ground,
            ESC => self.state = State::EscapeInString,
            _ => {
                self.len += 1;
                if self.len > STRING_CAP {
                    out.push(CAN);
                    self.state = State::Dropping;
                    return false;
                }
            }
        }
        true
    }

    /// The byte after an `ESC` inside a string: `\` ends the string; any
    /// other byte ends it too and is read as the byte after an `ESC`.
    fn escape_in_string(&mut self, byte: u8) {
        if byte == b'\\' {
            self.state = State::Ground;
        } else {
            self.after_escape(byte);
        }
    }

    /// One byte of a string past the cap: dropped until the string ends.
    fn while_dropping(&mut self, byte: u8, out: &mut Vec<u8>) {
        if self.state == State::Dropping {
            match byte {
                BEL | CAN | SUB => self.state = State::Ground,
                ESC => self.state = State::EscapeWhileDropping,
                _ => {}
            }
        } else if byte == b'\\' {
            self.state = State::Ground;
        } else {
            // The dropped string ended; this escape is a new one.
            out.push(ESC);
            self.after_escape(byte);
            out.push(byte);
        }
    }

    fn after_escape(&mut self, byte: u8) {
        self.state = match byte {
            b']' | b'P' | b'_' | b'^' | b'X' => {
                self.len = 0;
                State::InString
            }
            ESC => State::Escape,
            _ => State::Ground,
        };
    }
}

/// Tracks the scroll region (DECSTBM), which `alacritty_terminal` 0.26.0
/// keeps private. It reads the same bytes as the emulator and applies the
/// emulator's rules: an invalid region is ignored, the bottom is clamped to
/// the screen, a resize and a full reset (`ESC c`) clear it.
pub(super) struct RegionTracker {
    parser: Parser,
    state: Region,
}

struct Region {
    rows: usize,
    /// `(top, bottom)`: the first row (0-based) and the row after the last.
    /// `None` when the region is the whole screen.
    region: Option<(usize, usize)>,
}

impl RegionTracker {
    pub(super) fn new(rows: usize) -> Self {
        RegionTracker {
            parser: Parser::new(),
            state: Region { rows, region: None },
        }
    }

    pub(super) fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.state, bytes);
    }

    pub(super) fn resize(&mut self, rows: usize) {
        self.state.rows = rows;
        self.state.region = None;
    }

    /// `(top, bottom)` as DECSTBM writes them (1-based, inclusive), when a
    /// region other than the whole screen is set.
    pub(super) fn region(&self) -> Option<(usize, usize)> {
        self.state.region.map(|(top, end)| (top + 1, end))
    }
}

impl Perform for Region {
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if ignore || action != 'r' || !intermediates.is_empty() {
            return;
        }
        let mut params = params.iter();
        let top = match params.next() {
            Some(&[top, ..]) if top != 0 => usize::from(top),
            _ => 1,
        };
        let bottom = match params.next() {
            Some(&[bottom, ..]) if bottom != 0 => usize::from(bottom),
            _ => self.rows,
        };
        if top >= bottom {
            return;
        }
        let start = (top - 1).min(self.rows);
        let end = bottom.min(self.rows);
        self.region = if start == 0 && end == self.rows {
            None
        } else {
            Some((start, end))
        };
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        if intermediates.is_empty() && byte == b'c' {
            self.region = None;
        }
    }
}

#[cfg(test)]
mod tests;
