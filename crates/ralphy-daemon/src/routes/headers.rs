//! The security response headers (security audit 2026-09-21, F3; docs/adr/0032
//! amendment F), set on EVERY response by one layer over the router — API,
//! WS upgrades and the embedded UI alike — so no route can forget them and no
//! per-path rule can drift.
//!
//! What the policy buys, stated plainly: `frame-ancestors 'none'` (no
//! clickjacking), `connect-src` (an injected script cannot exfiltrate to a
//! foreign origin), `base-uri`/`form-action`/`object-src` closed, and no
//! foreign or injected `<script>` — a script tag needs `'self'` or a hash of
//! its exact bytes. What it does NOT buy: Alpine's standard build compiles
//! `x-data`/`@click` expressions through `AsyncFunction`, so `'unsafe-eval'`
//! stays and an injected Alpine attribute is still code — DOMPurify on every
//! rendered markdown is the control there, not this header.

use std::sync::OnceLock;

use axum::http::{header, HeaderValue, Response};
use sha2::{Digest, Sha256};

use crate::UI;

/// The three shells the daemon serves as documents. Every inline `<script>`
/// they carry is hashed into the policy; a popup's script that moved out of
/// this list would be blocked, and the pin test in `lib.rs` says so.
const SHELLS: [&str; 3] = ["index.html", "detached.html", "detached-fence.html"];

/// Append the security headers to `resp`. `map_response` middleware: runs
/// after every handler, including the fallback that serves the UI.
pub(crate) async fn security_headers<B>(mut resp: Response<B>) -> Response<B> {
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        content_security_policy().clone(),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    resp
}

/// The one policy, built at first use from the embedded shells.
///
/// Each `unsafe-*` and scheme is there for a named consumer: `'unsafe-eval'`
/// for Alpine 3.14's expression compiler and Monaco's AMD loader;
/// `style-src 'unsafe-inline'` for Alpine `:style`, Monaco, xterm and
/// mermaid, which all set inline styles; `worker-src blob:` for Monaco's
/// language workers; `img-src data: blob:` for the QR code, mermaid and
/// Monaco's icons; `font-src data:` for Monaco's codicon font (an inline
/// `data:font/ttf` in its bundle); `connect-src ws: wss:` for the daemon's own sockets (a
/// same-origin `ws://` is not covered by `'self'` in every browser). No
/// `upgrade-insecure-requests` and no HSTS: the daemon never terminates TLS
/// (ADR-0032 §4), a front does.
pub(crate) fn content_security_policy() -> &'static HeaderValue {
    static CSP: OnceLock<HeaderValue> = OnceLock::new();
    CSP.get_or_init(|| {
        let hashes = SHELLS
            .iter()
            .filter_map(|name| UI.get_file(name))
            .filter_map(|file| file.contents_utf8())
            .flat_map(inline_script_bodies)
            .map(|body| format!(" 'sha256-{}'", script_hash(body)))
            .collect::<String>();
        let policy = format!(
            "default-src 'self'; \
             script-src 'self' 'unsafe-eval'{hashes}; \
             style-src 'self' 'unsafe-inline'; \
             img-src 'self' data: blob:; \
             font-src 'self' data:; \
             connect-src 'self' ws: wss:; \
             worker-src 'self' blob:; \
             object-src 'none'; \
             base-uri 'none'; \
             form-action 'self'; \
             frame-ancestors 'none'"
        );
        HeaderValue::from_str(&policy).expect("the policy is ASCII by construction")
    })
}

/// The base64 sha256 the CSP `script-src` hash form wants, over the bytes
/// between `>` and `</script>` AS THE PARSER SEES THEM: a browser hashes the
/// script text after the HTML tokenizer's newline normalization (CR LF and a
/// lone CR become LF), so a CRLF checkout must hash like an LF one — measured
/// in Chromium 140, where the un-normalized hash of a CRLF popup was refused.
/// Nothing else is touched: a trimmed or re-indented body would not match.
pub(crate) fn script_hash(body: &str) -> String {
    let normalized = body.replace("\r\n", "\n").replace('\r', "\n");
    data_encoding::BASE64.encode(&Sha256::digest(normalized.as_bytes()))
}

/// Every inline `<script>` body in `html` (tags with no `src=`), commented-out
/// tags excluded — a parked `<!-- <script> -->` is not something the browser
/// runs, so it must not earn a hash either.
pub(crate) fn inline_script_bodies(html: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find("<script") {
        rest = &rest[at + "<script".len()..];
        let Some(open_end) = rest.find('>') else {
            break;
        };
        let attrs = &rest[..open_end];
        let after_open = &rest[open_end + 1..];
        let Some(close) = after_open.find("</script>") else {
            break;
        };
        if !attrs.contains("src=") && !inside_comment(html, rest) {
            out.push(&after_open[..close]);
        }
        rest = &after_open[close + "</script>".len()..];
    }
    out
}

/// Whether the position `rest` starts at (a suffix of `html`) sits inside an
/// unclosed `<!-- … -->`.
fn inside_comment(html: &str, rest: &str) -> bool {
    let pos = html.len() - rest.len();
    let before = &html[..pos];
    match before.rfind("<!--") {
        Some(open) => !before[open..].contains("-->"),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_bodies_skip_src_tags_and_commented_out_ones() {
        let html = r#"
            <script src="a.js"></script>
            <script>one</script>
            <!-- <script>parked</script> -->
            <script type="module">
              two
            </script>
            <!-- a note --><script>three</script>
        "#;
        assert_eq!(
            inline_script_bodies(html),
            vec!["one", "\n              two\n            ", "three"]
        );
    }

    /// The hash form is what a browser computes: sha256 of the exact body,
    /// base64 — pinned against a value computed outside this crate
    /// (`python -c "import hashlib,base64;print(base64.b64encode(hashlib.sha256(b'alert(1)').digest()).decode())"`).
    #[test]
    fn script_hash_matches_the_csp_hash_form() {
        assert_eq!(
            script_hash("alert(1)"),
            "bhHHL3z2vDgxUt0W3dWQOrprscmda2Y5pLsLg4GF+pI="
        );
        // CRLF hashes like LF: the parser normalized it before the browser
        // hashed it.
        assert_eq!(script_hash("a\r\nb\rc"), script_hash("a\nb\nc"));
    }

    #[test]
    fn the_policy_hashes_every_inline_script_of_every_shell() {
        let csp = content_security_policy().to_str().unwrap();
        for name in SHELLS {
            let html = UI.get_file(name).unwrap().contents_utf8().unwrap();
            let bodies = inline_script_bodies(html);
            assert!(!bodies.is_empty(), "{name} carries inline scripts today");
            for body in bodies {
                let want = format!("'sha256-{}'", script_hash(body));
                assert!(csp.contains(&want), "{name}: {want} missing from {csp}");
            }
        }
        assert!(
            !csp.split(';')
                .any(|d| d.trim().starts_with("script-src") && d.contains("'unsafe-inline'")),
            "script-src never carries 'unsafe-inline': {csp}"
        );
    }
}
