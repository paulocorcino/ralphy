//! The PTY byte stream as the terminal sees it: escape stripping, the logged-out
//! login-TUI watch and the cursor-position (DSR) scanner.

use ralphy_pty::CURSOR_POSITION_REQUEST;

/// Flatten raw PTY bytes into matchable text: ANSI escape sequences are
/// dropped, and the ones that *position* text (CSI cursor-forward `ESC[nC`,
/// cursor-position `ESC[r;cH`/`f`) become a single space — the interactive TUI
/// separates the words of one visual line with cursor moves instead of spaces
/// (`Not<ESC[1C>logged<ESC[1C>in`), so without this no substring can match.
/// CR/LF also become spaces so a phrase split across writes still joins.
pub(crate) fn strip_pty_escapes(raw: &[u8]) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            0x1b => i = skip_escape(raw, i + 1, &mut out),
            b'\r' | b'\n' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Skip the escape whose ESC byte sits just before `i`; return the index of the
/// next byte to read.
fn skip_escape(raw: &[u8], i: usize, out: &mut Vec<u8>) -> usize {
    match raw.get(i) {
        Some(b'[') => skip_csi(raw, i + 1, out),
        Some(b']') => skip_osc(raw, i + 1),
        // Two-byte escape (ESC c, ESC =, ...): drop the pair.
        Some(_) => i + 1,
        None => i,
    }
}

/// Skip a CSI body from `i`: params/intermediates until a final byte in
/// 0x40-0x7e. The cursor moves `C`, `H` and `f` leave a space in `out`.
fn skip_csi(raw: &[u8], mut i: usize, out: &mut Vec<u8>) -> usize {
    while i < raw.len() && !(0x40..=0x7e).contains(&raw[i]) {
        i += 1;
    }
    if matches!(raw.get(i), Some(b'C' | b'H' | b'f')) {
        out.push(b' ');
    }
    i + 1
}

/// Skip an OSC body from `i`: swallow until BEL or ST (ESC `\`).
fn skip_osc(raw: &[u8], mut i: usize) -> usize {
    while i < raw.len() {
        if raw[i] == 0x07 {
            return i + 1;
        }
        if raw[i] == 0x1b && raw.get(i + 1) == Some(&b'\\') {
            return i + 2;
        }
        i += 1;
    }
    i
}

/// Return `true` when raw interactive PTY output shows the logged-out REPL.
/// The signature is the status-line pair `Not logged in · Run /login`
/// (captured from CLI v2.1.198 — see tests/fixtures/login_tui_exec.log); the
/// headless banner says `Please run /login`, so `run /login` matches both.
/// Both substrings are required, the same AND rule as [`crate::auth::is_claude_auth_error`].
pub(super) fn is_login_tui_output(raw: &[u8]) -> bool {
    let text = strip_pty_escapes(raw).to_lowercase();
    text.contains("not logged in") && text.contains("run /login")
}

/// Rolling watch over the live PTY stream for the logged-out login TUI.
///
/// A logged-out *interactive* session renders the login TUI and stalls without
/// exiting, so the `child_exited` auth check never runs and the session used
/// to burn its whole wall budget and surface as a misleading `Timeout`
/// (issue #72). The watch accumulates a bounded tail of the raw output and is
/// consulted on the session's poll cadence.
///
/// Once the JSONL transcript shows any activity the watch disarms for the rest
/// of the session: a live transcript proves the model loop started (a
/// logged-out session never produces one), and from then on agent output that
/// merely *echoes* the signature — reading this source, for instance — must
/// not trip it.
pub(super) struct LoginTuiWatch {
    buf: Vec<u8>,
    disarmed: bool,
}

impl LoginTuiWatch {
    /// Plenty for the login screen; the TUI redraws, so the signature recurs.
    const MAX_BUF: usize = 32 * 1024;

    pub(super) fn new() -> Self {
        Self {
            buf: Vec::new(),
            disarmed: false,
        }
    }

    /// Accumulate a PTY chunk (keeps the most recent [`Self::MAX_BUF`] bytes).
    pub(super) fn feed(&mut self, chunk: &[u8]) {
        if self.disarmed {
            return;
        }
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > Self::MAX_BUF {
            let cut = self.buf.len() - Self::MAX_BUF;
            self.buf.drain(..cut);
        }
    }

    /// Transcript activity observed — stop watching and drop the buffer.
    pub(super) fn disarm(&mut self) {
        self.disarmed = true;
        self.buf = Vec::new();
    }

    pub(super) fn detected(&self) -> bool {
        !self.disarmed && is_login_tui_output(&self.buf)
    }
}

/// Index of the first occurrence of `needle` in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Rolling-tail DSR scanner. Appends `chunk` to `carry`, searches the combined
/// buffer for `CURSOR_POSITION_REQUEST`, then truncates `carry` to the last
/// `CURSOR_POSITION_REQUEST.len() - 1` bytes so a split sequence spanning the
/// next chunk can still match. Returns `true` if the sequence was found.
pub(super) fn scan_dsr_request(carry: &mut Vec<u8>, chunk: &[u8]) -> bool {
    carry.extend_from_slice(chunk);
    let found = find_subslice(carry, CURSOR_POSITION_REQUEST).is_some();
    let keep = CURSOR_POSITION_REQUEST.len().saturating_sub(1);
    if carry.len() > keep {
        carry.drain(..carry.len() - keep);
    }
    found
}
