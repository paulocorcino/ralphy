//! The rules: which added line or changed path gives the code a new power.
//!
//! The rules read the added lines of a diff, one line at a time. They do not
//! parse the code, so they can miss a capability that is split across lines or
//! built from parts. They exist to point a reviewer at the lines to read, not
//! to prove that a change is safe.

use std::sync::LazyLock;

use regex::Regex;

use super::{Capability, FileDiff, Finding};

/// Extensions of source files. A long line in one of these is minified or
/// generated code, which a reviewer cannot read.
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "js", "mjs", "cjs", "ts", "html", "css", "toml", "yml", "yaml", "sh", "ps1", "py",
];

/// A source line longer than this, with fewer spaces than `MINIFIED_SPACES`
/// percent, is treated as minified.
const LONG_LINE: usize = 1000;

/// Measured on 2026-09-24 over the vendored bundles in
/// `crates/ralphy-daemon/assets/ui/vendor/`: minified lines hold 0.5–9.7%
/// spaces. A long line of prose in a Rust string holds about 11%.
const MINIFIED_SPACES: usize = 10;

/// Paths whose text instructs an agent that has shell access on the user's
/// machine. A changed sentence changes what the agent does.
const AGENT_TEXT: &[&str] = &[
    "assets/prompts/",
    "assets/plugin/",
    "assets/agents_template/",
];

/// Files that configure the security checks, or implement this one. A change
/// here can switch a check off.
const SECURITY_POLICY: &[&str] = &[
    "deny.toml",
    ".gitleaks.toml",
    ".gitleaksignore",
    "crates/xtask/src/capabilities.rs",
    "crates/xtask/src/capabilities/",
];

static RUST_NETWORK: LazyLock<Regex> = LazyLock::new(|| {
    re(r"\b(reqwest|ureq|hyper|TcpStream|TcpListener|UdpSocket|tokio::net|std::net)\b")
});
static RUST_PROCESS: LazyLock<Regex> = LazyLock::new(|| {
    re(r"\b(Command::new|process::Command|CommandBuilder::new|portable_pty|libc::exec\w*)\b")
});
static RUST_UNSAFE: LazyLock<Regex> = LazyLock::new(|| re(r"\bunsafe\s*(\{|fn\b|impl\b|extern\b)"));
static RUST_SECRET_ENV: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r#"(?i)\b(env::var(_os)?|option_env!|env!)\s*\(\s*"[^"]*(token|secret|passw|api_?key|credential|private)"#,
    )
});
static CREDENTIAL_PATH: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(\.ssh[/\\]|\bid_rsa\b|\bid_ed25519\b|\.aws[/\\]credentials|\.netrc\b|\.git-credentials\b|\.npmrc\b|\.pypirc\b|\.docker[/\\]config\.json|\bdocument\.cookie\b)",
    )
});
static JS_DYNAMIC: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(\beval\s*\(|\bnew\s+Function\s*\(|\batob\s*\(|String\.fromCharCode|\bimport\s*\(\s*['`]https?:)",
    )
});
static URL_HOST: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\b(?:https?|wss?)://([a-z0-9][a-z0-9.-]*\.[a-z]{2,})"));
static BASE64_BLOB: LazyLock<Regex> = LazyLock::new(|| re(r"[A-Za-z0-9+/]{80,}={0,2}"));
static HEX_BLOB: LazyLock<Regex> = LazyLock::new(|| re(r"\b[0-9a-fA-F]{100,}\b"));

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("the rule patterns are constants and are covered by the tests")
}

/// The findings that come from the path alone, one per file.
pub(crate) fn path_findings(file: &FileDiff) -> Vec<Finding> {
    let path = file.path.as_str();
    let mut out = Vec::new();
    let mut push = |cap: Capability, detail: &str| {
        out.push(Finding::new(cap, path, None, detail));
    };
    if path.starts_with(".github/") {
        push(
            Capability::Workflow,
            "changes CI, a workflow, or code owners",
        );
    }
    if path == "build.rs" || path.ends_with("/build.rs") {
        push(
            Capability::BuildScript,
            "a build script runs on every machine that builds",
        );
    }
    if AGENT_TEXT.iter().any(|p| path.starts_with(p)) {
        push(
            Capability::AgentText,
            "text that instructs an agent with shell access",
        );
    }
    if SECURITY_POLICY
        .iter()
        .any(|p| path == *p || (p.ends_with('/') && path.starts_with(p)))
    {
        push(
            Capability::SecurityPolicy,
            "configures or implements a security check",
        );
    }
    if is_vendored(path) {
        push(
            Capability::Vendored,
            "third-party code copied into the repository",
        );
    }
    if is_npm_lockfile(path) {
        push(
            Capability::Dependency,
            "changes an npm lockfile: review the added packages",
        );
    }
    if file.binary {
        push(
            Capability::Binary,
            "a binary file cannot be reviewed as text",
        );
    }
    out
}

/// The findings in one added line, and the URL hosts it names.
pub(crate) fn line_findings(path: &str, line: u32, text: &str) -> (Vec<Finding>, Vec<String>) {
    let mut out = Vec::new();
    // A lockfile holds hashes and registry URLs on every line. Its changes are
    // reported as dependencies, by crate name or once per npm lockfile.
    if is_vendored(path) || path.ends_with("Cargo.lock") || is_npm_lockfile(path) {
        return (out, Vec::new());
    }
    let ext = extension(path);
    let mut push = |cap: Capability, detail: String| {
        out.push(Finding::new(cap, path, Some(line), &detail));
    };
    if ext == "rs" {
        if let Some(m) = RUST_NETWORK.find(text) {
            push(Capability::Network, format!("uses `{}`", m.as_str()));
        }
        if let Some(m) = RUST_PROCESS.find(text) {
            push(Capability::Process, format!("uses `{}`", m.as_str()));
        }
        if RUST_UNSAFE.is_match(text) {
            push(
                Capability::Unsafe,
                "adds an `unsafe` block or item".to_string(),
            );
        }
        if RUST_SECRET_ENV.is_match(text) {
            push(
                Capability::Credential,
                "reads a secret from the environment".to_string(),
            );
        }
    }
    if matches!(ext, "js" | "mjs" | "cjs" | "ts" | "html") {
        if let Some(m) = JS_DYNAMIC.find(text) {
            push(
                Capability::DynamicCode,
                format!("uses `{}`", m.as_str().trim()),
            );
        }
    }
    if let Some(m) = CREDENTIAL_PATH.find(text) {
        push(Capability::Credential, format!("names `{}`", m.as_str()));
    }
    if BASE64_BLOB.is_match(text) || HEX_BLOB.is_match(text) {
        push(Capability::Encoded, "a long encoded string".to_string());
    }
    let chars = text.chars().count();
    let spaces = text.chars().filter(|c| *c == ' ').count();
    if CODE_EXTENSIONS.contains(&ext) && chars > LONG_LINE && spaces * 100 < chars * MINIFIED_SPACES
    {
        push(
            Capability::Obfuscated,
            format!("a line of {chars} characters with few spaces"),
        );
    }
    let hosts = URL_HOST
        .captures_iter(text)
        .filter_map(|c| c.get(1))
        .map(|m| m.as_str().trim_end_matches('.').to_ascii_lowercase())
        .collect();
    (out, hosts)
}

fn is_vendored(path: &str) -> bool {
    path.starts_with("vendor/") || path.contains("/vendor/")
}

fn is_npm_lockfile(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(name, "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml")
}

fn extension(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext,
        _ => "",
    }
}

/// The crate names in a `Cargo.lock`.
pub(crate) fn lock_crates(lock: &str) -> std::collections::BTreeSet<String> {
    lock.lines()
        .filter_map(|l| l.strip_prefix("name = \""))
        .filter_map(|l| l.strip_suffix('"'))
        .map(str::to_string)
        .collect()
}
