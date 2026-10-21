use super::*;
use axum::body::Body;
use axum::http::Request;
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tower::ServiceExt;

use families::{console_family, shell_family, CONSOLE_FAMILY, SHELL_FAMILY};

/// A never-fired shutdown receiver for the in-process router tests (none of
/// them exercise `/ws`, so its sender dropping immediately is harmless).
fn idle_shutdown() -> tokio::sync::watch::Receiver<bool> {
    tokio::sync::watch::channel(false).1
}

async fn get(path: &str) -> Response {
    router(
        None,
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
    .await
    .unwrap()
}

/// A router rooted at a SCRATCH registry path, so its `desk.toml` sibling
/// lands in a temp dir. The shared `get()` helper passes a relative
/// `does-not-exist`, whose desk sibling would be written into the process
/// cwd — every test that PUTs must build its router this way.
fn desk_router(dir: &Path) -> Router {
    router(
        None,
        dir.join("repos.toml"),
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
}

async fn body_text(res: Response) -> String {
    String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap()
}

fn fleet_router(registry_path: PathBuf) -> Router {
    router(
        Some(identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        }),
        registry_path,
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
}

/// The RFC 6238 seed, wrapped for the session router tests.
fn rfc_seed() -> totp::Seed {
    totp::Seed::from_bytes(b"12345678901234567890".to_vec())
}

/// Build a router under a `Session` policy over `token` + the RFC seed, with a
/// baptized identity so `/api/identity` answers `200` once authorized.
fn session_router(token: &str) -> Router {
    session_router_at(PathBuf::from("does-not-exist"), token)
}

/// [`session_router`] with its store beside `registry_path`.
fn session_router_at(registry_path: PathBuf, token: &str) -> Router {
    // One shared epoch: the `SessionAuth` signs/verifies cookies under it and
    // the wrapping `AuthState` bumps the SAME counter on logout/invalidate.
    let session_epoch = epoch::SessionEpoch::in_memory_detached();
    let policy = auth::AuthPolicy::Session(std::sync::Arc::new(auth::SessionAuth {
        token: token.to_string(),
        totp: rfc_seed(),
        password: None,
        epoch: session_epoch.clone(),
    }));
    let id = identity::Identity {
        id: ulid::Ulid::nil(),
        name: "anvil".into(),
        avatar: "🐙".into(),
    };
    router(
        Some(id),
        registry_path,
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::fixed(policy, session_epoch),
    )
}

/// Log in on a fresh `session_router("tok")` with the current RFC code and
/// the given extra form fields, returning the `Set-Cookie` header value.
async fn login_set_cookie(extra_form: &str) -> String {
    let now = now_unix();
    let code = rfc_seed().code_at(now / 30);
    let resp = session_router("tok")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("code={code}{extra_form}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "valid TOTP → 200");
    resp.headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("a Set-Cookie header")
        .to_string()
}

/// Read the body of a response as a lossy UTF-8 string.
async fn body_string(resp: Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// GET a path on a Localhost router (the `get` helper above), returning the
/// response for further assertions.
async fn get_local(path: &str) -> Response {
    get(path).await
}
/// Every path embedded in [`UI`], slash-separated and relative to
/// `assets/ui` — `include_dir` exposes only one directory level at a time.
fn embedded_ui_paths() -> Vec<String> {
    fn walk(dir: &include_dir::Dir<'_>, out: &mut Vec<String>) {
        for f in dir.files() {
            out.push(f.path().to_string_lossy().replace('\\', "/"));
        }
        for d in dir.dirs() {
            walk(d, out);
        }
    }
    let mut out = Vec::new();
    walk(&UI, &mut out);
    out
}

/// The whole stylesheet, as the browser assembles it.
///
/// `styles.css` is thirteen partials under `assets/ui/styles/` (ADR-0057),
/// and CSS has no import: the browser sees one cascade because every
/// document links all of them in numeric order. So the pins below read that
/// cascade rather than a file — which is what they always meant, and could
/// not say while there was only one file to name.
///
/// Numeric order is load order by construction: the names are `NN-<what>`
/// and this sorts them, exactly as `every_shell_links_the_whole_cascade`
/// asserts the documents do.
/// `src` with every whitespace character removed: a pin over it matches the
/// code, not its line breaks and indentation.
fn squeeze(src: &str) -> String {
    src.split_whitespace().collect()
}

fn served_css() -> String {
    let mut parts: Vec<&str> = UI
        .get_dir("styles")
        .expect("the stylesheet partials must be embedded")
        .files()
        .map(|f| {
            f.path()
                .file_name()
                .and_then(|n| n.to_str())
                .expect("an embedded partial has a UTF-8 name")
        })
        .collect();
    parts.sort_unstable();
    assert!(
        parts.len() >= 13,
        "expected at least the thirteen partials the stylesheet was cut into, \
         found {} — a partial was deleted rather than emptied",
        parts.len()
    );
    parts
        .iter()
        .map(|name| {
            UI.get_file(format!("styles/{name}"))
                .and_then(|f| f.contents_utf8())
                .unwrap_or_else(|| panic!("styles/{name} must be embedded as UTF-8"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every asset this repo WROTE, as the path the router serves it under.
///
/// The sweeps below assert what our own copy must never say. Two exclusions,
/// both about authorship rather than convenience: `vendor/` is third-party
/// bytes nobody here writes, and a binary has no text to sweep. Everything
/// else is in, and stays in the day it is added — which is the point. The
/// hardcoded lists these replaced went stale within a week of being written
/// (`wb-release.js` landed in #391 and was born unswept), and a list that
/// only grows by someone remembering is not a gate.
fn swept_ui_assets() -> Vec<String> {
    let mut out: Vec<String> = embedded_ui_paths()
        .into_iter()
        .filter(|p| !p.starts_with("vendor/"))
        .filter(|p| p.ends_with(".js") || p.ends_with(".html") || p.ends_with(".css"))
        // A page is fetched at its route; its file name is a 404.
        .map(
            |p| match crate::assets::Shell::ALL.iter().find(|s| s.file() == p) {
                Some(shell) => format!("/{}", shell.route()),
                None => format!("/{p}"),
            },
        )
        .collect();
    out.sort();
    out
}

/// `refs` and every module they reach through static imports. Only a file
/// whose source is `.ts` is read for imports: it is an ES module (ADR-0075
/// D2), and a classic script has none.
fn with_module_imports(refs: Vec<String>) -> Vec<String> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut out = refs;
    let mut at = 0;
    while at < out.len() {
        let path = out[at].clone();
        at += 1;
        let Some(stem) = path.strip_suffix(".js") else {
            continue;
        };
        if !src.join(format!("{stem}.ts")).is_file() {
            continue;
        }
        let text = UI
            .get_file(&path)
            .and_then(|f| f.contents_utf8())
            .unwrap_or_else(|| panic!("{path} is embedded UTF-8"));
        for import in static_imports(text) {
            let Some(name) = import.strip_prefix("./") else {
                panic!("{path} imports {import}: a module imports its siblings by `./`")
            };
            if !out.iter().any(|p| p == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// The paths of the static `import`/`export … from` lines of a module. The
/// workbench modules put each import on one line, at the top.
fn static_imports(js: &str) -> Vec<&str> {
    js.lines()
        .map(str::trim_start)
        .filter(|l| l.starts_with("import ") || l.starts_with("export "))
        .filter_map(|l| {
            let from = l
                .find(" from \"")
                .map(|i| i + " from \"".len())
                .or_else(|| l.strip_prefix("import \"").map(|_| "import \"".len()))?;
            let rest = &l[from..];
            rest.find('"').map(|end| &rest[..end])
        })
        .collect()
}

/// Every `src`/`href` a `<script>` or `<link>` tag carries, in source order.
///
/// Deliberately not a general HTML parse: only these two tags are followed,
/// because only they make the browser FETCH another embedded asset. An `<a
/// href>` points at a route or the outside world and is somebody else's
/// invariant.
fn tag_references(html: &str) -> Vec<String> {
    // Commented-out tags are not references. Both gates that consume this
    // treat the result as what the browser will FETCH — the tag cross-check
    // and the cascade's exact-order equality — so a `<script src>` parked
    // inside `<!-- … -->` would satisfy them over a file the browser never
    // loads, which is the precise failure they exist to catch.
    let mut live = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find("<!--") {
        live.push_str(&rest[..at]);
        rest = match rest[at + 4..].find("-->") {
            Some(end) => &rest[at + 4 + end + 3..],
            None => "",
        };
    }
    live.push_str(rest);
    let html = live.as_str();

    let mut out = Vec::new();
    for (open, attr) in [("<script", "src=\""), ("<link", "href=\"")] {
        let mut rest = html;
        while let Some(at) = rest.find(open) {
            rest = &rest[at + open.len()..];
            let Some(end) = rest.find('>') else { break };
            let tag = &rest[..end];
            if let Some(from) = tag.find(attr) {
                let value = &tag[from + attr.len()..];
                if let Some(to) = value.find('"') {
                    out.push(value[..to].to_string());
                }
            }
        }
    }
    out
}

mod act_bar;
/// The body of one Alpine method in the shell family text, sliced from its
/// opener to the first four-space-indented `},` — the method terminator. Whole-file
/// `contains` is useless for these pins: a call such as `this.$nextTick(`
/// appears at dozens of sites, so a check that does not scope to the method
/// it is about passes no matter which one regressed.
fn js_method_body<'a>(js: &'a str, opener: &str) -> &'a str {
    js.split_once(opener)
        .unwrap_or_else(|| panic!("the shell family must define `{opener}`"))
        .1
        .split_once("\n    },")
        .unwrap_or_else(|| panic!("`{opener}` must close at method indent"))
        .0
}

/// The body of one CSS rule, sliced from its selector to the closing brace.
fn css_rule_body<'a>(css: &'a str, selector: &str) -> &'a str {
    css.split_once(selector)
        .unwrap_or_else(|| panic!("styles.css must define `{selector}`"))
        .1
        .split_once('}')
        .unwrap_or_else(|| panic!("the `{selector}` block must close"))
        .0
}

mod agents_repos;
mod asset_graph;
mod audit_routes;
mod auth_cookie;
mod auth_policy;
mod changes_pins;
mod console_clicks;
mod console_frame;
mod console_records;
mod css_rules;
/// The desk history routes (ADR-0050 amendment 2026-10-04).
mod desk_history;
mod desk_routes;
mod families;
mod fleet_routes;
mod module_graph;
mod page_wiring;
mod peer_command;
mod peer_session_query;
mod release_routes;
mod root_routes;
mod run_board;
mod security_store;
mod served_shell;
mod served_vendor;
mod stage_detach;
mod stage_fence_list;
mod stage_fences;
mod stage_plane;
mod template_sinks;
mod usage_records;
mod usage_spend;
mod vendor_manifest;
mod watch_bell;
