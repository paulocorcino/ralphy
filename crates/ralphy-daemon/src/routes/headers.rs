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

use std::sync::{Arc, OnceLock};

use axum::extract::State;
use axum::http::{header, HeaderValue, Response};
use sha2::{Digest, Sha256};

use crate::assets::Shell;
use crate::auth::AuthState;
use crate::UI;

/// Append the security headers to `resp`. `map_response_with_state`
/// middleware: runs after every handler, including the fallback that serves the
/// UI, and reads the remote-images flag from the live auth state.
pub(crate) async fn security_headers<B>(
    State(auth): State<Arc<AuthState>>,
    mut resp: Response<B>,
) -> Response<B> {
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        content_security_policy(auth.remote_images()).clone(),
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
    h.insert(
        header::HeaderName::from_static("accept-ch"),
        HeaderValue::from_static(ACCEPT_CH),
    );
    resp
}

/// The client hints the audit log records (ADR-0074). Chromium sends them on
/// the requests after a response that asks; Safari and Firefox send none.
pub(crate) const ACCEPT_CH: &str = "Sec-CH-UA-Platform-Version, Sec-CH-UA-Model, \
     Sec-CH-UA-Arch, Sec-CH-UA-Bitness, Sec-CH-UA-Full-Version-List, Sec-CH-UA-Form-Factors";

/// The policy, built at first use from the embedded shells, in two variants
/// that differ only in `img-src`.
///
/// Each `unsafe-*` and scheme is there for a named consumer: `'unsafe-eval'`
/// for Alpine 3.14's expression compiler and Monaco's AMD loader;
/// `style-src 'unsafe-inline'` for Alpine `:style`, Monaco, xterm and
/// mermaid, which all set inline styles; `worker-src blob:` for Monaco's
/// language workers; `img-src data: blob:` for the QR code, mermaid and
/// Monaco's icons; `font-src data:` for Monaco's codicon font (an inline
/// `data:font/ttf` in its bundle). `connect-src` is `'self'` alone: every
/// workbench socket is built from `location.host`, and `'self'` covers a
/// same-origin `ws:`/`wss:` (measured 2026-10-01 in Playwright Chromium,
/// Firefox and WebKit, on loopback, through ngrok and through dev tunnels:
/// `tests/browser/security/wb_csp_connect.py`; ADR-0072 D10). No
/// `upgrade-insecure-requests` and no HSTS: the daemon never terminates TLS
/// (ADR-0032 §4), a front does.
///
/// `remote_images` adds `img-src https:` for web images in rendered markdown
/// (ADR-0032 amendment §F, ADR-0049 §5). It is opt-in because it reopens a
/// GET channel to any origin: an injected Alpine attribute could put data in
/// an image URL. DOMPurify on every rendered markdown stays the control there.
pub(crate) fn content_security_policy(remote_images: bool) -> &'static HeaderValue {
    static STRICT: OnceLock<HeaderValue> = OnceLock::new();
    static REMOTE_IMAGES: OnceLock<HeaderValue> = OnceLock::new();
    if remote_images {
        REMOTE_IMAGES.get_or_init(|| build_policy(" https:"))
    } else {
        STRICT.get_or_init(|| build_policy(""))
    }
}

/// Every inline `<script>` of every [`Shell`] is hashed into the policy; a
/// page missing from `Shell` would have its scripts blocked.
fn build_policy(extra_img_src: &str) -> HeaderValue {
    let hashes = Shell::ALL
        .iter()
        .filter_map(|shell| UI.get_file(shell.file()))
        .filter_map(|file| file.contents_utf8())
        .flat_map(inline_script_bodies)
        .map(|body| format!(" 'sha256-{}'", script_hash(body)))
        .collect::<String>();
    let policy = format!(
        "default-src 'self';          script-src 'self' 'unsafe-eval'{hashes};          style-src 'self' 'unsafe-inline';          img-src 'self' data: blob:{extra_img_src};          font-src 'self' data:;          connect-src 'self';          worker-src 'self' blob:;          object-src 'none';          base-uri 'none';          form-action 'self';          frame-ancestors 'none'"
    );
    HeaderValue::from_str(&policy).expect("the policy is ASCII by construction")
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
        let csp = content_security_policy(false).to_str().unwrap();
        for name in Shell::ALL.map(Shell::file) {
            let html = UI.get_file(name).unwrap().contents_utf8().unwrap();
            let bodies = inline_script_bodies(html);
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

    #[test]
    fn the_remote_images_variant_differs_only_in_img_src() {
        let strict = content_security_policy(false).to_str().unwrap();
        let remote = content_security_policy(true).to_str().unwrap();
        assert!(strict.contains("img-src 'self' data: blob:; "), "{strict}");
        assert!(!strict.contains("https:"), "{strict}");
        assert!(
            remote.contains("img-src 'self' data: blob: https:; "),
            "{remote}"
        );
        assert!(!remote.contains("http:"), "https only: {remote}");
        assert_eq!(
            remote.replace(
                "img-src 'self' data: blob: https:;",
                "img-src 'self' data: blob:;"
            ),
            strict
        );
    }
}
