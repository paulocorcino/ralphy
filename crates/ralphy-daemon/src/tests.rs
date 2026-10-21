use super::*;
use axum::body::Body;
use axum::http::Request;
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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

/// #308 pins the editor swap where it can actually regress: the embedded
/// asset tree. Monaco is vendored, CodeMirror is gone, and the four heavy
/// language workers stay excluded (the exclusion rule is prefix-based
/// because Monaco 0.56 ships content-hashed chunk names).
#[test]
fn monaco_replaced_codemirror_in_the_embedded_ui() {
    let paths = embedded_ui_paths();

    for required in [
        "vendor/monaco/vs/loader.js",
        "vendor/monaco/vs/editor/editor.main.js",
    ] {
        assert!(
            paths.iter().any(|p| p == required),
            "{required} must be embedded in the UI assets"
        );
    }

    let leftover: Vec<_> = paths
        .iter()
        .filter(|p| p.starts_with("vendor/codemirror/"))
        .collect();
    assert!(
        leftover.is_empty(),
        "CodeMirror must be deleted from the tree, found: {leftover:?}"
    );

    for worker in [
        "vs/assets/ts.worker",
        "vs/assets/css.worker",
        "vs/assets/html.worker",
        "vs/assets/json.worker",
    ] {
        let hits: Vec<_> = paths.iter().filter(|p| p.contains(worker)).collect();
        assert!(
            hits.is_empty(),
            "language worker {worker} must not be vendored, found: {hits:?}"
        );
    }

    // The other three exclusion rows of docs/WORKBENCH-BUILD-GUIDE.md, so a
    // future Monaco bump that re-copies the tarball wholesale fails here
    // rather than silently doubling the payload.
    for excluded in ["vendor/monaco/vs/language/", "vendor/monaco/vs/nls/"] {
        let hits: Vec<_> = paths.iter().filter(|p| p.starts_with(excluded)).collect();
        assert!(
            hits.is_empty(),
            "{excluded} must not be vendored, found: {hits:?}"
        );
    }
    let typed: Vec<_> = paths
        .iter()
        .filter(|p| {
            p.starts_with("vendor/monaco/") && (p.ends_with(".d.ts") || p.ends_with(".map"))
        })
        .collect();
    assert!(
        typed.is_empty(),
        "no .d.ts / .map may be vendored with Monaco, found: {typed:?}"
    );

    assert!(
        include_str!("../assets/ui/wb-monaco.ts").contains("monaco.editor.create"),
        "wb-monaco.ts must build the editor through Monaco's own factory"
    );

    let viewer = include_str!("../assets/ui/wb-file-viewer.ts");
    assert!(
        viewer.contains("WBMonaco.create"),
        "wb-file-viewer.ts must mount its editor through WBMonaco"
    );
    // The mirror pane (ADR-0037 §3c) is a SECOND editor over the pane's
    // model, never a second model: `wb_monaco_308.py` counts models per
    // open pane and a mirror must not move that count.
    assert!(
        viewer.contains("WBMonaco.createOver"),
        "wb-file-viewer.ts must mount the mirror through WBMonaco.createOver"
    );
    // Built from parts so this pin cannot trip on its own source text.
    let outgoing = concat!("Code", "Mirror(");
    assert!(
        !viewer.contains(outgoing),
        "wb-file-viewer.ts must not construct a {outgoing} editor"
    );
}

/// #309's deliverable is JS/HTML no Rust gate compiles — this pin over the
/// served assets is the same reason the #308 Monaco pin above and
/// `usage.rs:544` exist, so `cargo test` reds after a deletion.
#[test]
fn the_changes_section_renders_a_status_marked_list() {
    let html = include_str!("../assets/ui/index.html");
    assert!(
        html.contains(r#"class="changes-list""#),
        "index.html must render the changes-list"
    );
    assert!(
        html.contains(r#"class="chg-mark""#),
        "index.html must render a chg-mark per row"
    );
    assert!(
        html.contains("showSideView('changes')"),
        "index.html must wire the rail's Changes button to showSideView"
    );

    // The status markers, the diff target, the staged/unstaged split and the
    // write-control folds of `wb-changes.js` are driven by
    // `ui-tests/wb-changes.test.mjs`; this test holds the markup they feed.

    // The diff tab (#311): the row's click wiring and the diff editor's
    // factory.
    assert!(
        html.contains("openDiff($store.projects.openSlug, c)"),
        "index.html must open a diff from a changes row"
    );
    assert!(
        include_str!("../assets/ui/wb-file-viewer.ts").contains("WBMonaco.createDiff"),
        "wb-file-viewer.ts must mount the diff through WBMonaco.createDiff"
    );

    // The staged/unstaged split (#315): the row's two halves, the group
    // headline, and the per-group `:key` prefix without which Alpine collides
    // a staged-then-modified path's two rows.
    for pin in [
        r#"class="chg-name""#,
        r#"class="chg-dir""#,
        r#"class="chg-group-head""#,
        ">Staged changes<",
        // BOTH keys: pinning only the staged one stays green if the two
        // templates are keyed identically.
        "'s:' + c.path",
        "'u:' + c.path",
    ] {
        assert!(
            html.contains(pin),
            "index.html must keep the changes-group pin {pin}"
        );
    }

    // The promotion to a rail view (#317): the view's root, the rail icon
    // that reaches it, and the per-project indicator that keeps the count
    // reachable with no navigation.
    for pin in [
        r#"class="changes-view""#,
        r#"x-icon="'git-compare'""#,
        r#"class="chg-badge""#,
    ] {
        assert!(
            html.contains(pin),
            "index.html must keep the rail-view pin {pin}"
        );
    }
    // NEGATED: the accordion is removed, not left standing beside the view.
    // Both strings occurred ONLY in the block #317 deleted, so either one
    // reappearing means the section came back.
    for gone in ["changes-sec", "toggleChanges"] {
        assert!(
            !html.contains(gone),
            "index.html must not resurrect the Changes accordion ({gone})"
        );
    }

    // The write controls (#318). The `node --test` suite CI runs never
    // renders markup and Playwright does not run there, so these substring
    // pins are the only CI-visible check of this markup — every control the
    // panel's write gesture needs is named.
    for pin in [
        r#"data-act="stage""#,
        r#"data-act="unstage""#,
        r#"data-act="stage-all""#,
        r#"data-act="unstage-all""#,
        r#"class="chg-commit""#,
        r#"x-model="commitMsg""#,
        "writeLocked()",
        "commitTarget().label",
    ] {
        assert!(
            html.contains(pin),
            "index.html must keep the write-control pin {pin}"
        );
    }
    // NEGATED: the message box is no longer inert. That title string existed
    // ONLY on the disabled `.chg-msg`, so its reappearance means the box
    // regressed to a placeholder.
    assert!(
        !html.contains("inert until the write controls land"),
        "the commit message box must no longer declare itself inert (#318)"
    );
}

/// The discard control (#319) — the same CI-visible substring gate the write
/// controls get, plus the NEGATED pin that keeps a group-level "discard all"
/// out: the issue asks for one file at a time, and a one-tap discard of every
/// path is precisely the mis-tap PRD #297 refused to ship.
#[test]
fn the_discard_control_is_pinned_in_the_markup() {
    let html = include_str!("../assets/ui/index.html");

    for pin in [
        r#"data-act="discard""#,
        r#"class="chg-group-note""#,
        "groupNote('unstaged')",
        "groupNote('staged')",
        "discardRow($store.projects.openSlug, c)",
    ] {
        assert!(
            html.contains(pin),
            "index.html must keep the discard pin {pin}"
        );
    }
    assert!(
        !html.contains(r#"data-act="discard-all""#),
        "there is no group-level discard: one file at a time (#319)"
    );
    // A conflicted row is all worktree work, so it lands in the UNSTAGED
    // group — but `restore --worktree` refuses an unmerged path, so the
    // control must not be offered there either.
    assert!(
        html.contains(r#"x-show="c.status !== 'conflicted'""#),
        "the discard control must be withheld from a conflicted row (#319)"
    );
    // The "unstaged rows ONLY" invariant, as a CI-VISIBLE oracle: this is
    // markup, which the `node --test` suite does not render, and Playwright
    // does not run in CI — so scenario 2 of `wb_changes_319.py` cannot be
    // the only thing proving it. The staged
    // list is the block between its `x-for` key and that list's close.
    let staged_from = html
        .find(r#"'s:' + c.path"#)
        .expect("index.html must keep the staged group's x-for key");
    let staged_block = &html[staged_from..];
    let staged_end = staged_block
        .find("</ul>")
        .expect("the staged group's list must close");
    assert!(
        !staged_block[..staged_end].contains(r#"data-act="discard""#),
        "the staged group must carry NO discard control — unstage comes first (#319)"
    );
}

/// A liveness flag has to be derived on a CLOCK, never inside the binding.
///
/// The uptime's `stale` class was bound to an inline
/// `Date.now() - _lastHeartbeat > 6000`. That expression can never fire: the
/// only reactive value in it is written by the heartbeat, so ticks STOPPING
/// — the very event it exists to report — is the one thing that does not
/// re-render it. A dead daemon read exactly like a live one, and the class
/// had no CSS either, so nothing on screen ever disagreed with the bug.
///
/// Pinned here because no node test renders this binding, and the failure is
/// silent by construction: the binding is present, the class is spelled correctly, and
/// the only symptom is an alarm that stays quiet.
#[test]
fn presence_staleness_is_derived_on_a_clock_not_inside_the_binding() {
    let html = include_str!("../assets/ui/index.html");
    assert!(
        html.contains(r#":class="{ stale: $store.connection.presenceStale }""#),
        "the uptime must bind the DERIVED flag"
    );
    assert!(
        !html.contains("Date.now() - _lastHeartbeat"),
        "staleness must not be computed inside a binding — it cannot re-fire"
    );
    let app = shell_family();
    assert!(
        app.contains("presenceStale: false,"),
        "the shell must declare the flag as reactive state"
    );
    // Inside the tick, not merely somewhere in the file: a computation that
    // is not on a clock is the bug this test is named after. The shell and
    // the store `connection` each keep a one-second tick: one of them.
    let ticks: Vec<&str> = app
        .split("this._clockTick = setInterval(")
        .skip(1)
        .map(|tick| &tick[..tick.find("}, 1000);").expect("the clock tick must close")])
        .collect();
    assert!(!ticks.is_empty(), "the shell must keep its clock tick");
    assert!(
        ticks
            .iter()
            .any(|tick| tick.contains("this.presenceStale =")),
        "the clock tick must recompute staleness — that is what makes it observable"
    );
    // The class must also LAND: it was bound and styled nowhere, so the flag
    // being right would still have shown the operator nothing.
    let css = served_css();
    assert!(
        css.contains(".uptime.stale {"),
        "styles.css must give the stale uptime a visible state"
    );
}

/// The routing head never reaches an operator-facing string. A peer repo is
/// `<daemon_id>/<owner>/<repo>` on the wire (ADR-0052 §5); the ULID is how
/// the fleet routes, not what the repo is called, and rendering it raw is
/// how a WSL project came to be titled `01KY…/paulocorcino/vibeforge`.
///
/// The fold itself (`refSlug`, `refLabel`) is driven by
/// `ui-tests/wb-fleet.test.mjs`. This test holds what no node test renders:
/// the script tags and the markup that must call it.
#[test]
fn the_workbench_never_titles_a_repo_with_its_routing_head() {
    // The popups load `wb-file-viewer.ts`/`wb-console.ts`, which now call the
    // fold — without the import or tag the label silently falls back to the
    // ref in exactly the two windows nobody tests by hand.
    for page in [
        include_str!("../assets/ui/detached.html"),
        include_str!("../assets/ui/detached-fence.html"),
    ] {
        assert!(
            with_module_imports(tag_references(page)).contains(&"wb-fleet.js".to_string()),
            "a detached popup must load the fold it calls"
        );
    }
    let store = include_str!("../assets/ui/wb-projects-store.ts");
    assert!(
        store.contains("projectLabel(ref: string | null | undefined) {"),
        "wb-projects-store.ts must keep the label helper the markup binds to"
    );
    // The crumb is the surface that STARTED this, and it no longer exists:
    // it went with the top bar, because the sidebar already names the open
    // project on its top row. So the pin moves to the surfaces that still
    // put the project's name on screen. It is deliberately NOT
    // `contains("$store.projects.projectLabel($store.projects.openSlug)")` any more — ten call sites satisfy
    // that substring, so it would stay green with every one of these
    // surfaces reverted to the raw ref.
    let html = include_str!("../assets/ui/index.html");
    for pin in [
        r#"<span class="kanban-scope" x-text="$store.projects.openSlug ? $store.projects.projectLabel("#,
        r#"<span class="spend-project" x-text="$store.projects.projectLabel($store.projects.openSlug)""#,
    ] {
        assert!(
            html.contains(pin),
            "a surface that names the open project must render the LABEL: {pin}"
        );
    }
    // The two raw-ref spellings, banned file-wide: the crumb's own (kept so
    // the regression that named this test can never return by that route)
    // and the bare binding any new surface would reach for first.
    for anti in [
        r#"x-text="$store.projects.openSlug ?? 'no project'""#,
        r#"x-text="$store.projects.openSlug""#,
    ] {
        assert!(
            !html.contains(anti),
            "index.html must never print the routing head raw: {anti}"
        );
    }
    // The console name's prefix is the project name's last segment (ADR-0066
    // §2 and its 2026-10-02 amendment); an unnamed ref falls back to the
    // slug. Taken from the ref, a peer console would be named after its ULID.
    // The prefix is in the title, the fallback in the console.
    assert!(
        include_str!("../assets/ui/wb-console-title.ts").contains("prefixOf(projectNameOf(repo))")
            && console_family().contains("WBFleet.refSlug(ref)"),
        "a console name prefix must come from the project name or the slug, not the ref"
    );
}

/// The fence floor (#340): the DOM half, the CSS and the wiring. The pure
/// folds (`nextFenceSlot`, `fenceSpawnRect`, `nextFenceName`, the cap) are
/// driven by `ui-tests/wb-console.test.mjs` and `wb-geometry.test.mjs`; the
/// node suite renders no DOM or CSS, so this test holds that half.
#[test]
fn shell_draws_fences_below_the_windows() {
    // The console, its fence list, its fences and its title, as one text: the
    // fence element lives in the fences, the fence list and its rendering in
    // the fence list, the console rename in the title.
    let js = &[
        include_str!("../assets/ui/wb-console.ts"),
        include_str!("../assets/ui/wb-stage-window.ts"),
        include_str!("../assets/ui/wb-stage-fence-list.ts"),
        include_str!("../assets/ui/wb-stage-fences.ts"),
        include_str!("../assets/ui/wb-stage-fence-gestures.ts"),
        include_str!("../assets/ui/wb-console-title.ts"),
    ]
    .concat();
    for pin in [
        "function renderFences(",
        "function renameFence(",
        "function removeFence(",
    ] {
        assert!(
            js.contains(pin),
            "wb-console.ts must keep the #340 pin {pin}"
        );
    }
    // The plane is sized to windows AND fences (ADR-0051 §2) AND note cards
    // (ADR-0064 §8). Reverting this ONE selector leaves every other test
    // green while a fence or a card past the last window becomes
    // unreachable — the stage never grows to hold it.
    assert!(
        js.contains(r#"querySelectorAll<HTMLElement>(".session-window, .fence, .note-card")"#),
        "applyExtent must fold the fences and the cards into the stage extent (#340, ADR-0064)"
    );
    let tab = include_str!("../assets/ui/wb-consoles-tab.ts");
    assert!(
        tab.contains("newFence("),
        "wb-consoles-tab.ts must wire the toolbar act"
    );
    let html = include_str!("../assets/ui/index.html");
    assert!(
        html.contains("newFence()"),
        "index.html must carry the Fence button (#340)"
    );

    // Scoped to the `.fence` rule's OWN body: `pointer-events` and a small
    // `z-index` both occur elsewhere in the sheet, so an unscoped substring
    // would pass with the fence tier deleted.
    let css = served_css();
    let rule = |head: &str| -> String {
        let after = css
            .split_once(head)
            .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
            .1;
        after[..after.find('}').expect("the rule must close")].to_string()
    };
    let fence = rule("\n.fence {");
    assert!(
        fence.contains("pointer-events: none"),
        "the fence floor is inert — it may never swallow a window gesture (#340)"
    );
    // The SEMICOLON is load-bearing: a bare `z-index: 1` substring is also
    // satisfied by `10`, `100` and `1000` — a fence raised over `Z_BASE`
    // (60), which is the exact defect this pin names.
    assert!(
        fence.contains("z-index: 1;"),
        "a fence draws BELOW every console window (#340)"
    );
    // NEGATIVE CONTROL: "make the whole thing inert" would delete the rename
    // affordance, and the two pins above would still be green.
    assert!(
        rule("\n.fence-name {").contains("pointer-events: auto"),
        "the name field must stay clickable — a fence is renamed in place (#340)"
    );
    // The name is inert until it is EDITED, and an edit is cancellable. The
    // press is cancelled in `buildFence` (no focus ring, no caret) and the
    // drag half is here, so a sweep across the head cannot paint the name.
    assert!(
        rule("\n.fence-name[readonly] {").contains("user-select: none"),
        "a read-only fence name must not be selectable by a sweep"
    );
    let squeezed: String = js.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        squeezed.contains(r#"name.addEventListener("mousedown", (e: MouseEvent) => { if (name.readOnly) e.preventDefault(); });"#),
        "a single click on a read-only fence name must leave no trace at all"
    );
    // Enter is the ONLY commit. `change` fires on blur, so committing there
    // made "click away" mean SAVE — with nothing able to undo it.
    assert!(
        !js.contains(r#"name.addEventListener("change""#),
        "the fence name must not commit on `change` — leaving the field CANCELS"
    );
    assert!(
        squeezed.contains("if (!commit) name.value = pristine;"),
        "ending an edit without a commit must restore the name"
    );
    // …and must leave the name UNSELECTED. MEASURED: neither `blur()` nor
    // re-assigning the same string clears the range `select()` made, so an
    // abandoned edit left the name highlighted on a field nobody was editing.
    // The commit path only looked right because `renameFence` replaces the
    // input outright.
    assert!(
        squeezed.contains("name.setSelectionRange(0, 0);"),
        "ending an edit must collapse the selection its `select()` made"
    );
    // THE CAP: refused, not absorbed (the refusal is driven by
    // `wb-console.test.mjs`). A 13th fence once cost the operator a DIFFERENT
    // one, dropped by its age; nothing on the page drops a record now
    // (ADR-0050 amendment 2026-10-04), and the daemon refuses a create past
    // the cap.
    assert!(
        squeezed.contains("if (atFenceCap()) return false;"),
        "a fence past the cap must be refused before it is born"
    );
    // …and the name must not be counted. MEASURED against the running shell:
    // `Fence ${fences.length + 1}` froze at the cap, so the 13th fence and every
    // one after were all born "Fence 13" — duplicates in the list that IS the
    // plane's map (there is no minimap, and the Alt+Shift+F<n> rows are picked
    // out by name).
    assert!(
        squeezed.contains("name: nextFenceName(fences()),"),
        "a new fence must be numbered from the names present, not from the count"
    );
    assert!(
        !squeezed.contains("name: `Fence ${fences.length + 1}`")
            && !squeezed.contains("name: `Fence ${fences().length + 1}`"),
        "numbering by the fence COUNT collides at the cap — measured"
    );

    // The console menus (wb-consoles-tab.ts) hold the toolbar acts and keys.
    let tab_js = include_str!("../assets/ui/wb-consoles-tab.ts");
    let app: String = tab_js.split_whitespace().collect::<Vec<_>>().join(" ");
    // The toolbar says why before the click and again if one gets through — the
    // #318 idiom, since `wb-console.ts` reaches no shell and can only refuse.
    assert!(
        app.contains("if (WBConsole.createFence() === false) this.$store.modals.flash(this.fenceCapMessage());"),
        "newFence must let the module refuse, and say so when it does"
    );
    // The dimming reads the REACTIVE snapshot, not the module's array. MEASURED:
    // a binding over `WBConsole.atFenceCap()` never re-evaluated (the array is
    // not Alpine state), so the row stayed enabled on a full plane.
    assert!(
        app.contains("return this.fenceItems.length >= window.WBConsole.FENCE_MAX;"),
        "fenceAtCap must read the snapshot Alpine can observe changing"
    );
    let shell = include_str!("../assets/ui/index.html");
    assert!(
        shell.contains(r#":disabled="fenceAtCap()""#),
        "the New fence row must be disabled at the cap, not merely refused"
    );
    // The key range IS the cap, so no fence the plane can hold is unreachable
    // and no menu row lacks an accelerator. MEASURED (Chromium 148): with
    // Alt+Shift held, F10/F11/F12 all reach the document — their reserved
    // neighbours (Shift+F10, F11, F12) each want their own exact combo.
    assert!(
        app.contains(r"if (!/^F(?:[1-9]|1[0-2])$/.test(e.code)) return;"),
        "the fence accelerators must run F1..F12, matching the cap"
    );
    assert!(
        !shell.contains(r#"x-show="i < 9""#),
        "every fence row now carries an accelerator — the F9 label cap is gone"
    );
    // The row carries the KEY, the head carries the MODIFIER. The pair never
    // varies across the twelve rows, and spelling it in each one crowded the
    // panel until the name wrapped mid-word beside `Alt+Shift+F10`.
    assert!(
        app.contains("fenceShortcutLabel(n: number) { return `F${n}`; }"),
        "a fence row's label must be the bare key"
    );
    assert!(
        shell.contains(r#"x-text="fenceShortcutHint()""#),
        "the fence menu's head must state the modifier pattern once"
    );
    // …and the cancel must not rely on `blur`: the plane's pan handler
    // `preventDefault()`s mousedown, so pressing the stage does not move focus
    // at all (measured — the field kept its caret and the half-typed name).
    assert!(
        squeezed.contains(r#"document.addEventListener("pointerdown", stopOutside, true);"#),
        "a press outside the field must end the edit, since blur cannot be trusted here"
    );
}

/// A press is a DRAG only past a threshold (4px mouse, 10px finger): a tap
/// on a titlebar, a resize band or a fence handle moves nothing and persists
/// nothing. The threshold folds (`dragThreshold`, `dragBegins`) are driven by
/// `ui-tests/wb-console-input.test.mjs`; the four gesture handlers are DOM wiring
/// that no node test runs, so they are pinned here on the predicate they
/// consult.
#[test]
fn shell_drags_only_past_a_threshold() {
    // The console, its fence list, its window chrome and its fences, as one
    // text: the window gestures live in the chrome, the fence gestures in the
    // fences, the fence records in the fence list.
    let js = &[
        include_str!("../assets/ui/wb-console.ts"),
        include_str!("../assets/ui/wb-stage-fence-list.ts"),
        include_str!("../assets/ui/wb-stage-chrome.ts"),
        include_str!("../assets/ui/wb-stage-fences.ts"),
        include_str!("../assets/ui/wb-stage-fence-gestures.ts"),
    ]
    .concat();
    let body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("the console must keep {name}"))
            .1;
        after[..after.find("\n  }").expect("the function must close")].to_string()
    };
    for handler in [
        "function makeDraggable(",
        "function startResize(",
        "function startFenceMove(",
        "function startFenceResize(",
    ] {
        let b = body(handler);
        assert!(
            b.contains("dragThreshold(e.pointerType)") && b.contains("dragBegins("),
            "{handler} must arm only past dragThreshold"
        );
    }
    // The console gestures write only once armed: a bare tap moved nothing.
    // `persist()` is the hook, not `setWin` directly, since the note card
    // drops through the same gesture into its own collection (ADR-0064 §8).
    // BOTH halves are pinned: that only an armed gesture writes, AND what the
    // hook defaults to — the rect alone, computed at the act (ADR-0050
    // amendment 2026-10-04). Pinning the call alone would let a regression
    // bind the default to a no-op — every test green while an armed window
    // drag writes nothing and the layout is lost on the next reload.
    for handler in ["function makeDraggable(", "function startResize("] {
        let b = body(handler);
        assert!(
            b.contains("if (armed) persist()"),
            "{handler} must persist only an armed gesture"
        );
        assert!(
            b.contains(
                "opts?.onDrop || (() => setWin(win as ConsoleWin, { rect: restoreRect(win) }))"
            ),
            "{handler}'s drop hook must default to writing the window's rect"
        );
    }
    // The projection a card's lock is derived from. Pinned in Rust because
    // the module's fence array is not reachable from a ui-test: an
    // implementation that handed out the LIVE records, or that dropped
    // `rect`/`locked` from the copy, would break `lockedBy`'s "fence"
    // verdict with nothing on either side to catch it.
    assert!(
        body("function fenceRecords(").contains("fences().map((f: DeskFence) => ({ ...f }))"),
        "fenceRecords must answer copies of the whole record (ADR-0064 §8)"
    );
    // And the card must actually PASS both hooks: the default is correct
    // for a window and wrong for a card, whose node has no `_deskLocked`
    // and whose rect belongs to another collection.
    let notes_js = include_str!("../assets/ui/wb-notes.ts");
    for pin in [
        "locked: () => !!el._noteLocked",
        "onDrop: () => persistCards(el)",
    ] {
        assert!(
            notes_js.contains(pin),
            "the card must hand the gesture its own {pin} (ADR-0064 §8)"
        );
    }
}

/// A finger on a console's titlebar: two taps maximize, a hold on the name
/// renames. `isDoubleTap` is driven by `ui-tests/wb-console-input.test.mjs`; the
/// wiring is DOM that no node test runs, so it is pinned here, and the
/// gestures themselves by `tests/browser/console/wb_console_touch.py`.
#[test]
fn titlebar_touch_double_taps_and_holds() {
    // The console, its window chrome and its title, as one text: the titlebar lives in the
    // chrome, the name's double-click in the title.
    let js = &[
        include_str!("../assets/ui/wb-console.ts"),
        include_str!("../assets/ui/wb-stage-chrome.ts"),
        include_str!("../assets/ui/wb-console-title.ts"),
    ]
    .concat();
    let after = js
        .split_once("function wireTitleTouch(")
        .expect("the console must keep wireTitleTouch")
        .1;
    let b = &after[..after.find("\n  }").expect("the function must close")];
    for pin in [
        // Each `dblclick` listener needs the press's input type.
        "win._lastPointerType = e.pointerType;",
        // Without it a `mousedown` after the hold blurs the new name input.
        "e.preventDefault();",
        // Without it iOS Safari zooms the page on the double tap.
        r#"if (!(e.target as HTMLElement).closest("button, .session-name-input")) e.preventDefault();"#,
        "{ passive: false }",
        "dragBegins(pressed,",
        "HOLD_MS)",
        "isDoubleTap(win._lastTap, tap) && !isFull(win)",
        "startRename(win, span!);",
        r#"document.addEventListener("pointercancel", onCancel);"#,
        r#"document.removeEventListener("pointercancel", onCancel);"#,
    ] {
        assert!(b.contains(pin), "wireTitleTouch must keep {pin}");
    }
    assert!(
        js.contains("wireTitleTouch(win, titlebar, maxOrRestore);"),
        "every console titlebar must get the touch gestures"
    );
    // Both `dblclick` listeners (bar and name) leave a finger's taps to
    // wireTitleTouch, or Chrome on Android toggles or renames twice.
    assert_eq!(
        js.matches(r#"if (win._lastPointerType !== "mouse") return;"#)
            .count(),
        2,
        "the titlebar and the name dblclick must both ignore a touch"
    );
}

/// A note card is a surface on the WINDOW tier and wears the plane's own
/// chrome (ADR-0064 §8, amendment 2026-09-22). Both halves were found by
/// the operator on the first plane that had a note on it, and neither is
/// visible to a fold test: a card without a `z-index` is drawn UNDER every
/// console, so a click on its body lands on a terminal's canvas and the
/// keystrokes go to the shell — a note that cannot be typed into.
#[test]
fn a_note_card_is_stacked_and_wears_the_console_chrome() {
    // Normalized: the pin below spans line ends, and a Windows checkout
    // (CI's included) embeds the asset with CRLF.
    let console = include_str!("../assets/ui/wb-console-api.ts").replace("\r\n", "\n");
    let stack = include_str!("../assets/ui/wb-stage-stack.ts");
    let notes = include_str!("../assets/ui/wb-notes.ts");
    let veil = include_str!("../assets/ui/wb-notes-veil.ts");
    // The seam: a place in the tier WITHOUT focus, because a restore
    // focuses nothing and `focusWin` is the only other way to get one.
    assert!(
        stack.contains("function stackWin(")
            && console.contains("\n    stackWin: stack.stackWin,\n"),
        "wb-stage-stack.ts must declare stackWin and wb-console-api.ts must export it, \
         the tier a restored surface enters by"
    );
    assert!(
        notes.contains("stack?.stackWin(el)"),
        "a card must enter the window tier when it is built (ADR-0064 §8 amendment)"
    );
    // One titlebar vocabulary on the plane: the card's controls are the
    // console's icons, and the console's own two lock glyphs verbatim.
    for pin in [
        r#"<i class="bi bi-grip-vertical"></i>"#,
        r#"<i class="bi bi-palette"></i>"#,
        r#"<i class="bi bi-gear"></i>"#,
        r#"<i class="bi bi-x-lg"></i>"#,
        r#"'<i class="bi bi-lock-fill"></i>' : '<i class="bi bi-unlock"></i>'"#,
    ] {
        assert!(
            notes.contains(pin),
            "the card's chrome must be the console's icon {pin}"
        );
    }
    // NEGATIVE CONTROL: the text glyphs this replaced, as the ASSIGNMENT
    // that drew them — the characters themselves still appear in prose
    // naming the controls, and a card drawing its own is the defect, not
    // the word for it.
    for glyph in [
        '\u{28ff}',
        '\u{25d1}',
        '\u{22ef}',
        '\u{1f512}',
        '\u{1f513}',
        '\u{d7}',
    ] {
        let drawn = format!("textContent = \"{glyph}\"");
        assert!(
            !notes.contains(&drawn),
            "the card must not draw the text glyph {glyph:?} for a control"
        );
    }
    // THE GEAR IS THE FOOTER'S, beside the path it acts on (ADR-0064,
    // amendment 2026-09-22). Nothing in its menu is about the card, and in
    // the head it put `Delete file…` two pixels from the close button. The
    // head's cluster is pinned WITHOUT it, which is the half that would
    // rot first — a control put back there would read as one of the
    // card's own.
    assert!(
        notes.contains("foot.append(more, path, dir, rename, state)"),
        "the file gear belongs in the footer, beside the path it acts on"
    );
    assert!(
        notes.contains("tools.append(tone, index, veil, lock, close, back)"),
        "the head's cluster is the CARD's controls; the file's is not among them"
    );
    // `Hide this note` is gone (the operator's call, 2026-09-22): the eye
    // hides any card, so the entry was a second door to one place. The
    // UNMARK is not a duplicate of anything — the eye can mark and reveal
    // but never unmark — so it stays, shown only in the state it undoes.
    assert!(
        [notes, veil]
            .iter()
            .map(|f| f.matches(r#""Hide this note""#).count())
            .sum::<usize>()
            == 1
            && veil.contains(r#"btn.title = !marked ? "Hide this note""#),
        "the eye is how a note is hidden; the menu must not offer it twice"
    );
    assert!(
        notes.contains(r#"mark.textContent = "Stop hiding this note""#)
            && notes.contains("mark.hidden = !veiledOf(el._noteMarkdown)"),
        "the one door OUT of the veil must stay, and show only on a veiled note"
    );
    // A veiled card holds only its header, so unmarking straight from it
    // would write that header over the note. The file is read back first.
    assert!(
        veil.contains("if (!marked && veiledNow(el))"),
        "unmarking a veiled card must re-read the file, not write its bare header"
    );
    // The title is `flex: 1` and therefore most of the HEAD, which is the
    // drag handle. Opening the rename on the way DOWN (and stopping the
    // press so it cannot arm a drag) left the card movable only by its
    // grip — measured. The decision is taken on the way up, against the
    // same 3 px the gesture calls a drag, and nothing is stopped.
    assert!(
        !notes.contains("ev.stopPropagation();\n      beginTitle(el)"),
        "the title must not swallow the head's press — the head is the drag handle"
    );
    assert!(
        notes.contains(r#"title.addEventListener("pointerup""#)
            && notes.contains("Math.abs(ev.clientX - from.x) > 3"),
        "the rename must open on a press that did not move (ADR-0064 §8 amendment)"
    );
    // The look's closed sets and the omitted defaults are driven by
    // `ui-tests/wb-notes-folds.test.mjs` (`withStyle`).
}

/// A console and a fence can be LOCKED in place (ADR-0050 / ADR-0051 lock
/// amendment, 2026-09-20): the four gesture handlers and the tiler consult
/// the lock and refuse, the chrome carries the toggle, and the stylesheet
/// drops the bands. Every pin is an expression, as above.
#[test]
fn shell_locks_consoles_and_fences() {
    // The whole console family, as one text: the lock, the fence's lock, the
    // lock buttons, the gestures and the desk records live in its modules.
    let js = &console_family();
    for pin in [
        "function fenceLocked(",
        "function isLocked(",
        "function applyLock(",
        "function toggleLock(",
        "function paintFenceLock(",
        "function setFenceLock(",
        "function converge(",
        "actions.append(colBtn, fullBtn, maxBtn, restartBtn, lockBtn, closeBtn)",
        "tools.append(tile, columns, lock, detach, drop)",
    ] {
        assert!(js.contains(pin), "the console must keep the lock pin {pin}");
    }
    let body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("the console must keep {name}"))
            .1;
        after[..after.find("\n  }").expect("the function must close")].to_string()
    };
    // `heldFast()` defaults to `isLocked(win)` and is the note card's own
    // lock when a card is dragging (ADR-0064 §8); the refusal is the same.
    for handler in ["function makeDraggable(", "function startResize("] {
        let b = body(handler);
        assert!(
            b.contains("if (heldFast()) return")
                && b.contains("opts?.locked || (() => isLocked(win as ConsoleWin))"),
            "{handler} must refuse a locked console"
        );
    }
    for handler in ["function startFenceMove(", "function startFenceResize("] {
        assert!(
            body(handler).contains("if (fenceLocked(f.id)) return"),
            "{handler} must refuse a locked fence"
        );
    }
    assert!(
        body("function arrangeFence(").contains("if (fenceLocked(id)) return"),
        "tiling a locked fence must be a no-op"
    );
    assert!(
        body("function recordOf(").contains("locked: !!win._deskLocked"),
        "recordOf must write the lock as a bool"
    );
    assert!(
        body("function deskOf(").contains("locked: !!win._deskLocked"),
        "deskOf must carry the lock into rebuilds and the popup"
    );
    assert!(
        body("function renderFences(").contains("paintFenceLock(el, !!f.locked)"),
        "renderFences is the one place a fence's lock reaches the DOM"
    );
    assert!(
        body("function ingestDesk(").contains("converge()")
            && body("function converge(").contains("applyLock(w, !!r.locked)"),
        "a lock set on another device must reach this page's windows on its next GET"
    );
    let css = served_css();
    assert!(
        css_rule_body(&css, ".fence-lock {").contains("pointer-events: auto"),
        ".fence-lock must opt back into pointer events — the cluster is inert"
    );
    assert!(
        css_rule_body(&css, ".session-window.held .session-handle {").contains("display: none"),
        "a locked console shows no resize bands"
    );
    assert!(
        css_rule_body(&css, ".fence.locked .fence-edge {").contains("display: none"),
        "a locked fence shows no resize bands"
    );
    assert!(
        css_rule_body(&css, ".fence-head {").contains("10.5rem"),
        "the head's reserve must make room for the fifth tool"
    );
    // The title bar's controls stack ABOVE the resize bands: the NE corner
    // band (26px under a coarse pointer) covered four fifths of the close
    // button, and a finger had one sliver to hit.
    let actions = css_rule_body(&css, ".session-actions {");
    assert!(
        actions.contains("position: relative") && actions.contains("z-index: 3"),
        "the actions cluster must sit above the resize bands"
    );
}

/// A fence is a GROUP (#341): derived membership, non-overlap, and the two
/// gestures that carry it. The three folds (`fenceMembership`, `fenceFits`,
/// `fenceMoveDelta`) are driven by `ui-tests/wb-geometry.test.mjs`; the
/// gestures and the CSS hit-test contract are pinned here, because the node
/// suite runs no DOM and no CSS.
#[test]
fn shell_fences_are_a_group() {
    // The console, its fences and its desk records, as one text: the two
    // gestures live in the fences, the window record in the desk records.
    let js = &[
        include_str!("../assets/ui/wb-console.ts"),
        include_str!("../assets/ui/wb-stage-fences.ts"),
        include_str!("../assets/ui/wb-stage-fence-gestures.ts"),
        include_str!("../assets/ui/wb-desk-records.ts"),
    ]
    .concat();
    for pin in ["function startFenceMove(", "function startFenceResize("] {
        assert!(
            js.contains(pin),
            "wb-console.ts must keep the #341 pin {pin}"
        );
    }
    // Membership is DERIVED, never stored: the only fence id in the shell is
    // the fence element's OWN `data-fence-id`. A desk record that carried one
    // is exactly the state that can disagree with the geometry.
    let record = js
        .split_once("function recordOf(")
        .expect("wb-console.ts must keep recordOf")
        .1;
    assert!(
        !record[..record.find("\n  }").expect("recordOf must close")].contains("fence"),
        "no window record may carry a stored fence id (#341)"
    );
    let css = served_css();
    let rule = |head: &str| -> String {
        let after = css
            .split_once(head)
            .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
            .1;
        after[..after.find('}').expect("the rule must close")].to_string()
    };
    // Both handles opt back IN, against a `.fence`/`.fence-head` that stay
    // inert — that pair is the whole hit-test contract of this slice. The
    // resize handle is `.fence-edge` since ADR-0051 §7a made every border a
    // handle; `.fence-grip` is the SE one's second class and no longer
    // carries the opt-in itself.
    for head in ["\n.fence-grab {", "\n.fence-edge {"] {
        assert!(
            rule(head).contains("pointer-events: auto"),
            "{head} must take pointer events — it is a gesture handle (#341)"
        );
    }
    assert!(
        rule("\n.fence-head {").contains("width: max-content"),
        "the head stays shrink-wrapped — a full-width band swallows the floor pan (#340/#341)"
    );
    // The STACKING ORDER of the head against those bands. Measured when §7a
    // landed: the bands are `position:absolute` and the head is static, so
    // they paint over it whatever the DOM order — the NW corner covered the
    // `⠿` grab and a fence MOVE silently became a resize toward the origin,
    // leaving every member behind. DOM order alone cannot express this, so
    // the lift is pinned here.
    for head in ["\n.fence-head {", "\n.fence-tools {"] {
        assert!(
            rule(head).contains("z-index: 2"),
            "{head} must sit ABOVE the resize bands, or they swallow its controls (§7a)"
        );
    }
    // The refusal must be VISIBLE: a silent revert reads as a dropped drag.
    assert!(
        rule("\n.fence-invalid {").contains("var(--danger)"),
        "a refused fence drop must show feedback, not just revert (#341)"
    );
}

/// Arrange moved INTO the fence (#342): the global control is retired and
/// tiling is a per-fence act over a pure fold. The fold (`tileIntoRect`) and
/// `fenceRepos` are driven by the node suite; the retirement, the fence
/// chrome and the CSS are pinned here, where a revert of either half fails.
#[test]
fn shell_arranges_into_the_fence() {
    // The console, its fence list and its fences, as one text: tiling lives
    // in the fences, the fence chrome readout in the fence list.
    let js = &[
        include_str!("../assets/ui/wb-console.ts"),
        include_str!("../assets/ui/wb-stage-fence-list.ts"),
        include_str!("../assets/ui/wb-stage-fences.ts"),
        include_str!("../assets/ui/wb-stage-fence-gestures.ts"),
    ]
    .concat();
    for pin in [
        "function arrangeFence(",
        "function refreshFenceChrome(",
        "fence-arrange",
    ] {
        assert!(
            js.contains(pin),
            "wb-console.ts must keep the #342 pin {pin}"
        );
    }
    // The global act is GONE, not wrapped: a surviving entry point is a
    // second meaning of "arrange" (ADR-0051 §7). Safe against the pin above
    // — `"function arrangeFence("` does not contain `"function arrange("`.
    assert!(
        !js.contains("function arrange("),
        "the global arrange must be retired, not kept as a wrapper (#342)"
    );
    // The #338 rule — a maximized console is never tiled — has no other home
    // now that the global arrange is gone. `.maximized` overrides all four
    // offsets with `!important`, so a tile rect written onto one is
    // invisible while it silently replaces the rect the restore reads back.
    let fence_arrange = js
        .split_once("function arrangeFence(")
        .expect("wb-console.ts must keep arrangeFence")
        .1;
    // The EXPRESSION, not the bare word: `"maximized"` alone is satisfied by
    // the comment that explains the rule, so deleting the filter leaves this
    // green — measured. A pin a comment can satisfy is not a pin.
    assert!(
        fence_arrange[..fence_arrange
            .find("\n  }")
            .expect("arrangeFence must close")]
            .contains(r#"!m.el.classList.contains("maximized")"#),
        "arrangeFence must exclude a maximized member from the grid (#338/#342)"
    );
    assert!(
        fence_arrange[..fence_arrange
            .find("\n  }")
            .expect("arrangeFence must close")]
            .contains("minWidth"),
        "arrangeFence must relax the CSS floor for a tile below it (#342)"
    );
    let app = shell_family();
    assert!(
        !app.contains("arrangeConsoles"),
        "the shell's global arrange action must be gone (#342)"
    );
    let html = include_str!("../assets/ui/index.html");
    assert!(
        !html.contains("Arrange"),
        "the canvas toolbar must carry no Arrange control (#342)"
    );
    // The fence's arrange button opts back INTO pointer events, against a
    // `.fence`/`.fence-head` that stay inert — without this the control is
    // drawn and unclickable, and every other pin here stays green.
    let css = served_css();
    let rule = |head: &str| -> String {
        let after = css
            .split_once(head)
            .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
            .1;
        after[..after.find('}').expect("the rule must close")].to_string()
    };
    assert!(
        rule("\n.fence-arrange {").contains("pointer-events: auto"),
        "the fence's arrange button must take pointer events (#342)"
    );
    // The same trap for the button that opens the fence as columns
    // (ADR-0051 §5, 2026-09-30).
    assert!(
        rule("\n.fence-columns {").contains("pointer-events: auto"),
        "the fence's columns button must take pointer events"
    );
    // The window floor lives in TWO files — the CSS declaration and the
    // constants `arrangeFence` relaxes it against. Nothing else notices when
    // they diverge: the tiles would silently render past the fence again.
    for (konst, decl) in [
        ("WIN_MIN_W = 240", "min-width: 240px"),
        ("WIN_MIN_H = 150", "min-height: 150px"),
    ] {
        assert!(
            include_str!("../assets/ui/wb-geometry.ts").contains(konst),
            "wb-geometry.ts must mirror the window floor as {konst} (#342)"
        );
        assert!(
            rule("\n.session-window {").contains(decl),
            "styles.css's window floor must still be `{decl}` — wb-console.ts mirrors it (#342)"
        );
    }
}

/// The fence list is the MAP (#343): the toolbar picker, the jump that
/// reuses #337's arithmetic, and the birth of a console inside the focused
/// fence. The pure folds (`rectHolds`, `fenceSummaries`, `spawnRectIn`,
/// `fenceCycle`) are driven by the node suite; the DOM wiring that calls
/// them runs in no node test, so a deletion there fails HERE or nowhere.
/// Every pin below is an EXPRESSION, not a bare noun: #342 measured that a
/// function's own explanatory comment satisfies a noun pin, leaving it green
/// over deleted code.
#[test]
fn shell_lists_the_fences() {
    // The console, its fence list, its window chrome and its view, as one text: the fence
    // list and the focused fence live in the fence list, the birth in the focused fence in the
    // chrome, the jumps and the slide in the view.
    let js = &[
        include_str!("../assets/ui/wb-console.ts"),
        include_str!("../assets/ui/wb-stage-fence-list.ts"),
        include_str!("../assets/ui/wb-stage-chrome.ts"),
        include_str!("../assets/ui/wb-stage-view.ts"),
    ]
    .concat();
    let geometry = include_str!("../assets/ui/wb-geometry.ts");
    for pin in [
        "function fenceList(",
        "function jumpToFence(",
        "function focusFence(",
        "function clearFenceFocus(",
    ] {
        assert!(js.contains(pin), "the console must keep the #343 pin {pin}");
    }
    let body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("the console must keep {name}"))
            .1;
        after[..after.find("\n  }").expect("the function must close")].to_string()
    };
    // The issue's own rule: the jump REUSES a shared fold, it does not
    // re-derive the arithmetic. Both halves are load-bearing — the call
    // site, and the fact that there is exactly one definition to call. A
    // second copy would satisfy the first assertion alone.
    //
    // ADR-0051 §7 (amended) changed WHICH fold: the jump ANCHORS the fence's
    // top-left corner rather than centring it. `bringIntoView` still exists
    // and still centres for the Go-to picker (#337), so both are pinned —
    // routing the jump back through the centring one is the regression this
    // catches.
    // Since ADR-0064 §10 a NOTE CARD jumps too, so the slide — and with it
    // the stored-offset invariant the jump learned the hard way — lives in
    // one `jumpToEl` that takes the fold. The two folds are NOT
    // interchangeable and the ADRs draw the contrast on purpose: a fence
    // is a region and anchors its corner (ADR-0051 §7 amended), a card is
    // a point of interest and is centred (ADR-0064 §10). Each jump is
    // pinned on its own fold AND on its own focus call, which is the other
    // half that differs between them.
    assert!(
        body("function jumpToEl(").contains("fold(restoreRect(el)"),
        "the jump must place the element through the fold it was handed (#343, §7)"
    );
    for (jump, fold, focus) in [
        (
            "function jumpToFence(",
            "jumpToEl(el, anchorIntoView)",
            "focusFence(id)",
        ),
        (
            "function jumpToNote(",
            "jumpToEl(el, bringIntoView)",
            "focusWin(el)",
        ),
    ] {
        let b = body(jump);
        assert!(
            b.contains(fold),
            "{jump} must route through the one slide with {fold} (#343, ADR-0064 §10)"
        );
        assert!(
            b.contains(focus),
            "{jump} must focus what it jumped to, not only slide to it"
        );
        assert!(
            !b.contains("anchorIntoView(restoreRect") && !b.contains("bringIntoView(restoreRect"),
            "{jump} must not carry a second copy of the arithmetic"
        );
    }
    for (name, what) in [
        ("function bringIntoView(", "bring-into-view"),
        ("function anchorIntoView(", "anchor-into-view"),
    ] {
        assert_eq!(
            geometry.matches(name).count(),
            1,
            "there may be exactly ONE {what} implementation, in wb-geometry.ts (#343)"
        );
        assert!(
            !js.contains(name),
            "wb-console.ts must not define {what} again (#343)"
        );
    }
    // The slide is a VIEW effect layered on top, never a substitute for the
    // committed offsets: `slideTo` is cancellable, and both the floor's pan
    // and the wheel take the view back from a jump still in flight.
    assert!(
        body("function jumpToEl(").contains("slideTo(ws, to)"),
        "the jump must travel through the cancellable slide (§7)"
    );
    for owner in ["function onFloorDown(", "function onWheel("] {
        assert!(
            body(owner).contains("cancelSlide()"),
            "{owner} must abandon a slide in flight — the operator's hand outranks it (§7)"
        );
    }
    // The keyboard walk, and the rule that makes it a sweep rather than a
    // teleport: READING ORDER off the geometry, not the desk array's order.
    let desk_folds = include_str!("../assets/ui/wb-desk-folds.ts");
    let cycle = desk_folds
        .split_once("function fenceCycle(")
        .expect("wb-desk-folds.ts must keep fenceCycle")
        .1;
    assert!(
        cycle[..cycle.find("\n}").expect("the function must close")].contains("fenceOrder("),
        "the walk must step through the reading-order fold, not the desk array (§7)"
    );
    assert!(
        body("function stepFence(").contains("jumpToFence("),
        "a keyboard step must be a real jump, not a bare focus flip (§7)"
    );
    // The focus STATE MACHINE, not just its function names: measured, a
    // `focusFence` gutted to an empty body leaves every other pin here, the
    // whole node table and clippy green — while no ring renders and every
    // console is born free, which is this issue's headline behaviour. The
    // module variable and the class are what make focus real.
    let focus = body("function focusFence(");
    assert!(
        focus.contains("focusedFence = id") && focus.contains("is-focused"),
        "focusFence must set the module's focused id AND mark the element (#343)"
    );
    assert!(
        body("function clearFenceFocus(").contains("focusFence(null)"),
        "clearing focus must go through the same one setter (#343)"
    );
    assert!(
        body("function jumpToFence(").contains("focusFence(id)"),
        "the jump must FOCUS the fence it lands on — that is what the birth path reads (#343)"
    );
    // The focus comes BEFORE the maximize guard: under a maximize only the
    // slide is skipped, and the birth path still reads the focused fence.
    let jump = body("function jumpToFence(");
    let guard = jump
        .find(r#"querySelector(".session-window.maximized")) return el"#)
        .expect("under a maximize the fence jump must not move the view (ADR-0051 §7)");
    assert!(
        jump.find("focusFence(id)").is_some_and(|at| at < guard),
        "the fence must take the focus even when the view does not move (ADR-0051 §7)"
    );
    assert!(
        body("function onFloorDown(").contains("clearFenceFocus()"),
        "a bare-floor press outside the focused fence must clear it (#343)"
    );
    // The containment predicate is REUSED, not re-spelled — the same rule
    // the issue states for `bringIntoView`. Pinning only the definition
    // lets an inlined comparison sit beside it as exported dead code.
    // The two owners now live in two files — `fenceMembership` went to
    // `wb-geometry.ts` with the predicate it shares, the floor's hit test
    // stayed with the DOM it reads — so the slicer is told which source to
    // carve. That is the whole change: the invariant ("one containment
    // predicate, two callers") is exactly what it was, and it is now stated
    // ACROSS the seam, which is where it can actually break.
    let geometry_body = |name: &str| -> String {
        let after = geometry
            .split_once(name)
            .unwrap_or_else(|| panic!("wb-geometry.ts must keep {name}"))
            .1;
        after[..after.find("\n  }").expect("the function must close")].to_string()
    };
    for (owner, what, found) in [
        (
            "function fenceMembership(",
            "membership",
            geometry_body("function fenceMembership("),
        ),
        (
            "function onFloorDown(",
            "the floor's focus hit test",
            body("function onFloorDown("),
        ),
    ] {
        let _ = owner;
        assert!(
            found.contains("rectHolds("),
            "{what} must go through the one containment predicate (#343)"
        );
    }
    // The birth site goes through the pure box, so "a console opened while a
    // fence is focused lands inside it" is arithmetic, not a hand-placed
    // rect that drifts from the fence's own geometry.
    assert!(
        body("function spawnInFence(").contains("spawnRectIn("),
        "spawnInFence must place a fence-born console through spawnRectIn (#343)"
    );
    // One fold feeds BOTH readouts: the fence's own chrome and the toolbar
    // row can never disagree about a count.
    assert!(
        body("function refreshFenceChrome(").contains("fenceSummaries("),
        "the fence chrome must read the same fold the list does (#343)"
    );
    let tab = include_str!("../assets/ui/wb-consoles-tab.ts");
    for pin in ["jumpFence(", "fenceList()"] {
        assert!(
            tab.contains(pin),
            "wb-consoles-tab.ts must keep the #343 pin {pin}"
        );
    }
    let html = include_str!("../assets/ui/index.html");
    // `class="fence-item"`, not the bare noun: the markup's own comment
    // names the class, so a rename on the real element would leave a bare
    // `"fence-item"` pin green while every row loses its styling.
    for pin in ["jumpFence(", r#"class="fence-item""#] {
        assert!(
            html.contains(pin),
            "index.html must keep the #343 pin {pin}"
        );
    }
    // The focused fence must be VISIBLE — an invisible focus makes "the next
    // console is born over there" unexplainable to the operator.
    let css = served_css();
    let rule = |head: &str| -> String {
        let after = css
            .split_once(head)
            .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
            .1;
        after[..after.find('}').expect("the rule must close")].to_string()
    };
    // A DEFINED token: an `outline` shorthand naming an undefined custom
    // property is invalid as a whole, so the ring silently never renders —
    // measured, with `--accent`, which this palette does not have.
    let focused = rule("\n.fence.is-focused {");
    assert!(
        focused.contains("outline: 2px solid var(--console-text)"),
        "the focused fence must carry a visible ring (#343)"
    );
    assert!(
        css.contains("--console-text:"),
        "the focus ring's colour token must exist — an undefined one voids the whole shorthand (#343)"
    );
}

/// A detached file comes home however its popup closes: the popup sends its
/// edited bytes on unload, and the shell polls `closed` for one that dies
/// without an unload. No node test fires an unload on the popup's page
/// script (`wb-detached.ts`), so a deletion fails HERE or nowhere. Pins are
/// expressions, not nouns (#342).
#[test]
fn a_detached_file_comes_home_when_its_popup_closes() {
    let popup = include_str!("../assets/ui/wb-detached.ts");
    for pin in [
        r#"window.addEventListener("beforeunload", sendHome)"#,
        r#"window.addEventListener("pagehide", sendHome)"#,
        r#"window.WBViewer?.descOf("detached")"#,
    ] {
        assert!(
            popup.contains(pin),
            "wb-detached.ts must keep the unload re-attach {pin}"
        );
    }
    let app = shell_family();
    for pin in [
        "detached.watch(win, desc);",
        "window.setInterval(pollDetached, 500)",
        "reattachFile(e.source as Window, m.desc);",
    ] {
        assert!(
            app.contains(pin),
            "the shell must keep the detached-file re-attach {pin}"
        );
    }
}

/// A fence detaches into its own window, and comes home (#346). The fold
/// (`detachFold`, with its cap of four) is driven by
/// `ui-tests/wb-console.test.mjs`; the DOM half, the popup document and the
/// CSS run in no node test, so a deletion there fails HERE or nowhere. Every
/// pin is an EXPRESSION, not a bare noun: #342 measured that a function's
/// own explanatory comment satisfies a noun pin, leaving it green over
/// deleted code.
#[test]
fn shell_detaches_a_fence() {
    // The console, its fence list, its fences, its desk and its detach, as
    // one text: the detach lives in the fences, the popup's mount and the
    // upload in the desk, the re-attach and the lifecycle channel in the
    // opener's side of the detach, the refused remove in the fence list.
    let js = &[
        include_str!("../assets/ui/wb-console.ts"),
        include_str!("../assets/ui/wb-stage-fence-list.ts"),
        include_str!("../assets/ui/wb-stage-fences.ts"),
        include_str!("../assets/ui/wb-stage-fence-gestures.ts"),
        include_str!("../assets/ui/wb-desk.ts"),
        include_str!("../assets/ui/wb-desk-restore.ts"),
        include_str!("../assets/ui/wb-desk-detach.ts"),
    ]
    .concat();
    for pin in [
        "function detachFence(",
        "function reattachFence(",
        "function mountDetached(",
        "fence-detach",
        "fence-detached",
    ] {
        assert!(
            js.contains(pin),
            "wb-console.ts must keep the #346 pin {pin}"
        );
    }
    let body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("wb-console.ts must keep {name}"))
            .1;
        after[..after.find("\n  }").expect("the function must close")].to_string()
    };
    // THE SINK SEAM. The persistence call sites must reach the desk through
    // the injected sink and carry NO `detached` branch of their own —
    // "incapable of writing", not "careful not to".
    assert!(
        squeeze(&body("function flushDesk(")).contains(".put(JSON.stringify(body))"),
        "flushDesk must write through the injected sink (#346)"
    );
    assert!(
        !body("function flushDesk(").contains("detach"),
        "flushDesk must carry no detached branch — the sink is the seam (#346)"
    );
    assert!(
        js.contains("deskSink.putSync("),
        "the pagehide flush must write through the injected sink (#346)"
    );
    // The PUT literal now lives in exactly one file. The second half is the
    // NEGATIVE CONTROL: deleting the write wholesale would satisfy the first
    // assertion alone.
    // The family holds the sink itself, which is the one place that writes.
    let outside_sink: String = CONSOLE_FAMILY
        .iter()
        .filter(|(name, _)| *name != "wb-desk-sink.ts")
        .map(|(_, text)| *text)
        .collect();
    assert!(
        !js.contains(r#""/api/desk", {"#)
            && !js.contains(r#""PUT /api/desk""#)
            && !outside_sink.contains(r#""/api/desk", {"#)
            && !outside_sink.contains(r#""PUT /api/desk""#),
        "the desk PUT must live only in wb-desk-sink.ts (#346)"
    );
    let sink = include_str!("../assets/ui/wb-desk-sink.ts");
    assert!(
        sink.contains(r#"apiFetch("PUT /api/desk", {"#) && sink.contains("query: { tab: TAB },"),
        "wb-desk-sink.ts must still perform the desk PUT (#346)"
    );
    assert!(
        sink.contains("keepalive: true"),
        "the sink's putSync must outlive the closing document (#346)"
    );
    // Arrange is a no-op on a detached fence (ADR-0051 §7a): its consoles are
    // in another window, and tiling the empty box would rewrite the very
    // rects the re-attach restores from.
    assert!(
        body("function arrangeFence(").contains("if (popups.isDetached(id)) return;"),
        "arrangeFence must bail on a detached fence (#346, §7a)"
    );
    // Removing a detached fence would destroy the glyph that is the only way
    // home and keep its DETACH_MAX slot consumed — refuse it, do not silently
    // strand the popup.
    assert!(
        body("function removeFence(").contains("if (isDetached(id)) {"),
        "removeFence must refuse a detached fence (#346, §7a)"
    );
    // The re-attach message must trust the SOURCE lookup, not the payload:
    // `m.fenceId` would let a popup for fence A re-attach fence B.
    assert!(
        !js.contains("reattachFence(m.fenceId"),
        "the re-attach message must use the proven owner, never its payload (#346)"
    );
    // The INVARIANT: either the popup exists and the members are torn down,
    // or neither. `window.open` must therefore be reached before a single
    // window is touched, and a null handle must bail.
    let detach = body("function detachFence(");
    assert!(
        detach.contains("window.open(\"fence\""),
        "detachFence must open the popup document (#346)"
    );
    assert!(
        detach.contains("if (!handle)"),
        "a blocked popup must bail BEFORE anything is torn down (#346)"
    );
    assert!(
        detach
            .find("window.open(")
            .expect("detachFence must open a popup")
            < detach
                .find("tearDownMember(")
                .expect("detachFence must tear its members down"),
        "the popup must be open before a member is torn down — the #346 invariant"
    );
    // A member leaves the plane WITHOUT losing its desk record and WITHOUT
    // closing its daemon session: the record is shared state a second client
    // still renders, and the socket close is the writer-slot release (§9).
    let teardown = body("function tearDownMember(");
    assert!(
        teardown.contains("win._term?.dispose(") && teardown.contains("wins.delete(win)"),
        "tearDownMember must dispose the terminal and drop the window (#346, §9)"
    );
    assert!(
        !teardown.contains("forgetRecord") && !teardown.contains("sessions/close"),
        "a detached member must keep its desk record and its daemon session (#346)"
    );
    // THE POPUP'S DENIED CAPABILITIES. Each is what stops a window holding a
    // FRAGMENT of the plane from writing the whole desk or the shell's view.
    // The options are built by the page's entry module,
    // `detached-fence-main.ts`; the handshake is `wb-detached-fence.ts`.
    let html = include_str!("../assets/ui/detached-fence.html");
    let entry = include_str!("../assets/ui/detached-fence-main.ts");
    let page = include_str!("../assets/ui/wb-detached-fence.ts");
    for pin in [
        "WBDeskSink.none()",
        "autoBoot: false",
        "canLaunch: false",
        "read: () => null",
    ] {
        assert!(
            entry.contains(pin),
            "detached-fence-main.ts must keep the #346 pin {pin}"
        );
    }
    for pin in [
        "WBConsole.mountDetached(",
        "wb-fence-ready",
        "wb-fence-reattach",
    ] {
        assert!(
            page.contains(pin),
            "wb-detached-fence.ts must keep the #346 pin {pin}"
        );
    }
    assert!(
        !html.contains("WBConsole.open(")
            && !entry.contains("WBConsole.open(")
            && !page.contains("WBConsole.open("),
        "the popup must expose no way to open a new console (#346)"
    );
    // The handshake's confidentiality control: a concrete targetOrigin, so a
    // page on any other origin never receives this fence's members. Pinned as
    // the ASSIGNMENT, not the bare noun — `window.location.origin` also
    // appears in the inbound guard below it, so rewriting `PEER` to `"*"`
    // would leave a noun pin green. Both scripts post to the opener, and
    // each has its own `PEER`.
    for (name, text) in [
        ("detached-fence-main.ts", entry),
        ("wb-detached-fence.ts", page),
    ] {
        assert!(
            text.contains("const PEER = window.location.origin;"),
            "{name}: the popup's PEER must resolve to a concrete origin (#346)"
        );
    }
    assert!(
        !page.contains(r#"postMessage({ type: "wb-fence-ready" }, "*")"#),
        "the popup must never broadcast its handshake to \"*\" (#346)"
    );
    // The opener's reply has the same control as the popup's PEER: a concrete
    // origin, never a broadcast.
    let opener = body("window.addEventListener(\"message\"");
    assert!(
        opener.contains("if (e.origin !== location.origin) return;") && !opener.contains("\"*\""),
        "the opener's handover must answer only its own origin (#346)"
    );
    // The tree-wide sweep in `shell_stores_only_the_view_in_the_browser`
    // scans .html too; keep this document out of the browser's stores.
    assert!(
        !html.contains("localStorage")
            && !html.contains("sessionStorage")
            && !entry.contains("localStorage")
            && !entry.contains("sessionStorage"),
        "the popup must store nothing in the browser (#346)"
    );
    // `.fence-tools` and `.fence` are transparent to pointer events, so a
    // control that does not opt back IN is drawn and unclickable — while
    // every source-text pin above stays green. Measured in #342.
    let css = served_css();
    let rule = |head: &str| -> String {
        let after = css
            .split_once(head)
            .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
            .1;
        after[..after.find('}').expect("the rule must close")].to_string()
    };
    for head in ["\n.fence-detach {", "\n.fence-detached {"] {
        assert!(
            rule(head).contains("pointer-events: auto"),
            "{head} must take pointer events, or the control is inert (#346)"
        );
    }
}

/// A page writes the desk only as desk changes, each with the fields its act
/// changed (ADR-0050 amendment 2026-10-04, changes, not the desk). The old
/// writer built a whole record from the screen with a fresh clock, and seven of
/// its eleven callers were not about the rect. These pins keep the shape out:
/// no whole-record writer, no whole-desk body, no client clock in a record, and
/// one place that builds the body.
#[test]
fn the_shell_writes_the_desk_only_as_changes() {
    let console = include_str!("../assets/ui/wb-console.ts");
    let desk = include_str!("../assets/ui/wb-desk.ts");
    let records = include_str!("../assets/ui/wb-desk-records.ts");
    let notes = include_str!("../assets/ui/wb-notes.ts");
    let folds = include_str!("../assets/ui/wb-notes-folds.ts");
    let sync = include_str!("../assets/ui/wb-desk-sync.ts");
    // The whole console family, and the other modules that write the desk.
    let others = [
        ("wb-notes.ts", notes),
        ("wb-notes-folds.ts", folds),
        (
            "wb-notes-stage.ts",
            include_str!("../assets/ui/wb-notes-stage.ts"),
        ),
        (
            "wb-notes-editor.ts",
            include_str!("../assets/ui/wb-notes-editor.ts"),
        ),
        (
            "wb-notes-on-top.ts",
            include_str!("../assets/ui/wb-notes-on-top.ts"),
        ),
        (
            "wb-notes-veil.ts",
            include_str!("../assets/ui/wb-notes-veil.ts"),
        ),
        (
            "wb-notes-map.ts",
            include_str!("../assets/ui/wb-notes-map.ts"),
        ),
        (
            "wb-messages.ts",
            include_str!("../assets/ui/wb-messages.ts"),
        ),
    ];
    for (name, js) in [CONSOLE_FAMILY, &others].concat() {
        for banned in [
            "persistWin(",
            "deskBody(",
            "mergeDesk(",
            "removed: {",
            "ts: Date.now() }",
        ] {
            assert!(
                !js.contains(banned),
                "{name} must not contain `{banned}`: a desk write carries only the changed fields"
            );
        }
    }
    // Every window write names its fields, and the body is built in one place.
    let squeezed = squeeze(records);
    assert!(
        squeezed.contains(r#"emitDesk({op:"set",type:"window",id:win._deskId,fields});"#),
        "setWin must send the fields it was handed, and nothing else"
    );
    // The family holds the sync module itself, which builds the body.
    let outside_sync: String = CONSOLE_FAMILY
        .iter()
        .filter(|(name, _)| *name != "wb-desk-sync.ts")
        .map(|(_, text)| *text)
        .collect();
    assert!(
        !outside_sync.contains("changes:")
            && !console.contains("changes:")
            && !desk.contains("changes:")
            && !records.contains("changes:")
            && sync.contains("changes: pending.slice(0, inflight.count)"),
        "the upload body is built by wb-desk-sync.ts alone"
    );
}

/// ADR-0066 Consequences: the shell builds a record field by field in more than
/// one place, and a copy that misses `consoleName` drops the name — on a
/// create (`recordOf`), on a restart or a worktree switch (`deskOf`, the carry
/// of `relaunchIn`), or at birth (`buildChrome`, through the window inventory).
#[test]
fn console_name_rides_every_record_copy() {
    // The whole console family, as one text: the record copies and the birth
    // live in its modules.
    let js = &console_family();
    let body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("the console must keep {name}"))
            .1;
        after[..after.find("\n  }").expect("the function must close")].to_string()
    };
    for copy in [
        "function recordOf(",
        "function deskOf(",
        "function buildChrome(",
    ] {
        assert!(
            body(copy).contains("consoleName"),
            "{copy} must carry the console name (#479)"
        );
    }
    assert!(
        include_str!("../assets/ui/wb-window-state.ts").contains("_deskConsoleName:"),
        "the window inventory must declare the console name (#479)"
    );
}

/// Every agent launch path — a new console, a restart, a worktree restart, a
/// relaunch after a daemon restart — reaches `spawnWindow`, so that is where
/// the console name joins the launch. Neither the Playwright suite nor the
/// node table covers the wiring in CI, so a deletion fails HERE (#480).
#[test]
fn spawn_window_sends_the_console_name_on_a_new_agent_launch() {
    let js = console_family();
    let after = js
        .split_once("function spawnWindow(")
        .expect("the console family must keep spawnWindow")
        .1;
    let body = squeeze(&after[..after.find("\n  }").expect("the function must close")]);
    // The guard of the statement that adds the name, whatever its layout
    // and whatever the order of its two conditions.
    let set = body
        .find("termOpts={...termOpts,name:win._deskConsoleName}")
        .expect("spawnWindow must send the console name (#480)");
    let guard = &body[body[..set]
        .rfind("if(")
        .expect("the name is sent under a guard")..set];
    for condition in ["termOpts.id==null", "!termOpts.console"] {
        assert!(
            guard.contains(condition),
            "spawnWindow must send the console name with a new agent launch only (#480): {guard}"
        );
    }
}

/// The detach survives an F5, and dies with the tab that opened it (#347).
/// Same split as `shell_detaches_a_fence`: the fold (`peerFold`) is driven by
/// `ui-tests/wb-console.test.mjs`, and the wiring and the popup document are
/// pinned here. Every pin is an EXPRESSION — #342 measured that a function's
/// own explanatory comment satisfies a bare-noun pin over deleted code.
#[test]
fn shell_survives_a_reload_with_its_detach() {
    // The console, its popup registry, its fences, its desk and its detach,
    // as one text: the registry write lives in the registry, the detach in
    // the fences, the restore in the desk, and the lifecycle channel in the
    // opener's side of the detach.
    let js = &[
        include_str!("../assets/ui/wb-console.ts"),
        include_str!("../assets/ui/wb-desk-popups.ts"),
        include_str!("../assets/ui/wb-stage-fences.ts"),
        include_str!("../assets/ui/wb-stage-fence-gestures.ts"),
        include_str!("../assets/ui/wb-desk.ts"),
        include_str!("../assets/ui/wb-desk-restore.ts"),
        include_str!("../assets/ui/wb-desk-detach.ts"),
    ]
    .concat();
    let link = include_str!("../assets/ui/wb-detach-link.ts");
    let html = include_str!("../assets/ui/detached-fence.html");
    let entry = include_str!("../assets/ui/detached-fence-main.ts");
    let page = include_str!("../assets/ui/wb-detached-fence.ts");
    let body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("wb-console.ts must keep {name}"))
            .1;
        after[..after.find("\n  }").expect("the function must close")].to_string()
    };

    // The shell reaches storage and channel only through the injected link.
    for pin in ["link.readRegistry()", "link.writeRegistry("] {
        assert!(
            js.contains(pin),
            "wb-console.ts must keep the #347 pin {pin}"
        );
    }
    // THE STORE SEAM, with its NEGATIVE CONTROL: no console family module may
    // name the session store, and the link module must be the one that does
    // — deleting the store wholesale has to be red, not green.
    for (name, src) in CONSOLE_FAMILY {
        assert!(
            !src.contains("sessionStorage"),
            "{name} must reach the registry only through the injected link (#347)"
        );
    }
    // The CALLS, not the bare nouns: this file's own prose names
    // `sessionStorage` three times, so a noun pin stays green over
    // no-op'd `readRecord`/`writeRecord` bodies — the exact defect the
    // doc comment above warns about.
    assert!(
        link.contains("sessionStorage.getItem(KEY)")
            && link.contains("sessionStorage.setItem(KEY,")
            && link.contains("new BroadcastChannel(CHANNEL)"),
        "wb-detach-link.ts IS the registry and the channel — deleting it is not how #347 stays green"
    );
    assert!(
        link.contains(r#"const KEY = "wb.detach.v1""#)
            && link.contains(r#"const CHANNEL = "wb.detach.v1""#),
        "wb-detach-link.ts must keep the one registry key and channel name (#347)"
    );
    // The boot-ordering INVARIANT: a fence detached before the reload must
    // never have its members put back on the plane, for ANY verdict —
    // `relaunch` would spawn a SECOND PTY against a console the popup drives.
    assert!(
        body("function restoreDesk(").contains("record && away.has(record.id)"),
        "restoreDesk must skip a detached fence's members (#347)"
    );
    // `reconcileDesk` emits `record: null` on the `adopt` verdict, and the
    // skip runs BEFORE the dispatch — an unguarded read threw into the
    // swallowing `.catch`, costing every fence and every glyph on any boot
    // carrying a live session no record claims.
    assert!(
        include_str!("../assets/ui/wb-desk-folds.ts")
            .contains(r#"out.push({ record: null, session: s, action: "adopt", id: null })"#),
        "the adopt verdict's null record is what the guard above exists for (#347)"
    );
    // The registry carries the MEMBER IDS, not just the fence ids: a
    // detached fence may still be moved on the plane (§7a), after which a
    // geometry-derived membership answers "no members" and every one of
    // them comes back under a live popup.
    assert!(
        link.contains("readMembers()") && js.contains("link.readMembers()"),
        "the registry must carry each detached fence's member ids (#347)"
    );
    // The registry mirror and the store change in ONE place, so no return
    // path can leave them disagreeing.
    assert!(
        body("function commitDetached(")
            .contains("link.writeRegistry(detached, detachedMembers())"),
        "every detach transition must go through commitDetached (#347)"
    );
    assert!(
        !js.contains("detached = out.registry")
            && !console_family().contains("detached = out.registry"),
        "no caller may assign the registry behind commitDetached's back (#347)"
    );
    // THE F5 RULE: `pagehide` fires on a reload exactly as on a close, so it
    // must no longer close a popup. The `putSync` pin is the NEGATIVE
    // CONTROL that the listener itself was not simply deleted.
    assert!(
        !body("window.addEventListener(\"pagehide\"").contains(".handle.close()"),
        "pagehide must not close a detached popup — that is the F5 (#347)"
    );
    assert!(
        js.contains("deskSink.putSync("),
        "the pagehide flush must survive the popup-close deletion (#347)"
    );
    // THE POPUP'S FIFTH DENIED CAPABILITY, in its options.
    assert!(
        entry.contains("detachLink: WBDetachLink.none()"),
        "detached-fence-main.ts must keep the #347 pin detachLink: WBDetachLink.none()"
    );
    // Its half of the lifecycle, in its page script.
    for pin in [
        // The channel-only factory: this document must reach no store, not
        // even to read the copy `window.open` handed it.
        "WBDetachLink.channel()",
        "\"popup-here\"",
        "\"popup-gone\"",
        // The BEHAVIOUR, not the class name: `detached-lost` alone is
        // satisfied by the CSS rule in the page's own <style> block, so
        // deleting `lost()` outright would keep a bare-noun pin green.
        r#"'<p class="detached-lost">"#,
        "window.close()",
        "WBConsole.peerFold(",
    ] {
        assert!(
            page.contains(pin),
            "wb-detached-fence.ts must keep the #347 pin {pin}"
        );
    }
    // #346's confidentiality control is UNCHANGED: the channel carries only
    // lifecycle chatter, the members still ride the concrete-origin handshake.
    assert!(
        page.contains("const PEER = window.location.origin;"),
        "the initial handover must keep its concrete targetOrigin (#346, #347)"
    );
    assert!(
        !html.contains("localStorage")
            && !html.contains("sessionStorage")
            && !entry.contains("localStorage")
            && !entry.contains("sessionStorage")
            && !page.contains("localStorage")
            && !page.contains("sessionStorage"),
        "the popup must still store nothing in the browser (#346)"
    );
}

/// Peer session ownership must survive every browser reconnect and close
/// path; exact repo equality keeps a local slug from lighting a peer row.
/// The route's folds are driven by `ui-tests/wb-session-route.test.mjs`, and
/// the session name's tooltip by `sessionPresentation` in
/// `wb-console.test.mjs`; this test holds the call sites that use them.
#[test]
fn workbench_session_assets_preserve_composite_repo_identity() {
    // The whole console family: the terminal reconnects and reads the
    // announcement, the console closes.
    let console = console_family();
    // The close is pinned in the module that closes a console: the worktree
    // switcher also calls `closeUrl`, so the family text alone would hide a
    // lost close.
    assert!(
        include_str!("../assets/ui/wb-console-spawn.ts")
            .contains("fetch(WBSessionRoute.closeUrl(id, win._deskRepo)"),
        "the console close must keep its closeUrl call in wb-console-spawn.ts"
    );
    for pin in [
        r#"connect({ id: currentSessionId, repo: currentRepo"#,
        r#"c.verb === "session-open""#,
        "WBSessionRoute.url(",
        "WBSessionRoute.announcement(",
    ] {
        assert!(console.contains(pin), "the console must keep {pin}");
    }
    assert!(shell_family().contains("WBSessionRoute.matchesRepo("));
}

/// The stage/viewport shell (#336). A clamp lives in CSS and markup, which
/// the `node --test` suite never renders and Playwright — which does — does
/// not run in CI. So this is the only CI-visible gate that the deletion
/// stays deleted: a re-added clamp would pass every unit test in the tree.
#[test]
fn shell_has_no_clamp_and_carries_the_stage() {
    // The whole console family; the terminal holds the fit observer.
    let js = console_family();
    assert!(
        !js.contains("clampAll"),
        "the clamp-and-refit is deleted, not renamed (#336)"
    );
    assert!(
        !js.contains("observeWorkspace"),
        "nothing observes the viewport to reposition a window (#336)"
    );
    // The NEGATIVE control for the two pins above: the per-window fit
    // observer must SURVIVE. It resizes a terminal, never a window rect, so
    // a blanket "no ResizeObserver" edit would be the wrong fix.
    assert!(
        js.contains("new ResizeObserver"),
        "the per-window terminal fit observer must survive the deletion (#336)"
    );
    // The extent itself (`stageExtent`) is driven by the node suite.

    let html = include_str!("../assets/ui/index.html");
    assert!(
        html.contains(r#"id="stage""#),
        "index.html must carry the stage plane (#336)"
    );
    assert!(
        !html.contains(r#"class="stage""#),
        "the old tab-body class is renamed .tabbody — one meaning per name (#336)"
    );

    let css = served_css();
    for pin in ["#stage {", "#workspace.maxlock {", ".tabbody {"] {
        assert!(css.contains(pin), "styles.css must keep the #336 pin {pin}");
    }
}

/// The navigation layer over that plane (#337). The folds (`bringIntoView`,
/// `panNudge`) are driven by `ui-tests/wb-console.test.mjs`; the gestures
/// that use them are DOM wiring that no node test runs, so they are pinned
/// here, each inside the function that must hold it.
#[test]
fn shell_navigates_the_plane() {
    // The console, its window chrome, its fences and its view, as one text:
    // the plane lives in the console, the window drag in the chrome, the
    // fence drag in the fences, the reveal, the pan and the wheel in the view.
    let js = &[
        include_str!("../assets/ui/wb-console.ts"),
        include_str!("../assets/ui/wb-stage-chrome.ts"),
        include_str!("../assets/ui/wb-stage-fences.ts"),
        include_str!("../assets/ui/wb-stage-fence-gestures.ts"),
        include_str!("../assets/ui/wb-stage-view.ts"),
    ]
    .concat();
    for pin in [
        "function reveal(",
        "function onFloorDown(",
        "function onWheel(",
        // The REGISTRATION, not the phrase: pinning a bare `passive: false`
        // is satisfied by the comment that explains it, so the option could
        // be deleted with this gate still green.
        r#"addEventListener("wheel", onWheel, { passive: false })"#,
    ] {
        assert!(js.contains(pin), "the console must keep the #337 pin {pin}");
    }
    // Scoped to each gesture: the file carries each of these statements
    // several times, so a whole-file match stays green when one gesture
    // loses its own copy.
    let body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("the console must keep {name}"))
            .1;
        squeeze(&after[..after.find("\n  }").expect("the function must close")])
    };
    // The auto-pan loop's teardown: an uncancelled rAF pans forever after
    // the button is released. One loop cancels it, and each gesture stops it.
    assert!(
        body("function autoPan(").contains("cancelAnimationFrame(panRaf)"),
        "autoPan must cancel its loop (#337)"
    );
    for gesture in ["function makeDraggable(", "function startFenceMove("] {
        assert!(
            body(gesture).contains("pan.stop();"),
            "{gesture} must stop its auto-pan loop (#337)"
        );
    }
    // …and the two lost-mouseup recoveries, which are the only reason that
    // teardown is reachable when the release never arrives.
    for gesture in [
        "function makeDraggable(",
        "function startFenceMove(",
        "function startFenceResize(",
    ] {
        let b = body(gesture);
        assert!(
            b.contains("if(ev.buttons===0){onUp();")
                && b.contains(r#"window.addEventListener("blur",onUp)"#),
            "{gesture} must end on a lost mouseup (#337)"
        );
    }
    assert!(
        !js.contains("scrollIntoView"),
        "the centring is a tabled pure function, not the browser's heuristic (#337)"
    );

    let css = served_css();
    for pin in [
        // The VALUE, not the property: `overscroll-behavior: auto` is the
        // exact mutation `wb_pan_337.py` measures as chaining the wheel out
        // of the terminal (plane 200 -> 0), and a property-name pin passes
        // straight through it.
        "overscroll-behavior: contain",
        "#stage.panning {",
        "cursor: grabbing",
    ] {
        assert!(css.contains(pin), "styles.css must keep the #337 pin {pin}");
    }
    // `cursor: grab` alone is satisfied by `.session-titlebar`, so scope the
    // pin to the stage's OWN rule — the floor advertising itself as the pan
    // surface is the affordance this issue added.
    let stage_rule = css
        .split_once("\n#stage {")
        .expect("styles.css must keep the #stage rule")
        .1;
    assert!(
        stage_rule[..stage_rule.find('}').expect("the #stage rule must close")]
            .contains("cursor: grab"),
        "the stage itself must advertise the grab cursor (#337)"
    );

    // The picker's wiring lives in wb-consoles-tab.ts; without this the
    // `@click` handlers in index.html can go dangling with every test still green.
    let tab = include_str!("../assets/ui/wb-consoles-tab.ts");
    for pin in ["toggleWindowMenu(", "revealWindow(", "windowList"] {
        assert!(
            tab.contains(pin),
            "wb-consoles-tab.ts must keep the #337 pin {pin}"
        );
    }

    let html = include_str!("../assets/ui/index.html");
    for pin in [r#"class="dropdown window-menu""#, r#"class="window-item""#] {
        assert!(
            html.contains(pin),
            "index.html must keep the #337 pin {pin}"
        );
    }
}

/// The FRAME chrome over that plane (#338): the footer pills and the
/// empty-stage hint belong to `.consoles-tab`, never to `#stage`, and the
/// maximize pin is a DERIVED fact. Same bargain as the two above — the CI
/// suites do not render this markup — so this is the only gate that notices
/// the chrome sliding back onto the plane.
#[test]
fn shell_pins_the_frame_chrome() {
    let html = include_str!("../assets/ui/index.html");
    // The stage element is EMPTY in the markup, so the chrome physically
    // cannot be a child of it — a stronger pin than "is not inside" prose.
    assert!(
        html.contains(r#"<div id="stage"></div>"#),
        "the stage must stay an empty element the shell fills (#338)"
    );
    let tab = html
        .find(r#"class="consoles-tab""#)
        .expect("index.html must keep the consoles tab (#338)");
    // The section's OWN close, not a later landmark: bounding by `#viewers`
    // would still pass for chrome that escaped the tab entirely.
    let close = tab
        + html[tab..]
            .find("</section>")
            .expect("the consoles tab must close (#338)");
    // The viewport as a CLOSED element: an offset past its `</div>` cannot be
    // inside `#workspace` — and therefore cannot be inside `#stage` either.
    // Chrome inside the scrolling box pans away just as surely as chrome on
    // the plane, so "after the stage" alone would not be enough.
    let viewport = r#"<div id="workspace"><div id="stage"></div></div>"#;
    let ws = html
        .find(viewport)
        .expect("index.html must keep the closed viewport element (#338)")
        + viewport.len();
    assert!(
        ws > tab,
        "the viewport must live inside the consoles tab (#338)"
    );
    // `canvas-empty` was the second piece of frame chrome here. The
    // empty-stage caption is gone — the toolbar's New-console button says
    // the same thing where the operator acts — so the footer carries the
    // rule alone, and the caption's absence is asserted below.
    let pin = r#"class="canvas-foot""#;
    let at = html
        .find(pin)
        .unwrap_or_else(|| panic!("index.html must carry the frame chrome {pin} (#338)"));
    assert!(
        at > ws && at < close,
        "{pin} must be a SIBLING of the viewport inside .consoles-tab (#338)"
    );
    // The ELEMENT, not the words: the comment that stands where the caption
    // did quotes them, and a pin that cannot tell prose from markup would
    // make documenting the removal the thing that fails.
    assert!(
        !html.contains(r#"class="canvas-empty""#),
        "the empty-stage caption is not to come back"
    );

    // Whitespace-free text: the code, not its layout. The console and its
    // view, as one text: the pin and the maximize lock live in the console,
    // the scroll registration and the reveal in the view, so the negative
    // pin below covers both.
    let js = squeeze(
        &[
            include_str!("../assets/ui/wb-console.ts"),
            include_str!("../assets/ui/wb-stage-window.ts"),
            include_str!("../assets/ui/wb-stage-view.ts"),
        ]
        .concat(),
    );
    for pin in [
        "functionsyncMaxPin(",
        // The REGISTRATION, not the function: without it the pin is only
        // re-derived on a maximize and a programmatic pan desyncs it.
        r#"ws.addEventListener("scroll",syncMaxPin)"#,
        "workbench:stage-extent",
        // The NEGATIVE control: the scroll freeze must SURVIVE. A blanket
        // deletion of the maximize machinery would satisfy every "no longer
        // contains" pin below and must be red, not green.
        "functionsyncMaxLock(",
    ] {
        assert!(
            js.contains(pin),
            "wb-console.ts, wb-stage-window.ts or wb-stage-view.ts must keep the #338 pin {pin}"
        );
    }
    // The POSITIVE half of the `reveal()` change, in either operand order:
    // the negative pin below is one spelling and a requote would slip past
    // it, and scenario 4 (the only behavioural gate) does not run in CI.
    let maximized = r#"it.classList.contains("maximized")"#;
    let column = r#"it.classList.contains("column")"#;
    assert!(
        js.contains(&format!("if({maximized}||{column})returnit;"))
            || js.contains(&format!("if({column}||{maximized})returnit;")),
        "reveal() must return a maximized or column window as it is (#338)"
    );
    assert!(
        !js.contains(r#"if(ws.classList.contains("maxlock"))returnit;"#),
        "reveal() must pan the plane while maximized — Go-to is the path (#338)"
    );

    let css = served_css();
    let foot = css
        .split_once("\n.canvas-foot {")
        .expect("styles.css must keep the .canvas-foot rule (#338)")
        .1;
    let foot = &foot[..foot.find('}').expect("the .canvas-foot rule must close")];
    for pin in ["pointer-events: none", "z-index: 130"] {
        assert!(
            foot.contains(pin),
            "the .canvas-foot rule must keep {pin} — above the consoles, inert to the pointer (#338)"
        );
    }
}

/// Per-console fullscreen, and the tablet's way out of it.
///
/// Everything here is a SAFETY pin, not a feature pin. Fullscreen puts the
/// window in the top layer, where the browser sizes it to the display and
/// outranks every author rule — so the two places that read a window's box
/// (the stage extent and the desk record) must read the INLINE rect instead,
/// or a reload brings the console back the size of a monitor. And the
/// control's look must be derived from the browser's own event, because on a
/// tablet there is no Esc: a stale "exit" icon over a window that already
/// left fullscreen is the operator's only exit, pointing at nothing.
///
/// `fullscreenOffered` is driven by the node suite; the rest is DOM wiring
/// and CSS that no node test runs, so it fails here or nowhere.
#[test]
fn a_console_can_take_the_whole_screen() {
    // The console, its window chrome and its view, as one text: the full-screen state lives
    // in the console, its button and the gesture guards in the chrome, the registration of
    // the browser's event in the view.
    let js = &[
        include_str!("../assets/ui/wb-console.ts"),
        include_str!("../assets/ui/wb-stage-window.ts"),
        include_str!("../assets/ui/wb-stage-chrome.ts"),
        include_str!("../assets/ui/wb-stage-view.ts"),
    ]
    .concat();
    for pin in [
        "function toggleFull(",
        "function syncFullState(",
        "function isFull(",
        // The registration, not the function: without it nothing re-derives
        // the control after an Esc, a system swipe, or the browser dropping
        // fullscreen on its own.
        r#"document.addEventListener("fullscreenchange", syncFullState)"#,
        // Built only where the browser can HOLD it, which is two
        // questions, not one: is the API there at all, and is this the
        // engine that hands fullscreen back the moment the keyboard
        // rises. A control the next tap cancels is worse than no control.
        "fullBtn.hidden = !fullscreenOffered(document.fullscreenEnabled, navigator.vendor)",
        "win.requestFullscreen()",
        "document.exitFullscreen()",
    ] {
        assert!(
            js.contains(pin),
            "the console must keep the fullscreen pin {pin}"
        );
    }
    // The guards that keep the inline rect honest while the top layer owns
    // the geometry: the rect read skips a fullscreen window, and the two
    // gestures refuse one. Each is found in its own function, in any layout.
    let fn_body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("the console must keep {name}"))
            .1;
        squeeze(&after[..after.find("\n  }").expect("the function must close")])
    };
    assert!(
        fn_body("function restoreRect(").contains("!isFull(win)"),
        "restoreRect must not read a fullscreen window's box"
    );
    for gesture in ["function makeDraggable(", "function startResize("] {
        assert!(
            fn_body(gesture).contains("isFull(win)"),
            "{gesture} must refuse a fullscreen window"
        );
    }
    // The click handler must NOT paint the icon: that is `syncFullState`'s
    // single job, and a second writer is exactly how the stale-exit bug
    // comes back. `toggleFull` is bounded by the next function comment.
    let body = js
        .split_once("function toggleFull(")
        .expect("toggleFull must exist")
        .1;
    let body = &body[..body
        .find("\n  // The fullscreen control's LOOK")
        .unwrap_or(body.len())];
    assert!(
        !body.contains("bi-fullscreen-exit"),
        "toggleFull must not write the control's icon — syncFullState derives it"
    );

    let css = served_css();
    let rule = |head: &str| -> String {
        let after = css
            .split_once(head)
            .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
            .1;
        after[..after.find('}').expect("the rule must close")].to_string()
    };
    // The touch target IS the exit on a tablet. 44px is Apple's HIG floor;
    // the 22px resting chrome is a mouse-only size.
    assert!(
        rule("\n.session-window.fullscreen .session-actions button {").contains("width: 44px"),
        "the fullscreen titlebar must offer a 44px touch target — a tablet has no Esc"
    );
    // The home indicator overlays the bottom of a fullscreen element.
    assert!(
        rule("\n.session-window:fullscreen {").contains("env(safe-area-inset-bottom"),
        "a fullscreen console must inset for the home indicator"
    );

    // The iPad's OTHER answer, which needs no fullscreen state at all:
    // installed to the home screen the shell runs chrome-less for good.
    let html = include_str!("../assets/ui/index.html");
    for pin in [
        r#"<link rel="manifest" href="manifest.webmanifest" />"#,
        r#"<link rel="apple-touch-icon" href="icon-192.png" />"#,
        r#"<meta name="apple-mobile-web-app-capable" content="yes" />"#,
    ] {
        assert!(
            html.contains(pin),
            "index.html must carry the install pin {pin}"
        );
    }
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../assets/ui/manifest.webmanifest"))
            .expect("the manifest must be valid JSON — a broken one is ignored silently");
    assert_eq!(
        manifest["display"], "standalone",
        "the installed shell must launch without browser chrome"
    );
    // Served as octet-stream the manifest is ignored by every browser, and
    // the failure is silent: the shell renders, the install never offers.
    assert_eq!(
        content_type("manifest.webmanifest"),
        "application/manifest+json"
    );
    for icon in ["icon-192.png", "icon-512.png"] {
        assert!(
            UI.get_file(icon).is_some(),
            "{icon} must be embedded — the manifest and apple-touch-icon both name it"
        );
    }
}

/// The per-client view (#339) and — the part that can silently regress — the
/// ONE place allowed to name `localStorage`. ADR-0050 §3 dropped the browser
/// desk store; ADR-0051 §8 narrows that to "no *desk* in browser storage",
/// which is only honest while the view store stays a single module holding a
/// single key. Same bargain as the pins above — and this one is a TREE-WIDE
/// negative, which no per-module `node --test` file can state — so this is
/// the gate that notices the desk creeping back into the browser.
#[test]
fn shell_stores_only_the_view_in_the_browser() {
    // The console family and the shell own the state being persisted, and
    // must reach it only through `WBView` — a direct write from any of them
    // is how a second store starts.
    for (name, src) in [CONSOLE_FAMILY, SHELL_FAMILY].concat() {
        assert!(
            !src.contains("localStorage"),
            "{name} must not touch localStorage — wb-client-view.ts owns the store (#339)"
        );
    }

    let view = include_str!("../assets/ui/wb-client-view.ts");
    // NEGATIVE CONTROL: deleting the store wholesale would satisfy every
    // "does not contain" assertion above. It must be red, not green.
    assert!(
        view.contains("localStorage"),
        "wb-client-view.ts IS the browser store — deleting it is not how #339 stays green"
    );
    assert!(
        view.contains(r#"const KEY = "wb.view.v1""#),
        "wb-client-view.ts must keep the one view key (#339)"
    );

    let js = include_str!("../assets/ui/wb-stage-view.ts");
    // `viewLanding` is driven by `ui-tests/wb-geometry.test.mjs`, the landing
    // by `ui-tests/wb-stage-view.test.mjs`.
    for pin in [
        "function applyLanding(",
        // The REGISTRATION, not the function: without it the offset is never
        // persisted and the landing has nothing to restore.
        r#"ws.addEventListener("scroll", saveOffset)"#,
    ] {
        assert!(
            js.contains(pin),
            "wb-stage-view.ts must keep the #339 pin {pin}"
        );
    }

    // Tree-wide, not just the two modules above: any non-vendor asset that
    // starts naming `localStorage` is a second store by definition. `.html`
    // is swept too, so a store cannot come back as an inline `<script>`
    // block without touching a single `.js`.
    for path in embedded_ui_paths() {
        let scanned = path.ends_with(".js") || path.ends_with(".html");
        if !scanned || path.starts_with("vendor/") || path == "wb-client-view.js" {
            continue;
        }
        let src = UI
            .get_file(&path)
            .and_then(|f| f.contents_utf8())
            .unwrap_or_else(|| panic!("{path} must be embedded as UTF-8"));
        assert!(
            !src.contains("localStorage"),
            "{path} must not touch localStorage — wb-client-view.js is the only store (#339)"
        );
    }
}

/// Silence is not death. A browser throttles a hidden tab's timers to one
/// tick per minute after about five minutes, so the six-second heartbeat
/// window closed a working popup and pulled its consoles home mid-work. Both
/// documents now demand a better witness before acting — a window handle,
/// or a probe that message delivery (which is not timer-throttled) still
/// answers. `wb_fence_347.py` scenario 5c drives it; this is CI's view.
#[test]
fn a_quiet_detach_peer_is_challenged_before_it_is_buried() {
    // Whitespace-free text: the code, not its layout. The opener's side of
    // the detach is `wb-desk-detach.ts`.
    let js = squeeze(include_str!("../assets/ui/wb-desk-detach.ts"));
    assert!(
        js.contains("functionstillThere("),
        "the origin must ask whether a quiet popup is really gone"
    );
    assert!(
        js.contains("&&!stillThere("),
        "the re-attach must be gated on that answer, not on the fold alone"
    );
    assert!(
        js.contains(r#"m.type==="popup-ping""#) && js.contains(r#"type:"origin-here""#),
        "the origin must answer a probe from its MESSAGE handler, not a timer"
    );
    let popup = squeeze(include_str!("../assets/ui/wb-detached-fence.ts"));
    assert!(
        popup.contains("constopenerGone=()=>opener()?.closed??true;"),
        "the popup's verdict is the opener HANDLE, which owes nothing to a timer"
    );
    assert!(
        popup.contains(r#"==="peer-lost"))silent();"#),
        "a lost peer must reach `silent`, which probes, and never `lost` directly"
    );
}

/// Three pieces of chrome that only a browser can really prove, pinned here
/// because the browser pass — Playwright — does not run in CI.
#[test]
fn the_console_chrome_holds_its_three_rules() {
    let app = shell_family();
    let tab = include_str!("../assets/ui/wb-consoles-tab.ts");
    let html = include_str!("../assets/ui/index.html");
    // ONE dropdown at a time. Every toggler sends ONE event,
    // `workbench:menus-close`, and the shell and the two consoles components
    // each close their own menus on it — the account menu and the toolbar's
    // pickers used to enumerate each other and left both open, overlapping.
    // wb-consoles-tab.test.mjs and app.test.mjs drive the event.
    let close = r#"sendWindow(window, "workbench:menus-close");"#;
    let hear = r#"addEventListener("workbench:menus-close", "#;
    // Counted, not `contains`: the family also holds the two consoles
    // components, so a bare `contains` stays green when the shell's own send
    // or hear is gone. Four is the shell's one, their two, and the + menu's.
    assert!(
        app.matches(close).count() == 4
            && app.matches(hear).count() == 4
            && app.contains("this.avatarMenu = false;"),
        "the shell must close the account menu on the one menus event, and send it"
    );
    // The account menu's send and hear are the third of each above.
    let account = include_str!("../assets/ui/wb-account.ts");
    assert!(
        account.matches(close).count() == 1
            && account.matches(hear).count() == 1
            && account.contains("this.avatarMenu = false;"),
        "the account menu (wb-account.ts) must close on the one menus event, and send it"
    );
    assert!(
        tab.matches(close).count() == 2 && tab.matches(hear).count() == 2,
        "both consoles components must send the one menus event and hear it"
    );
    for pin in ["toggleAvatarMenu()", "toggleAgentMenu()"] {
        assert!(html.contains(pin), "index.html must toggle through {pin}");
    }
    assert!(
        !html.contains("avatarMenu = !avatarMenu"),
        "the account button must not toggle its own flag past the others"
    );
    // One picture for one thing: the Consoles tab, the New-console button and
    // the rows in its menu all wear the same terminal glyph.
    assert!(
        app.contains(r#"icon: "bi bi-terminal""#) && !app.contains("bi-robot"),
        "the Consoles tab must wear the terminal glyph, not a robot"
    );
    // A fence name is read-only until asked for twice: its title bar is also
    // what the operator clicks to reach the fence, and an always-live input
    // turned every such slip into a rename.
    let js = include_str!("../assets/ui/wb-stage-fences.ts");
    assert!(
        js.contains("name.readOnly = true;") && js.contains(r#"name.addEventListener("dblclick""#),
        "the fence name must open on a double click and close on blur"
    );
}

/// The three clicks that ask first. Tiling a fence moves every console in
/// it, removing a fence takes the region out from under them, and a
/// console's × ends a live session — all one pixel from something harmless
/// on the same title bar. The dialog is built in `wb-console.ts` rather
/// than borrowed from the shell's Alpine one, because the module also runs
/// in the detached-fence popup, which has neither; `window.confirm` is
/// pinned OUT because an automated browser dismisses it by default, which
/// would turn every guarded click into a silently cancelled one.
#[test]
fn the_destructive_console_clicks_confirm_first() {
    // The whole console family and the door for operator messages, as one
    // text: the console's ×, the fence's tile and remove buttons, the remove
    // verb and the two restarts live in the family, and the dialog in the
    // door. The exact counts below hold over every family module.
    let js = &(console_family() + include_str!("../assets/ui/wb-messages.ts"));
    assert!(
        js.contains("function askConfirm({"),
        "wb-messages.ts must own a confirmation dialog of its own"
    );
    assert!(
        !js.contains("window.confirm("),
        "the native confirm is not the seam — an automated browser dismisses it"
    );
    // Each call site acts on the answer: the statement right after the
    // `askConfirm({ … });` call is the one that reads `ok`. A site that
    // ignored the answer would still show the dialog.
    for (title, sites, then) in [
        (
            r#"title: "Tile this fence?""#,
            1,
            "if (ok) arrangeFence(f.id);",
        ),
        (
            r#"title: "Remove this fence?""#,
            1,
            "if (ok) removeFence(f.id);",
        ),
        ("title: `Restart in ${", 1, "if (!ok) return;"),
        (r#"title: "Restart session?""#, 1, "if (!ok) return;"),
        // The live console's ×, and the dormant one's.
        (r#"title: "Close this console?""#, 2, "if (!ok) return;"),
    ] {
        let found: Vec<&str> = js
            .match_indices(title)
            .map(|(at, _)| {
                assert!(
                    js[..at].trim_end().ends_with("askConfirm({"),
                    "{title} must be the first key of an askConfirm call"
                );
                let rest = &js[at..];
                let close = rest.find("});").expect("the askConfirm call must close");
                rest[close + 3..]
                    .trim_start()
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
            })
            .collect();
        assert_eq!(
            found,
            vec![then; sites],
            "each {title} dialog must be followed by {then}"
        );
    }
    // The EXPORTED verbs stay unguarded: a caller that names `arrangeFence`
    // has already decided, and the dialog belongs to the accidental click.
    let verb = js
        .split_once("\n  function removeFence(id: string) {")
        .expect("wb-console.ts must keep removeFence")
        .1;
    assert!(
        !verb[..verb.find("\n  }").expect("removeFence must close")].contains("askConfirm"),
        "the verb must not ask — only the button does"
    );
    // The dialog wears the shell's own modal classes, so the popup (which
    // loads the same stylesheet and no Alpine) shows the same dialog.
    assert!(
        js.contains(r#"scrim.className = "modal-scrim wb-confirm""#),
        "the dialog must reuse the shared modal chrome"
    );
}

/// The standing authorization to relaunch agent consoles on load. It is a
/// spending decision — one vendor CLI per saved agent console, on every page
/// load — so what this pins is that it stays OFF unless the operator turned
/// it on, and that it stays in the BROWSER: daemon-wide, every tab pointed
/// at the same desk would relaunch the same consoles and spend the quota
/// once per tab. Playwright proves the fold (`wb_desk_303.py` scenario 2);
/// this is the gate CI can see.
#[test]
fn relaunching_agent_consoles_on_load_is_opt_in() {
    // Every pin reads whitespace-free text: the code, not its layout. The
    // restore is in the console's desk restore.
    let js = squeeze(include_str!("../assets/ui/wb-desk-restore.ts"));
    let folds = squeeze(include_str!("../assets/ui/wb-desk-folds.ts"));
    assert!(
        folds.contains(r#"record.kind==="console"||relaunchAgents?"relaunch":"placeholder""#),
        "the restore fold must relaunch an agent console ONLY under the opt-in"
    );
    // The DEFAULT is the whole guard: a caller that omits the option — the
    // fold's own tests, a later call site — must get the parked placeholder.
    assert!(
        folds.contains("relaunchAgents=false}"),
        "an omitted `relaunchAgents` must default to false, never to launching"
    );
    // The relaunch verdict must ask for the record's OWN kind. `{ console:
    // true }` is the shell request, and reaching an agent record with it —
    // which the opt-in made possible — opened a plain shell in the agent's
    // box (measured in `wb_desk_303.py` scenario 10 before this line).
    // Since #411 the request is `relaunchRequest`'s, which also carries
    // the worktree the record was in; the fold is pinned by
    // `ui-tests/wb-console-session.test.mjs`, and what stays here is that the
    // restore path goes through it and nothing else.
    let session = squeeze(include_str!("../assets/ui/wb-console-session.ts"));
    assert!(
        session.contains(
            r#"if(record.kind!=="agent")return{console:true,repo,command:consoleCommand(record.agent)};"#
        ),
        "a relaunched agent console must be requested by its vendor, not as a shell"
    );
    assert!(
        js.contains("spawnOrMissing(relaunchRequest(record),record.agent,record.repo,record)"),
        "the restore fold's relaunch must go through relaunchRequest"
    );
    // The popup holds a fragment of the plane and authors no session; its
    // injected `viewStore` reads nothing, and `canLaunch` refuses besides.
    assert!(
        js.contains("OPTS.canLaunch!==false&&viewStore?.read()?.relaunch===true"),
        "the opt-in must be read through the injected view store, and never in the popup"
    );

    // The knob, and the store it writes to. `config.set` would put a
    // per-browser choice in a repo's settings.json for every client to obey.
    let settings = squeeze(include_str!("../assets/ui/wb-settings.ts"));
    for pin in [
        r#"scope:"client""#,
        r#"key:"consoles.relaunch_on_load""#,
        r#"type:"toggle""#,
    ] {
        assert!(
            settings.contains(pin),
            "wb-settings.ts must keep the pin {pin}"
        );
    }
    let app = squeeze(include_str!("../assets/ui/wb-settings-dialog.ts"));
    assert!(
        app.contains("window.WBView.patch({relaunch:value===true})"),
        "wb-settings-dialog.ts must persist the toggle through the view store"
    );
    assert!(
        app.contains("if(this.CLIENT_KEYS.has(key)){"),
        "a client-scoped key must return before the config.set path"
    );
    let html = squeeze(include_str!("../assets/ui/index.html"));
    assert!(
        html.contains(r#"it.type==='toggle'"#),
        "index.html must render the toggle control"
    );
}

/// A project-scoped key in the settings schema is an INTENT ON THE CLI: the
/// panel persists it with `config.set`, the daemon relays that to
/// `ralphy config set` verbatim (ADR-0036 — the daemon shape-checks the key
/// and keeps no value allowlist of its own), and the CLI refuses anything
/// outside `SUPPORTED_KEYS`. So a key the UI offers and the CLI does not
/// know is a control that cannot be operated: it renders, it accepts a
/// click, and the save comes back refused.
///
/// Nothing else notices. The daemon may not depend on `ralphy-cli` (the
/// arrow points inward), so the two lists are joined the only way that seam
/// allows — by reading the CLI's source as text, the precedent
/// `session.rs` sets for the adapters' settings schemas.
///
/// This gate found three such controls when it was written: a Schedule
/// section backed by the `ralphy schedule` SUBCOMMAND rather than by any
/// persisted key, and an "Eligible labels" field backed by nothing at all —
/// no `SUPPORTED_KEYS` entry, no field in `ralphy-core`'s `Settings`. All
/// three are gone from the schema; this is what keeps the fourth out.
#[test]
fn every_settable_key_the_panel_offers_is_a_key_the_cli_accepts() {
    let schema = include_str!("../assets/ui/wb-settings.ts");
    let cli = include_str!("../../ralphy-cli/src/config.rs");
    let supported = cli
        .split_once("const SUPPORTED_KEYS: &[&str] = &[")
        .expect("the CLI renamed the key registry the panel is written against")
        .1
        .split_once("];")
        .expect("unterminated SUPPORTED_KEYS")
        .0;

    // A section's `scope` precedes its `items`, so one linear pass over the
    // two literals attributes every key to the section it was declared in,
    // and each item's own text carries its `readonly` flag.
    let mut checked = 0;
    for (i, _) in schema.match_indices("scope: \"") {
        let rest = &schema[i + "scope: \"".len()..];
        let scope = &rest[..rest.find('"').expect("unterminated scope")];
        // Everything up to the NEXT section's scope belongs to this one.
        let section = match rest.find("scope: \"") {
            Some(end) => &rest[..end],
            None => rest,
        };
        if scope != "project" {
            continue;
        }
        for (j, _) in section.match_indices("key: \"") {
            let tail = &section[j + "key: \"".len()..];
            let key = &tail[..tail.find('"').expect("unterminated key")];
            // The item runs to the next key, or to the end of the section.
            let item = match tail.find("key: \"") {
                Some(end) => &tail[..end],
                None => tail,
            };
            assert!(
                supported.contains(&format!("\"{key}\"")),
                "the panel offers {key} but `ralphy config set` refuses it — \
                 a control that renders, takes an edit and answers 'refused'. \
                 Add the key to SUPPORTED_KEYS, or take the item out of the schema."
            );
            // The other half: a key the CLI knows but the DAEMON denies is
            // just as inert from a browser, and the schema is where that has
            // to be admitted.
            assert_eq!(
                dispatch::LOCAL_ONLY_KEYS.contains(&key),
                item.contains("readonly: true"),
                "{key}: a key denied at the daemon boundary must be declared \
                 `readonly: true`, and only such a key may be"
            );
            checked += 1;
        }
    }
    // The daemon-scope sections carry keys the CLI never sees (`daemon.*`,
    // `telegram.*`), so they are not cross-checked above — but a LOCAL-ONLY
    // key offered there (`events.token`, audit F12) must still be declared
    // `readonly`, wherever it sits.
    for key in dispatch::LOCAL_ONLY_KEYS {
        let Some(at) = schema.find(&format!("key: \"{key}\"")) else {
            continue;
        };
        let tail = &schema[at..];
        let item = match tail[1..].find("key: \"") {
            Some(end) => &tail[..end],
            None => tail,
        };
        assert!(
            item.contains("readonly: true"),
            "{key} is denied at the daemon boundary but the panel offers it editable"
        );
    }
    // The declaration is worth nothing if the markup ignores it: an
    // `it.readonly` the input never reads is a field that still takes an
    // edit and still comes back refused.
    // One binding per control a readonly key can use: text, password, toggle.
    assert_eq!(
        include_str!("../assets/ui/index.html")
            .matches(r#":disabled="it.readonly === true""#)
            .count(),
        3,
        "index.html must disable the text, password and toggle control a readonly item declares"
    );
    // Non-vacuous: a scan that stopped recognizing the schema's shape would
    // otherwise pass by checking nothing at all.
    assert!(
        checked > 10,
        "only {checked} project keys were cross-checked — the scan stopped seeing the schema"
    );
}

/// The run picker answers "what is running?" with the MODEL, and "for how
/// long?" with a clock counting from the document's own phase anchor.
///
/// Both facts already existed on the wire and the panel was discarding them:
/// `fromSnapshot` dropped `model`/`effort`/`budget_min`, and the title bound
/// the vendor. A vendor name does not say which of sol/terra/luna is burning
/// quota, and nothing said whether a phase was two minutes or forty in.
#[test]
fn the_run_picker_names_the_model_and_clocks_the_phase() {
    // The folds behind it (`fromSnapshot`, `modelEffort`, `fmtClock`,
    // `phaseClock`, `runTitle`) are driven by `ui-tests/wb-runs.test.mjs`.
    let shell = include_str!("../assets/ui/index.html");
    for pin in [
        r#"<span class="run-select-title" x-text="runTitle($store.runs.currentRun())"></span>"#,
        r#"<span class="run-sub-phase" x-text="runIdentity($store.runs.currentRun())"></span>"#,
        r#"<span class="run-clock" x-show="runClock($store.runs.currentRun())""#,
    ] {
        assert!(
            shell.contains(pin),
            "index.html must keep the picker pin {pin}"
        );
    }
    // The vendor MOVED, it was not deleted: `runIdentity` is what carries it,
    // and the negative pin is what stops the title reverting to it.
    assert!(
        !shell.contains(r#"x-text="$store.runs.currentRun()?.agent""#),
        "the picker's headline must be the model, not the vendor"
    );
    assert!(
        !shell.contains(r#"x-text="currentRun()?.agent""#),
        "the picker's headline must be the model, not the vendor"
    );

    let app_js = shell_family();
    let app: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
    // The tick is what makes the clock live, and reading `nowMs` in the getter
    // is what subscribes the binding to it — a `Date.now()` inside `runClock`
    // would leave the clock frozen with every pin above still green.
    assert!(
        app.contains("return WBRun.phaseClock(run, this.nowMs);"),
        "runClock must read the reactive `nowMs`, not the clock directly"
    );
    // The GUARD and the period, not the whole interval body: the tick is a
    // shared seam (presence staleness rides it too), and quoting every
    // statement in it made this run-panel test fail for a change that had
    // nothing to do with the run panel.
    assert!(
        app.contains("this._clockTick = setInterval(() => { if (this.runsOpen)"),
        "the shell must own a one-second tick for the phase clock"
    );
    assert!(
        app.contains("if (this.runsOpen) this.nowMs = Date.now();"),
        "the phase clock must advance only while the panel is open"
    );
    // The period of the shell's own tick, the one that holds the phase
    // guard: the store `connection` has a one-second presence tick of its
    // own, which the family also holds.
    let phase_tick = app
        .split("this._clockTick = setInterval(")
        .find(|tick| tick.starts_with("() => { if (this.runsOpen)"))
        .unwrap_or_default();
    assert!(
        phase_tick
            .find("}, ")
            .is_some_and(|end| phase_tick[end..].starts_with("}, 1000);")),
        "the phase clock must tick once a second"
    );
}

/// The design-system scrollbar is the DEFAULT, not a list of opted-in
/// selectors. It was the latter for four rounds, and every round shipped one
/// more surface wearing the platform's chrome bar — the operator reported the
/// same defect three times. This pin protects the inversion, so the next
/// scrolling surface is born correct instead of born reported.
///
/// It also protects two MEASURED facts that are easy to "tidy" back into a
/// bug (the browser-side numbers live in `tests/wb_scrollbars.py`):
///   * setting `scrollbar-color` makes the browser IGNORE that element's
///     `::-webkit-scrollbar` rules and fall back to the platform width, so the
///     two mechanisms cannot be combined — every such block this file used to
///     carry was dead code drawing a platform-width bar in ralphy's colours;
///   * `scrollbar-color` inherits but `scrollbar-width` DOES NOT, so the rule
///     must be `*` and not `html`, or the width silently stays `auto`.
#[test]
fn the_design_system_scrollbar_is_the_default_not_a_list() {
    let css = served_css();
    let squeezed: String = css.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        squeezed
            .contains("* { scrollbar-width: thin; scrollbar-color: var(--border) transparent; }"),
        "the default must be `*` — `scrollbar-width` does not inherit"
    );
    assert!(
        !squeezed.contains("html { scrollbar-width"),
        "a root rule themes the colour and leaves the width `auto` — measured"
    );
    // THE CONTRACT, as a negative, and the reason the mechanisms must not be
    // mixed: NO `::-webkit-scrollbar` RULE may return to this file. Pinning the
    // covered surfaces by name would instead be satisfied by re-adding the very
    // list being removed.
    //
    // Comments are stripped first, because a comment is exactly where this
    // pseudo-element SHOULD still be named — the explanation above the default
    // names it three times, and a pin that forbade the words would forbid the
    // reasoning along with the code.
    let mut code = String::with_capacity(css.len());
    let mut rest = css.as_str();
    while let Some(start) = rest.find("/*") {
        code.push_str(&rest[..start]);
        rest = match rest[start + 2..].find("*/") {
            Some(end) => &rest[start + 2 + end + 2..],
            None => "",
        };
    }
    code.push_str(rest);
    assert!(
        !code.contains("::-webkit-scrollbar"),
        "a ::-webkit-scrollbar rule is inert beside `scrollbar-color` — do not re-add one"
    );
    // Monaco is the documented exception: it paints its own slider, so it is
    // the one surface no scrollbar property reaches.
    assert!(
        css.contains(".monaco-scrollable-element > .scrollbar > .slider {"),
        "Monaco's own slider must keep its palette rule"
    );
}

/// The board's label editor: not clipped, and not inviting a refused click.
///
/// Two independent defects. The menu was a `.dropdown` (absolutely
/// positioned) inside `.kd-inner`'s scroll inside `.kanban-detail`'s
/// `overflow: hidden`, so it was clipped on every card past the middle of the
/// drawer. And `label.set` is a run-lock-aware Mutate (`mutate.rs`'s
/// `guard_run_lock(&ws, "label set", …)`), so with a live run every toggle was
/// refused, the optimistic chip snapped back, and the editor was the ONE write
/// control in the shell with no gate.
#[test]
fn the_label_editor_is_unclipped_and_closed_under_a_live_run() {
    let shell = include_str!("../assets/ui/index.html");
    // The clip fix is structural: it must stop being a floating dropdown.
    assert!(
        shell.contains(r#"<div class="kd-label-menu" x-show="labelMenuOpen""#),
        "the label menu must be in flow, not a `.dropdown`"
    );
    assert!(
        !shell.contains("dropdown kd-label-menu"),
        "a `.dropdown` label menu is clipped by the drawer — that is the defect"
    );
    let css = served_css();
    let squeezed: String = css.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        squeezed.contains(".kd-label-menu { flex-basis: 100%;"),
        "the label menu must claim its own row inside the wrapping .kd-labels"
    );
    // The gate, on the opener AND on every row: a menu that opens onto sixteen
    // live-looking options is the same invitation one click deeper.
    for pin in [
        r#":disabled="labelsLocked()""#,
        r#":title="labelLockReason() || 'Add or remove labels on this issue'""#,
    ] {
        assert!(
            shell.contains(pin),
            "index.html must keep the label gate {pin}"
        );
    }
    assert_eq!(
        shell.matches(r#":disabled="labelsLocked()""#).count(),
        2,
        "both the edit button and the option rows must be gated"
    );

    let app_js = shell_family();
    // ONE predicate, two subjects — the drift #318 avoided. A second
    // "does this repo have a live run" test is how the gate and the controls
    // beside it start disagreeing. The sentence itself is driven by
    // `ui-tests/wb-changes.test.mjs`.
    assert!(
        js_method_body(&app_js, "labelLockReason() {").contains("WBChanges.writeLockReason("),
        "the label reason must reuse writeLockReason, not parallel it"
    );
    assert!(
        squeeze(&app_js).contains("if(this.labelsLocked())return;"),
        "toggleLabel must refuse behind the disabled rows too"
    );
}

/// Stopping a run — the one control in the Runs panel that throws away work
/// in progress (ADR-0054, ADR-0032 §6) — asks through the shell's own dialog.
/// `window.confirm` names the origin, ignores the theme and blocks the page,
/// which is the wrong furniture for the panel's most consequential click.
#[test]
fn stopping_a_run_confirms_through_the_design_system_dialog() {
    let code = squeeze(&shell_family());
    let start = code
        .find("asyncstopRun(")
        .expect("the shell must keep stopRun");
    let body = &code[start..];
    let body = &body[..body[1..].find("async").map_or(body.len(), |i| i + 1)];
    assert!(
        body.contains(r#"constok=awaitthis.$store.modals.askConfirm({title:"Stopthisrun?","#),
        "stopRun must confirm through askConfirm: {body}"
    );
    assert!(
        !body.contains("window.confirm("),
        "stopRun must not confirm through window.confirm"
    );
}

/// The board can see, read and throw away the plan that the NEXT RUN will
/// execute. Same CI bargain as the pins around it; the structural half of
/// the slice, since Playwright — which renders it — does not run in CI.
#[test]
fn the_board_surfaces_the_plan_the_next_run_would_execute() {
    // The plan folds (`planSummary`, the pill, `isBundleReason`, and the
    // zero-open-steps verdict that mirrors `plan::count_open_steps`) are
    // driven by `ui-tests/wb-runs.test.mjs`.
    let app_js = shell_family();
    for pin in [
        r#"path: ".ralphy/plan.md","#,
        r#"window.WBDaemon.write("plan.discard", { repo: slug })"#,
    ] {
        assert!(app_js.contains(pin), "the shell must keep {pin}");
    }
    // The discard is confirmed, and the plan is only ever shown against the
    // issue its trailer names (`planFor`) — a plan offered on the wrong card
    // would invite a discard of the wrong work.
    let app_squeezed: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        app_squeezed.contains(
            r#"const ok = await this.$store.modals.askConfirm({ title: "Discard this plan?","#
        ),
        "discardPlan must confirm first"
    );
    assert!(
        app_squeezed.contains("return held && held.summary.issue === number ? held : null;"),
        "a plan is shown against the issue it names, and no other"
    );

    let html = include_str!("../assets/ui/index.html");
    for pin in [
        r#"class="kc-plan""#,
        r#"class="kanban-plan-chip""#,
        r#"class="modal plan-modal""#,
        r#"data-act="plan-discard""#,
    ] {
        assert!(html.contains(pin), "index.html must keep {pin}");
    }
    // Gated while a run holds the repo: a run owns the plan it is executing.
    let discard = html
        .find(r#"data-act="plan-discard""#)
        .expect("the discard control must exist");
    assert!(
        html[discard..discard + 220].contains(":disabled=\"$store.connection.writeLocked()\""),
        "the discard must be gated while a run holds the repo"
    );
    // A dialog a MODAL asks for must sit above it. Every `.modal-scrim` shares
    // `z-index: 500`, so the winner was DOM order — the plan modal's scrim
    // covered the confirm it had just raised, leaving Escape as the only
    // reachable control. Measured with Playwright, which named the plan scrim
    // as the interceptor; the fix raises the ASKED-FOR dialog rather than
    // reordering the markup, so it cannot regress by a paste in the wrong spot.
    let css = served_css();
    assert!(
        css.contains(".modal-scrim:has(> .confirm-modal),")
            && css.contains(".modal-scrim:has(> .prompt-modal)"),
        "the confirm/prompt dialogs must outrank the modal that raised them"
    );
}

/// The plan blocks' chrome: the note explains the list from ABOVE it, and the
/// section picker looks like the dropdown it is. Structural, not cosmetic —
/// both defects are invisible to every other test in this file.
#[test]
fn the_plan_blocks_explain_themselves_before_the_space_they_describe() {
    let html = include_str!("../assets/ui/index.html");
    let note = html
        .find(r#"class="plan-steps-note""#)
        .expect("index.html must keep the steps note");
    let list = html
        .find(r#"<ul class="plan-steps">"#)
        .expect("index.html must keep the steps list");
    assert!(
        note < list,
        "the steps note must precede the list it explains — under an empty \
         list it sits at the bottom of the block, away from the space it is about"
    );
    // `all: unset` on `.plan-picker` removes the native arrow, so the caret is
    // the only thing saying the head opens. Its inertness is the other half:
    // a caret that swallows the click advertises an act it then prevents.
    assert!(
        html.contains(r#"class="plan-picker-caret" x-icon="'chevron-down'""#),
        "the section picker must carry a caret (`all: unset` drops the native one)"
    );
    let css = served_css();
    let caret = css
        .find(".plan-picker-caret {")
        .expect("styles.css must style the picker caret");
    assert!(
        css[caret..caret + 200].contains("pointer-events: none"),
        "the caret must be inert to the pointer so the click reaches the select"
    );
}

/// The runs panel's chrome (#331). The suite CI runs calls functions and
/// never renders markup, and Playwright does not run there — so these
/// substrings are the only CI-visible gate over this markup, the same
/// bargain #318/#319 struck for the write controls.
#[test]
fn the_runs_feed_is_contained_in_the_markup() {
    let html = include_str!("../assets/ui/index.html");
    for pin in [
        r#"class="runs-feed""#,
        r#"data-act="feed-collapse""#,
        r#"data-act="feed-dismiss""#,
        r#"x-show="rawFeedOpen""#,
        r#"class="runs-verb-error""#,
        "verbLocked()",
        "verbTitle('triage')",
    ] {
        assert!(
            html.contains(pin),
            "index.html must keep the #331 pin {pin}"
        );
    }
    // The NEGATED pin, written STRUCTURALLY rather than as one spelling of
    // the old tag: `<pre x-show="rawFeed" class="runs-raw">` is the same
    // defect with the attributes swapped. The invariant is that the feed
    // occurs exactly once and is INSIDE the sized box, so the box's class
    // must appear before it.
    assert_eq!(
        html.matches("runs-raw").count(),
        1,
        "the raw feed must occur exactly once in index.html (#331)"
    );
    let feed_box = html
        .find(r#"class="runs-feed""#)
        .expect("index.html must keep the .runs-feed box");
    let raw = html
        .find("runs-raw")
        .expect("index.html must keep the raw feed");
    assert!(
        feed_box < raw,
        "the raw feed must stay INSIDE its sized .runs-feed box (#331)"
    );

    // The toolbar states the run lock in each disabled control's `title` and
    // NOWHERE else: the visible note was removed on the operator's own
    // request, so its absence is the pin. Negated, because a reflex to
    // "explain the dimmed button on screen" is exactly what would put it back.
    assert!(
        !html.contains("runs-lock-note"),
        "the runs toolbar carries no standing lock message — the reason rides \
         each disabled verb's title (`verbTitle`)"
    );

    let app_js = shell_family();
    // `rawFeedOpen: false` is the DEFAULT, not an incidental initialiser: the
    // feed can take 30vh of a panel whose job is the trail and the plan, so
    // the bytes are opt-in and only the head arrives with the output.
    for pin in ["dismissFeed()", "runVerbFailed(", "rawFeedOpen: false"] {
        assert!(
            app_js.contains(pin),
            "the shell must keep the #331 pin {pin}"
        );
    }
    assert!(
        !app_js.contains("rawFeedOpen = true"),
        "no reset may re-open the feed: dismiss and every verb click return it \
         to the collapsed default"
    );
    // The gate REUSES the Changes derivation rather than paralleling it —
    // that reuse is the acceptance criterion, so it is pinned. Whitespace
    // is collapsed first so the pin judges the CODE and not its layout: it
    // must survive a reformat and a CRLF checkout (this host holds LF in
    // the blob and CRLF on disk) without blaming a design rule for either.
    let squeezed: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        squeezed.contains("verbLocked() { return this.writeLocked(); }"),
        "verbLocked() must be literally writeLocked(), not a second predicate (#331)"
    );
    // `verbLockTitle` and `exitNote` are driven by `ui-tests/wb-runs.test.mjs`.
    // Both terminal frames, the refusal and the exit, go to the panel.
    assert_eq!(
        include_str!("../assets/ui/wb-daemon.ts")
            .matches(r#"sendWindow(window, "workbench:run-verb-failed", { message: "#)
            .count(),
        2,
        "wb-daemon.ts must route a terminal verb frame to the panel (#331)"
    );
    assert!(
        app_js.contains(
            r#"addEventListener("workbench:run-verb-failed", (e) => this.runVerbFailed(e.detail.message));"#
        ),
        "the Runs panel must hear a terminal verb frame (#331)"
    );
}

/// A refusal of an act dispatched from the Changes panel must render IN that
/// panel. It did not: every one of those handlers reported through
/// `_flashAction`, whose only renderer is inside `aside.runs`
/// (`x-show="runsOpen"`, closed by default) — so `sync push` could answer
/// "cannot push: this branch has no remote to push to" and the operator saw
/// a button that did nothing. These are the CI-visible pins over that fix,
/// on the same bargain as the #331 gate above: no Playwright in CI.
#[test]
fn a_refused_change_act_reports_in_the_changes_panel() {
    let html = include_str!("../assets/ui/index.html");
    for pin in [
        r#"class="chg-error""#,
        r#"x-show="changesError""#,
        r#"x-text="changesError""#,
        r#"data-act="chg-error-dismiss""#,
    ] {
        assert!(html.contains(pin), "index.html must keep the pin {pin}");
    }
    // The note is pinned BELOW the compose box and ABOVE the remote bar, in
    // the strip of the panel that never scrolls: a refusal parked behind a
    // long change list is the defect again, wearing a different hat.
    let compose = html.find(r#"class="chg-compose""#).expect("compose box");
    let err = html.find(r#"class="chg-error""#).expect("error note");
    let bar = html.find(r#"class="chg-bar""#).expect("remote bar");
    assert!(
        compose < err && err < bar,
        "the refusal note sits between the compose box and the remote bar"
    );

    // That every act in the panel lands its refusal in `changesError` and
    // still flashes it is driven by `ui-tests/app.test.mjs`.

    let css = served_css();
    assert!(
        css.contains(".chg-error {") && css.contains(".chg-error span {"),
        "the note must be styled and its text bounded, like .runs-verb-error"
    );
    assert!(
        css.contains("color: var(--danger)"),
        "the note carries the token palette's danger colour, not a literal"
    );
}

/// A remote act in flight must SAY so. It did not: a click on Push sent
/// the verb and the button sat unchanged for the whole round trip, so an
/// operator read it as dead and clicked again — racing a second push
/// against the first. One `syncBusy` slot locks the whole bar and swaps
/// the busy act's icon for a ring. Pinned here on the #331 bargain: no
/// Playwright in CI.
#[test]
fn a_remote_act_in_flight_locks_the_bar_and_shows_a_ring() {
    let html = include_str!("../assets/ui/index.html");
    for pin in [
        r#"data-act="fetch" :disabled="$store.connection.writeLocked() || !!syncBusy""#,
        r#"data-act="pull" :disabled="$store.connection.writeLocked() || !!syncBusy || !!pullBlocked()""#,
        r#"data-act="push" :disabled="$store.connection.writeLocked() || !!syncBusy""#,
        r#":class="{ busy: syncBusy === 'fetch' }""#,
        r#":class="{ busy: syncBusy === 'pull' }""#,
        r#":class="{ busy: syncBusy === 'push' }""#,
    ] {
        assert!(html.contains(pin), "index.html must keep the pin {pin}");
    }
    // One ring per act, a sibling of the icon that the CSS swaps in.
    assert_eq!(
        html.matches(r#"<span class="bar-spinner" aria-hidden="true"></span>"#)
            .count(),
        3,
        "each of the three remote acts carries its own ring"
    );

    // That each act takes the slot, refuses while another holds it and frees
    // it on every exit is driven by `ui-tests/app.test.mjs`.

    let css = served_css();
    for pin in [
        ".bar-act .bar-spinner {",
        ".bar-act.busy svg {",
        ".bar-act.busy .bar-spinner {",
        ".bar-act.busy:disabled {",
    ] {
        assert!(css.contains(pin), "the stylesheet must keep the pin {pin}");
    }
}

/// On a branch with no upstream, Pull has nothing to pull from until the
/// branch is published, so it leaves the bar and Publish takes its width.
/// "Publish branch" did not fit in a third of the bar, and the centred
/// label was cut on both sides.
#[test]
fn pull_leaves_the_bar_while_the_branch_has_no_upstream() {
    let html = include_str!("../assets/ui/index.html");
    let pull = html
        .split(r#"data-act="pull""#)
        .nth(1)
        .and_then(|rest| rest.split('>').next())
        .expect("index.html has a Pull button in the remote bar");
    assert!(
        pull.contains(r#"x-show="!pullBlocked()""#),
        "Pull must be hidden while pullBlocked() names a reason: {pull}"
    );
    assert_eq!(
        html.matches(r#"<span class="bar-label""#).count(),
        3,
        "each of the three remote acts wraps its label for the ellipsis"
    );

    let css = served_css();
    let label = css
        .split(".bar-act .bar-label {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("the stylesheet styles .bar-act .bar-label");
    assert!(
        label.contains("text-overflow: ellipsis"),
        "a label that does not fit ends in an ellipsis: {label}"
    );
}

/// The same defect on the branch chip, which lives in the PROJECTS panel:
/// `_mutateBranch` reverted the optimistic chip and sent its reason to
/// `_flashAction`, so a refused switch was a chip that snapped back saying
/// nothing. Its own state and its own gutter — dressing a branch refusal as
/// `treeError` would make a working project read as an unreadable one.
#[test]
fn a_refused_branch_change_reports_in_the_projects_panel() {
    let html = include_str!("../assets/ui/index.html");
    for pin in [
        r#"class="files-error branch-error""#,
        r#"class="files-error branch-error" x-show="$store.checkouts.branchError""#,
        r#"x-text="$store.checkouts.branchError""#,
    ] {
        assert!(html.contains(pin), "index.html must keep the pin {pin}");
    }
    // Under the Files bar, above the tree: the answer sits beside the row
    // whose chip asked, not below rows that can run off screen.
    let head = html
        .find(r#"class="side-head files-sec""#)
        .expect("files bar");
    let err = html
        .find(r#"class="files-error branch-error""#)
        .expect("note");
    let host = html.find(r#"class="wb-host""#).expect("tree host");
    assert!(
        head < err && err < host,
        "the branch refusal sits between the files bar and the tree"
    );

    let app_js = shell_family();
    for pin in [
        "branchError: \"\"",
        "_branchRefused(msg: string) {",
        "this.branchError = msg || \"\";",
    ] {
        assert!(app_js.contains(pin), "the shell must keep the pin {pin}");
    }
    // Both `_mutateBranch` arms report: the refusal AND, in daemon mode, the
    // transport throw. The throw is the arm that used to be deliberately
    // silent, and it is the one that leaves the optimistic chip standing —
    // silence there is the chip claiming a switch nobody confirmed.
    // Both arms are pinned below, inside `_mutateBranch`. There is NO
    // client-side refusal under a selected worktree any more (#407): the
    // act is SENT with the checkout.
    let mutate = squeeze(js_method_body(
        &app_js,
        "async _mutateBranch(verb: string, slug: string | null, name: string, revert: () => void) {",
    ));
    assert!(
        mutate.contains(
            r#"this._branchRefused("Couldnotreachthedaemon.Checkwhetherthebranchchanged.")"#
        ),
        "the daemon-mode throw arm must report; an unanswered branch change must not read as a completed one"
    );
    assert!(
        app_js.contains("WBDaemon.withCheckout({ repo: slug, name }, this.$store.checkouts.checkoutOf(slug))"),
        "a branch act under a selected worktree is sent WITH the checkout — the worktree's HEAD moves, never the primary's"
    );
    assert!(
        !app_js.contains("pick primary before switching branches"),
        "the #406 client-side refusal is gone"
    );
    // The create lives in the console's prompt (wb-console-checkout.ts): an
    // unanswered add re-opens it with the same honest line.
    assert!(
        console_family().contains(
            r#"error = "Could not reach the daemon. Check whether the worktree was created.";"#
        ),
        "an unanswered worktree create must not read as a completed one"
    );
    assert!(
        app_js.contains(
            r#"failure = "Could not reach the daemon. Check whether the worktree was deleted.";"#
        ),
        "an unanswered worktree remove must not read as a completed one"
    );
    // The revert is on the REFUSAL arm only: a throw may have landed, and
    // reverting a switch that happened would put a lie in the chip.
    assert!(
        mutate.contains("revert();this._branchRefused(WBFail.failed(reply,"),
        "only a refusal reverts the optimistic chip, and it reports"
    );

    assert!(
        served_css().contains(".branch-error {"),
        "the branch note bounds its own text: it is the CLI's prose, above the tree"
    );
}

/// The picker row's remove action (#409, ADR-0063 §4): the `.stop` is
/// load-bearing — the row's own click selects the checkout and closes the
/// modal, so without it a remove is also a select-and-dismiss.
#[test]
fn the_worktree_row_remove_action_stops_the_selecting_click() {
    // The rows are the checkout menu's (wb-console-checkout.ts `checkoutMenu`),
    // opened from the Files bar's chip with `onRemove` (ADR-0063
    // amendment 2026-09-16 b): the trash is its own element whose click
    // stops before the row's own pick, and the primary row never has one.
    let console_js = console_family();
    assert!(console_js.contains(r#"trash.className = "session-checkout-remove";"#));
    assert!(
        console_js.contains("if (onRemove && !row.primary) {"),
        "the primary tree has no remove action"
    );
    let trash_click = console_js
        .find("trash.addEventListener(\"click\", (e) => {")
        .expect("the trash has a click handler");
    assert!(
        console_js[trash_click..trash_click + 200].contains("e.stopPropagation();"),
        "the remove action must stop the row's selecting click"
    );
    let html = include_str!("../assets/ui/index.html");
    assert!(
        html.contains(r#"onRemove: (row) => this.removeWorktree(ref, row),"#)
            || shell_family().contains("onRemove: (row) => this.removeWorktree(ref, row),"),
        "the Files chip's menu wires the remove action"
    );
    let app_js = shell_family();
    assert!(
        app_js.contains("WBProject.checkoutAfterListing(ck, listing) === null"),
        "the selection resets from the re-read listing, never from the reply's status"
    );
    let remove = app_js
        .find("async removeWorktree(slug: string, w: CheckoutRow) {")
        .expect("removeWorktree exists");
    let body = &app_js[remove..];
    let ask = body
        .find("await this.$store.modals.askConfirm({")
        .expect("a worktree remove asks the operator first");
    let call = body
        .find(r#"observe("worktree.remove""#)
        .expect("removeWorktree sends worktree.remove");
    assert!(ask < call, "the confirm comes before the daemon call");
    assert!(
        body[ask..call].contains("if (!ok || this._removing(slug, w.name)) return;"),
        "a cancel makes no daemon call"
    );
    assert!(
        served_css().contains(".session-checkout-remove {"),
        "the remove action is styled in the served CSS"
    );
}

/// The CSS between a marked block's `/* <name> */` and `/* <name> end */`
/// comments.
fn css_block<'a>(css: &'a str, name: &str) -> &'a str {
    let open = format!("/* {name} */");
    let close = format!("/* {name} end */");
    let start = css
        .find(&open)
        .unwrap_or_else(|| panic!("styles.css must keep the {name} opening marker"))
        + open.len();
    let end = css
        .find(&close)
        .unwrap_or_else(|| panic!("styles.css must keep the {name} closing marker"));
    &css[start..end]
}

/// Each marked block of new chrome speaks the shell's token language
/// (ADR-0035): `var(--…)` tokens only, never a hex literal. Each block has its
/// own marker pair, so a pin never widens over another issue's CSS. A block
/// emptied by mistake would pass the negative check, so each must also have
/// content.
#[test]
fn marked_css_blocks_add_no_colour_outside_the_token_set() {
    let css = served_css();
    for name in [
        "#331 runs chrome",
        "#319 discard",
        "#318 write controls",
        "#317 rail view",
    ] {
        let block = css_block(&css, name);
        assert!(
            !block.trim().is_empty(),
            "the {name} block must not be empty"
        );
        assert!(
            !block.contains('#'),
            "the {name} CSS must reference var(--…) tokens only, no hex literals"
        );
    }
}

/// The runs chrome's declarations that actually DO the bounding. The markup
/// pins above prove the box exists; only these
/// prove it is bounded, and `max-height` is a single line whose deletion
/// restores the original defect with every other pin still green.
#[test]
fn the_runs_feed_is_bounded_and_wraps() {
    let css = served_css();
    let block = css_block(&css, "#331 runs chrome");
    // The bound, the wrap, and the containment: the three declarations the
    // issue's criteria rest on. The browser pass measures them, and that
    // pass — Playwright — does not run in CI (lib.rs doc above).
    for decl in [
        "max-height: 30vh",
        "overflow-wrap: anywhere",
        "white-space: pre-wrap",
        "flex: 0 0 auto",
    ] {
        assert!(
            block.contains(decl),
            "the feed's containment rests on `{decl}` — it must stay in the #331 block"
        );
    }
    // The phone width is a criterion, so the narrow rule is pinned by its
    // BREAKPOINT and its payload: a bare `@media` would be satisfied by
    // `@media print {}` while the narrow cap was deleted.
    let narrow = block
        .find("@media (max-width: 560px)")
        .expect("the phone width is a criterion — keep the (max-width: 560px) rule");
    assert!(
        block[narrow..].contains("max-height: 22vh"),
        "the narrow-width rule must still cap the feed"
    );
}

/// The write and discard controls are reachable on a touch screen: no
/// `:hover` rule reveals them. The discard block also keeps its `@media`
/// rule: the touch de-emphasis IS the criterion, and a block that lost it
/// would pass the rest vacuously.
#[test]
fn the_write_and_discard_controls_are_not_hover_gated() {
    let css = served_css();
    assert!(
        css_block(&css, "#319 discard").contains("@media"),
        "the touch de-emphasis is the criterion — the discard block must keep its @media rule"
    );
    // (block, the control it styles)
    for (name, control) in [
        ("#318 write controls", "write control"),
        ("#319 discard", "discard control"),
    ] {
        // The touch criterion, pinned where CI can see it: a hover-gated
        // `opacity`/`visibility` is exactly the affordance a phone cannot find.
        // Split on `}` so each chunk is one rule — selector, then its body —
        // and judge the BODY of any rule whose selector mentions `:hover`.
        // Comments are stripped FIRST: one of them names
        // `.branch-chip.disabled:hover` as prior art, and a raw split would
        // read that prose as a selector.
        let declarations = strip_css_comments(css_block(&css, name));
        let mut hover_rules = 0;
        for rule in declarations.split('}') {
            let Some((selector, body)) = rule.split_once('{') else {
                continue;
            };
            if !selector.contains(":hover") {
                continue;
            }
            hover_rules += 1;
            // `display` and `max-height` are in the list because the TEXTBOOK
            // hover-gated affordance is `display: none` + `:hover { display:
            // … }` — banning only `opacity`/`visibility` would leave the most
            // obvious spelling of the defect green.
            for banned in ["opacity", "visibility", "display", "max-height"] {
                assert!(
                    !body.contains(banned),
                    "a {control} must not be hover-gated on {banned}: {selector}"
                );
            }
        }
        // …and the scan must have had something to judge: a block that
        // stopped carrying `:hover` rules would satisfy the loop vacuously.
        assert!(
            hover_rules >= 2,
            "the {name} block must still carry its :hover rules, found {hover_rules}"
        );
    }
}

/// CSS text with every `/* … */` comment removed, so a rule scan judges
/// selectors and not the prose that names one as prior art.
fn strip_css_comments(block: &str) -> String {
    let mut out = String::new();
    let mut rest = block;
    while let Some(at) = rest.find("/*") {
        out.push_str(&rest[..at]);
        match rest[at + 2..].find("*/") {
            Some(end_at) => rest = &rest[at + 2 + end_at + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// No top-level selector declares the same property twice with two values.
///
/// `styles.css` is 6,400 lines and a selector is free to appear in several
/// sections — that is normal and additive, and this gate allows it. What it
/// forbids is the same selector setting the same PROPERTY twice: source
/// order silently picks a winner, and the loser sits in the file reading
/// like an intention that someone can maintain. `.run-verb:disabled` carried
/// `opacity: 0.45` at line 1041 and `opacity: 0.4` at 5451 for as long as
/// both existed; the 0.45 never rendered once.
///
/// It is also the precondition for splitting this file into partials: rules
/// can be regrouped safely only while no pair of them is deciding an outcome
/// by which one comes last.
///
/// Top-level only, and by design. A declaration inside `@media` is SUPPOSED
/// to override the base one — that is the mechanism, not a collision — so
/// anything nested is skipped rather than reported.
#[test]
fn no_selector_sets_one_property_twice() {
    let css = strip_css_comments(&served_css());
    let mut seen: std::collections::HashMap<(String, String), String> =
        std::collections::HashMap::new();
    let mut depth = 0usize;
    let mut selector = String::new();
    let mut body = String::new();
    let mut in_body = false;

    // A quoted value can contain `{`, `}` or `;` — `content: "{"` is legal —
    // and an unaware walk desyncs `depth` and silently drops every rule after
    // it. The only guard is the `seen.len()` floor, which a truncated walk
    // still clears, so the under-report would be invisible. No such value
    // exists today; this keeps it that way.
    let mut in_string: Option<char> = None;
    for ch in css.chars() {
        if in_string.is_some() || ch == '"' || ch == '\'' {
            match in_string {
                Some(quote) if ch == quote => in_string = None,
                Some(_) => {}
                None => in_string = Some(ch),
            }
            // The character still belongs to whatever encloses it — a quoted
            // attribute value is part of the SELECTOR (`[data-dir="n"]`), a
            // quoted value is part of the body. Only the brace/semicolon
            // MEANING is suspended inside the quotes.
            if depth == 0 {
                selector.push(ch);
            } else if depth == 1 && in_body {
                body.push(ch);
            }
            continue;
        }
        match ch {
            '{' => {
                depth += 1;
                if depth == 1 {
                    in_body = true;
                    body.clear();
                }
            }
            '}' => {
                if depth == 1 && in_body {
                    // An at-rule (`@media`, `@supports`) holds nested rules
                    // rather than declarations; its overrides are the point.
                    if !selector.trim().starts_with('@') {
                        let sel = selector.split_whitespace().collect::<Vec<_>>().join(" ");
                        // One entry per MEMBER of a comma group. Keying on
                        // the raw selector text made `.a { opacity: 1 }` and
                        // `.a, .b { opacity: 0.5 }` two different keys —
                        // which is the commonest real shape of the very
                        // collision this gate was written for.
                        // NOT split on commas, and this was measured rather
                        // than assumed. A review asked for the split: keying
                        // on the raw selector text means `.a { … }` and
                        // `.a, .b { … }` are different keys, so a collision
                        // between them is missed. Splitting the group does
                        // find those — and it also reds on the ordinary CSS
                        // idiom of a base rule for a group followed by a
                        // refinement for one member, which this stylesheet
                        // uses correctly: `.md-body h1..h4` set
                        // `letter-spacing: -0.011em`, then `.md-body h1` sets
                        // `-0.02em`. A gate that fails conformant code is
                        // worse than one with a known blind spot, so the
                        // blind spot is stated instead — a collision is
                        // reported only between two blocks whose selector
                        // text is identical.
                        // Within ONE block, re-declaring a property is the
                        // documented CSS fallback idiom — `height: 100vh`
                        // then `height: 100dvh` is how a browser without
                        // `dvh` still gets a height. So each block is folded
                        // to its own last-wins map first, and only the
                        // ACROSS-block collisions are reported.
                        let mut block: std::collections::HashMap<String, String> =
                            std::collections::HashMap::new();
                        for decl in body.split(';') {
                            let Some((prop, value)) = decl.split_once(':') else {
                                continue;
                            };
                            let prop = prop.trim().to_string();
                            let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
                            if prop.is_empty() || prop.starts_with("--") {
                                continue;
                            }
                            block.insert(prop, value);
                        }
                        for (prop, value) in block {
                            if let Some(first) =
                                seen.insert((sel.clone(), prop.clone()), value.clone())
                            {
                                assert_eq!(
                                    first, value,
                                    "the stylesheet declares `{prop}` on `{sel}` in two \
                                     separate blocks with different values ({first} then \
                                     {value}) — source order decides which one renders, and \
                                     the other is dead"
                                );
                            }
                        }
                    }
                    in_body = false;
                    selector.clear();
                }
                depth = depth.saturating_sub(1);
            }
            _ if depth == 0 => selector.push(ch),
            _ if depth == 1 => body.push(ch),
            _ => {}
        }
    }

    // NEGATIVE CONTROL: a parse that found nothing would pass silently. The
    // stylesheet is thousands of declarations; this states that the walk
    // actually reached them.
    assert!(
        seen.len() > 1_000,
        "the stylesheet walk collected only {} declarations — it is not \
         parsing the file",
        seen.len()
    );
}

/// The run-completion nudge (#310) across the assets. `shouldReload` and
/// `subscribeChanges` are driven by `wb-changes.test.mjs` and
/// `wb-daemon.test.mjs`, and `toggle` remounting the subscription by
/// `wb-changes-panel.test.mjs`. No node test reaches the export
/// `wb-changes-panel.ts` guards on or the filter call inside
/// `mountChangesSub`, so those two are pinned here.
#[test]
fn the_run_completion_nudge_is_wired_through_the_ui_assets() {
    assert!(
        include_str!("../assets/ui/wb-daemon.ts").contains("subscribeChanges,"),
        "wb-daemon.ts must EXPORT subscribeChanges — app.ts guards on it (#310)"
    );
    assert!(
        shell_family().contains("WBChanges?.shouldReload?.(frame, this.$store.projects.openSlug)"),
        "the shell must filter each nudge through shouldReload (#310)"
    );
}

/// The wake affordance, pinned from CI. `toggle` calling `wakePeerFor` is
/// driven by `app.test.mjs`, and `wakePeerFor` calling `wakePeer` by
/// `wb-projects-store.test.mjs`; `wakeable`
/// by `wb-fleet.test.mjs`. The markup and the body of `wakePeer` are reached
/// by no node test, and Playwright does not run in CI — these substrings are
/// the only CI-visible gate over the one consumer `/api/fleet/nudge` has, and
/// a route with no caller is a route that rots.
#[test]
fn the_peer_wake_is_wired_through_the_ui_assets() {
    let app_js = shell_family();
    for symbol in [
        r#"apiFetch("POST /api/fleet/nudge", { query: { daemon_id: daemonId } })"#,
        "async wakePeer(",
        // Readiness, not the spawn: a caller that acted on `nudged` would be
        // back to reporting a peer as woken while it is still booting.
        "woke?.ready",
    ] {
        assert!(
            app_js.contains(symbol),
            "the shell must keep {symbol} to wake a peer"
        );
    }
    let html = include_str!("../assets/ui/index.html");
    assert!(
        html.contains(r#"@click.stop="$store.projects.wakePeer(g.daemon)""#),
        "the group header must carry the wake control"
    );
    assert!(
        html.contains(r#"x-show="$store.projects.peerWakeable(g)""#),
        "the wake control must be withheld from states a nudge cannot fix"
    );
    // Alpine stringifies an object nested inside a `:class` ARRAY, so the
    // object form emits a literal `[object Object]` class and the state it
    // meant to set never applies. Measured in the browser, since no unit test
    // in this tree renders Alpine.
    assert!(
        html.contains(
            r#":class="[g.state, $store.projects.peerFault(g) ? 'fault' : '', $store.projects.waking[g.daemon] ? 'waking' : '']""#
        ),
        "the wake glyph's :class must bind plain strings, never a nested object"
    );
    // `asleep` is the ordinary course of a day, not a fault. The rule is in
    // `stateFault` (driven by ui-tests/wb-fleet.test.mjs); the CSS only
    // colours what it marks.
    assert!(
        served_css().contains(".env-group .peer-state.fault {"),
        "styles must colour the state glyph `stateFault` marks"
    );
}

/// Creating must be reachable for every target the operator can point at:
/// a folder, a file (meaning its parent), and the repo root. The root has
/// no node — a right-click on empty tree space resolves to `null` — so the
/// context handler must NOT bail on a missing node, or a top-level file is
/// uncreatable. The Files header carries the same two actions, because
/// right-clicking empty space is an affordance nothing on screen advertises.
/// The directory `emitCreate` sends is driven by `wb-files.test.mjs`.
#[test]
fn the_explorer_can_create_at_every_target_including_the_repo_root() {
    let js = include_str!("../assets/ui/wb-files.ts");
    assert!(
        js.contains("this.showMenu(ev.clientX, ev.clientY, node || null)"),
        "the tree's contextmenu handler must open the menu for a NULL node \
         (empty space = the repo root), not return early"
    );
    assert!(
        js.contains("createHere(kind: \"file\" | \"folder\") {"),
        "wb-files.ts must keep createHere(kind) — the Files header calls it"
    );
    let html = include_str!("../assets/ui/index.html");
    for symbol in ["createHere('file')", "createHere('folder')"] {
        assert!(
            html.contains(symbol),
            "index.html's Files header must wire {symbol}"
        );
    }
}

/// Naming a new entry goes through the design-system prompt, not the
/// browser's: `window.prompt` is unstyled, is suppressible for the whole
/// origin by one "prevent this page from creating more dialogues" tick, and
/// never renders in a detached popup. It stays only as the fallback for a
/// shell that cannot be reached. The create action calling `askPrompt`, and
/// `askPrompt` settling with the name `promptSubmit` accepts, are driven by
/// `app.test.mjs`; this pins the markup the dialog renders into.
#[test]
fn naming_a_new_entry_uses_the_design_system_prompt() {
    let html = include_str!("../assets/ui/index.html");
    for symbol in ["prompt-modal", "id=\"prompt-input\"", "promptSubmit()"] {
        assert!(
            html.contains(symbol),
            "index.html must render the prompt dialog ({symbol})"
        );
    }
}

/// A Changes row's action buttons must sit OUTSIDE the clipped region.
///
/// `.chg-name` is frozen on purpose (`flex: 0 0 auto`, so an absurd name is
/// never ellipsized while a directory can still drain), which means a long
/// name genuinely overflows and something must clip. When that clip lived on
/// `.chg-row` and the buttons were the row's LAST children, the clip ate the
/// buttons: `docs/adr/0032-daemon-mode-supervised-launcher.md` pushed `+`/`×`
/// to x=384/414 against a row edge of 347, so a path with a long file name
/// could not be staged or discarded from the UI at all. `.chg-face` owns the
/// overflow now and the controls are its siblings.
///
/// Only a browser computes that geometry and CI runs none, so what is pinned
/// here is the structure the geometry follows from: the face wraps every
/// descriptive span, closes, and only then come the actions.
#[test]
fn a_changes_row_keeps_its_actions_outside_the_clipped_face() {
    let html = include_str!("../assets/ui/index.html");
    let rows: Vec<&str> = html.matches("class=\"chg-row\"").collect();
    assert_eq!(
        rows.len(),
        2,
        "expected the staged and unstaged row templates; the count changed, \
         so re-check that each still keeps its actions outside the face"
    );
    for (i, block) in html.split("class=\"chg-row\"").skip(1).enumerate() {
        let row = block.split_once("</li>").map_or(block, |(head, _)| head);
        let face = row
            .find("class=\"chg-face\"")
            .unwrap_or_else(|| panic!("row {i} must wrap its spans in .chg-face"));
        let act = row
            .find("class=\"chg-act")
            .unwrap_or_else(|| panic!("row {i} must carry at least one .chg-act"));
        assert!(
            face < act,
            "row {i}: the face must open BEFORE the actions — an action inside \
             the overflow region is an action a long file name can clip away"
        );
        // The face must CLOSE before the first action. Counting `</span>`
        // alone cannot see that — each descriptive child closes itself — so
        // balance the tags: inside the face, every child pairs up, and the ONE
        // unmatched close is the face's own. Equal counts mean the face is
        // still open when the button arrives, which is the bug's exact shape.
        let inner = {
            let after_tag = row[face..]
                .find('>')
                .map(|gt| face + gt + 1)
                .expect("the .chg-face opening tag must close");
            &row[after_tag..act]
        };
        let opens = inner.matches("<span").count();
        let closes = inner.matches("</span>").count();
        assert!(
            inner.contains("chg-name"),
            "row {i}: the file name belongs inside the face; found: {inner:?}"
        );
        assert_eq!(
            closes,
            opens + 1,
            "row {i}: the face must close before the actions — {opens} span(s) \
             opened and {closes} closed, so the button is INSIDE the clipped \
             region a long file name overflows; found: {inner:?}"
        );
    }

    // The overflow belongs to the face. `.chg-row` keeps one only as a
    // backstop, and the face is what may shrink (`min-width: 0`).
    let css = served_css();
    let face = css
        .split_once(".chg-face {")
        .expect("styles.css must define .chg-face")
        .1
        .split_once('}')
        .expect("the .chg-face block must close")
        .0;
    for decl in ["overflow: hidden", "min-width: 0", "flex: 1 1 auto"] {
        assert!(
            face.contains(decl),
            ".chg-face must declare {decl} — it is the clipping, shrinkable \
             region; found: {face:?}"
        );
    }
}

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

/// Icons are drawn by the `x-icon` directive, which Alpine runs wherever it
/// initializes an element (#487). Nothing scans the document for lucide
/// placeholders any more, so a `data-lucide` attribute in the markup would
/// ship as an empty icon, and a `createIcons()` call would bring back the
/// scan that missed every `x-if` and `x-for` built after it ran (#332).
/// `wb_icons_487.py` owns the rendering; this pin keeps the markup on it.
#[test]
fn every_icon_is_drawn_by_the_x_icon_directive() {
    let html = include_str!("../assets/ui/index.html");
    let js = shell_family();
    assert!(
        js.contains("export function iconDirective(")
            && include_str!("../assets/ui/main.ts")
                .contains(r#"Alpine.directive("icon", iconDirective);"#),
        "main.ts must register the x-icon directive of the shell family"
    );
    assert!(
        html.matches("x-icon=").count() >= 40,
        "index.html must draw its icons with x-icon"
    );
    for (file, text) in [("index.html", html)]
        .into_iter()
        .chain(SHELL_FAMILY.iter().copied())
    {
        assert!(
            !text.contains("data-lucide=") && !text.contains("createIcons()"),
            "{file} must not use lucide placeholders or the createIcons scan"
        );
    }
}

/// A repo with no `origin` is keyed `path-<hash>` (ADR-0008 D7). That stays
/// the identity; only the LABEL becomes the directory basename (#332).
#[test]
fn a_remoteless_project_is_labelled_by_its_directory() {
    let js = shell_family();
    let load = js_method_body(&js, "async loadRepos({ git = true } = {}) {");
    assert!(
        load.contains("name: x.name"),
        "`loadRepos` must keep `/api/repos`'s name — the label reads it; \
         found: {load:?}"
    );

    // The label fold (`repoLabel`) is driven by `ui-tests/wb-project.test.mjs`,
    // except one clause no row reaches: an OWNER that starts with `path-`
    // (`path-org/tool`) is a GitHub repo and must not be relabelled off disk.
    let project = include_str!("../assets/ui/wb-project.ts");
    let label = project
        .split_once("function repoLabel(p: Project) {")
        .expect("wb-project.ts must define repoLabel")
        .1
        .split_once("\n}")
        .expect("repoLabel must close at module indent")
        .0;
    assert!(
        label.contains("projectName(p)"),
        "the label is built on the daemon's project name; found: {label:?}"
    );

    let filter = js_method_body(&js, "filteredProjects() {");
    assert!(
        filter.contains("repoLabel("),
        "the filter must match the VISIBLE label: typing what the row prints \
         and getting an empty list is the defect a directory label would \
         otherwise introduce; found: {filter:?}"
    );

    let html = include_str!("../assets/ui/index.html");
    assert!(
        html.contains(r#"x-text="repoLabel(p)""#),
        "the row must render the label through `repoLabel`"
    );
    assert!(
        html.contains(r#":data-slug="p.slug""#),
        "the label is a view concern; `.project-slug` must keep the canonical \
         ADR-0008 D7 slug in `data-slug` (the browser tests locate a row by it)"
    );
}

/// A twenty-character label in a column fixed at 300px wrapped the row to
/// two lines (#332).
#[test]
fn the_project_name_truncates_instead_of_wrapping() {
    let css = served_css();
    let body = css_rule_body(&css, ".project-slug {");
    for (decl, why) in [
        (
            "flex: 1 1 auto",
            "the name is the row's ONE growing child — it is what pushes \
             the branch chip to the right edge",
        ),
        (
            "min-width: 0",
            "a flex item does not shrink below its content without it; that, \
             not the missing ellipsis, is what produced the wrap",
        ),
        ("overflow: hidden", "the clip the ellipsis needs"),
        ("text-overflow: ellipsis", "the truncation marker"),
        ("white-space: nowrap", "one line"),
    ] {
        assert!(
            body.contains(decl),
            ".project-slug must declare {decl} — {why}; found: {body:?}"
        );
    }
}

/// The chip sits on the project row, merged with the change count: collapsed
/// it is only the count, open it grows the branch and switches it. The Files
/// bar is left to the checkout chip. Neither block nests a `<div>`, so slicing
/// each to its first `</div>` is exact.
#[test]
fn the_branch_chip_carries_the_change_count_on_the_project_row() {
    let html = include_str!("../assets/ui/index.html");
    let slice = |open: &str| -> String {
        html.split_once(open)
            .unwrap_or_else(|| panic!("index.html must carry `{open}`"))
            .1
            .split_once("</div>")
            .unwrap_or_else(|| panic!("the `{open}` block must close"))
            .0
            .to_string()
    };

    let row = slice(r#"class="project-head""#);
    // Proves the slice LANDED. Without it the negative below passes
    // vacuously on any mis-sliced or empty string.
    assert!(
        row.contains("project-slug"),
        "the .project-head slice must contain the project name; found: {row:?}"
    );
    for needle in [
        r#"class="branch-chip""#,
        r#"class="chg-badge""#,
        "branchChipClick(p, $event)",
        r#"x-show="$store.projects.rowOpen(p)""#,
    ] {
        assert!(
            row.contains(needle),
            "the row's chip must carry {needle:?} — the count, and the branch \
             switcher once the row is open; found: {row:?}"
        );
    }
    assert!(
        row.contains("rowTitle(p)"),
        "a collapsed row shows no branch while `filteredProjects` still \
         matches on branch, so it must name the branch in its title; \
         found: {row:?}"
    );

    let files = slice(r#"class="side-head files-sec""#);
    assert!(
        files.contains("createHere('file')"),
        "the Files bar must keep its own actions; found: {files:?}"
    );
    assert!(
        !files.contains("branch-chip"),
        "the Files bar is the checkout chip's — a worktree name needs the \
         width; found: {files:?}"
    );
    for (pin, what) in [
        (r#"class="branch-chip""#, "chip"),
        (r#"class="chg-badge""#, "count"),
    ] {
        assert_eq!(
            html.matches(pin).count(),
            1,
            "one {what} per row — two would let the count and the branch \
             disagree about which tree they describe"
        );
    }

    // Capped at 48% on every row, the chip cost the name half its width
    // (#332). Now the name wins: the chip takes the squeeze first, and its
    // bound sits on the branch name — a `max-width` on the chip freezes it at
    // the cap (flex shrinks from the unclamped size) and squeezes the name.
    let css = served_css();
    let chip = css_rule_body(&css, ".branch-chip {");
    assert!(
        chip.contains("flex: 0 10000 auto") && !chip.contains("max-width"),
        ".branch-chip must shrink first and carry no max-width; found: {chip:?}"
    );
    let name = css_rule_body(&css, ".branch-chip-name {");
    assert!(
        name.contains("max-width:"),
        ".branch-chip-name must bound the chip; found: {name:?}"
    );
    let collapsed = css_rule_body(&css, ".branch-chip.collapsed {");
    assert!(
        collapsed.contains("padding: 0"),
        "a collapsed chip is only its count; found: {collapsed:?}"
    );

    let js = shell_family();
    for pin in [
        "rowTitle(p: Project) {",
        "branchChipClick(p: Project, ev: Event) {",
    ] {
        assert!(js.contains(pin), "the shell must define {pin}");
    }
}

/// The sidebar's left edge was ragged (0.8rem for the headers, search box
/// and rows; 0.5rem for the Changes toolbar and compose box), so tightening
/// only the project row would have made it worse. One token, six rules
/// (#332). `.runs-head` keeps its own value: it is not this column.
#[test]
fn the_sidebar_column_keeps_one_gutter() {
    let css = served_css();
    assert!(
        css.contains("--side-gutter:"),
        "the column's gutter must be a token, so it moves once"
    );
    for selector in [
        ".side-head {",
        ".side-search {",
        ".project-head {",
        ".chg-toolbar {",
        ".side-empty {",
        ".chg-compose {",
    ] {
        let body = css_rule_body(&css, selector);
        assert!(
            body.contains("var(--side-gutter)"),
            "`{selector}` shares the sidebar's left edge — a literal value \
             here re-rags the column; found: {body:?}"
        );
    }
}

/// POST `body` to `/api/peer/command` on a daemon with one registered repo,
/// `owner/cap`; return the status.
async fn peer_command_status(body: Vec<u8>) -> StatusCode {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("owner/cap", &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();
    let resp = fleet_router(registry_path)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/peer/command")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

/// A paste forwarded to a peer carries a whole image: a 4 MiB `image.write`
/// is under the transport cap, so the peer route takes it.
#[tokio::test]
async fn peer_command_takes_a_4_mib_image() {
    let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
    png.resize(tree::MAX_IMAGE_BYTES as usize, 0);
    let body = serde_json::json!({
        "id": 1,
        "verb": "image.write",
        "payload": {"repo": "owner/cap", "base64": data_encoding::BASE64.encode(&png)},
    });
    let status = peer_command_status(body.to_string().into_bytes()).await;
    assert_ne!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(status, StatusCode::OK);
}

/// A body over the transport cap is refused before it is parsed.
#[tokio::test]
async fn peer_command_refuses_a_body_over_the_cap() {
    let body = vec![b' '; tree::MAX_COMMAND_BYTES + 1];
    assert_eq!(
        peer_command_status(body).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

/// The `innerHTML` writes of `src` that take a template string with a value
/// in it, or a bare variable: each is a sink that would parse a file name as
/// markup.
fn template_html_sinks(src: &str) -> Vec<String> {
    let template = regex::Regex::new(r"innerHTML\s*=\s*`[^`]*\$\{").expect("valid regex");
    let variable = regex::Regex::new(r"innerHTML\s*=\s*[A-Za-z_$][\w$]*\s*;").expect("valid regex");
    template
        .find_iter(src)
        .chain(variable.find_iter(src))
        .map(|m| m.as_str().to_string())
        .collect()
}

/// The context menu and the console key bar build their icons as elements
/// and their text with `textContent`: no `innerHTML` there takes a template
/// string with a value in it, or a variable.
#[test]
fn no_menu_or_key_sink_takes_a_template_string() {
    // The matcher sees both shapes, across a line break too.
    assert_eq!(template_html_sinks("b.innerHTML = `<i>${x}</i>`;").len(), 1);
    assert_eq!(
        template_html_sinks("h.innerHTML =\n  `<i class=\"${a}\">`;").len(),
        1
    );
    assert_eq!(template_html_sinks("b.innerHTML = text;").len(), 1);
    assert!(template_html_sinks("b.innerHTML = '<i class=\"bi bi-x\"></i>';").is_empty());
    assert!(template_html_sinks("menu.innerHTML = \"\";").is_empty());
    // The whole console family, and the whole shell family: the shell,
    // `wb-files.ts`, `wb-move-dialog.ts` and `wb-consoles-tab.ts` build menus.
    for (name, src) in [CONSOLE_FAMILY, SHELL_FAMILY].concat() {
        let sinks = template_html_sinks(src);
        assert!(sinks.is_empty(), "{name}: {sinks:?}");
    }
}

/// Every problem `vendor/manifest.json` has against `files` (path relative to
/// `vendor/`, embedded bytes). Empty when each file has exactly one entry
/// whose SHA-256 matches, and each entry names a file that exists.
fn check_manifest(files: &[(String, Vec<u8>)], manifest: &serde_json::Value) -> Vec<String> {
    use sha2::{Digest, Sha256};
    let mut problems = Vec::new();
    let mut entries: std::collections::BTreeMap<String, String> = Default::default();
    let libraries = manifest["libraries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if libraries.is_empty() {
        problems.push("the manifest lists no libraries".to_string());
    }
    for lib in &libraries {
        for key in ["name", "version", "source"] {
            if lib[key].as_str().is_none_or(str::is_empty) {
                problems.push(format!("{lib}: `{key}` is missing"));
            }
        }
        for f in lib["files"].as_array().into_iter().flatten() {
            let (Some(path), Some(sha)) = (f["path"].as_str(), f["sha256"].as_str()) else {
                problems.push(format!("{f}: an entry needs `path` and `sha256`"));
                continue;
            };
            if entries.insert(path.to_string(), sha.to_string()).is_some() {
                problems.push(format!("{path}: listed twice in the manifest"));
            }
        }
    }
    for (path, bytes) in files {
        let actual: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        match entries.remove(path) {
            None => problems.push(format!("{path}: not in the manifest")),
            Some(sha) if sha != actual => problems.push(format!(
                "{path}: sha256 is {actual}, the manifest says {sha}"
            )),
            Some(_) => {}
        }
    }
    for path in entries.keys() {
        problems.push(format!("{path}: in the manifest, but not vendored"));
    }
    problems
}

/// Each vendored file is the one its manifest entry names: a changed byte or
/// a file nobody recorded fails here, so a library bump also updates the
/// version and source the vulnerability scan reads (ADR-0072 D12).
#[test]
fn vendored_files_match_the_manifest() {
    fn walk(dir: &include_dir::Dir<'_>, out: &mut Vec<(String, Vec<u8>)>) {
        for f in dir.files() {
            let path = f.path().to_string_lossy().replace('\\', "/");
            let rel = path.strip_prefix("vendor/").unwrap_or(&path).to_string();
            if rel != "manifest.json" {
                out.push((rel, f.contents().to_vec()));
            }
        }
        for d in dir.dirs() {
            walk(d, out);
        }
    }
    let manifest: serde_json::Value = serde_json::from_slice(
        UI.get_file("vendor/manifest.json")
            .expect("vendor/manifest.json is embedded")
            .contents(),
    )
    .expect("vendor/manifest.json is JSON");
    let mut files = Vec::new();
    walk(
        UI.get_dir("vendor").expect("vendor/ is embedded"),
        &mut files,
    );
    assert!(files.len() > 100, "the walk found the vendored files");
    let problems = check_manifest(&files, &manifest);
    assert!(problems.is_empty(), "{}", problems.join("\n"));

    // Negative controls: the checker sees a changed byte and an extra file.
    let mut flipped = files.clone();
    flipped[0].1[0] ^= 1;
    let problems = check_manifest(&flipped, &manifest);
    assert!(
        problems.len() == 1 && problems[0].starts_with(&format!("{}: sha256 is", flipped[0].0)),
        "{problems:?}"
    );
    let mut extra = files.clone();
    extra.push(("stray.js".to_string(), b"x".to_vec()));
    assert_eq!(
        check_manifest(&extra, &manifest),
        vec!["stray.js: not in the manifest".to_string()]
    );
    let missing = &files[1..];
    assert_eq!(
        check_manifest(missing, &manifest),
        vec![format!("{}: in the manifest, but not vendored", files[0].0)]
    );
}

/// The vendored Alpine is the version its manifest entry names. The hash
/// check cannot see this: the npm tarballs of alpinejs 3.14.0 to 3.14.9 ship
/// a `dist/module.esm.min.js` built from 3.13.10 (measured on 3.14.1 and
/// 3.14.9), which throws `u is not a function` when an `x-show` element is
/// hidden twice inside a parent that is being hidden (issue #579).
#[test]
fn vendored_alpine_is_the_manifest_version() {
    let manifest: serde_json::Value = serde_json::from_slice(
        UI.get_file("vendor/manifest.json")
            .expect("vendor/manifest.json is embedded")
            .contents(),
    )
    .expect("vendor/manifest.json is JSON");
    let lib = manifest["libraries"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|l| l["npm"] == "alpinejs")
        .expect("the manifest lists alpinejs");
    let path = lib["files"][0]["path"]
        .as_str()
        .expect("the alpinejs entry names its file");
    let text = UI
        .get_file(format!("vendor/{path}"))
        .and_then(|f| f.contents_utf8())
        .unwrap_or_else(|| panic!("vendor/{path} is embedded UTF-8"));
    let built = text
        .split_once("version: \"")
        .or_else(|| text.split_once("version:\""))
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(v, _)| v);
    assert_eq!(built, lib["version"].as_str(), "vendor/{path}");
}

mod agents_repos;
mod asset_graph;
/// The desk history routes (ADR-0050 amendment 2026-10-04).
mod audit_routes;
mod auth_cookie;
mod auth_policy;
mod desk_history;
mod desk_routes;
mod families;
mod fleet_routes;
mod module_graph;
mod peer_session_query;
mod release_routes;
mod root_routes;
mod security_store;
mod served_shell;
mod served_vendor;
mod usage_records;
mod usage_spend;
mod watch_bell;
