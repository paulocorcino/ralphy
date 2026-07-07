//! Mechanical, guardrailed pre-fetch of a triage issue's **text** attachments
//! (ADR-0025 §1–§3, §5–§7). The security axis — host allowlist, format
//! allowlist, login-HTML masquerade guard, by-category truncation, dedup,
//! deterministic order, and reject-visibility — is pure over the extracted link
//! list and unit-tested with NO network. Only the orchestrator
//! [`fetch_triage_attachments`] touches `gh`; it is best-effort and never aborts
//! triage. Images (§4) are deliberately out of scope for this pass.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use regex::Regex;

use crate::github::client::{gh, gh_output};

/// Only files a reporter attached through GitHub's own UI are fetchable — this is
/// what closes SSRF: the model never chooses a fetch target (ADR-0025 §3.1).
pub const ATTACHMENT_HOST_PREFIX: &str = "https://github.com/user-attachments/";
/// At most this many attachments per issue, counted after dedup (ADR-0025 §3.4).
pub const COUNT_CAP: usize = 10;
/// Free-text over this cap is kept head+tail with an elision marker (ADR-0025 §3.3).
pub const TEXT_CAP: usize = 1 << 20;
/// Structured over this cap is never truncated — half a JSON is noise, not
/// evidence — it is dropped as `too large` (ADR-0025 §3.3).
pub const STRUCTURED_CAP: usize = 1 << 20;

/// Text the model reads as prose; an over-cap file is truncated head+tail.
const FREE_TEXT_EXTS: &[&str] = &["log", "txt", "md", "diff", "patch"];
/// Structured text; never truncated (over-cap → dropped).
const STRUCTURED_EXTS: &[&str] = &["json", "yaml", "yml", "toml", "csv"];

/// Which fetch/truncation policy an attachment's extension selects. `Denied` is
/// deny-by-default: any extension not on either allowlist lands here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatClass {
    FreeText,
    Structured,
    Denied,
}

/// Classify an attachment filename by its extension (case-insensitive). Anything
/// not on the free-text or structured allowlist is `Denied` (ADR-0025 §3.2).
pub fn classify_format(filename: &str) -> FormatClass {
    let ext = filename
        .rsplit('.')
        .next()
        .filter(|e| !e.is_empty() && *e != filename)
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some(e) if FREE_TEXT_EXTS.contains(&e) => FormatClass::FreeText,
        Some(e) if STRUCTURED_EXTS.contains(&e) => FormatClass::Structured,
        _ => FormatClass::Denied,
    }
}

/// Extract `github.com/user-attachments/...` links from the body then each
/// comment in order, dedup by URL preserving first occurrence. Trailing markdown
/// punctuation (`)`, `]`, `.`, …) is trimmed so a `[name](url)` link yields the
/// bare URL. Non-attachment URLs pasted in the prose are never returned — the
/// host allowlist is applied here, at extraction (ADR-0025 §3.1, §5).
pub fn extract_user_attachment_links(body: &str, comments: &[String]) -> Vec<String> {
    let re = Regex::new(r"https?://github\.com/user-attachments/\S+").expect("static regex");
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let sources = std::iter::once(body).chain(comments.iter().map(|s| s.as_str()));
    for text in sources {
        for m in re.find_iter(text) {
            let url = m
                .as_str()
                .trim_end_matches([')', ']', '}', '>', ',', '.', ';', '!', '?', '"', '\'', '`'])
                .to_string();
            if seen.insert(url.clone()) {
                out.push(url);
            }
        }
    }
    out
}

/// True when a response expected to be text/structured is actually an HTML login
/// page: content-type is `text/html`, or the body head opens with
/// `<!doctype html`/`<html`. Guards against an anonymous/expired redirect saving
/// a login page *as if it were* the file on a private repo (ADR-0025 §2).
pub fn looks_like_login_html(content_type: Option<&str>, body: &[u8]) -> bool {
    if let Some(ct) = content_type {
        if ct.to_ascii_lowercase().contains("text/html") {
            return true;
        }
    }
    let head = &body[..body.len().min(512)];
    let head = String::from_utf8_lossy(head);
    let head = head.trim_start().to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html")
}

/// Keep an over-cap free-text file as head `cap/2` + elision marker + tail
/// `cap/2` — a log's error is usually at the tail and its context at the head, so
/// cutting the middle preserves both (ADR-0025 §3.3). Under-cap input is returned
/// as-is.
pub fn truncate_free_text(bytes: &[u8], cap: usize) -> Vec<u8> {
    if bytes.len() <= cap {
        return bytes.to_vec();
    }
    let half = cap / 2;
    let elided = bytes.len() - 2 * half;
    let mut out = Vec::with_capacity(2 * half + 40);
    out.extend_from_slice(&bytes[..half]);
    out.extend_from_slice(format!("\n[... {elided} bytes elided ...]\n").as_bytes());
    out.extend_from_slice(&bytes[bytes.len() - half..]);
    out
}

/// The outcome of one attachment: fetched to a local path, or not fetched with a
/// visible reason. `silence never` — every negative outcome is rendered in the
/// manifest with its reason (ADR-0025 §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentOutcome {
    Fetched {
        path: PathBuf,
    },
    /// Reasons: `denied format`, `too large`, `auth`, `download failed: <code>`,
    /// `attachment cap reached`.
    NotFetched {
        reason: String,
    },
}

/// Render the inline `## Attachments (issue #N)` manifest block: one line per
/// attachment, `name → path (fetched)` or `name → not fetched (<reason>)`.
/// Returns `""` when there are no attachments so the prompt stays clean.
pub fn render_manifest(issue: u64, entries: &[(String, AttachmentOutcome)]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut s = format!("## Attachments (issue #{issue})\n");
    for (name, outcome) in entries {
        match outcome {
            AttachmentOutcome::Fetched { path } => {
                s.push_str(&format!("{name} → {} (fetched)\n", path.display()));
            }
            AttachmentOutcome::NotFetched { reason } => {
                s.push_str(&format!("{name} → not fetched ({reason})\n"));
            }
        }
    }
    s
}

/// The fetched attachment directory plus the rendered manifest. The `TempDir` is
/// owned here so its `Drop` deletes the directory when this value drops — the
/// caller keeps it alive until after the triage session returns (ADR-0025 §7).
pub struct TriageAttachments {
    #[allow(dead_code)] // owned solely so Drop deletes the dir at end of triage.
    dir: tempfile::TempDir,
    pub manifest: String,
}

/// The filename an attachment URL's last path segment names (query/fragment
/// stripped), or `attachment` as a fallback.
fn filename_from_url(url: &str) -> String {
    let last = url.rsplit('/').next().unwrap_or("attachment");
    let name = last.split(['?', '#']).next().unwrap_or(last);
    if name.is_empty() {
        "attachment".to_string()
    } else {
        name.to_string()
    }
}

/// The HTTP status code an error mentions (`... (HTTP 404)`), or `error`.
fn http_code(err: &str) -> String {
    Regex::new(r"HTTP (\d{3})")
        .expect("static regex")
        .captures(err)
        .map(|c| c[1].to_string())
        .unwrap_or_else(|| "error".to_string())
}

/// Body + comments of one issue, as `gh issue view <n> --json body,comments`
/// renders them.
#[derive(serde::Deserialize)]
struct IssueBodyComments {
    #[serde(default)]
    body: String,
    #[serde(default)]
    comments: Vec<CommentBody>,
}

#[derive(serde::Deserialize)]
struct CommentBody {
    #[serde(default)]
    body: String,
}

/// Apply the by-category size policy to already-downloaded, non-login bytes
/// (ADR-0025 §3.3): free-text is truncated head+tail; structured over cap is
/// rejected as `too large` (never half a JSON); `Denied` never reaches here.
/// Pure so the deny/truncate decision is unit-tested without network.
fn classify_payload(class: FormatClass, bytes: Vec<u8>) -> Result<Vec<u8>, &'static str> {
    match class {
        FormatClass::FreeText => Ok(truncate_free_text(&bytes, TEXT_CAP)),
        FormatClass::Structured if bytes.len() > STRUCTURED_CAP => Err("too large"),
        FormatClass::Structured => Ok(bytes),
        FormatClass::Denied => Err("denied format"),
    }
}

/// Split deduped links into `(url, within_cap)` preserving order: the first
/// [`COUNT_CAP`] are within cap, the rest are dropped as `attachment cap reached`
/// (ADR-0025 §3.4, §5 — the cap drops the *last* in deterministic order). Pure so
/// the cap boundary is unit-tested without network.
fn count_capped(links: &[String]) -> Vec<(String, bool)> {
    links
        .iter()
        .enumerate()
        .map(|(i, u)| (u.clone(), i < COUNT_CAP))
        .collect()
}

/// Download one already-host-vetted attachment through the authenticated `gh`
/// credential (`gh api <url>`), apply the login-HTML guard and by-category
/// truncation, and write it under `dir`. Returns the visible outcome; a `gh`
/// failure maps to `download failed: <code>` and never propagates (best-effort).
fn fetch_one(repo: &Path, dir: &Path, url: &str) -> AttachmentOutcome {
    let name = filename_from_url(url);
    let class = classify_format(&name);
    if class == FormatClass::Denied {
        return AttachmentOutcome::NotFetched {
            reason: "denied format".to_string(),
        };
    }
    // `gh api <url>` carries `gh`'s own credential — never anonymous (ADR-0025 §2).
    // The exact argv is indicative; the network path is unverified this pass.
    let out = gh_output(&format!("gh api {url}"), || {
        let mut c = gh(repo);
        c.args(["api", url]);
        c
    });
    let bytes = match out {
        Ok(o) => o.stdout,
        Err(e) => {
            return AttachmentOutcome::NotFetched {
                reason: format!("download failed: {}", http_code(&e.to_string())),
            };
        }
    };
    if looks_like_login_html(None, &bytes) {
        return AttachmentOutcome::NotFetched {
            reason: "auth".to_string(),
        };
    }
    let payload = match classify_payload(class, bytes) {
        Ok(p) => p,
        Err(reason) => {
            return AttachmentOutcome::NotFetched {
                reason: reason.to_string(),
            };
        }
    };
    let path = dir.join(&name);
    match std::fs::write(&path, &payload) {
        Ok(()) => AttachmentOutcome::Fetched { path },
        Err(e) => AttachmentOutcome::NotFetched {
            reason: format!("download failed: {e}"),
        },
    }
}

/// Fetch every issue's text attachments into a per-run OS temp dir and build the
/// combined inline manifest (ADR-0025). Best-effort and never blocking: only a
/// `TempDir` creation failure returns `Err`; a per-issue `gh` failure or a
/// download error becomes a visible `not fetched` manifest line, never an abort.
pub fn fetch_triage_attachments(repo: &Path, issue_numbers: &[u64]) -> Result<TriageAttachments> {
    let dir = tempfile::TempDir::with_prefix("ralphy-triage-")
        .context("creating triage attachment temp dir")?;
    let mut manifest = String::new();
    for &n in issue_numbers {
        // Best-effort: a `gh issue view` failure leaves this issue without an
        // attachment block rather than aborting the whole triage run.
        let Ok(out) = gh_output(&format!("gh issue view {n} --json body,comments"), || {
            let mut c = gh(repo);
            c.args(["issue", "view", &n.to_string(), "--json", "body,comments"]);
            c
        }) else {
            continue;
        };
        let parsed: IssueBodyComments =
            match serde_json::from_slice(&out.stdout).context("parsing issue body,comments") {
                Ok(p) => p,
                Err(_) => continue,
            };
        let comments: Vec<String> = parsed.comments.into_iter().map(|c| c.body).collect();
        let links = extract_user_attachment_links(&parsed.body, &comments);
        if links.is_empty() {
            continue;
        }
        let issue_dir = dir.path().join(n.to_string());
        if std::fs::create_dir_all(&issue_dir).is_err() {
            continue;
        }
        let mut entries: Vec<(String, AttachmentOutcome)> = Vec::new();
        for (url, within_cap) in count_capped(&links) {
            let name = filename_from_url(&url);
            let outcome = if within_cap {
                fetch_one(repo, &issue_dir, &url)
            } else {
                AttachmentOutcome::NotFetched {
                    reason: "attachment cap reached".to_string(),
                }
            };
            entries.push((name, outcome));
        }
        let block = render_manifest(n, &entries);
        if !block.is_empty() {
            manifest.push('\n');
            manifest.push_str(&block);
        }
    }
    Ok(TriageAttachments { dir, manifest })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_keeps_only_user_attachments_deduped_body_first() {
        let body = "See [diagnose.log](https://github.com/user-attachments/files/1/diagnose.log) \
                    and https://evil.example/x.log for details.";
        let comments =
            vec!["re-pasting https://github.com/user-attachments/files/1/diagnose.log".to_string()];
        let links = extract_user_attachment_links(body, &comments);
        assert_eq!(
            links,
            vec!["https://github.com/user-attachments/files/1/diagnose.log".to_string()],
            "evil excluded, deduped, body-first"
        );
    }

    #[test]
    fn classify_format_allowlists() {
        for f in ["a.log", "a.txt", "a.md", "a.diff", "a.patch"] {
            assert_eq!(classify_format(f), FormatClass::FreeText, "{f}");
        }
        for f in ["a.json", "a.yaml", "a.yml", "a.toml", "a.csv"] {
            assert_eq!(classify_format(f), FormatClass::Structured, "{f}");
        }
        for f in ["a.exe", "a.zip", "a.png", "foo"] {
            assert_eq!(classify_format(f), FormatClass::Denied, "{f}");
        }
    }

    #[test]
    fn login_html_guard() {
        assert!(looks_like_login_html(Some("text/html"), b"anything"));
        assert!(looks_like_login_html(None, b"<!DOCTYPE html>\n<html>"));
        assert!(looks_like_login_html(None, b"  <html lang=\"en\">"));
        assert!(!looks_like_login_html(None, b"{\"ok\":true}"));
    }

    #[test]
    fn truncate_free_text_keeps_head_and_tail_with_marker() {
        let cap = 100;
        let mut input = vec![b'H'; cap / 2];
        input.extend(vec![b'M'; 100]); // middle to elide
        input.extend(vec![b'T'; cap / 2]);
        let out = truncate_free_text(&input, cap);
        let rendered = String::from_utf8_lossy(&out);
        assert!(out.len() < input.len());
        assert!(rendered.contains("[... "), "{rendered}");
        assert!(rendered.contains("bytes elided ...]"), "{rendered}");
        assert_eq!(&out[..8], &[b'H'; 8], "head survives");
        assert_eq!(&out[out.len() - 8..], &[b'T'; 8], "tail survives");
    }

    #[test]
    fn truncate_free_text_passthrough_under_cap() {
        assert_eq!(truncate_free_text(b"short", 100), b"short");
    }

    #[test]
    fn render_manifest_emits_heading_and_each_outcome() {
        let entries = vec![
            (
                "diagnose.log".to_string(),
                AttachmentOutcome::Fetched {
                    path: PathBuf::from("/tmp/t/133/diagnose.log"),
                },
            ),
            (
                "report.exe".to_string(),
                AttachmentOutcome::NotFetched {
                    reason: "denied format".to_string(),
                },
            ),
            (
                "big.json".to_string(),
                AttachmentOutcome::NotFetched {
                    reason: "too large".to_string(),
                },
            ),
        ];
        let m = render_manifest(133, &entries);
        assert!(m.contains("## Attachments (issue #133)"));
        assert!(
            m.contains("report.exe → not fetched (denied format)"),
            "{m}"
        );
        assert!(m.contains("big.json → not fetched (too large)"), "{m}");
        assert!(m.contains("diagnose.log → "));
        assert!(m.contains("(fetched)"));
    }

    #[test]
    fn render_manifest_empty_is_blank() {
        assert_eq!(render_manifest(1, &[]), "");
    }

    #[test]
    fn classify_payload_structured_over_cap_is_too_large() {
        let big = vec![b'{'; STRUCTURED_CAP + 1];
        assert_eq!(
            classify_payload(FormatClass::Structured, big),
            Err("too large")
        );
        // Under-cap structured passes through untouched.
        assert_eq!(
            classify_payload(FormatClass::Structured, b"{\"ok\":true}".to_vec()),
            Ok(b"{\"ok\":true}".to_vec())
        );
    }

    #[test]
    fn classify_payload_free_text_over_cap_truncates() {
        let big = vec![b'x'; TEXT_CAP + 100];
        let out = classify_payload(FormatClass::FreeText, big.clone()).unwrap();
        assert!(out.len() < big.len(), "free-text over cap is truncated");
        assert!(String::from_utf8_lossy(&out).contains("bytes elided ...]"));
    }

    #[test]
    fn count_cap_drops_the_last_after_ten() {
        let links: Vec<String> = (0..12).map(|i| format!("https://x/{i}")).collect();
        let capped = count_capped(&links);
        assert_eq!(capped.len(), 12);
        assert!(capped[..COUNT_CAP].iter().all(|(_, w)| *w), "first 10 kept");
        assert!(
            capped[COUNT_CAP..].iter().all(|(_, w)| !*w),
            "last two over cap"
        );
    }

    #[test]
    fn filename_from_url_last_segment() {
        assert_eq!(
            filename_from_url("https://github.com/user-attachments/files/29722076/diagnose.log"),
            "diagnose.log"
        );
        assert_eq!(filename_from_url("https://x/y/a.json?sig=abc"), "a.json");
    }
}
