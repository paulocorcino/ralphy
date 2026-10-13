//! The pass of `js::scan`: it follows the open brackets and the bodies of
//! copy helpers, and hands each name to the sink it can be.

use super::super::lex::Tok;
use super::super::{Found, Kind};
use super::scan::Scan;
use super::{anchor, owner, screaming, HELPER_SUFFIXES, SHOWN_KEYS};

/// One open bracket: its kind, the call that owns it (for `(`, and for a `{`
/// passed straight to a call) and whether it is a helper's body.
struct Open<'a> {
    kind: &'static str,
    owner: Option<&'a str>,
    helper: bool,
}

/// The identifier at token `i` and what surrounds it.
struct Site<'a> {
    i: usize,
    name: &'a str,
    /// A member access, plain or optional (`WBFail?.failed(`).
    prev_dot: bool,
    after_fn: bool,
    calls: bool,
}

pub(super) struct Walk<'a> {
    pub(super) sc: &'a Scan<'a>,
    pub(super) out: &'a mut Vec<Found>,
    stack: Vec<Open<'a>>,
    helper_body_at: Option<usize>,
}

impl<'a> Walk<'a> {
    pub(super) fn new(sc: &'a Scan<'a>, out: &'a mut Vec<Found>) -> Self {
        Walk {
            sc,
            out,
            stack: Vec::new(),
            helper_body_at: None,
        }
    }

    pub(super) fn run(mut self) {
        for i in 0..self.sc.toks.len() {
            self.visit(i);
        }
    }

    fn visit(&mut self, i: usize) {
        self.track_brackets(i);
        let Some(name) = self.sc.toks[i].ident() else {
            return;
        };
        let site = self.site(i, name);
        self.note_helper_declaration(&site);
        self.sink(&site);
    }

    fn site(&self, i: usize, name: &'a str) -> Site<'a> {
        let sc = self.sc;
        Site {
            i,
            name,
            prev_dot: sc.is_at(i.wrapping_sub(1), ".") || sc.is_at(i.wrapping_sub(1), "?."),
            after_fn: sc.ident_at(i.wrapping_sub(1)) == Some("function"),
            calls: sc.is_at(i + 1, "("),
        }
    }

    fn track_brackets(&mut self, i: usize) {
        match &self.sc.toks[i].tok {
            Tok::Punct(p @ ("(" | "[" | "{")) => {
                let owner = self.bracket_owner(p, i);
                let helper = self.helper_body_at == Some(i);
                self.stack.push(Open {
                    kind: p,
                    owner,
                    helper,
                });
            }
            Tok::Punct(")" | "]" | "}") => {
                self.stack.pop();
            }
            _ => {}
        }
    }

    fn bracket_owner(&self, open: &str, i: usize) -> Option<&'a str> {
        let sc = self.sc;
        match open {
            "(" => sc.ident_at(i.wrapping_sub(1)),
            "{" if sc.is_at(i.wrapping_sub(1), "(") => self.stack.last().and_then(|o| o.owner),
            _ => None,
        }
    }

    /// A call to a copy helper that is followed by a body: remember where the
    /// body opens, so its `return` statements are rows.
    fn note_helper_declaration(&mut self, site: &Site<'_>) {
        if !(self.is_helper_name(site.name) && site.calls && !site.prev_dot) {
            return;
        }
        let sc = self.sc;
        let body = sc.after_return_type(sc.close_of(site.i + 1) + 1);
        if sc.is_at(body, "{") {
            self.helper_body_at = Some(body);
        }
    }

    fn is_helper_name(&self, name: &str) -> bool {
        HELPER_SUFFIXES.iter().any(|s| name.ends_with(s))
            || self.sc.fns.helpers.iter().any(|h| h == name)
    }

    /// The first sink that matches wins.
    fn sink(&mut self, site: &Site<'_>) {
        match site.name {
            key if self.is_shown_key(site, key) => self.shown_key(site, key),
            "confirm" | "prompt" | "alert" if self.is_dialog_call(site) => self.dialog_call(site),
            "textContent" | "innerText" | "innerHTML" | "title" | "placeholder" | "ariaLabel"
                if self.is_dom_assignment(site) =>
            {
                self.dom_assignment(site);
            }
            "setAttribute" if site.calls => self.set_attribute(site),
            "write" | "writeln" if self.is_terminal_write(site) => self.terminal_write(site),
            "key" if site.calls && !site.prev_dot && !site.after_fn => self.key_bar(site),
            "this" if self.is_state_assignment(site) => self.state_assignment(site),
            "const" if self.is_copy_const(site) => self.copy_const(site),
            call if self.is_listed_call(site, call) => self.listed_call(site, call),
            "return" if self.stack.iter().any(|o| o.helper) => self.helper_return(site),
            _ => {}
        }
    }

    /// Emit one expression and book `before` as the source text ahead of it.
    fn emit_anchored(
        &mut self,
        range: (usize, usize),
        kind: Kind,
        strip_html: bool,
        before: String,
    ) {
        let from = self.out.len();
        self.sc
            .emit(range.0, range.1, kind, strip_html, false, self.out);
        anchor(&mut self.out[from..], before);
    }

    fn is_shown_key(&self, site: &Site<'_>, key: &str) -> bool {
        let (sc, i) = (self.sc, site.i);
        SHOWN_KEYS.contains(&key)
            && sc.is_at(i + 1, ":")
            && (sc.is_at(i.wrapping_sub(1), "{") || sc.is_at(i.wrapping_sub(1), ","))
            && self.stack.last().is_some_and(|o| o.kind == "{")
    }

    fn shown_key(&mut self, site: &Site<'_>, key: &str) {
        let kind = match self.stack.last().and_then(|o| o.owner) {
            Some("toast") => Kind::Toast,
            Some("askConfirm" | "askNotice" | "askPrompt" | "askInShell") => Kind::Confirm,
            _ => Kind::Property,
        };
        let end = self.sc.expr_end(site.i + 2, true);
        self.emit_anchored((site.i + 2, end), kind, false, format!("{key}: "));
    }

    fn is_dialog_call(&self, site: &Site<'_>) -> bool {
        site.calls
            && !site.after_fn
            && (!site.prev_dot || self.sc.ident_at(site.i.wrapping_sub(2)) == Some("window"))
    }

    fn dialog_call(&mut self, site: &Site<'_>) {
        if let Some(&(a, b)) = self.sc.args(site.i + 1).first() {
            self.sc.emit(a, b, Kind::Confirm, false, false, self.out);
        }
    }

    fn is_dom_assignment(&self, site: &Site<'_>) -> bool {
        site.prev_dot && (self.sc.is_at(site.i + 1, "=") || self.sc.is_at(site.i + 1, "+="))
    }

    fn dom_assignment(&mut self, site: &Site<'_>) {
        let kind = match site.name {
            "title" | "placeholder" | "ariaLabel" => Kind::Attribute,
            _ => Kind::TextContent,
        };
        let end = self.sc.expr_end(site.i + 2, false);
        let html = site.name == "innerHTML";
        self.emit_anchored((site.i + 2, end), kind, html, format!("{} = ", site.name));
    }

    fn set_attribute(&mut self, site: &Site<'_>) {
        let sc = self.sc;
        let args = sc.args(site.i + 1);
        let [(a0, b0), (a1, b1), ..] = args[..] else {
            return;
        };
        let Tok::Str(attr) = &sc.toks[a0].tok else {
            return;
        };
        if b0 == a0 + 1 && matches!(attr.as_str(), "title" | "aria-label" | "placeholder") {
            self.emit_anchored((a1, b1), Kind::Attribute, false, format!("\"{attr}\", "));
        }
    }

    fn is_terminal_write(&self, site: &Site<'_>) -> bool {
        site.calls && site.prev_dot && self.sc.ident_at(site.i.wrapping_sub(2)) == Some("term")
    }

    fn terminal_write(&mut self, site: &Site<'_>) {
        if let Some(&(a, b)) = self.sc.args(site.i + 1).first() {
            self.sc.emit(a, b, Kind::TermNotice, false, false, self.out);
        }
    }

    fn key_bar(&mut self, site: &Site<'_>) {
        let sc = self.sc;
        let args = sc.args(site.i + 1);
        if let [(a0, b0), (a1, b1), (a2, b2), ..] = args[..] {
            if b0 == a0 + 1 && matches!(sc.toks[a0].tok, Tok::Str(_)) {
                sc.emit(a1, b1, Kind::KeyBar, true, false, self.out);
                sc.emit(a2, b2, Kind::KeyBar, false, false, self.out);
            }
        }
    }

    fn is_state_assignment(&self, site: &Site<'_>) -> bool {
        let (sc, i) = (self.sc, site.i);
        sc.is_at(i + 1, ".") && sc.ident_at(i + 2).is_some() && sc.is_at(i + 3, "=")
    }

    fn state_assignment(&mut self, site: &Site<'_>) {
        let end = self.sc.expr_end(site.i + 4, false);
        self.sc
            .emit(site.i + 4, end, Kind::State, false, true, self.out);
    }

    fn is_copy_const(&self, site: &Site<'_>) -> bool {
        let (sc, i) = (self.sc, site.i);
        sc.ident_at(i + 1).is_some_and(screaming) && sc.is_at(i + 2, "=")
    }

    fn copy_const(&mut self, site: &Site<'_>) {
        let (sc, i) = (self.sc, site.i);
        let end = sc.expr_end(i + 3, false);
        let from = self.out.len();
        sc.emit(i + 3, end, Kind::Const, false, true, self.out);
        // A media query or a selector (`"(max-width: 560px)"`) has
        // spaces and is still not copy.
        let emitted = self.out.split_off(from);
        self.out.extend(
            emitted
                .into_iter()
                .filter(|f| !f.text.starts_with(['(', '['])),
        );
        let named = sc.ident_at(i + 1).unwrap_or_default();
        anchor(&mut self.out[from..], format!("{named} = "));
    }

    fn is_listed_call(&self, site: &Site<'_>, call: &str) -> bool {
        let on = owner(self.sc, site.i, site.prev_dot);
        site.calls && !site.after_fn && self.sc.fns.calls.iter().any(|c| c.matches(call, on))
    }

    fn listed_call(&mut self, site: &Site<'_>, call: &str) {
        let sc = self.sc;
        let args = sc.args(site.i + 1);
        let on = owner(sc, site.i, site.prev_dot);
        for c in sc.fns.calls.iter().filter(|c| c.matches(call, on)) {
            if let Some(&(a, b)) = args.get(c.arg) {
                sc.emit(a, b, Kind::Call, false, false, self.out);
            }
        }
    }

    fn helper_return(&mut self, site: &Site<'_>) {
        let end = self.sc.expr_end(site.i + 1, false);
        self.sc
            .emit(site.i + 1, end, Kind::Helper, false, false, self.out);
    }
}
