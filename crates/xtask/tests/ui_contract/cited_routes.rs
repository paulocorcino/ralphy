//! Every `/api/…` or `/ws/…` path the repo cites is a route the daemon
//! serves. A route that goes away leaves its citations behind: a browser
//! check that counts sockets of a removed route passes with zero, a comment
//! sends the reader to a route that is not there. The served routes are the
//! `.route("…")` literals of every router in `crates/`, test routers
//! included, because a test may cite the fake route it serves.
//!
//! The scan reads the code, the tests, the browser checks, the mocks, the
//! scripts and the docs that describe the product as it is. It skips the
//! records of the past, which name the routes of their time: the ADRs, the
//! plans, the research, the spikes, the archive, a doc whose name has a date,
//! and the changelog.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use regex::Regex;

use super::{read, workspace_root};

/// Citations of a route that is not served, on purpose: the file, the path
/// as cited, and why.
const NOT_SERVED: &[(&str, &str, &str)] = &[
    (
        "crates/ralphy-daemon/src/tests/root_routes.rs",
        "/ws/command",
        "the test proves the old socket route is gone",
    ),
    (
        "crates/ralphy-daemon/src/tests/root_routes.rs",
        "/ws/tree",
        "the test proves the old socket route is gone",
    ),
    (
        "crates/ralphy-daemon/src/tests/root_routes.rs",
        "/ws/session",
        "the test proves the old socket route is gone",
    ),
    (
        "crates/ralphy-daemon/src/tests/audit_routes.rs",
        "/api/nope",
        "the audit log records a request to an unknown route",
    ),
    (
        "crates/ralphy-daemon/src/tests/served_shell.rs",
        "/api/nope",
        "a refusal carries the security headers too",
    ),
    (
        "tests/browser/security/wb_csp_connect.py",
        "/api/tunnels",
        "the local API of ngrok, not of the daemon",
    ),
    (
        "crates/ralphy-daemon/src/audit/tests.rs",
        "/api/ui/thing",
        "the test router serves this path from a variable",
    ),
];

/// The folders the scan reads, from the workspace root.
const ROOTS: &[&str] = &["crates", "tests", "mocks", "scripts", "assets", "docs"];

/// The files at the workspace root the scan reads.
const ROOT_FILES: &[&str] = &["AGENTS.md", "CONTEXT.md", "README.md", "SECURITY.md"];

const EXTENSIONS: &[&str] = &[
    "rs", "ts", "mjs", "js", "html", "md", "toml", "yml", "yaml", "ps1", "sh", "py",
];

/// Folders the scan never enters, at any depth.
const SKIPPED_DIRS: &[&str] = &["target", "node_modules", "vendor", "fixtures", ".git"];

/// Folders of `docs/` that hold records of the past.
const HISTORY_DIRS: &[&str] = &["adr", "plans", "research", "spike", "archive"];

#[test]
fn every_cited_route_is_served() {
    let root = workspace_root();
    let files = scanned_files(&root);
    let served = served_routes(&files);
    let cited = cited_paths();
    let mut stale = Vec::new();
    let mut allowed = BTreeSet::new();
    for (rel, path) in &files {
        for (line_no, line) in read(path).lines().enumerate() {
            for citation in citations(&cited, line) {
                if is_served(&citation, &served) {
                    continue;
                }
                match NOT_SERVED
                    .iter()
                    .find(|(f, p, _)| *f == rel && *p == citation)
                {
                    Some(entry) => {
                        allowed.insert(*entry);
                    }
                    None => stale.push(format!("{rel}:{}: {citation}", line_no + 1)),
                }
            }
        }
    }
    assert!(
        stale.is_empty(),
        "these paths name no route the daemon serves; point them at the route \
         that does the work now, or delete them:\n{}",
        stale.join("\n")
    );
    let unused: Vec<_> = NOT_SERVED
        .iter()
        .filter(|e| !allowed.contains(*e))
        .collect();
    assert!(
        unused.is_empty(),
        "these NOT_SERVED entries match no citation; remove them: {unused:?}"
    );
}

/// The files of the scan, each with its path from the root (`/` separated).
fn scanned_files(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for dir in ROOTS {
        collect(root, &root.join(dir), &mut out);
    }
    for name in ROOT_FILES {
        out.push(((*name).to_string(), root.join(name)));
    }
    // This file cites routes in its own tests.
    out.retain(|(rel, _)| rel != "crates/xtask/tests/ui_contract/cited_routes.rs");
    out.sort();
    out
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", dir.display()))
            .path();
        let rel = path
            .strip_prefix(root)
            .expect("a walked path is under the root")
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            if !skipped_dir(&rel) {
                collect(root, &path, out);
            }
        } else if scanned_file(&rel) {
            out.push((rel, path));
        }
    }
}

fn skipped_dir(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    SKIPPED_DIRS.contains(&name) || HISTORY_DIRS.iter().any(|dir| rel == format!("docs/{dir}"))
}

fn scanned_file(rel: &str) -> bool {
    let extension = rel.rsplit_once('.').map_or("", |(_, ext)| ext);
    let dated = Regex::new(r"-20\d\d-\d\d-\d\d").expect("a valid regex");
    let name = rel.rsplit('/').next().unwrap_or(rel);
    EXTENSIONS.contains(&extension) && !(rel.starts_with("docs/") && dated.is_match(name))
}

/// The `.route("…")` paths of every Rust file of the scan.
fn served_routes(files: &[(String, PathBuf)]) -> BTreeSet<String> {
    let route = Regex::new(r#"\.route\(\s*"([^"]+)""#).expect("a valid regex");
    files
        .iter()
        .filter(|(rel, _)| rel.ends_with(".rs"))
        .flat_map(|(_, path)| {
            let text = read(path);
            route
                .captures_iter(&text)
                .map(|c| c[1].to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A path that starts `/api/` or `/ws/`, not glued to a word before it (a
/// file path such as `src/ws/x` is not a route). A segment starts with a
/// letter, a digit or a placeholder, so `/ws/.ralphy` is a folder.
fn cited_paths() -> Regex {
    Regex::new(r"(?:^|[^A-Za-z_.\-/])(/(?:api|ws)(?:/[A-Za-z0-9_{<:$*][A-Za-z0-9_\-{}<>:$*.]*)*/?)")
        .expect("a valid regex")
}

/// The citations on `line`, without the punctuation that ends a sentence.
fn citations(cited: &Regex, line: &str) -> Vec<String> {
    cited
        .captures_iter(line)
        .map(|c| c[1].trim_end_matches(['.', ':', ',']).to_string())
        .filter(|c| c != "/api" && c != "/ws" && c != "/api/" && c != "/ws/")
        .collect()
}

/// Whether `citation` names a served route. A placeholder segment (`{id}`,
/// `<id>`, `:id`, `${…}`) stands for any segment. A citation that ends in
/// `*` or `/` names a family of routes, and it is served when one route
/// starts with it.
fn is_served(citation: &str, served: &BTreeSet<String>) -> bool {
    if let Some(prefix) = family_prefix(citation) {
        return served.iter().any(|route| route.starts_with(prefix));
    }
    let cited: Vec<&str> = citation.trim_start_matches('/').split('/').collect();
    served.iter().any(|route| {
        let segments: Vec<&str> = route.trim_start_matches('/').split('/').collect();
        segments.len() == cited.len()
            && segments
                .iter()
                .zip(&cited)
                .all(|(r, c)| r == c || is_placeholder(r) || is_placeholder(c))
    })
}

fn family_prefix(citation: &str) -> Option<&str> {
    citation
        .strip_suffix("/*")
        .or_else(|| citation.strip_suffix('*'))
        .or_else(|| citation.strip_suffix('/'))
}

fn is_placeholder(segment: &str) -> bool {
    segment.starts_with('{')
        || segment.starts_with('<')
        || segment.starts_with(':')
        || segment.starts_with('$')
}

#[test]
fn the_scan_reads_citations_as_routes() {
    let served: BTreeSet<String> = ["/api/repos", "/api/sessions/{id}", "/ws"]
        .into_iter()
        .map(String::from)
        .collect();
    let cited = cited_paths();
    let line = "see `/api/repos`, `/api/sessions/${id}`, /api/gone. and src/ws/x";
    let found = citations(&cited, line);
    assert_eq!(found, ["/api/repos", "/api/sessions/${id}", "/api/gone"]);
    assert!(is_served("/api/repos", &served));
    assert!(is_served("/api/sessions/<id>", &served));
    assert!(
        is_served("/api/${which}", &served),
        "a placeholder in the citation stands for a literal segment"
    );
    assert!(is_served("/api/sess*", &served), "a family of routes");
    assert!(!is_served("/api/gone", &served));
    assert!(!is_served("/ws/tree", &served));
    assert!(citations(&cited, "\"/ws/.ralphy/gemini-home\"").is_empty());
    assert_eq!(
        citations(&cited, "http://127.0.0.1:7431/api/repos?x=1"),
        ["/api/repos"],
        "a full URL"
    );
}
