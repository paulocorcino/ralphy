//! Ralphy's resident daemon (docs/adr/0032): a foreground HTTP listener bound
//! to localhost, serving the embedded workbench UI. This is the tracer bullet —
//! no sessions, no command vocabulary yet — but the shape is the decided one:
//! a library crate wired by `ralphy-cli`, the workspace's async runtime (tokio +
//! axum) confined here, runs reached only by spawning `ralphy` processes (never
//! by importing the core).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use axum::Router;
use include_dir::{include_dir, Dir};

pub mod agent_state;
pub mod assets;
pub mod auth;
pub mod autostart;
pub mod checkout;
pub mod clipboard;
pub mod confine;
pub mod cookie;
pub mod desk;
pub mod dispatch;
pub mod epoch;
pub mod fleet;
pub mod fswrite;
pub mod identity;
pub mod note;
pub mod password;
pub mod peer;
pub mod pidfile;
pub mod protocol;
pub mod registry;
mod rekey;
pub mod release;
pub mod roster;
pub mod session;
pub mod spend;
pub mod textcodec;
pub mod totp;
pub mod tree;
pub mod usage;
pub mod watch;

mod routes;
mod serve;

use routes::*;
use serve::serve;

/// The daemon's default TCP port. "ralphy" on a phone keypad starts 7-2-5-7.
pub const DEFAULT_PORT: u16 = 7257;

/// The embedded workbench UI, baked in at build time like `assets/prompts` — the
/// daemon reads no files from disk at runtime (ADR-0032 §4). Promoted to the
/// daemon's `/` in #200 (PRD #185); the SPA self-gates its login (see
/// [`routes::require_auth`]), so there is no separate server-rendered login page.
pub(crate) static UI: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/ui");

/// What the composition root decides; everything else is the daemon's.
pub struct DaemonConfig {
    /// TCP port for the listener.
    pub port: u16,
    /// The interface to bind. Defaults to `127.0.0.1` (loopback only); a
    /// non-localhost bind is an explicit opt-in that REQUIRES a bearer access
    /// token, enforced at boot by [`auth::AuthPolicy::for_bind`] (ADR-0032 §4).
    pub bind: IpAddr,
    /// Extra host names this daemon answers as, beyond the ones its bind implies:
    /// a MagicDNS name, a reverse-proxy hostname. The cross-site gate refuses any
    /// other `Host`, which is what keeps DNS rebinding out — so reaching the
    /// daemon by NAME (rather than by the bound IP) is an explicit declaration.
    pub allowed_hosts: Vec<String>,
    /// Directories this daemon announces itself into as a peer descriptor
    /// (ADR-0052 §3) — typically the OTHER environment's `.ralphy` store, e.g.
    /// `/mnt/c/Users/<user>/.ralphy` from inside WSL. A directory is the only
    /// thing an announcer can know about its peer; empty means "a fleet of one".
    pub peer_stores: Vec<PathBuf>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            bind: Ipv4Addr::LOCALHOST.into(),
            allowed_hosts: Vec::new(),
            peer_stores: Vec::new(),
        }
    }
}

/// Compose the bind address from an interface and port. Centralized so the
/// resolved interface flows through one place (the auth policy keys on
/// `addr.ip()`).
pub fn bind_addr(ip: IpAddr, port: u16) -> SocketAddr {
    SocketAddr::new(ip, port)
}

/// Run the daemon in the foreground until Ctrl+C. Blocking on purpose: the
/// tokio runtime is created and dropped inside, so callers (the sync CLI)
/// never see async types.
pub fn run(config: DaemonConfig) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building the daemon's tokio runtime")?;
    runtime.block_on(serve(
        bind_addr(config.bind, config.port),
        config.allowed_hosts,
        config.peer_stores,
    ))
}

/// The per-vendor interactive session-store paths resolved once at daemon boot
/// and handed to the `/api/usage` scan — one `PathBuf` per vendor store. Grouped
/// so onboarding a vendor is a new field, not another positional threaded through
/// every `router` call site (#267); eight adjacent same-typed paths were also
/// transposition-prone (the compiler can't catch two swapped stores). `Default`
/// yields empty paths — a "no store" set the scans tolerate (ADR-0040 C6) — which
/// the daemon's tests use as their all-missing base.
#[derive(Clone, Default)]
pub struct StorePaths {
    pub claude_projects_dir: PathBuf,
    pub codex_dir: PathBuf,
    pub opencode_db: PathBuf,
    pub kimi_dir: PathBuf,
    pub kimi_code_dir: PathBuf,
    pub copilot_db: PathBuf,
    pub cursor_dir: PathBuf,
    pub gemini_dir: PathBuf,
}

/// The daemon's HTTP surface. Real routes sit *before* the embedded-UI
/// fallback. `GET /api/identity` returns the loaded identity as JSON, or 404
/// when the daemon is un-baptized, so the static page can render "avatar name"
/// at runtime (the embedded HTML bakes in no identity).
pub fn router(
    identity: Option<identity::Identity>,
    registry_path: PathBuf,
    usage_dir: PathBuf,
    stores: StorePaths,
    start: Instant,
    shutdown: tokio::sync::watch::Receiver<bool>,
    auth: Arc<auth::AuthState>,
) -> Router {
    router_with_roster(
        identity,
        registry_path,
        usage_dir,
        stores,
        start,
        shutdown,
        RouterDependencies {
            auth,
            roster_locator: Arc::new(session::Agent::locate_program),
        },
    )
}

/// `GET /ws/command`: one remote command per connection. Read the first frame; a
/// `Frame::Command{verb}` naming a blessed [`dispatch::Verb`] for a registered
/// repo spawns the run and reports its lifecycle — an ack (`status:"spawned"` +
/// pid), a stream of live output (`status:"output"` + `chunk`, issue #180), then
/// the child's exit (`status:"exited"` + code). An unknown verb or an unregistered
/// repo gets one `status:"error"` frame and spawns nothing.
///
/// TEARDOWN INVARIANT (the INVERSE of `session_ws`): the dispatched run keeps its
/// OWN lifecycle. NONE of the `select!` arms — daemon shutdown, client
/// close/error, output, wait-complete — kills the child; the
/// `Box<dyn dispatch::Child>` has no kill and dropping it does not kill (std
/// semantics). A daemon shutdown or a browser disconnect stops us serving THIS
/// socket but never the run (PRD #157 story 18/20). Do not add a kill to any arm.
/// The output DRAIN task is likewise detached: it reads the child's pipe to EOF
/// regardless of client presence, so a disconnect never stalls the child on a
/// full pipe. Do not await it on a teardown arm.
// The router's per-route dependencies, one parameter each (precedent: `usage_route`).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Frame;
    use crate::serve::{announce_peer, announced_descriptor};
    use axum::body::Body;
    use axum::http::Request;
    use axum::http::{header, StatusCode};
    use axum::response::Response;
    use axum::{Json, Router};
    use http_body_util::BodyExt;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tower::ServiceExt;

    /// A never-fired shutdown receiver for the in-process router tests (none of
    /// them exercise `/ws`, so its sender dropping immediately is harmless).
    fn idle_shutdown() -> tokio::sync::watch::Receiver<bool> {
        tokio::sync::watch::channel(false).1
    }

    /// The peer relay forwards the checkout on the LAUNCH shape only; a reattach
    /// names a record the peer already owns, and the no-checkout launch stays
    /// byte-identical for an older peer.
    #[test]
    fn peer_session_query_forwards_the_checkout_only_when_present() {
        let launch = |checkout: Option<&str>| SessionQuery {
            repo: Some("x".into()),
            agent: Some("claude".into()),
            id: None,
            takeover: None,
            watch: None,
            console: None,
            checkout: checkout.map(str::to_string),
            command: None,
            holder: None,
        };
        assert_eq!(
            peer_session_query(&launch(Some("wt-a")), "owner/repo"),
            "repo=owner%2Frepo&agent=claude&checkout=wt-a"
        );
        assert_eq!(
            peer_session_query(&launch(None), "owner/repo"),
            "repo=owner%2Frepo&agent=claude"
        );
        let reattach = SessionQuery {
            id: Some(7),
            ..launch(Some("wt-a"))
        };
        assert_eq!(
            peer_session_query(&reattach, "owner/repo"),
            "id=7&repo=owner%2Frepo"
        );
    }

    /// The owning daemon keeps the writer slot, so the relay forwards the tab's
    /// holder on a launch and on a reattach — and drops a malformed one rather
    /// than splicing it into the peer's query string.
    #[test]
    fn peer_session_query_forwards_a_well_formed_holder() {
        let reattach = |holder: &str| SessionQuery {
            repo: Some("x".into()),
            agent: None,
            id: Some(7),
            takeover: None,
            watch: None,
            console: None,
            checkout: None,
            command: None,
            holder: Some(holder.to_string()),
        };
        assert_eq!(
            peer_session_query(&reattach("tab-1_A"), "owner/repo"),
            "id=7&repo=owner%2Frepo&holder=tab-1_A"
        );
        let launch = SessionQuery {
            id: None,
            agent: Some("claude".into()),
            ..reattach("tab-1_A")
        };
        assert_eq!(
            peer_session_query(&launch, "owner/repo"),
            "repo=owner%2Frepo&agent=claude&holder=tab-1_A"
        );
        for bad in ["", "a&takeover=1", &"x".repeat(65)] {
            assert_eq!(
                peer_session_query(&reattach(bad), "owner/repo"),
                "id=7&repo=owner%2Frepo",
                "{bad:?} is not a holder"
            );
        }
    }

    /// A peer on an older build sends no `checkout`; the listing must still
    /// parse, and a row without one serialises without the key.
    #[test]
    fn a_peer_sessions_body_without_checkout_still_parses() {
        let body = r#"[{"id":1,"repo":"o/r","agent":"claude","kind":"agent","started_at":1,"daemon_id":"d","environment":"Windows"}]"#;
        let rows: Vec<HostedSessionInfo> = serde_json::from_str(body).unwrap();
        assert_eq!(rows[0].checkout, None);
        let back = serde_json::to_value(&rows[0]).unwrap();
        assert!(back.get("checkout").is_none(), "{back}");

        let with = body.replacen(
            r#""environment":"Windows""#,
            r#""environment":"Windows","checkout":"wt-a""#,
            1,
        );
        let rows: Vec<HostedSessionInfo> = serde_json::from_str(&with).unwrap();
        assert_eq!(rows[0].checkout.as_deref(), Some("wt-a"));
    }

    /// A folder expanded while a poll is in flight has to reach the peer NOW: the
    /// peer only learns of a dir a request named, so without the bell the browser
    /// opened a project blind for a whole 25 s window — the burst of `watch`
    /// frames lands after the first poll is already out.
    #[tokio::test]
    async fn a_watch_added_after_the_first_poll_rings_the_bell() {
        let set = PeerWatchSet::new("", false);
        let bell = set.changed.clone();
        let waiting = tokio::spawn(async move { bell.notified().await });
        tokio::task::yield_now().await;

        set.add("docs/agents", false);
        tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .expect("adding a watched dir must wake the poll in flight")
            .unwrap();
        assert!(set.snapshot().contains(&"docs/agents".to_string()));
        assert!(!set.runs());
    }

    /// A repeated `watch` for a dir already held is a no-op, and must not cost a
    /// re-post: a browser re-sending its set would otherwise restart every poll.
    #[tokio::test]
    async fn re_adding_a_held_dir_does_not_ring_the_bell() {
        let set = PeerWatchSet::new("docs", false);
        set.add("docs", false);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), set.changed.notified())
                .await
                .is_err(),
            "an idempotent watch must not restart the poll in flight"
        );
    }

    /// The poll deadline has to outlast the window the peer was ASKED to hold,
    /// with room for the peer's own work either side of it. When it did not, every
    /// poll timed out 800 ms before the answer arrived and the browser stopped
    /// receiving `tree.dirty` altogether.
    #[test]
    fn a_poll_deadline_outlasts_the_window_it_asks_for() {
        let window = Duration::from_millis(PEER_POLL_WINDOW_MS);
        assert!(
            PEER_POLL_DEADLINE >= window + Duration::from_secs(10),
            "a {PEER_POLL_DEADLINE:?} deadline leaves too little over a {window:?} window"
        );
    }

    /// Only a poll that actually reported a change may be followed by an instant
    /// re-post. Every other outcome — including a 200 nobody can read — has to
    /// wait, because each poll costs one TCP connection and one ephemeral port on
    /// the polling host (the 2026-09-01 exhaustion).
    #[test]
    fn only_a_changed_poll_re_posts_at_once() {
        assert_eq!(next_poll_delay(PollCycle::Changed), Duration::ZERO);
        assert_eq!(next_poll_delay(PollCycle::Quiet), PEER_POLL_QUIET_PAUSE);
        assert_eq!(next_poll_delay(PollCycle::Unreadable), PEER_POLL_BACKOFF);
        assert_eq!(next_poll_delay(PollCycle::Failed), PEER_POLL_BACKOFF);
        for cycle in [PollCycle::Quiet, PollCycle::Unreadable, PollCycle::Failed] {
            assert!(
                !next_poll_delay(cycle).is_zero(),
                "{cycle:?} must not re-post at once"
            );
        }
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

    async fn desk_get(dir: &Path) -> String {
        let res = desk_router(dir)
            .oneshot(
                Request::builder()
                    .uri("/api/desk")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        body_text(res).await
    }

    /// PUT a RAW body — the only way to exercise a shape the `DeskUpload`
    /// extractor must refuse (a bare array, an out-of-range float literal).
    async fn desk_put_raw(dir: &Path, body: String) -> Response {
        desk_router(dir)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/desk")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn desk_put(dir: &Path, body: &serde_json::Value) -> Response {
        desk_put_raw(dir, body.to_string()).await
    }

    /// The `{ windows, fences }` upload body (#340).
    fn desk_body(windows: serde_json::Value, fences: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "windows": windows, "fences": fences })
    }

    fn fence_json(id: &str, name: &str, ts: i64) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name": name,
            "rect": { "left": 40.0, "top": 40.0, "width": 720.0, "height": 460.0 },
            "ts": ts,
        })
    }

    fn desk_json(id: &str, ts: i64, session_id: serde_json::Value, max: bool) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "repo": "owner/repo",
            "agent": "claude",
            "kind": "console",
            "rect": { "left": 10.0, "top": 20.0, "width": 640.0, "height": 480.0 },
            "max": max,
            "sessionId": session_id,
            "ts": ts,
        })
    }

    #[tokio::test]
    async fn api_desk_empty_when_no_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            desk_get(dir.path()).await,
            r#"{"windows":[],"fences":[],"notes":[]}"#
        );
        assert!(
            !dir.path().join("desk.toml").exists(),
            "a GET must not create the store"
        );
    }

    /// The desk route's body is an OBJECT carrying both record types (#340), so
    /// an empty desk is `{"windows":[],"fences":[],"notes":[]}` — not a bare `[]`.
    #[tokio::test]
    async fn api_desk_serves_windows_and_fences_together() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            desk_get(dir.path()).await,
            r#"{"windows":[],"fences":[],"notes":[]}"#,
            "the desk body carries both record types"
        );
    }

    #[tokio::test]
    async fn api_desk_put_then_get_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let payload = desk_body(
            serde_json::json!([
                desk_json("w-a", 1, serde_json::json!(7), true),
                desk_json("w-b", 2, serde_json::Value::Null, false),
            ]),
            serde_json::json!([]),
        );
        let res = desk_put(dir.path(), &payload).await;
        assert_eq!(res.status(), StatusCode::OK);

        let body = desk_get(dir.path()).await;
        assert!(
            body.contains("\"sessionId\":7"),
            "camelCase wire key: {body}"
        );
        assert!(body.contains("\"max\":true"), "maximized survives: {body}");
        let a = body.find("w-a").expect("first record present");
        let b = body.find("w-b").expect("second record present");
        assert!(a < b, "layout order is preserved: {body}");
    }

    #[tokio::test]
    async fn api_desk_put_prunes_to_24_newest_by_ts() {
        let dir = tempfile::tempdir().unwrap();
        let payload = desk_body(
            serde_json::Value::Array(
                (1..=30)
                    .map(|n| desk_json(&format!("w{n}"), n, serde_json::Value::Null, false))
                    .collect(),
            ),
            serde_json::json!([]),
        );
        let res = desk_put(dir.path(), &payload).await;
        assert_eq!(res.status(), StatusCode::OK);
        let put_body: desk::DeskStore = serde_json::from_str(&body_text(res).await).unwrap();
        let ids: Vec<String> = put_body.windows.into_iter().map(|r| r.id).collect();
        let expected: Vec<String> = (7..=30).map(|n| format!("w{n}")).collect();
        assert_eq!(ids, expected, "the PUT answers with the pruned truth");

        let get_body: desk::DeskStore = serde_json::from_str(&desk_get(dir.path()).await).unwrap();
        let ids: Vec<String> = get_body.windows.into_iter().map(|r| r.id).collect();
        assert_eq!(ids, expected, "and the persisted desk holds the same 24");
    }

    #[tokio::test]
    async fn api_desk_put_rejects_a_malformed_body_without_touching_the_store() {
        let dir = tempfile::tempdir().unwrap();
        desk_put(
            dir.path(),
            &desk_body(
                serde_json::json!([desk_json("w-a", 1, serde_json::Value::Null, false)]),
                serde_json::json!([]),
            ),
        )
        .await;
        let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();

        let res = desk_put(dir.path(), &serde_json::json!({ "not": "an array" })).await;
        assert_eq!(
            res.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "the strict DeskUpload extractor rejects an unknown-field body — it \
             must never read as an EMPTY desk that wipes the layout"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
            before,
            "a rejected upload never reaches the store"
        );
    }

    /// A note card on the wire (ADR-0064 §2): placement only.
    fn note_json(id: &str, path: &str, ts: i64) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "path": path,
            "rect": { "left": 80.0, "top": 120.0, "width": 240.0, "height": 180.0 },
            "ts": ts,
        })
    }

    /// The third collection travels the same route as the other two: a PUT
    /// carrying `notes` stores them, and a GET serves them back.
    #[tokio::test]
    async fn api_desk_round_trips_notes() {
        let dir = tempfile::tempdir().unwrap();
        let mut body = desk_body(serde_json::json!([]), serde_json::json!([]));
        body["notes"] = serde_json::json!([note_json("n1", ".ralphy/notes/a.note", 1)]);
        let res = desk_put(dir.path(), &body).await;
        assert_eq!(res.status(), StatusCode::OK);

        let served = desk_get(dir.path()).await;
        assert!(served.contains(r#""id":"n1""#), "{served}");
        assert!(
            served.contains(r#""path":".ralphy/notes/a.note""#),
            "{served}"
        );
        // Placement only: the wire record carries no text and no colour.
        assert!(!served.contains("markdown"), "{served}");
        assert!(!served.contains("color"), "{served}");
    }

    /// The rect guard names the record type it refused, so the shell's console
    /// says which card is off the stage.
    #[tokio::test]
    async fn api_desk_refuses_a_note_with_an_out_of_frame_rect() {
        let dir = tempfile::tempdir().unwrap();
        let mut bad = note_json("n-huge", "a.note", 1);
        bad["rect"]["top"] = serde_json::json!(-1.0);
        let mut body = desk_body(serde_json::json!([]), serde_json::json!([]));
        body["notes"] = serde_json::json!([bad]);
        let res = desk_put(dir.path(), &body).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let text = body_text(res).await;
        assert!(
            text.contains("note n-huge has an out-of-frame rect"),
            "the refusal names the note: {text}"
        );
        assert!(
            !dir.path().join("desk.toml").exists(),
            "a refused upload writes nothing"
        );
    }

    /// A card's `checkout` is the same kind of name as a window's, gated
    /// before anything is written (ADR-0064 §4).
    #[tokio::test]
    async fn api_desk_refuses_a_note_whose_checkout_is_not_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut bad = note_json("n-bad", "a.note", 1);
        bad["checkout"] = serde_json::json!("../escape");
        let mut body = desk_body(serde_json::json!([]), serde_json::json!([]));
        body["notes"] = serde_json::json!([bad]);
        let res = desk_put(dir.path(), &body).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let text = body_text(res).await;
        assert!(
            text.contains("checkout ../escape on record n-bad is not a valid name"),
            "{text}"
        );
    }

    /// The daemon caps the collection whatever the browser uploads.
    #[tokio::test]
    async fn api_desk_prunes_notes_to_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let notes: Vec<serde_json::Value> = (1..=desk::NOTE_MAX as i64 + 3)
            .map(|n| note_json(&format!("n{n}"), &format!("a{n}.note"), n))
            .collect();
        let mut body = desk_body(serde_json::json!([]), serde_json::json!([]));
        body["notes"] = serde_json::json!(notes);
        let res = desk_put(dir.path(), &body).await;
        assert_eq!(res.status(), StatusCode::OK);
        let stored = desk::load_from(&dir.path().join("desk.toml"));
        assert_eq!(stored.notes.len(), desk::NOTE_MAX);
        assert!(
            !stored.notes.iter().any(|n| n.id == "n1"),
            "the oldest card was evicted"
        );
    }

    /// A shell older than this slice sends no `notes` key at all, and its
    /// upload must not wipe the cards another page owns.
    #[tokio::test]
    async fn an_upload_without_notes_keeps_the_stored_cards() {
        let dir = tempfile::tempdir().unwrap();
        let mut body = desk_body(serde_json::json!([]), serde_json::json!([]));
        body["notes"] = serde_json::json!([note_json("n1", "a.note", 1)]);
        body["removed"] = serde_json::json!({ "windows": [], "fences": [], "notes": [] });
        desk_put(dir.path(), &body).await;

        let mut older = desk_body(serde_json::json!([]), serde_json::json!([]));
        older["removed"] = serde_json::json!({ "windows": [], "fences": [] });
        let res = desk_put(dir.path(), &older).await;
        assert_eq!(res.status(), StatusCode::OK);
        let stored = desk::load_from(&dir.path().join("desk.toml"));
        assert_eq!(stored.notes.len(), 1, "the card survived the fold");
    }

    /// ADR-0063 §4: the selected checkout per repo ref rides the desk body,
    /// answered on the PUT and served on the next GET.
    #[tokio::test]
    async fn api_desk_round_trips_checkouts() {
        let dir = tempfile::tempdir().unwrap();
        let res = desk_put(
            dir.path(),
            &serde_json::json!({
                "windows": [],
                "fences": [],
                "checkouts": { "owner/repo": "wt-a" },
            }),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let put_body = body_text(res).await;
        assert!(
            put_body.contains(r#""checkouts":{"owner/repo":"wt-a"}"#),
            "the PUT answers the checkouts: {put_body}"
        );
        let get_body = desk_get(dir.path()).await;
        assert!(
            get_body.contains(r#""checkouts":{"owner/repo":"wt-a"}"#),
            "the GET serves them: {get_body}"
        );

        // Clearing the selection drops the key from the wire body entirely —
        // an empty map is not serialised, so the old exact shape holds.
        let res = desk_put(
            dir.path(),
            &serde_json::json!({ "windows": [], "fences": [], "checkouts": {} }),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            desk_get(dir.path()).await,
            r#"{"windows":[],"fences":[],"notes":[]}"#
        );
    }

    /// A registry whose `owner/repo` entry lists `path-abc` as a former slug —
    /// what the CLI's migration leaves behind after a gained remote.
    fn registry_with_former_slug(dir: &Path) {
        let mut store = registry::RegistryStore::default();
        store.upsert("path-abc", "/repo");
        store.rekey("path-abc", "owner/repo");
        registry::save_to(&store, &dir.join("repos.toml")).unwrap();
    }

    /// A desk saved BEFORE a re-key still names the former slug on disk; the
    /// GET serves it under the canonical key so the migrated project's
    /// consoles come back to it (ADR-0036 amendment 2026-09-16).
    #[tokio::test]
    async fn desk_get_serves_a_former_slug_as_its_canonical_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut stale = desk::DeskStore::default();
        stale.windows.push(desk::DeskRecord {
            id: "w1".into(),
            repo: "path-abc".into(),
            rect: desk::DeskRect {
                left: 1.0,
                top: 1.0,
                width: 300.0,
                height: 200.0,
            },
            ..desk::DeskRecord::default()
        });
        stale.checkouts.insert("path-abc".into(), "wt-a".into());
        desk::save_to(&stale, &dir.path().join("desk.toml")).unwrap();
        registry_with_former_slug(dir.path());

        let body = desk_get(dir.path()).await;
        assert!(
            body.contains(r#""repo":"owner/repo""#) && !body.contains("path-abc"),
            "the record follows the key: {body}"
        );
        assert!(
            body.contains(r#""checkouts":{"owner/repo":"wt-a"}"#),
            "the selection follows the key: {body}"
        );
    }

    /// A tab that read the desk before the re-key uploads the former slug
    /// back; the PUT normalizes it, so `desk.toml` converges on the first save
    /// whichever tab saves — and never regresses to the hash.
    #[tokio::test]
    async fn desk_put_normalizes_a_stale_upload_through_former_slugs() {
        let dir = tempfile::tempdir().unwrap();
        registry_with_former_slug(dir.path());
        let res = desk_put(
            dir.path(),
            &serde_json::json!({
                "windows": [{
                    "id": "w1", "repo": "path-abc", "agent": "claude", "kind": "agent",
                    "rect": { "left": 1, "top": 1, "width": 300, "height": 200 },
                    "max": false, "sessionId": null, "ts": 1,
                }],
                "fences": [],
                "checkouts": { "path-abc": "wt-stale", "owner/repo": "wt-live" },
            }),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let put_body = body_text(res).await;
        assert!(
            put_body.contains(r#""repo":"owner/repo""#) && !put_body.contains("path-abc"),
            "the answer is the canonical truth: {put_body}"
        );
        assert!(
            put_body.contains(r#""checkouts":{"owner/repo":"wt-live"}"#),
            "the canonical selection wins the collision: {put_body}"
        );
        let on_disk = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();
        assert!(
            !on_disk.contains("path-abc"),
            "the former slug never reaches the store: {on_disk}"
        );
    }

    /// Two pages on one desk (ADR-0050 amendment 2026-09-20). Page A records
    /// a console; page B, whose mirror predates it, flushes a body without
    /// it — and WITH `removed`, which says B deleted nothing. A's record
    /// survives the fold; a close B does name is dropped; and a body without
    /// `removed` (a shell from before the amendment) is still the wholesale
    /// replace, so an old page loses nothing it could not have said.
    #[tokio::test]
    async fn api_desk_folds_a_page_s_upload_into_the_other_pages_records() {
        let dir = tempfile::tempdir().unwrap();
        let rect = serde_json::json!({ "left": 1, "top": 1, "width": 300, "height": 200 });
        let record = |id: &str, ts: i64, session: Option<u64>| {
            serde_json::json!({
                "id": id, "repo": "owner/repo", "agent": "console", "kind": "console",
                "rect": rect, "max": false, "sessionId": session, "ts": ts,
            })
        };
        let empty_removed = serde_json::json!({ "windows": [], "fences": [], "checkouts": [] });
        // Page A: its console, with the daemon's session id.
        let res = desk_put(
            dir.path(),
            &serde_json::json!({ "windows": [record("a", 10, Some(5))], "fences": [], "removed": empty_removed }),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        // Page B: a stale mirror that never saw `a`, plus its own window.
        let res = desk_put(
            dir.path(),
            &serde_json::json!({ "windows": [record("b", 11, Some(6))], "fences": [], "removed": empty_removed }),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let got = desk_get(dir.path()).await;
        assert!(
            got.contains(r#""id":"b""#)
                && got.contains(r#""id":"a""#)
                && got.contains(r#""sessionId":5"#),
            "A's record and its session id survive B's flush: {got}"
        );
        // Page B again, with a STALE copy of `a` (older ts, no session id): the
        // store's newer copy wins.
        let res = desk_put(
            dir.path(),
            &serde_json::json!({ "windows": [record("a", 1, None), record("b", 12, Some(6))], "fences": [], "removed": empty_removed }),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let got = desk_get(dir.path()).await;
        assert!(
            got.contains(r#""sessionId":5"#),
            "the newer copy of `a` wins: {got}"
        );
        // Page A closes `a`: said in `removed`, so the fold drops it.
        let res = desk_put(
            dir.path(),
            &serde_json::json!({ "windows": [], "fences": [], "removed": { "windows": ["a"], "fences": [], "checkouts": [] } }),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let got = desk_get(dir.path()).await;
        assert!(
            !got.contains(r#""id":"a""#) && got.contains(r#""id":"b""#),
            "a close is a close: {got}"
        );
        // A shell from before the amendment: no `removed`, wholesale replace.
        let res = desk_put(
            dir.path(),
            &serde_json::json!({ "windows": [record("c", 1, None)], "fences": [] }),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let got = desk_get(dir.path()).await;
        assert!(
            got.contains(r#""id":"c""#) && !got.contains(r#""id":"b""#),
            "an old shell replaces: {got}"
        );
    }

    /// A checkout value that is not one path component is refused as `400`
    /// before any write — the desk is the one place a name is stored, so a
    /// traversal must never be persisted for a later verb to prefix.
    /// #411: a record's own `checkout` round-trips and is gated by the same
    /// name check as the selection map.
    #[tokio::test]
    async fn api_desk_round_trips_and_gates_a_records_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let record = |checkout: &str| {
            serde_json::json!({
                "id": "w1", "repo": "owner/repo", "agent": "claude", "kind": "agent",
                "rect": { "left": 1, "top": 1, "width": 300, "height": 200 },
                "max": false, "sessionId": null, "checkout": checkout, "ts": 1,
            })
        };
        let res = desk_put(
            dir.path(),
            &serde_json::json!({ "windows": [record("wt-a")], "fences": [], "checkouts": {} }),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let get_body = desk_get(dir.path()).await;
        assert!(
            get_body.contains(r#""checkout":"wt-a""#),
            "the GET serves the record's checkout: {get_body}"
        );
        let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();
        let res = desk_put(
            dir.path(),
            &serde_json::json!({ "windows": [record("../x")], "fences": [], "checkouts": {} }),
        )
        .await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
            before,
            "a refused record checkout never reaches the store"
        );
    }

    /// Lock amendment (ADR-0050/0051, 2026-09-20): a locked window and a
    /// locked fence round-trip through the route, a newer unlock clears the key
    /// from the wire again, and a body without the key still parses.
    #[tokio::test]
    async fn api_desk_round_trips_a_lock_on_a_record_and_a_fence() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = desk_json("w1", 1, serde_json::Value::Null, false);
        w["locked"] = serde_json::json!(true);
        let mut f = fence_json("f1", "backend", 1);
        f["locked"] = serde_json::json!(true);
        let res = desk_put(
            dir.path(),
            &serde_json::json!({ "windows": [w], "fences": [f], "checkouts": {} }),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let get_body = desk_get(dir.path()).await;
        assert_eq!(
            get_body.matches(r#""locked":true"#).count(),
            2,
            "the GET serves both locks: {get_body}"
        );
        let toml = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();
        assert_eq!(toml.matches("locked = true").count(), 2, "toml={toml}");
        // A newer copy without the key is an unlock — and the key leaves the wire.
        let res = desk_put(
            dir.path(),
            &serde_json::json!({
                "windows": [desk_json("w1", 2, serde_json::Value::Null, false)],
                "fences": [fence_json("f1", "backend", 2)],
                "checkouts": {},
            }),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let get_body = desk_get(dir.path()).await;
        assert!(
            !get_body.contains("locked"),
            "an unlocked desk carries no key: {get_body}"
        );
    }

    #[tokio::test]
    async fn api_desk_refuses_a_malformed_checkout_name() {
        let dir = tempfile::tempdir().unwrap();
        desk_put(
            dir.path(),
            &serde_json::json!({
                "windows": [],
                "fences": [],
                "checkouts": { "owner/repo": "wt-a" },
            }),
        )
        .await;
        let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();

        for bad in ["a/b", "../x", "", "a\\b", "."] {
            let res = desk_put(
                dir.path(),
                &serde_json::json!({
                    "windows": [],
                    "fences": [],
                    "checkouts": { "owner/repo": bad },
                }),
            )
            .await;
            assert_eq!(res.status(), StatusCode::BAD_REQUEST, "checkout {bad:?}");
            assert_eq!(
                std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
                before,
                "a refused checkout {bad:?} never reaches the store"
            );
        }
    }

    #[tokio::test]
    async fn api_desk_put_rejects_a_negative_left_without_touching_the_store() {
        let dir = tempfile::tempdir().unwrap();
        desk_put(
            dir.path(),
            &desk_body(
                serde_json::json!([desk_json("w-a", 1, serde_json::Value::Null, false)]),
                serde_json::json!([]),
            ),
        )
        .await;
        let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();

        let mut bad = desk_json("w-neg", 2, serde_json::Value::Null, false);
        bad["rect"]["left"] = serde_json::json!(-1.0);
        let res = desk_put(
            dir.path(),
            &desk_body(serde_json::json!([bad]), serde_json::json!([])),
        )
        .await;
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "the stage origin is pinned at 0,0 — a negative left is off the plane"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
            before,
            "a rejected rect never reaches the store"
        );
    }

    #[tokio::test]
    async fn api_desk_put_rejects_a_non_finite_rect_without_touching_the_store() {
        let dir = tempfile::tempdir().unwrap();
        desk_put(
            dir.path(),
            &desk_body(
                serde_json::json!([desk_json("w-a", 1, serde_json::Value::Null, false)]),
                serde_json::json!([]),
            ),
        )
        .await;
        let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();

        // An out-of-range literal, spelled in the RAW body — a Rust `1e400_f64`
        // will not compile, and `json!(f64::INFINITY)` becomes `null`, so the
        // only way to reproduce what a browser can actually send is the wire.
        let bad = desk_json("w-huge", 2, serde_json::Value::Null, false)
            .to_string()
            .replace("\"left\":10.0", "\"left\":1e400");
        let res = desk_put_raw(dir.path(), format!(r#"{{"windows":[{bad}],"fences":[]}}"#)).await;
        assert_ne!(
            res.status(),
            StatusCode::OK,
            "a non-finite rect must never be stored"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
            before,
            "a rejected rect never reaches the store"
        );
    }

    /// The pre-#340 wire shape must be refused WHOLESALE, not half-applied: a
    /// stale client that still PUTs a bare array would otherwise be read as an
    /// empty desk and wipe the operator's layout.
    #[tokio::test]
    async fn api_desk_put_rejects_the_pre_340_bare_array() {
        let dir = tempfile::tempdir().unwrap();
        desk_put(
            dir.path(),
            &desk_body(
                serde_json::json!([desk_json("w-a", 1, serde_json::Value::Null, false)]),
                serde_json::json!([fence_json("f-a", "backend", 1)]),
            ),
        )
        .await;
        let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();

        let stale = serde_json::json!([desk_json("w-b", 2, serde_json::Value::Null, false)]);
        let res = desk_put_raw(dir.path(), stale.to_string()).await;
        assert_eq!(
            res.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "the bare-array body is not a desk upload any more"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
            before,
            "a rejected upload never reaches the store"
        );

        // Every sequence shape that could satisfy the struct POSITIONALLY, each
        // its own leg — these are the bodies that wipe the desk when they land,
        // and each defeats a different half-fix. `[]` needs both fields
        // defaulted; `[[],[]]` supplies both required fields as two elements and
        // survived dropping the defaults; a map missing one key is the shape
        // that goes green again if `#[serde(default)]` is ever restored to a
        // single field. All three were measured green-then-red on this route.
        for body in ["[]", "[[],[]]", r#"{"windows":[]}"#, r#"{"fences":[]}"#] {
            let res = desk_put_raw(dir.path(), body.into()).await;
            assert_eq!(
                res.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "`{body}` must not read as a desk upload"
            );
            assert_eq!(
                std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
                before,
                "the operator's desk survives `{body}`"
            );
        }
    }

    #[tokio::test]
    async fn api_desk_put_rejects_a_fence_with_a_non_finite_rect() {
        let dir = tempfile::tempdir().unwrap();
        desk_put(
            dir.path(),
            &desk_body(
                serde_json::json!([]),
                serde_json::json!([fence_json("f-a", "backend", 1)]),
            ),
        )
        .await;
        let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();

        // LEG 1 — the wire. Measured: `serde_json` refuses an out-of-range float
        // literal as a SYNTAX error ("number out of range"), which axum maps to
        // 400 — so a non-finite rect dies in the extractor and never reaches the
        // route's own guard. Same status, different body.
        let bad = fence_json("w-huge", "planning", 2)
            .to_string()
            .replace("\"left\":40.0", "\"left\":1e999");
        let res = desk_put_raw(dir.path(), format!(r#"{{"windows":[],"fences":[{bad}]}}"#)).await;
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "an out-of-range literal dies in the extractor"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
            before,
            "a rejected fence never reaches the store"
        );

        // LEG 2 — the route's OWN guard, which the wire can no longer reach:
        // called directly with an infinity the extractor would have refused, so
        // the 400 and its wording are proved rather than assumed. A fresh
        // response, not the one leg 1 asserted on.
        let res = desk_put_route(
            dir.path().join("desk.toml"),
            dir.path().join("repos.toml"),
            desk::DeskUpload {
                windows: vec![],
                fences: vec![desk::DeskFence {
                    id: "w-huge".into(),
                    name: "planning".into(),
                    rect: desk::DeskRect {
                        left: f64::INFINITY,
                        top: 40.0,
                        width: 720.0,
                        height: 460.0,
                    },
                    locked: false,
                    ts: 2,
                }],
                notes: vec![],
                checkouts: std::collections::BTreeMap::new(),
                removed: None,
            },
        )
        .await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let body = body_text(res).await;
        assert!(
            body.contains("fence w-huge has an out-of-frame rect"),
            "the refusal names the fence: {body}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
            before,
            "the guard returns BEFORE any write"
        );
    }

    #[tokio::test]
    async fn api_desk_put_prunes_fences_to_the_12_newest() {
        let dir = tempfile::tempdir().unwrap();
        let payload = desk_body(
            serde_json::json!([]),
            serde_json::Value::Array(
                (1..=13)
                    .map(|n| fence_json(&format!("f{n}"), "region", n))
                    .collect(),
            ),
        );
        let res = desk_put(dir.path(), &payload).await;
        assert_eq!(res.status(), StatusCode::OK);
        let put_body: desk::DeskStore = serde_json::from_str(&body_text(res).await).unwrap();
        let ids: Vec<String> = put_body.fences.into_iter().map(|f| f.id).collect();
        let expected: Vec<String> = (2..=13).map(|n| format!("f{n}")).collect();
        assert_eq!(ids, expected, "the PUT answers with the pruned truth");

        let get_body: desk::DeskStore = serde_json::from_str(&desk_get(dir.path()).await).unwrap();
        let ids: Vec<String> = get_body.fences.into_iter().map(|f| f.id).collect();
        assert_eq!(ids, expected, "and the persisted desk holds the same 12");
    }

    #[test]
    fn security_state_reflects_the_stores() {
        let dir = tempfile::tempdir().unwrap();
        // Empty store → every factor unset.
        let s = security_state_at(dir.path());
        assert!(!s.token_set && !s.password_set && !s.totp_enrolled && !s.require_login);
        // Writing a seed flips totp_enrolled — but require_login is now the
        // PERSISTED opt-in flag (amendment §A), NOT derived from the seed.
        totp::save_seed_to(&totp::generate_seed(), &totp::seed_path_in(dir.path())).unwrap();
        let s = security_state_at(dir.path());
        assert!(
            s.totp_enrolled && !s.require_login,
            "seed → enrolled, but require_login stays off until opted in"
        );
        // Setting the flag flips require_login on its own.
        auth::set_require_login_in(dir.path(), true).unwrap();
        assert!(
            security_state_at(dir.path()).require_login,
            "the flag drives require_login"
        );
        assert!(!s.token_set && !s.password_set, "other factors still unset");
    }

    #[test]
    fn enroll_totp_is_mint_once_with_ralphy_uri() {
        let dir = tempfile::tempdir().unwrap();
        let (uri, minted) = enroll_totp_at(dir.path()).unwrap();
        assert!(minted, "first enrol mints");
        assert!(
            uri.starts_with("otpauth://totp/ralphy:"),
            "the real provisioning URI; got {uri}"
        );
        let secret_of = |u: &str| {
            u.split("secret=")
                .nth(1)
                .and_then(|s| s.split('&').next())
                .unwrap()
                .to_string()
        };
        let (uri2, minted2) = enroll_totp_at(dir.path()).unwrap();
        assert!(!minted2, "second enrol does not re-mint");
        assert_eq!(secret_of(&uri), secret_of(&uri2), "same secret returned");
    }

    #[test]
    fn enroll_then_confirm_arms_the_live_seed() {
        let dir = tempfile::tempdir().unwrap();
        // Seed the PENDING slot with the RFC vector so we know a valid code.
        totp::save_seed_to(
            &totp::Seed::from_bytes(b"12345678901234567890".to_vec()),
            &totp::pending_seed_path_in(dir.path()),
        )
        .unwrap();
        // Pending enrolment does not count as enrolled yet.
        assert!(
            !security_state_at(dir.path()).totp_enrolled,
            "a pending seed is not enrolled"
        );
        // A wrong code arms nothing.
        assert!(!confirm_totp_at(dir.path(), "999999", 59).unwrap());
        assert!(!security_state_at(dir.path()).totp_enrolled);
        // The RFC vector code (T=59 → 287082) confirms and arms the live seed.
        assert!(confirm_totp_at(dir.path(), "287082", 59).unwrap());
        assert!(
            security_state_at(dir.path()).totp_enrolled,
            "confirming arms TOTP"
        );
        // Revoke clears both live and (already-consumed) pending.
        revoke_totp_at(dir.path()).unwrap();
        assert!(!security_state_at(dir.path()).totp_enrolled);
    }

    #[test]
    fn set_password_round_trips_set_then_clear() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            set_password_at(dir.path(), Some("pw")).unwrap(),
            "set → true"
        );
        assert!(
            security_state_at(dir.path()).password_set,
            "state reflects the set"
        );
        assert!(!set_password_at(dir.path(), None).unwrap(), "clear → false");
        assert!(
            !security_state_at(dir.path()).password_set,
            "state reflects the clear"
        );
    }

    #[test]
    fn remint_token_yields_a_new_distinct_token() {
        let dir = tempfile::tempdir().unwrap();
        remint_token_at(dir.path()).unwrap();
        let first = auth::load_token_from(&auth::token_path_in(dir.path()))
            .unwrap()
            .expect("token written");
        assert_eq!(first.len(), 64, "64-hex token");
        remint_token_at(dir.path()).unwrap();
        let second = auth::load_token_from(&auth::token_path_in(dir.path()))
            .unwrap()
            .unwrap();
        assert_ne!(first, second, "remint rotates the token");
    }

    #[test]
    fn require_login_gate_needs_an_enrolled_seed() {
        let dir = tempfile::tempdir().unwrap();
        // Enabling with no seed is refused; disabling is always Ok.
        assert!(require_login_at(dir.path(), true).is_err());
        assert!(require_login_at(dir.path(), false).is_ok());
        // With a seed enrolled, enabling is Ok.
        totp::save_seed_to(&totp::generate_seed(), &totp::seed_path_in(dir.path())).unwrap();
        assert!(require_login_at(dir.path(), true).is_ok());
    }

    #[tokio::test]
    async fn root_serves_the_embedded_page() {
        let resp = get("/").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        // Anchored on the document TITLE, not on chrome text. The old anchor was
        // the login card's "ralphy daemon" brand, which made a copy edit on one
        // screen look like a broken asset pipeline — and "daemon" left that
        // brand naming the process rather than the thing being logged into.
        assert!(
            body.contains("<title>ralphy · workbench shell</title>"),
            "the page must identify the daemon; got: {body}"
        );
    }

    #[tokio::test]
    async fn xterm_asset_is_served() {
        // The embedded xterm.js loads over HTTP with a JS content-type — the
        // terminal UI can pull it from `/vendor/xterm.js`.
        let resp = get("/vendor/xterm.js").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(!body.is_empty(), "the embedded xterm.js must be non-empty");
    }

    /// `/api/session` is allowlisted pre-login, so what it carries is what an
    /// UNAUTHENTICATED caller can read. It must carry the avatar (the login card
    /// wears this daemon's face) and it must NOT carry the name — the gate stays
    /// opaque about everything the operator wrote themselves.
    #[tokio::test]
    async fn api_session_carries_the_avatar_but_never_the_name() {
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let resp = router(
            Some(id),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/session")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("🐙"), "must carry the avatar; got: {body}");
        assert!(
            !body.contains("anvil"),
            "must NOT carry the name; got: {body}"
        );
    }

    /// An un-baptized daemon has no avatar, and `skip_serializing_if` keeps the
    /// key off the wire entirely — the SPA's `if (s.avatar)` then leaves the
    /// fallback mark in place rather than painting an empty box.
    #[tokio::test]
    async fn api_session_omits_the_avatar_when_unbaptized() {
        let body = body_string(get_local("/api/session").await).await;
        assert!(
            !body.contains("avatar"),
            "no avatar key expected; got: {body}"
        );
    }

    #[tokio::test]
    async fn unknown_path_is_404() {
        let resp = get("/no-such-asset").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_identity_route_returns_name_and_avatar() {
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let resp = router(
            Some(id),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/identity")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("anvil"),
            "body must carry the name; got: {body}"
        );
        assert!(
            body.contains("🐙"),
            "body must carry the avatar; got: {body}"
        );

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/identity")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn announced_descriptor_advertises_a_nudge_only_inside_wsl() {
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let inside = announced_descriptor(&id, 7443, Some("Ubuntu-22.04"), "tok".into());
        assert_eq!(inside.environment, "WSL: Ubuntu-22.04");
        assert_eq!(
            inside.nudge,
            Some(peer::NudgeSpec {
                distro: "Ubuntu-22.04".into(),
                unit: autostart::UNIT_NAME.into(),
            }),
            "a WSL daemon advertises how to wake it"
        );
        assert_eq!(
            inside.address, "127.0.0.1",
            "the peer transport is loopback"
        );
        assert_eq!(inside.port, 7443, "the BOUND port is announced");
        assert_eq!(inside.token, "tok", "the resolved token is announced as-is");
        assert_eq!(inside.daemon_id, id.id.to_string());

        let outside = announced_descriptor(&id, 7443, None, "tok".into());
        assert_eq!(
            outside.nudge, None,
            "no `wsl.exe` can reach a non-WSL daemon, so it advertises no nudge"
        );
        assert_ne!(outside.environment, "WSL: Ubuntu-22.04");
    }

    /// Announcing must never touch the auth policy: it takes the token it is
    /// given rather than minting over it (AC5).
    #[test]
    fn announce_skips_an_un_baptized_daemon_and_a_non_loopback_bind() {
        let dir = tempfile::tempdir().unwrap();
        let loopback = SocketAddr::from(([127, 0, 0, 1], 7443));
        announce_peer(
            &[dir.path().to_path_buf()],
            None,
            loopback,
            Some("tok".into()),
        );
        assert!(
            !dir.path().join("peers").exists(),
            "an un-baptized daemon announces nothing"
        );

        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        announce_peer(
            &[dir.path().to_path_buf()],
            Some(&id),
            SocketAddr::from(([10, 0, 0, 5], 7443)),
            Some("tok".into()),
        );
        assert!(
            !dir.path().join("peers").exists(),
            "a daemon that does not listen on loopback cannot be a peer"
        );

        // The happy path, and a SECOND store whose parent is a file — the write
        // fails there and must not stop the good one.
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "not a directory").unwrap();
        announce_peer(
            &[blocked, dir.path().to_path_buf()],
            Some(&id),
            loopback,
            Some("tok-given".into()),
        );
        let written = dir
            .path()
            .join("peers")
            .join(format!("{}.toml", ulid::Ulid::nil()));
        let back: peer::PeerDescriptor =
            toml::from_str(&std::fs::read_to_string(&written).unwrap()).unwrap();
        assert_eq!(
            back.token, "tok-given",
            "the resolved token is announced, never re-minted over"
        );
    }

    #[tokio::test]
    async fn api_peer_hello_serves_the_handshake_and_404s_un_baptized() {
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let resp = router(
            Some(id),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/peer/hello")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("\"daemon_id\":\"00000000000000000000000000\""),
            "the handshake must carry the daemon_id; got: {body}"
        );
        assert!(
            body.contains(&format!(
                "\"protocol_version\":{}",
                peer::PEER_PROTOCOL_VERSION
            )),
            "the handshake must carry the peer protocol version; got: {body}"
        );
        assert!(
            body.contains("\"environment\":\"") && !body.contains("\"environment\":\"\""),
            "the handshake must name a non-empty environment; got: {body}"
        );

        // Un-baptized: 404, matching `/api/identity` — nothing for a peer to key on.
        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/peer/hello")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// Seed a scratch store with `repos.toml` plus a `peers/` descriptor pointing
    /// at a closed loopback port, and return the registry path `router` takes.
    fn seed_fleet_store(dir: &Path, peer_port: u16) -> PathBuf {
        let registry_path = dir.join("repos.toml");
        let mut store = registry::RegistryStore::default();
        store.upsert("owner/local", &dir.to_string_lossy());
        registry::save_to(&store, &registry_path).unwrap();
        peer::write_descriptor(
            dir,
            &peer::PeerDescriptor {
                daemon_id: "01PEERFAKE".into(),
                name: "wsl-box".into(),
                avatar: "🐺".into(),
                address: "127.0.0.1".into(),
                port: peer_port,
                environment: "WSL: Ubuntu-22.04".into(),
                token: "tok".into(),
                protocol_version: peer::PEER_PROTOCOL_VERSION,
                // Deliberately NOT a WSL peer: this descriptor points at a closed
                // loopback port, and announcing a distro it does not have would
                // make a failed dial ask the HOST whether that distro is running —
                // which is a real question with a real answer, so the state these
                // tests assert would then vary with the machine running them.
                nudge: None,
            },
        )
        .unwrap();
        registry_path
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

    #[tokio::test]
    async fn api_fleet_marks_an_unreachable_peer_and_keeps_the_local_repos() {
        let dir = tempfile::tempdir().unwrap();
        // A port nothing listens on: bound to learn a free one, then dropped.
        let closed = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        let registry_path = seed_fleet_store(dir.path(), closed);
        // A second peer, this one announcing a distro, so the view's `nudgeable`
        // has something to be true about.
        peer::write_descriptor(
            dir.path(),
            &peer::PeerDescriptor {
                daemon_id: "01PEERWSL".into(),
                name: "wsl-box".into(),
                avatar: "🐺".into(),
                address: "127.0.0.1".into(),
                port: closed,
                environment: "WSL: Ubuntu-22.04".into(),
                token: "tok".into(),
                protocol_version: peer::PEER_PROTOCOL_VERSION,
                nudge: Some(peer::NudgeSpec {
                    distro: "Ubuntu-22.04".into(),
                    unit: "ralphy-daemon.service".into(),
                }),
            },
        )
        .unwrap();
        // A file that is not a descriptor at all — it must degrade to one entry,
        // never fail the route.
        std::fs::write(
            dir.path().join("peers").join("junk.toml"),
            "not toml at all",
        )
        .unwrap();

        let resp = fleet_router(registry_path)
            .oneshot(
                Request::builder()
                    .uri("/api/fleet")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();

        let peers = body["peers"].as_array().unwrap();
        assert_eq!(peers.len(), 3, "both peers AND the bad record: {peers:?}");
        let live = peers
            .iter()
            .find(|p| p["daemon_id"] == "01PEERFAKE")
            .expect("the announced peer must be listed");
        assert_eq!(live["state"], "unreachable", "got: {live}");
        assert_eq!(live["environment"], "WSL: Ubuntu-22.04");
        assert!(
            live["diagnosis"]
                .as_str()
                .unwrap()
                .contains("WSL: Ubuntu-22.04"),
            "the diagnosis must name the environment; got: {live}"
        );
        assert!(
            !live["nudgeable"].as_bool().unwrap(),
            "a peer that announced no distro cannot be woken: {live}"
        );
        // The nudgeable surface, on the one peer that does advertise a distro. Its
        // STATE is host-dependent by design — a Windows host with WSL reads the
        // distro as stopped and says `asleep`, a host that cannot be asked degrades
        // to `unreachable` — so only the flag is asserted here; which unreachable
        // is which is pinned deterministically in `peer::tests`.
        let wsl = peers
            .iter()
            .find(|p| p["daemon_id"] == "01PEERWSL")
            .expect("the WSL peer must be listed");
        assert!(
            wsl["nudgeable"].as_bool().unwrap(),
            "an announced distro is what makes a peer nudgeable: {wsl}"
        );
        assert_ne!(wsl["state"], "reachable", "got: {wsl}");
        let bad = peers
            .iter()
            .find(|p| p["state"] == "malformed")
            .expect("a fold rejection is degraded, never dropped");
        assert_eq!(bad["name"], "junk.toml");

        // Federation must never blank the local sidebar.
        let repos = body["repos"].as_array().unwrap();
        assert_eq!(repos.len(), 1, "the local row survives: {repos:?}");
        assert_eq!(repos[0]["key"], "00000000000000000000000000/owner/local");
        assert_eq!(repos[0]["peer_state"], "local");
        assert_eq!(repos[0]["local"], true);
    }

    /// The route-level proof of "marked, never removed": a peer that answered
    /// once keeps its rows listed after it stops answering, with its state
    /// changed rather than its rows dropped. Liveness is still fresh — only the
    /// repo list is remembered.
    #[tokio::test]
    async fn api_fleet_keeps_a_peers_last_known_repos_after_it_stops_answering() {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let stub = Router::new()
            .route(
                "/api/peer/hello",
                axum::routing::get(|| async {
                    Json(serde_json::json!({"protocol_version": peer::PEER_PROTOCOL_VERSION}))
                }),
            )
            .route(
                "/api/repos",
                axum::routing::get(|| async {
                    // The shape a real peer serves: its own verdict on its own
                    // path travels with the row, because this daemon may not
                    // stat it (ADR-0052 §1).
                    Json(serde_json::json!([{
                        "slug": "owner/theirs",
                        "path": "/home/p/theirs",
                        "reachable": true,
                        "branch": "main",
                    }]))
                }),
            );
        let serving = tokio::spawn(async move {
            let _ = axum::serve(listener, stub).await;
        });

        let registry_path = seed_fleet_store(dir.path(), port);
        let app = fleet_router(registry_path);

        let fleet = |app: Router| async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .uri("/api/fleet")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = resp.into_body().collect().await.unwrap().to_bytes();
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()
        };

        let up = fleet(app.clone()).await;
        assert_eq!(up["peers"][0]["state"], "reachable", "got: {up}");
        let up_rows: Vec<&serde_json::Value> = up["repos"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["local"] == false)
            .collect();
        assert_eq!(up_rows.len(), 1, "the peer's repo is federated: {up}");
        assert_eq!(up_rows[0]["slug"], "owner/theirs");

        serving.abort();
        let _ = serving.await;

        let down = fleet(app.clone()).await;
        assert_eq!(
            down["peers"][0]["state"], "unreachable",
            "liveness is re-computed, never cached: {down}"
        );
        let down_rows: Vec<&serde_json::Value> = down["repos"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["local"] == false)
            .collect();
        assert_eq!(
            down_rows.len(),
            1,
            "the peer is MARKED, not removed — its last-known repos stay listed: {down}"
        );
        assert_eq!(down_rows[0]["peer_state"], "unreachable");
        assert_eq!(down_rows[0]["reachable"], false);
    }

    fn nudge_target(port: u16, address: &str) -> peer::PeerDescriptor {
        peer::PeerDescriptor {
            daemon_id: "01PEERWSL".into(),
            name: "wsl-box".into(),
            avatar: "🐺".into(),
            address: address.into(),
            port,
            environment: "WSL: Ubuntu-22.04".into(),
            token: "tok".into(),
            protocol_version: peer::PEER_PROTOCOL_VERSION,
            nudge: None,
        }
    }

    /// `nudged` used to mean "spawned `wsl.exe`", which is true a beat after the
    /// request and useless: the peer is still booting, and the caller's next act
    /// gets a 502. It must mean "answering".
    #[tokio::test]
    async fn a_nudge_reports_ready_only_once_the_peer_answers() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // Bound but unserved for now: the peer exists and refuses, exactly as a
        // distro that has started while its daemon has not.
        let late = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let stub = Router::new().route(
                "/api/peer/hello",
                axum::routing::get(|| async {
                    Json(serde_json::json!({
                        "daemon_id": "01PEERWSL",
                        "protocol_version": peer::PEER_PROTOCOL_VERSION,
                    }))
                }),
            );
            let _ = axum::serve(listener, stub).await;
        });

        let d = nudge_target(port, "127.0.0.1");
        let (ready, waited, status) = nudge_await_ready(
            &d,
            Some("00000000000000000000000000"),
            1,
            Duration::from_secs(10),
        )
        .await;
        late.abort();
        assert!(
            ready,
            "the peer came up and must be reported ready: {status:?}"
        );
        assert!(
            waited >= Duration::from_millis(250),
            "it answered in {waited:?} — too fast to prove anything was waited for"
        );
    }

    /// The give-up path: a peer that never comes back must end the wait with a
    /// verdict and the reason, not hang on the operator.
    #[tokio::test]
    async fn a_nudge_that_never_answers_gives_up_with_a_diagnosis() {
        let closed = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        let d = nudge_target(closed, "127.0.0.1");
        let (ready, _, status) = nudge_await_ready(
            &d,
            Some("00000000000000000000000000"),
            1,
            Duration::from_millis(200),
        )
        .await;
        assert!(!ready);
        assert_eq!(status.state(), "unreachable");
        assert!(
            status
                .diagnosis(&d.environment)
                .contains("WSL: Ubuntu-22.04"),
            "the give-up must still name the environment"
        );
    }

    /// A refusal is a verdict about the DESCRIPTOR, not a boot in progress. Waiting
    /// on one burns the whole deadline to reach the answer it already had.
    #[tokio::test]
    async fn a_nudge_does_not_wait_out_a_refusal() {
        let d = nudge_target(7257, "10.0.0.5");
        let started = Instant::now();
        let (ready, _, status) = nudge_await_ready(
            &d,
            Some("00000000000000000000000000"),
            1,
            Duration::from_secs(30),
        )
        .await;
        assert!(!ready);
        assert_eq!(status.state(), "refused");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a refusal was waited on for {:?} — it can never become reachable",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn api_fleet_nudge_404s_an_unknown_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let registry_path = seed_fleet_store(dir.path(), 7257);
        let resp = fleet_router(registry_path)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/fleet/nudge?daemon_id=01NOSUCHPEER")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_fleet_nudge_refuses_a_peer_that_announced_no_way_to_wake_it() {
        let dir = tempfile::tempdir().unwrap();
        let registry_path = seed_fleet_store(dir.path(), 7257);
        // Re-announce the same peer WITHOUT a nudge spec.
        peer::write_descriptor(
            dir.path(),
            &peer::PeerDescriptor {
                daemon_id: "01PEERFAKE".into(),
                name: "wsl-box".into(),
                avatar: "🐺".into(),
                address: "127.0.0.1".into(),
                port: 7257,
                environment: "WSL: Ubuntu-22.04".into(),
                token: "tok".into(),
                protocol_version: peer::PEER_PROTOCOL_VERSION,
                nudge: None,
            },
        )
        .unwrap();

        let resp = fleet_router(registry_path)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/fleet/nudge?daemon_id=01PEERFAKE")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("loginctl enable-linger"),
            "the refusal must name the prerequisite no nudge can substitute for; got: {body}"
        );
    }

    #[tokio::test]
    async fn api_about_route_reports_version_and_facts() {
        let resp = get("/api/about").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        // The build-embedded version string (git tag or Cargo fallback) — never empty.
        assert!(
            body.contains("\"version\":\"") && !body.contains("\"version\":\"\""),
            "about must carry a non-empty version; got: {body}"
        );
        assert!(
            body.contains("GPL-3.0"),
            "about must carry the license; got: {body}"
        );
        assert!(
            body.contains("Paulo Corcino"),
            "about must carry the creator; got: {body}"
        );
    }

    /// The workbench half of ADR-0056 is JS/HTML/CSS that no Rust gate compiles.
    /// CI runs `node --test`, but that suite calls module functions; it never
    /// renders this markup, and Playwright — which would — does not run there.
    /// So this pin over the served assets is what reds `cargo test` if the
    /// surface is deleted.
    #[test]
    fn the_release_badge_and_panel_are_pinned_in_the_served_assets() {
        let html = include_str!("../assets/ui/index.html");
        let app = include_str!("../assets/ui/app.js");
        let module = include_str!("../assets/ui/wb-release.js");
        let css = served_css();

        // The module is a plain global loaded by a tag, and the order matters:
        // app.js seeds its state from WBRelease.EMPTY at parse time.
        let module_tag = html.find("wb-release.js").expect("wb-release.js is loaded");
        let app_tag = html.find("src=\"app.js\"").expect("app.js is loaded");
        assert!(
            module_tag < app_tag,
            "wb-release.js must load before app.js, which seeds from it"
        );

        // The dot renders the daemon's severity; it must not be computed here.
        assert!(html.contains("class=\"rel-dot\" :class=\"release.severity\""));
        assert!(html.contains("x-show=\"releaseUnread\""));
        assert!(html.contains("@click=\"openWhatsNew()\""));
        assert!(html.contains("x-show=\"whatsNewOpen\""));
        // The whole gap, not just the newest release.
        assert!(html.contains("x-for=\"entry in release.gap\""));
        // The upgrade is a command the operator runs, never a button that
        // replaces a binary on the daemon's host from the browser.
        assert!(html.contains("ralphy update"));

        assert!(
            app.contains("this.loadRelease()"),
            "the shell reads it at init"
        );
        assert!(app.contains("get releaseUnread()"));
        assert!(
            app.contains("window.WBRelease.isSticky(this.release)"),
            "an urgent release must survive a dismissal"
        );
        assert!(
            app.contains("if (this.whatsNewOpen) return true;"),
            "the panel must join the focus trap"
        );

        assert!(module.contains("fetch('/api/release'"));
        assert!(
            module.contains("view.severity !== 'none'"),
            "loudness is the daemon's answer, not a browser-side derivation"
        );

        assert!(css.contains(".rel-dot.urgent"));

        // Every custom property the release block reads must be defined, or the
        // declaration is invalid at computed-value time and the property falls
        // back to its initial value. Not a hypothetical: `var(--muted)` and
        // `var(--panel)` are not in this palette, and the quiet dot — the
        // fixes-only severity, the most common one — rendered with no fill at
        // all while both `is_visible()` and a width assertion passed.
        let start = css
            .find("The release dot and the What's new panel")
            .expect("styles.css: the release block moved");
        let mut missing = Vec::new();
        for (at, _) in css[start..].match_indices("var(--") {
            let name = css[start + at + 4..]
                .split(')')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            if !css.contains(&format!("{name}:")) {
                missing.push(name);
            }
        }
        assert!(
            missing.is_empty(),
            "the release styles read custom properties this palette does not define: {missing:?}"
        );
    }

    #[tokio::test]
    async fn api_release_watch_turns_the_watch_off_and_on() {
        // A mutating route with no test at all: it calls
        // `set_watch_disabled_in(dir, !enable)`, and a sign flip would turn the
        // watch OFF when the operator asks to check again, silently.
        let dir = std::env::temp_dir().join(format!("ralphy-watch-route-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let registry = dir.join("repos.toml");

        let post = |enable: &'static str| {
            let registry = registry.clone();
            async move {
                router(
                    None,
                    registry,
                    PathBuf::from("does-not-exist"),
                    StorePaths::default(),
                    Instant::now(),
                    idle_shutdown(),
                    auth::AuthState::localhost(),
                )
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/release/watch")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .body(Body::from(format!("enable={enable}")))
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };

        let resp = post("false").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(
            String::from_utf8_lossy(&body).contains("\"enabled\":false"),
            "the answer states what took: {}",
            String::from_utf8_lossy(&body)
        );
        assert!(
            release::watch_disabled_in(&dir),
            "enable=false must write the marker the poll reads"
        );

        let resp = post("true").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            !release::watch_disabled_in(&dir),
            "enable=true must remove it, not set it — the inversion is the bug this guards"
        );

        // Idempotent both ways, like the require-login toggle it is modelled on.
        assert_eq!(post("true").await.status(), StatusCode::OK);
        assert!(!release::watch_disabled_in(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn api_release_answers_from_the_cache_without_reaching_the_network() {
        // The route reads only what the background watch cached: a page load
        // must never wait on github.com, nor trigger a request of its own.
        let resp = get("/api/release").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let view: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert!(
            view["current"].as_str().is_some_and(|v| !v.is_empty()),
            "the view must name the running build; got: {view}"
        );
        assert_eq!(view["channel"], "rc");
        assert!(
            ["behind", "level", "ahead", "unknown"]
                .contains(&view["standing"].as_str().unwrap_or_default()),
            "standing is a closed set; got: {view}"
        );
        assert!(view["gap"].is_array());
        // The watch is spawned by `serve`, never by `router`: building a router
        // must not reach the network, nor drop a cache beside the test.
        assert!(
            !std::path::Path::new("releases.json").exists(),
            "a router built in a test must not have written a release cache"
        );
        // This tree's binary is built from a working copy, so it is ahead of its
        // tag and is never offered an update — whatever happens to be cached.
        assert_eq!(view["severity"], "none");
    }

    #[tokio::test]
    async fn api_agents_serves_the_roster() {
        let resp = get("/api/agents").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let rows: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), session::Agent::ALL.len());
        let served: std::collections::BTreeSet<String> = rows
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect();
        let expected: std::collections::BTreeSet<String> = [
            "claude", "codex", "opencode", "kimi", "copilot", "cursor", "gemini",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(served, expected, "served roster ids: {served:?}");
        assert_eq!(rows[0]["id"], "claude");
        assert_eq!(rows[0]["label"], "claude");
        assert_eq!(rows[0]["accelerator"], "1");
        assert!(rows[0]["available"].is_boolean());
        if rows[0]["available"] == false {
            assert_eq!(rows[0]["reason"], "not installed here");
        } else {
            assert!(rows[0]["reason"].is_null());
        }
    }

    #[tokio::test]
    async fn api_agents_uses_the_owning_daemons_locator() {
        const LOCAL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        const PEER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
        let peer_store = tempfile::tempdir().unwrap();
        let peer_identity = identity::Identity {
            id: PEER_ID.parse().unwrap(),
            name: "peer".to_string(),
            avatar: "🐙".to_string(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let peer_port = listener.local_addr().unwrap().port();
        let (peer_shutdown, peer_rx) = tokio::sync::watch::channel(false);
        let peer_router = router_with_roster(
            Some(peer_identity),
            peer_store.path().join("repos.toml"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            peer_rx,
            RouterDependencies {
                auth: auth::AuthState::fixed(
                    auth::AuthPolicy::Bearer("peer-token".to_string()),
                    epoch::SessionEpoch::in_memory_detached(),
                ),
                roster_locator: Arc::new(|_| Some(PathBuf::from("/usr/local/bin/vendor"))),
            },
        );
        let peer_task = tokio::spawn(async move {
            let _shutdown = peer_shutdown;
            axum::serve(listener, peer_router).await.unwrap();
        });

        let local_store = tempfile::tempdir().unwrap();
        peer::write_descriptor(
            local_store.path(),
            &peer::PeerDescriptor {
                daemon_id: PEER_ID.to_string(),
                name: "peer".to_string(),
                avatar: "🐙".to_string(),
                address: "127.0.0.1".to_string(),
                port: peer_port,
                environment: "WSL: Ubuntu-22.04".to_string(),
                token: "peer-token".to_string(),
                protocol_version: peer::PEER_PROTOCOL_VERSION,
                nudge: None,
            },
        )
        .unwrap();
        let local_router = router_with_roster(
            Some(identity::Identity {
                id: LOCAL_ID.parse().unwrap(),
                name: "local".to_string(),
                avatar: "🐙".to_string(),
            }),
            local_store.path().join("repos.toml"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            RouterDependencies {
                auth: auth::AuthState::localhost(),
                roster_locator: Arc::new(|_| None),
            },
        );

        let local = local_router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/agents")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let local_rows: serde_json::Value = serde_json::from_str(&body_text(local).await).unwrap();
        assert_eq!(local_rows[0]["available"], false);

        let peer = local_router
            .oneshot(
                Request::builder()
                    .uri(format!("/api/agents?repo={PEER_ID}%2Fowner%2Fshared"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(peer.status(), StatusCode::OK);
        let peer_rows: serde_json::Value = serde_json::from_str(&body_text(peer).await).unwrap();
        assert_eq!(peer_rows[0]["available"], true);
        assert_eq!(peer_rows[0]["reason"], serde_json::Value::Null);
        peer_task.abort();
    }

    #[tokio::test]
    async fn api_repos_reports_reachability() {
        // Write a temp repos.toml with one existing-dir entry (reachable) and one
        // bogus-path entry (unreachable), then read it back through the route.
        let dir = tempfile::tempdir().unwrap();
        let registry_path = dir.path().join("repos.toml");
        let mut store = registry::RegistryStore::default();
        store.upsert("owner/here", &dir.path().to_string_lossy());
        store.upsert("owner/gone", "/no/such/path/exists");
        registry::save_to(&store, &registry_path).unwrap();

        let resp = router(
            None,
            registry_path,
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/repos")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("owner/here") && body.contains("owner/gone"),
            "body must carry both slugs; got: {body}"
        );
        assert!(
            body.contains("\"reachable\":true"),
            "the existing-dir entry must be reachable; got: {body}"
        );
        assert!(
            body.contains("\"reachable\":false"),
            "the bogus-path entry must be unreachable; got: {body}"
        );
    }

    #[tokio::test]
    async fn api_repos_reports_branch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git").join("HEAD"),
            "ref: refs/heads/feat/mini-ide\n",
        )
        .unwrap();
        let registry_path = dir.path().join("repos.toml");
        let mut store = registry::RegistryStore::default();
        store.upsert("owner/here", &dir.path().to_string_lossy());
        store.upsert("owner/gone", "/no/such/path/exists");
        registry::save_to(&store, &registry_path).unwrap();

        let resp = router(
            None,
            registry_path,
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/repos")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("\"branch\":\"feat/mini-ide\""),
            "the reachable repo's branch must be reported; got: {body}"
        );
        assert!(
            body.contains("\"branch\":null"),
            "the unreachable repo's branch must be null; got: {body}"
        );
    }

    #[tokio::test]
    async fn api_repos_reports_dirty_and_remote() {
        fn git(dir: &std::path::Path, args: &[&str]) {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .expect("git (CI and the build machine have git)");
        }

        // (a) a dirty repo (untracked file) WITH an origin remote.
        let dirty = tempfile::tempdir().unwrap();
        git(dirty.path(), &["init"]);
        git(
            dirty.path(),
            &["remote", "add", "origin", "https://github.com/o/r.git"],
        );
        std::fs::write(dirty.path().join("untracked.txt"), "x").unwrap();
        // (b) a clean repo with NO remote.
        let clean = tempfile::tempdir().unwrap();
        git(clean.path(), &["init"]);

        let reg = tempfile::tempdir().unwrap();
        let registry_path = reg.path().join("repos.toml");
        let mut store = registry::RegistryStore::default();
        store.upsert("owner/dirty", &dirty.path().to_string_lossy());
        store.upsert("owner/clean", &clean.path().to_string_lossy());
        registry::save_to(&store, &registry_path).unwrap();

        let resp = router(
            None,
            registry_path,
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/repos")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("\"dirty\":true"),
            "the untracked-file repo must be dirty; got: {body}"
        );
        assert!(
            body.contains("\"dirty\":false"),
            "the clean repo must not be dirty; got: {body}"
        );
        assert!(
            body.contains("\"remote\":\"https://github.com/o/r.git\""),
            "the origin url must be reported; got: {body}"
        );
        assert!(
            body.contains("\"remote\":null"),
            "the remoteless repo must report null; got: {body}"
        );
    }

    #[tokio::test]
    async fn api_usage_serves_run_records_and_honors_since() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("owner-repo.jsonl"),
            "{\"project\":\"owner/repo\",\"issue\":1,\"phase\":\"plan\",\"agent\":\"a\",\"model\":\"m\",\"session_id\":\"sess-a\",\"outcome\":\"ok\",\"tokens\":{\"input\":10,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:00:00+00:00\"}\n\
             {\"project\":\"owner/repo\",\"issue\":1,\"phase\":\"execute\",\"agent\":\"a\",\"model\":\"m\",\"session_id\":\"sess-b\",\"outcome\":\"ok\",\"tokens\":{\"input\":20,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:05:00+00:00\"}\n",
        )
        .unwrap();

        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let resp = router(
            Some(id),
            PathBuf::from("does-not-exist"),
            dir.path().to_path_buf(),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("\"sess-a\""),
            "must carry sess-a; got: {body}"
        );
        assert!(
            body.contains("\"sess-b\""),
            "must carry sess-b; got: {body}"
        );
        assert!(
            body.contains("00000000000000000000000000"),
            "must carry the daemon_id; got: {body}"
        );
        assert!(
            !body.contains("usd"),
            "must not carry a usd field; got: {body}"
        );
        assert!(
            !body.contains("cost"),
            "must not carry a cost field; got: {body}"
        );

        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let resp = router(
            Some(id),
            PathBuf::from("does-not-exist"),
            dir.path().to_path_buf(),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage?since=2026-06-15T12:05:00%2B00:00")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("\"sess-b\"") && !body.contains("\"sess-a\""),
            "since must keep only sess-b; got: {body}"
        );
    }

    /// `/api/spend` serves the project's priced summary, and serves it SMALL: the
    /// document is a fixed set of fields, so opening the tab never transfers the
    /// ledger. The two ledger lines below are for different projects, so the
    /// response also proves the route scopes on the one it was asked for.
    ///
    /// The assertions are on SHAPE and on the floor marker, not on a dollar
    /// figure: `PriceTable::load()` reads the host's own `pricing.toml`, and
    /// pinning an amount here would make the test fail on an operator's machine
    /// for doing exactly what that file is for.
    #[tokio::test]
    async fn api_spend_serves_one_projects_priced_summary_without_the_ledger() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("owner-repo.jsonl"),
            "{\"project\":\"owner/repo\",\"issue\":1,\"phase\":\"execute\",\"agent\":\"a\",\"model\":\"unknown\",\"outcome\":\"ok\",\"tokens\":{\"input\":700,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:00:00+00:00\"}\n\
             {\"project\":\"other/repo\",\"issue\":2,\"phase\":\"execute\",\"agent\":\"a\",\"model\":\"claude-opus-4-8\",\"session_id\":\"sess-b\",\"outcome\":\"ok\",\"tokens\":{\"input\":50000,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:05:00+00:00\"}\n",
        )
        .unwrap();

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            dir.path().to_path_buf(),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/spend?project=owner%2Frepo")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);

        let summary: spend::SpendSummary = serde_json::from_str(&body).expect("a spend document");
        assert_eq!(summary.project, "owner/repo");
        assert_eq!(
            summary.tokens.total, 700,
            "the other project's 50k must not be in it; got: {body}"
        );
        assert_eq!(summary.tokens.meter, "↑700 ⚡0 ❄0 ↓0");
        // `unknown` with no session_id: unpriceable and unrecoverable.
        assert!(
            summary.floor,
            "an unpriceable line makes it a floor: {body}"
        );
        assert_eq!(summary.total, "~$?", "never `$0`; got: {body}");
        assert_eq!(
            summary
                .unpriced
                .causes
                .iter()
                .map(|c| (c.key.as_str(), c.tokens))
                .collect::<Vec<_>>(),
            [("lost", 700)],
            "the only cause with volume is the one the line has; got: {body}"
        );
        assert!(
            !body.contains("\"ts\"") && !body.contains("\"phase\""),
            "the summary must not ship ledger rows; got: {body}"
        );
    }

    /// The Ledger grid's feed: `?project=` narrows the raw usage dump to one
    /// project AND hands each surviving row its unpriced verdict, so the grid's
    /// "unpriced only" filter never has to re-derive in JavaScript what the
    /// price table already decided.
    #[tokio::test]
    async fn api_usage_scopes_to_a_project_and_marks_unpriced_rows() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("owner-repo.jsonl"),
            "{\"project\":\"owner/repo\",\"issue\":1,\"phase\":\"execute\",\"agent\":\"a\",\"model\":\"unknown\",\"session_id\":\"sess-a\",\"outcome\":\"ok\",\"tokens\":{\"input\":700,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:00:00+00:00\"}\n\
             {\"project\":\"other/repo\",\"issue\":2,\"phase\":\"execute\",\"agent\":\"a\",\"model\":\"unknown\",\"outcome\":\"ok\",\"tokens\":{\"input\":50000,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:05:00+00:00\"}\n",
        )
        .unwrap();

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            dir.path().to_path_buf(),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage?project=owner%2Frepo")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);

        assert_eq!(
            body.matches("other/repo").count(),
            0,
            "another project's rows must not reach the grid; got: {body}"
        );
        assert!(
            body.contains("\"sess-a\""),
            "the open project's row must survive; got: {body}"
        );
        assert!(
            body.contains("\"unpriced_cause\":\"recoverable\""),
            "an `unknown` model WITH a session is recoverable, not lost; got: {body}"
        );
    }

    /// The route is scoped to ONE project by construction: `project` is a required
    /// query field, so a caller that forgets it gets a refusal rather than a
    /// silently cross-project total.
    #[tokio::test]
    async fn api_spend_refuses_a_request_with_no_project() {
        let dir = tempfile::tempdir().unwrap();
        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            dir.path().to_path_buf(),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/spend")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// The period is a CLOSED vocabulary at the boundary: a key the surface does
    /// not offer is refused, never quietly served as all-time figures under a
    /// window label. The recognized key answers `200` and echoes itself, so the
    /// client renders the window it is actually reading.
    #[tokio::test]
    async fn api_spend_refuses_an_unknown_period() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("owner-repo.jsonl"),
            "{\"project\":\"owner/repo\",\"issue\":1,\"phase\":\"execute\",\"agent\":\"a\",\"model\":\"unknown\",\"outcome\":\"ok\",\"tokens\":{\"input\":700,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:00:00+00:00\"}\n",
        )
        .unwrap();
        let spend_get = |uri: &'static str| {
            let path = dir.path().to_path_buf();
            async move {
                router(
                    None,
                    PathBuf::from("does-not-exist"),
                    path,
                    StorePaths::default(),
                    Instant::now(),
                    idle_shutdown(),
                    auth::AuthState::localhost(),
                )
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap()
            }
        };

        let refused = spend_get("/api/spend?project=owner%2Frepo&period=fortnight").await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);

        let accepted = spend_get("/api/spend?project=owner%2Frepo&period=7d").await;
        assert_eq!(accepted.status(), StatusCode::OK);
        let body = accepted.into_body().collect().await.unwrap().to_bytes();
        let summary: spend::SpendSummary = serde_json::from_slice(&body).expect("a spend document");
        assert_eq!(summary.period.key, "7d");
        assert_eq!(summary.period.label, "last 7 days");
        assert!(
            summary.period.since.is_some(),
            "a bounded window carries the instant its figures start at"
        );
        // The fixture row is from June: a 7-day window must not contain it.
        assert_eq!(summary.tokens.total, 0, "the window scopes the figures");
    }

    /// `/api/usage` now carries an `interactive` array from the Claude scan
    /// alongside the ledger's `records`. A session the ledger already owns
    /// (`run-sess`) is excluded from `interactive`; a genuinely-interactive one
    /// (`int-sess`) appears. The scan runs against a temp store, so no operator
    /// state is read.
    #[tokio::test]
    async fn api_usage_carries_run_and_interactive_records() {
        let usage_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            usage_dir.path().join("owner-repo.jsonl"),
            "{\"project\":\"owner/repo\",\"issue\":1,\"phase\":\"plan\",\"session_id\":\"run-sess\",\"ts\":\"2026-06-15T12:00:00+00:00\"}\n",
        )
        .unwrap();

        let claude_dir = tempfile::tempdir().unwrap();
        let ws = claude_dir.path().join("ws-key");
        std::fs::create_dir_all(&ws).unwrap();
        let line = |id: &str| {
            format!(
                "{{\"requestId\":\"r1\",\"timestamp\":\"2026-07-10T10:00:00Z\",\"message\":{{\"id\":\"{id}\",\"model\":\"claude-opus-4-8\",\"usage\":{{\"input_tokens\":10,\"output_tokens\":1,\"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":0}}}}}}"
            )
        };
        std::fs::write(ws.join("run-sess.jsonl"), line("m1")).unwrap();
        std::fs::write(ws.join("int-sess.jsonl"), line("m2")).unwrap();

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            usage_dir.path().to_path_buf(),
            StorePaths {
                claude_projects_dir: claude_dir.path().to_path_buf(),
                ..Default::default()
            },
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let raw = resp.into_body().collect().await.unwrap().to_bytes();
        let body_string = String::from_utf8_lossy(&raw);
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();

        let interactive = body["interactive"].as_array().expect("interactive array");
        let has = |sid: &str| {
            interactive
                .iter()
                .any(|r| r.get("session_id").and_then(|v| v.as_str()) == Some(sid))
        };
        assert!(
            has("int-sess"),
            "interactive must carry int-sess; got: {body_string}"
        );
        assert!(
            !has("run-sess"),
            "the run-owned session must be excluded; got: {body_string}"
        );

        let records = body["records"].as_array().expect("records array");
        assert!(
            records
                .iter()
                .any(|r| r.get("session_id").and_then(|v| v.as_str()) == Some("run-sess")),
            "records must still carry the run line; got: {body_string}"
        );
        assert!(
            !body_string.contains("usd"),
            "no pricing in the payload; got: {body_string}"
        );

        // A vendor that writes every token to disk reports a total, not a floor:
        // a blanket `lower_bound: true` would mislabel the whole modal.
        let claude = interactive
            .iter()
            .find(|r| r.get("session_id").and_then(|v| v.as_str()) == Some("int-sess"))
            .unwrap();
        assert_eq!(claude["lower_bound"].as_bool(), Some(false), "{claude}");
    }

    /// `/api/usage` also carries Codex interactive records: a rollout under the
    /// codex base dir's `sessions/` tree flows through the scan and appears in the
    /// `interactive` array with `agent=="codex"` and its `session_meta.id`. Proves
    /// the codex_dir router arg is threaded end-to-end, not just Claude.
    #[tokio::test]
    async fn api_usage_carries_codex_interactive_records() {
        let codex_dir = tempfile::tempdir().unwrap();
        let roll = codex_dir
            .path()
            .join("sessions")
            .join("2026")
            .join("07")
            .join("10");
        std::fs::create_dir_all(&roll).unwrap();
        let meta_id = "019c5131-651b-78f2-b8e7-93995bff4dad";
        let body = format!(
            "{{\"timestamp\":\"2026-07-10T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{meta_id}\",\"cwd\":\"c:\\\\Dev\\\\x\"}}}}\n\
             {{\"timestamp\":\"2026-07-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{{\"model\":\"gpt-5.3-codex\"}}}}\n\
             {{\"timestamp\":\"2026-07-10T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":1000,\"cached_input_tokens\":800,\"output_tokens\":200}}}}}}}}\n"
        );
        std::fs::write(roll.join("rollout-int-abc.jsonl"), body).unwrap();

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths {
                codex_dir: codex_dir.path().to_path_buf(),
                ..Default::default()
            },
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let raw = resp.into_body().collect().await.unwrap().to_bytes();
        let body_string = String::from_utf8_lossy(&raw);
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let interactive = body["interactive"].as_array().expect("interactive array");
        assert!(
            interactive.iter().any(|r| {
                r.get("agent").and_then(|v| v.as_str()) == Some("codex")
                    && r.get("session_id").and_then(|v| v.as_str()) == Some(meta_id)
            }),
            "interactive must carry a codex record with the meta id; got: {body_string}"
        );
        assert!(
            !body_string.contains("usd"),
            "no pricing in the payload; got: {body_string}"
        );
    }

    /// `/api/usage` also carries OpenCode interactive records: an assistant row in
    /// a seeded `opencode.db` flows through the scan and appears in the
    /// `interactive` array with `agent=="opencode"` and its `session_id`. Proves
    /// the `opencode_db` router arg is threaded end-to-end.
    #[tokio::test]
    async fn api_usage_carries_opencode_interactive_records() {
        use rusqlite::Connection;
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("opencode.db");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute(
                "CREATE TABLE message (id TEXT, session_id TEXT, data TEXT)",
                [],
            )
            .unwrap();
            conn.execute("CREATE TABLE session (id TEXT, directory TEXT)", [])
                .unwrap();
            let data = r#"{"role":"assistant","modelID":"k2p6","tokens":{"input":2168,"output":100,"cache":{"write":0,"read":11264}}}"#;
            conn.execute(
                "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
                rusqlite::params!["msg_1", "ses_oc", data],
            )
            .unwrap();
        }

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths {
                opencode_db: db.clone(),
                ..Default::default()
            },
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let raw = resp.into_body().collect().await.unwrap().to_bytes();
        let body_string = String::from_utf8_lossy(&raw);
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let interactive = body["interactive"].as_array().expect("interactive array");
        assert!(
            interactive.iter().any(|r| {
                r.get("agent").and_then(|v| v.as_str()) == Some("opencode")
                    && r.get("session_id").and_then(|v| v.as_str()) == Some("ses_oc")
            }),
            "interactive must carry an opencode record with the session id; got: {body_string}"
        );
        assert!(
            !body_string.contains("usd"),
            "no pricing in the payload; got: {body_string}"
        );
    }

    /// `/api/usage` also carries Copilot interactive records: a row in a seeded
    /// `session-store.db` flows through the scan and appears in the `interactive`
    /// array with `agent=="copilot"` and its `session_id`. Proves the `copilot_db`
    /// router arg is threaded end-to-end.
    #[tokio::test]
    async fn api_usage_carries_copilot_interactive_records() {
        use rusqlite::Connection;
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("session-store.db");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute(
                "CREATE TABLE assistant_usage_events (id INTEGER PRIMARY KEY AUTOINCREMENT, \
                 session_id TEXT, model TEXT, input_tokens INTEGER, output_tokens INTEGER, \
                 cache_read_tokens INTEGER, cache_write_tokens INTEGER, created_at TEXT)",
                [],
            )
            .unwrap();
            conn.execute("CREATE TABLE sessions (id TEXT PRIMARY KEY, cwd TEXT)", [])
                .unwrap();
            conn.execute(
                "INSERT INTO assistant_usage_events (session_id, model, input_tokens, \
                 output_tokens, cache_read_tokens, cache_write_tokens, created_at) \
                 VALUES ('ses_cp', 'claude-sonnet-5', 22913, 350, 0, 22903, \
                 '2026-07-20T11:54:33.066Z')",
                [],
            )
            .unwrap();
        }

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths {
                copilot_db: db.clone(),
                ..Default::default()
            },
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let raw = resp.into_body().collect().await.unwrap().to_bytes();
        let body_string = String::from_utf8_lossy(&raw);
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let interactive = body["interactive"].as_array().expect("interactive array");
        assert!(
            interactive.iter().any(|r| {
                r.get("agent").and_then(|v| v.as_str()) == Some("copilot")
                    && r.get("session_id").and_then(|v| v.as_str()) == Some("ses_cp")
            }),
            "interactive must carry a copilot record with the session id; got: {body_string}"
        );
        assert!(
            !body_string.contains("usd"),
            "no pricing in the payload; got: {body_string}"
        );
    }

    /// `/api/usage` also carries Kimi interactive records: a legacy `wire.jsonl`
    /// with one non-zero `StatusUpdate` under the kimi base dir's `sessions/` tree
    /// flows through the scan and appears in the `interactive` array with
    /// `agent=="kimi"` and its parent-dir session id. Proves the `kimi_dir` router
    /// arg is threaded end-to-end.
    #[tokio::test]
    async fn api_usage_carries_kimi_interactive_records() {
        let kimi_dir = tempfile::tempdir().unwrap();
        let sess = kimi_dir.path().join("sessions").join("GRP").join("SESS");
        std::fs::create_dir_all(&sess).unwrap();
        let line = "{\"timestamp\": 1770983410.0, \"message\": {\"type\": \"StatusUpdate\", \"payload\": {\"token_usage\": {\"input_other\": 100, \"output\": 10, \"input_cache_read\": 0, \"input_cache_creation\": 0}, \"message_id\": \"m1\"}}}";
        std::fs::write(sess.join("wire.jsonl"), line).unwrap();

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths {
                kimi_dir: kimi_dir.path().to_path_buf(),
                ..Default::default()
            },
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let raw = resp.into_body().collect().await.unwrap().to_bytes();
        let body_string = String::from_utf8_lossy(&raw);
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let interactive = body["interactive"].as_array().expect("interactive array");
        assert!(
            interactive.iter().any(|r| {
                r.get("agent").and_then(|v| v.as_str()) == Some("kimi")
                    && r.get("session_id").and_then(|v| v.as_str()) == Some("SESS")
            }),
            "interactive must carry a kimi record with the session id; got: {body_string}"
        );
        assert!(
            !body_string.contains("usd"),
            "no pricing in the payload; got: {body_string}"
        );
    }

    /// `/api/usage` also carries Cursor interactive records, and their `tokens` is
    /// JSON `null` — the key is PRESENT and is not `0` (ADR-0042 D11: Cursor keeps
    /// no token count anywhere, so "unavailable" must not serialize as "spent
    /// nothing"). Proves the `cursor_dir` router arg is threaded end-to-end.
    #[tokio::test]
    async fn api_usage_carries_cursor_interactive_records() {
        let cursor_dir = tempfile::tempdir().unwrap();
        let sid = "33333333-3333-3333-3333-333333333333";
        let sess = cursor_dir.path().join("chats").join("aaaa").join(sid);
        std::fs::create_dir_all(&sess).unwrap();
        std::fs::write(
            sess.join("meta.json"),
            r#"{"schemaVersion":1,"createdAtMs":1784593842510,"hasConversation":true,"updatedAtMs":1784593855173,"cwd":"C:\\Dev\\FinCal"}"#,
        )
        .unwrap();

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths {
                cursor_dir: cursor_dir.path().to_path_buf(),
                ..Default::default()
            },
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let raw = resp.into_body().collect().await.unwrap().to_bytes();
        let body_string = String::from_utf8_lossy(&raw);
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let interactive = body["interactive"].as_array().expect("interactive array");
        let record = interactive
            .iter()
            .find(|r| {
                r.get("agent").and_then(|v| v.as_str()) == Some("cursor")
                    && r.get("session_id").and_then(|v| v.as_str()) == Some(sid)
            })
            .unwrap_or_else(|| panic!("no cursor record for {sid}; got: {body_string}"));
        assert!(
            record.get("tokens").is_some(),
            "the tokens key must be PRESENT, not omitted; got: {record}"
        );
        assert!(
            record["tokens"].is_null(),
            "tokens must be null (unavailable), never 0; got: {record}"
        );
    }

    /// `/api/usage` also carries Gemini interactive records, with REAL counts —
    /// unlike Cursor's `null`, the Gemini store keeps per-turn tokens (a lower
    /// bound, ADR-0043 D10). Proves the `gemini_dir` router arg is threaded
    /// end-to-end.
    #[tokio::test]
    async fn api_usage_carries_gemini_interactive_records() {
        let gemini_dir = tempfile::tempdir().unwrap();
        let chats = gemini_dir.path().join("tmp").join("fincal").join("chats");
        std::fs::create_dir_all(&chats).unwrap();
        std::fs::write(
            gemini_dir
                .path()
                .join("tmp")
                .join("fincal")
                .join(".project_root"),
            "c:\\dev\\fincal",
        )
        .unwrap();
        std::fs::write(
            chats.join("session-x.jsonl"),
            "{\"sessionId\":\"ralphy-probe-p1p2p3p4p6\",\"startTime\":\"2026-07-21T00:56:00Z\",\"lastUpdated\":\"2026-07-21T01:00:00Z\",\"kind\":\"main\"}\n\
             {\"id\":\"78d80d17\",\"type\":\"gemini\",\"content\":\"OK\",\"tokens\":{\"input\":20637,\"output\":30,\"cached\":0,\"thoughts\":257,\"tool\":0,\"total\":20924},\"model\":\"gemini-3.1-pro-preview-customtools\"}\n",
        )
        .unwrap();

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths {
                gemini_dir: gemini_dir.path().to_path_buf(),
                ..Default::default()
            },
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let raw = resp.into_body().collect().await.unwrap().to_bytes();
        let body_string = String::from_utf8_lossy(&raw);
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let interactive = body["interactive"].as_array().expect("interactive array");
        let record = interactive
            .iter()
            .find(|r| {
                r.get("agent").and_then(|v| v.as_str()) == Some("gemini")
                    && r.get("session_id").and_then(|v| v.as_str())
                        == Some("ralphy-probe-p1p2p3p4p6")
            })
            .unwrap_or_else(|| panic!("no gemini record; got: {body_string}"));
        // `total - input`, not the bare `output` field — the arithmetic survives
        // the whole route, not just the scan's own unit test.
        assert_eq!(record["tokens"]["output"].as_u64(), Some(287), "{record}");
        assert_eq!(record["tokens"]["input"].as_u64(), Some(20637), "{record}");
        // ADR-0043 D10: the served record must carry the floor label, or the UI
        // has nothing to render `≥ n (lower bound)` from.
        assert_eq!(record["lower_bound"].as_bool(), Some(true), "{record}");
    }

    #[test]
    fn build_presence_carries_identity_and_uptime() {
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let frame = build_presence(Some(&id), Duration::from_secs(5));
        match frame {
            Frame::Presence(p) => {
                assert_eq!(p.name, Some("anvil".into()));
                assert_eq!(p.avatar, Some("🐙".into()));
                assert_eq!(p.uptime_secs, 5);
            }
            other => panic!("expected a presence frame, got {other:?}"),
        }
    }

    #[test]
    fn bind_addr_default_is_loopback() {
        let addr = bind_addr(Ipv4Addr::LOCALHOST.into(), DEFAULT_PORT);
        assert!(addr.ip().is_loopback(), "default bind must be 127.0.0.1");
        assert_eq!(addr.port(), DEFAULT_PORT);
    }

    /// A router under a `Bearer` policy rejects a request with no
    /// `Authorization` header — the guard covers the API surface, not just `/ws`.
    #[tokio::test]
    async fn bearer_policy_rejects_missing_header() {
        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::fixed(
                auth::AuthPolicy::Bearer("tok".into()),
                epoch::SessionEpoch::in_memory_detached(),
            ),
        )
        .oneshot(
            Request::builder()
                .uri("/api/identity")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// The same router passes a request carrying the correct bearer token.
    #[tokio::test]
    async fn bearer_policy_accepts_correct_header() {
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let resp = router(
            Some(id),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::fixed(
                auth::AuthPolicy::Bearer("tok".into()),
                epoch::SessionEpoch::in_memory_detached(),
            ),
        )
        .oneshot(
            Request::builder()
                .uri("/api/identity")
                .header(header::AUTHORIZATION, "Bearer tok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// A `Localhost` policy serves the API with no `Authorization` header.
    #[tokio::test]
    async fn localhost_policy_serves_without_token() {
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let resp = router(
            Some(id),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/identity")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// A Bearer router with the WRONG token returns `401` — the guard checks the
    /// token VALUE, not merely the header's presence (a presence-only bug would
    /// pass every other test here).
    #[tokio::test]
    async fn bearer_policy_rejects_wrong_token() {
        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::fixed(
                auth::AuthPolicy::Bearer("tok".into()),
                epoch::SessionEpoch::in_memory_detached(),
            ),
        )
        .oneshot(
            Request::builder()
                .uri("/api/identity")
                .header(header::AUTHORIZATION, "Bearer wrong")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// The RFC 6238 seed, wrapped for the session router tests.
    fn rfc_seed() -> totp::Seed {
        totp::Seed::from_bytes(b"12345678901234567890".to_vec())
    }

    /// Build a router under a `Session` policy over `token` + the RFC seed, with a
    /// baptized identity so `/api/identity` answers `200` once authorized.
    fn session_router(token: &str) -> Router {
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::fixed(policy, session_epoch),
        )
    }

    /// The full browser-login round trip under a `Session` policy (issue #179,
    /// promoted in #200): no-cookie `401` on data, the SPA shell is served ungated
    /// at `/` (it renders its own login gate), a valid-TOTP `POST /api/login` `200`
    /// + `Set-Cookie`, the cookie authorizes a follow-up, and a machine `Bearer`
    /// still authorizes. Plumbing only — the code itself is pinned by the `totp`
    /// RFC-vector unit test.
    #[tokio::test]
    async fn session_policy_login_flow() {
        // 1. No cookie / no bearer → the API is 401.
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .uri("/api/identity")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "no cookie → 401");

        // 2. The shell (which hosts its own login gate) is served without a cookie.
        let resp = session_router("tok")
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "GET / → 200 (ungated shell)");

        // 3. A valid current TOTP mints a session cookie.
        let now = now_unix();
        let code = rfc_seed().code_at(now / 30);
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!("code={code}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "valid TOTP → 200");
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("a Set-Cookie header")
            .to_string();
        assert!(
            set_cookie.contains("ralphy_session="),
            "cookie name: {set_cookie}"
        );
        assert!(set_cookie.contains("HttpOnly"), "HttpOnly: {set_cookie}");
        assert!(
            set_cookie.contains("SameSite=Strict"),
            "SameSite: {set_cookie}"
        );

        // The cookie value is everything up to the first attribute `;`.
        let cookie_pair = set_cookie.split(';').next().unwrap().to_string();

        // 4. That cookie authorizes a follow-up request.
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .uri("/api/identity")
                    .header(header::COOKIE, &cookie_pair)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "cookie authorizes → 200");

        // 5. The machine path is unchanged: a correct Bearer still authorizes.
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .uri("/api/identity")
                    .header(header::AUTHORIZATION, "Bearer tok")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "Bearer authorizes under Session"
        );
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

    #[tokio::test]
    async fn remember_me_mints_a_week_long_cookie_that_authorizes() {
        // ADR-0032 amendment 2026-09-16: the box is opt-in — absent, the cookie
        // carries the standard 30 min Max-Age; `remember=true` the 7-day one.
        let standard = login_set_cookie("").await;
        assert!(
            standard.contains("Max-Age=1800"),
            "no box → standard idle: {standard}"
        );
        let remembered = login_set_cookie("&remember=true").await;
        assert!(
            remembered.contains("Max-Age=604800"),
            "remember=true → 7-day idle: {remembered}"
        );
        let cookie_pair = remembered.split(';').next().unwrap().to_string();
        assert!(
            cookie_pair.contains(".r."),
            "the value carries the remembered kind: {cookie_pair}"
        );
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .uri("/api/identity")
                    .header(header::COOKIE, &cookie_pair)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "a remembered cookie authorizes → 200"
        );
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

    #[tokio::test]
    async fn root_serves_workbench_shell() {
        let resp = get_local("/").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET / → 200");
        let body = body_string(resp).await;
        assert!(
            body.contains(r#"x-data="shell()""#),
            "the workbench shell HTML must render at the root; got: {}",
            &body[..body.len().min(200)]
        );
        let resp = get_local("/app.js").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET /app.js → 200");
    }

    /// The security headers ride EVERY response (audit F3): the shell, an
    /// asset, an API answer and a refusal alike, from one layer over the
    /// router. The CSP allows the shell's own inline script by hash — the one
    /// spelling of `script-src` that admits no injected tag.
    #[tokio::test]
    async fn every_response_carries_the_security_headers() {
        for path in ["/", "/app.js", "/api/session", "/api/nope"] {
            let resp = get_local(path).await;
            let h = resp.headers();
            assert_eq!(h[header::X_FRAME_OPTIONS], "DENY", "{path}");
            assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff", "{path}");
            assert_eq!(h[header::REFERRER_POLICY], "no-referrer", "{path}");
            let csp = h[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
            assert!(csp.contains("frame-ancestors 'none'"), "{path}: {csp}");
            assert!(
                csp.contains("script-src 'self' 'unsafe-eval' 'sha256-"),
                "{path}: {csp}"
            );
        }
        // The hash in the header is the hash of the bytes the browser receives:
        // recompute it from the served shell.
        let shell = body_string(get_local("/").await).await;
        let bodies = routes::inline_script_bodies(&shell);
        assert!(
            !bodies.is_empty(),
            "index.html carries the demo-seed gate inline"
        );
        let csp = routes::content_security_policy().to_str().unwrap();
        for body in bodies {
            let want = format!("'sha256-{}'", routes::script_hash(body));
            assert!(
                csp.contains(&want),
                "served shell script not in the CSP: {want}"
            );
        }
    }

    /// The one inline form a hash cannot cover: an `on*=` event-handler
    /// attribute (it would need `'unsafe-hashes'`). None of the shells has one —
    /// Alpine's `@click` is the idiom — and this keeps it that way.
    #[tokio::test]
    async fn no_shell_carries_an_inline_event_handler() {
        let re = regex::Regex::new(r#"\son[a-z]+="#).unwrap();
        for path in ["/index.html", "/detached.html", "/detached-fence.html"] {
            let body = body_string(get_local(path).await).await;
            for line in body.lines() {
                let live = line.split("<!--").next().unwrap_or(line);
                assert!(!re.is_match(live), "{path}: inline handler in {line:?}");
            }
        }
    }

    #[tokio::test]
    async fn root_serves_vendored_xterm() {
        let resp = get_local("/vendor/xterm.js").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET vendor/xterm.js → 200");
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        let resp = get_local("/vendor/xterm.css").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET vendor/xterm.css → 200");
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "text/css; charset=utf-8"
        );

        let shell = body_string(get_local("/").await).await;
        assert!(
            shell.contains("vendor/xterm.js"),
            "the shell HTML must load the vendored xterm"
        );

        let console = body_string(get_local("/wb-console.js").await).await;
        assert!(
            console.contains("new Terminal("),
            "wb-console.js must construct a real xterm terminal"
        );
        assert!(
            console.contains("/ws/session"),
            "wb-console.js must open the session WebSocket"
        );
    }

    /// The workbench routes a `.note` to its CARD and never to a tab
    /// (ADR-0064 §11), and mirrors the daemon's one denylist carve-out.
    ///
    /// Both halves are load-bearing and neither is visible to a substring pin
    /// of the other: a `.note` opened as a tab would be a SECOND editor over a
    /// file a card already holds — the exact thing §11 exists to prevent — and
    /// a UI that still believed `.ralphy/notes/` was protected would hide the
    /// gestures the daemon now accepts there.
    #[test]
    fn the_explorer_opens_a_note_as_a_card() {
        let app = include_str!("../assets/ui/app.js");
        assert!(
            app.contains(r#"if (ext === "note") return "note";"#),
            "classify must name a `.note` (ADR-0064 §11)"
        );
        for pin in [
            "openNote(path)",
            "WBNotes.openFromExplorer(",
            "function isNoteInNotesDir(",
        ] {
            assert!(app.contains(pin), "app.js must keep the ADR-0064 pin {pin}");
        }
        // The UI's mirror of the denylist carve-out, CLAUSE BY CLAUSE. The two
        // shapes cannot be compared across languages by a test, so each half of
        // the predicate the daemon enforces (`fswrite::is_note_in_notes_dir` —
        // exactly three components, the two literal directory names, a name
        // longer than the extension) is pinned in the mirror. Dropping the
        // three-component clause is the drift that matters: it would offer the
        // operator rename and delete on `.ralphy/notes/sub/x.note`, which the
        // daemon refuses.
        for clause in [
            "parts.length === 3",
            r#"parts[0] === ".ralphy""#,
            r#"parts[1] === "notes""#,
            r#"parts[2].length > ".note".length"#,
            r#"parts[2].endsWith(".note")"#,
        ] {
            assert!(
                app.contains(clause),
                "app.js's carve-out mirror must keep the clause {clause}"
            );
        }
        // The card's own file actions go through the GENERIC byte-ops — there
        // is no `note.rename`/`note.delete`, and adding one would re-derive the
        // confinement the carve-out already gives.
        let notes = include_str!("../assets/ui/wb-notes.js");
        for pin in [r#"write("file.rename""#, r#"write("file.delete""#] {
            assert!(notes.contains(pin), "wb-notes.js must keep the pin {pin}");
        }
        // The native dialogs are pinned OUT for the reason `wb-console.js`
        // records: an automated browser dismisses them by default, which turns
        // a guarded click into a silently cancelled one.
        assert!(
            !notes.contains("window.confirm(") && !notes.contains("window.prompt("),
            "a note's file actions must use the workbench's own dialog and field"
        );
        assert!(
            notes.contains("WBConsole.askConfirm({"),
            "deleting a note's FILE must ask first (ADR-0064 §11)"
        );
        // THE LOCK PINS THE CARD AND NOTHING ELSE (ADR-0064, amendment
        // 2026-09-22): §8 gave it a second job — put the editor in read-only —
        // and the two are different questions. A pinned note is still typed
        // into. This is the NEGATIVE CONTROL for that clause: the gesture
        // guards in `makeDraggable`/`startResize` are the whole of the lock,
        // and a `setReadonly` wired back to it would silently take the note
        // away again.
        assert!(
            !notes.contains("setReadonly(!!locked)"),
            "the lock pins the card; it must not put the editor in read-only \
             (ADR-0064 amendment)"
        );
        assert!(
            notes.contains("locked: () => !!el._noteLocked"),
            "the lock must reach the drag and the resize, which are what it IS"
        );
    }

    /// The vendored Crepe bundle states where it came from, and the recipe that
    /// built it is in the repository (ADR-0064 §6, ADR-0057).
    ///
    /// Crepe is the workbench's first vendored asset that is BUILT rather than
    /// copied, so "which upstream, at which version, with which features" is
    /// not recoverable by diffing a tarball — the header line is the record,
    /// and this test is what keeps it honest. The three constants are read from
    /// the recipe itself, so a bump that edits `package.json` without rerunning
    /// `node build.mjs` reds here rather than shipping a bundle whose header
    /// lies about it.
    ///
    /// A note's HAND and SIZE are a closed set on both sides of the seam
    /// (ADR-0064 §8, amendment 2026-09-22). The shell writes the name into the
    /// file's front matter and onto a `data-*`; the stylesheet is what turns it
    /// into a face. A name in one list and not the other is the silent failure
    /// this closes — the card would carry `data-font="hand"`, no rule would
    /// match, and it would paint in the default with the palette still showing
    /// the chip as chosen.
    ///
    /// `sans` and `m` are deliberately absent from the CSS: they are the
    /// declared defaults on `.note-card` itself, so a rule for them would be a
    /// second place to change one value.
    #[test]
    fn a_notes_hand_and_size_are_a_closed_set_on_both_sides() {
        let notes = include_str!("../assets/ui/wb-notes.js");
        let css = include_str!("../assets/ui/styles/13-notes.css");
        assert!(
            notes.contains(r#"const FONTS = ["sans", "serif", "mono"]"#)
                && notes.contains(r#"const SIZES = ["xs", "s", "m", "l", "xl"]"#),
            "the hand and the size must be closed sets, not a free font-family string"
        );
        for name in ["serif", "mono"] {
            assert!(
                css.contains(&format!(r#".note-card[data-font="{name}"]"#))
                    && css.contains(&format!(r#".note-swatch-font[data-name="{name}"]"#)),
                "the font {name} must have a face on the card AND on its chip"
            );
        }
        for name in ["xs", "s", "l", "xl"] {
            assert!(
                css.contains(&format!(r#".note-card[data-size="{name}"]"#)),
                "the size {name} must have a rule, or the chip sets nothing"
            );
        }
        // `hidden` MUST BE DRAWN, not merely set. `.note-menu-item` carries
        // `all: unset`, which is an author `display: inline` and therefore
        // beats the UA's `[hidden] { display: none }` — SEEN in a browser
        // 2026-09-22 with the missing card's `Use another file…` sitting in a
        // healthy note's menu. The popovers learned this twice already; this
        // is the assertion for the items, where two verbs now depend on it.
        assert!(
            css.contains(".note-menu-item[hidden]"),
            "an item this shell hides must be hidden in paint too (`all: unset` resets display)"
        );
        // The EDITOR is what the operator is actually looking at: the card's
        // tokens have to reach Crepe, or only the card's own padding changes.
        assert!(
            css.contains("--crepe-base-font-size: var(--note-size);")
                && css.contains("--crepe-font-default: var(--note-font);"),
            "the card's hand and size must reach the editor, not stop at the body"
        );
        // A MERMAID FENCE IS DRAWN ON THE CARD, not in a window cut out of it
        // (asked 2026-09-22). Mermaid derives its whole palette from a few
        // seeds by lightening, darkening and inverting them and never looks at
        // the page, so a built-in theme lands its own canvas on whatever tone
        // the note is wearing. The seeds come from the card instead, and the
        // host paints nothing behind the drawing.
        let crepe = include_str!("../vendor-build/crepe/entry.js");
        assert!(
            !crepe.contains("theme: 'dark'") && crepe.contains("theme: 'base'"),
            "only `base` takes replacement theme variables; the built-ins fight the card"
        );
        assert!(
            crepe.contains("themeDirective(host) + source"),
            "the palette must travel WITH the diagram — `initialize` is global and two \
             cards render at once"
        );
        assert!(
            css.contains("background: transparent;"),
            "the mermaid host must paint nothing behind the drawing"
        );
        // The colours are inline fills in the SVG, so the cascade cannot reach
        // them: a restyle has to redraw or the diagram keeps the old tone.
        assert!(
            notes.contains("el._noteEditor?.redrawDiagrams?.()")
                && crepe.contains("redrawDiagrams:"),
            "restyling a card must repaint the diagrams in it (ADR-0064 §15 amendment)"
        );
        // Written to the FILE, so a note keeps its face when the card is
        // closed and opened again — the look is the note's, not the desk's.
        assert!(
            notes.contains("lines.push(`font: ${font}`)")
                && notes.contains("lines.push(`size: ${size}`)"),
            "the hand and the size belong in the note's front matter (ADR-0064 §8)"
        );
    }

    /// The recipe is read with `include_str!` from OUTSIDE `assets/ui/`: it must
    /// not be embedded (`include_dir!` would serve `node_modules/` to the
    /// browser), and this is also the assertion that it exists.
    #[test]
    fn vendored_crepe_states_its_recipe() {
        let recipe = include_str!("../vendor-build/crepe/package.json");
        let version = |name: &str| -> String {
            let after = recipe
                .split_once(&format!("\"{name}\": \""))
                .unwrap_or_else(|| panic!("the recipe must pin {name}"))
                .1;
            after[..after.find('"').expect("a closing quote")].to_string()
        };
        let build = include_str!("../vendor-build/crepe/build.mjs");
        let features: Vec<&str> = build
            .split_once("const FEATURES = [")
            .expect("build.mjs must list the features")
            .1
            .split_once("].join")
            .expect("the feature list must close")
            .0
            .split('\'')
            .filter(|s| !s.trim().is_empty() && !s.contains(','))
            .collect();
        let header = format!(
            "/* crepe {} · esbuild {} · features: {} */",
            version("@milkdown/crepe"),
            version("esbuild"),
            features.join(",")
        );

        for artefact in ["vendor/crepe/crepe.js", "vendor/crepe/crepe.css"] {
            let src = UI
                .get_file(artefact)
                .and_then(|f| f.contents_utf8())
                .unwrap_or_else(|| panic!("{artefact} must be embedded as UTF-8"));
            assert_eq!(
                src.lines().next().unwrap_or_default(),
                header,
                "{artefact}'s header must name the recipe that built it — \
                 rerun `node build.mjs` in vendor-build/crepe"
            );
        }
        // The mermaid node view is OURS and rides in the same bundle (ADR-0064
        // §15), so it is named in the header for the same reason the features
        // are — and both shells must carry the two globals it reads.
        assert!(
            features.contains(&"mermaid-view"),
            "the bundle carries the mermaid node view (ADR-0064 §15)"
        );
        // The header names it; this proves it is actually IN the artefact —
        // a class only our node view emits, so a stale rebuild reds here
        // rather than shipping a header that promises a view the bundle lost.
        assert!(
            UI.get_file("vendor/crepe/crepe.js")
                .and_then(|f| f.contents_utf8())
                .is_some_and(|src| src.contains("note-mermaid-figure")),
            "crepe.js must carry the mermaid node view it advertises"
        );
        // `htmlLabels: false` in BOTH renderers, and it is not a style: with
        // mermaid's default HTML labels every label is removed on the way in,
        // because DOMPurify 3.4 dropped `foreignObject` from its SVG
        // allowlist — the diagram arrives as unlabelled boxes (measured).
        for (what, src) in [
            (
                "vendor-build/crepe/entry.js",
                include_str!("../vendor-build/crepe/entry.js"),
            ),
            ("wb-viewer.js", include_str!("../assets/ui/wb-viewer.js")),
        ] {
            assert!(
                src.contains("htmlLabels: false"),
                "{what} must keep mermaid's labels as plain SVG text"
            );
        }
        for shell in ["index.html", "detached-fence.html"] {
            let html = UI
                .get_file(shell)
                .and_then(|f| f.contents_utf8())
                .expect("the shell must be embedded");
            for tag in ["vendor/mermaid.min.js", "vendor/dompurify.min.js"] {
                assert!(
                    html.contains(tag),
                    "{shell} must load {tag} — the note's mermaid fence reads it"
                );
            }
        }
        // The feature list is the lean bundle's whole argument: CodeMirror is a
        // SECOND editor engine beside Monaco (#308) and LaTeX drags KaTeX in.
        // Neither may return without a decision.
        for absent in ["code-mirror", "latex", "image-block", "top-bar", "ai"] {
            assert!(
                !features.contains(&absent),
                "{absent} is deliberately not in the lean bundle (ADR-0064 §6)"
            );
        }
        // The licence travels with the code.
        assert!(
            UI.get_file("vendor/crepe/LICENSE").is_some(),
            "the vendored bundle must carry its licence"
        );
    }

    /// The editor is served, and both shells that can hold a card load it.
    #[tokio::test]
    async fn root_serves_the_vendored_crepe() {
        let resp = get_local("/vendor/crepe/crepe.js").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        let resp = get_local("/vendor/crepe/crepe.css").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "text/css; charset=utf-8"
        );
        for shell in ["index.html", "detached-fence.html"] {
            let html = UI
                .get_file(shell)
                .and_then(|f| f.contents_utf8())
                .expect("the shell must be embedded");
            assert!(
                html.contains("vendor/crepe/crepe.js") && html.contains("vendor/crepe/crepe.css"),
                "{shell} must load the note editor (ADR-0064 §6)"
            );
        }
    }

    /// The terminal wears the workbench's palette, and wears it in lockstep.
    ///
    /// xterm.js reads no CSS variable — WebGL paints the glyphs — so the theme is
    /// literal hex mirroring `:root`. That mirror is the only thing a drift can
    /// break, and it breaks silently: a console a different shade from the pane
    /// around it. The ANSI slots stay xterm's own, so this pins the base colours
    /// only.
    #[test]
    fn the_console_terminal_is_themed_in_lockstep_with_the_stylesheet() {
        let js = include_str!("../assets/ui/wb-console.js");
        let css = served_css();
        assert!(
            js.contains("new Terminal({ convertEol: false, theme: TERMINAL_THEME })"),
            "wb-console.js must hand xterm a theme — an unthemed Terminal is xterm's black default"
        );
        assert!(
            js.contains("background: \"#000000\""),
            "the terminal surface is pure black — the same exception wb-monaco.js makes for code"
        );
        for (token, hex) in [
            ("--text", "#d4ccc0"),
            ("--console-text", "#e8d9a8"),
            ("--surface-hi", "#423a31"),
        ] {
            assert!(
                js.contains(hex),
                "TERMINAL_THEME must still carry {token}'s value {hex}"
            );
            assert!(
                css.contains(&format!("{token}: {hex};")),
                "styles.css must still define {token} as {hex} — wb-console.js mirrors it"
            );
        }
    }

    /// The console's clipboard contract, pinned where deleting it would regress
    /// in SILENCE.
    ///
    /// Every assertion here guards a property no rendering test can see: that an
    /// agent's OSC 52 is honoured at all, that it is honoured only ONCE and only
    /// in the window that owns the clipboard, and that nothing in this file ever
    /// reads the clipboard back. The feature itself is visible the moment you
    /// copy; these are the invariants that are not.
    #[test]
    fn the_console_clipboard_is_write_only_and_refused_on_replay() {
        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            // The gap that made an agent announce a copy it never made.
            "registerOscHandler(52",
            // The read form is refused, never answered.
            "payload === \"?\"",
            // The replay gate, and the write callback that is the ONLY correct
            // way to clear it — reverting to a bare `term.write(a.subarray(9))`
            // reintroduces the clipboard clobber with every other pin still green.
            "replaying = connOpts.id != null",
            "replaying = false;",
            // Whose clipboard it is: not every attached window's.
            "if (replaying || watching) return true;",
            // Ctrl+Insert, and NOT Ctrl+Shift+C (the DevTools accelerator).
            "e.key !== \"Insert\"",
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the console-clipboard pin {pin}"
            );
        }
        // The read property. The clipboard is read in exactly ONE place — the
        // key bar's paste key, an operator's gesture — and never on a path an
        // agent can trigger: the OSC 52 read form stays refused above, and no
        // other call site may appear. `readClipboard` wraps the API so the raw
        // `readText` has one occurrence to count, plus the feature-detect in
        // `pasteOffered` (a `typeof`, not a call).
        assert_eq!(
            js.matches("navigator.clipboard.readText()").count(),
            1,
            "wb-console.js reads the clipboard in readClipboard() only"
        );
        assert_eq!(
            js.matches("readClipboard()").count(),
            2,
            "readClipboard() has one definition and one caller (the paste key)"
        );
        assert!(
            js.contains("name === \"paste\" ? readClipboard() : null"),
            "the one clipboard read is the paste key's tap"
        );
        // The OSC handler must not RETURN the clipboard promise: xterm's
        // `OscHandler.end` awaits a returned Promise and pauses the parser until
        // it settles, so a rejected write would stall the terminal.
        let handler = js
            .split_once("registerOscHandler(52")
            .expect("the OSC 52 handler")
            .1
            .split_once("\n    });")
            .expect("the end of the OSC 52 handler")
            .0;
        assert!(
            !handler.contains("return writeClipboard") && !handler.contains("return navigator"),
            "the OSC 52 handler must not return a promise — it stalls xterm's parser"
        );
        assert!(
            handler.contains("scrubClipboard"),
            "an agent's clipboard text is scrubbed before it reaches the operator"
        );
    }

    #[tokio::test]
    async fn root_serves_wb_daemon() {
        let resp = get_local("/wb-daemon.js").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET wb-daemon.js → 200");
        let daemon = body_string(resp).await;
        assert!(
            daemon.contains("ACTION_TO_VERB"),
            "wb-daemon.js must ship the action→verb map"
        );
        assert!(
            daemon.contains("/ws/command"),
            "wb-daemon.js must open the command WebSocket"
        );

        let shell = body_string(get_local("/").await).await;
        assert!(
            shell.contains("wb-daemon.js"),
            "the shell HTML must load the daemon adapter"
        );
    }

    #[tokio::test]
    async fn root_serves_wb_mode() {
        let resp = get_local("/wb-mode.js").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET wb-mode.js → 200");
        let mode = body_string(resp).await;
        assert!(
            mode.contains("function modeFor"),
            "wb-mode.js must ship the pure mode predicate"
        );

        let shell = body_string(get_local("/").await).await;
        assert!(
            shell.contains("wb-mode.js"),
            "the shell HTML must load the mode module"
        );
    }

    #[tokio::test]
    async fn favicon_is_served_for_both_pages_and_the_bare_request() {
        // The browser asks for /favicon.ico on its own, whatever the markup says,
        // so the .ico must exist even though the SVG is what a modern browser
        // picks. A 404 here is the console error this replaced.
        let resp = get_local("/favicon.ico").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET /favicon.ico → 200");
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/x-icon"
        );
        let resp = get_local("/favicon.svg").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET /favicon.svg → 200");
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/svg+xml"
        );
        for page in ["/index.html", "/detached.html", "/detached-fence.html"] {
            let html = body_string(get_local(page).await).await;
            assert!(
                html.contains("favicon.svg") && html.contains("favicon.ico"),
                "{page} must link both favicon forms"
            );
        }
    }

    #[tokio::test]
    async fn served_ui_copy_has_no_mock_or_false_claims() {
        for path in swept_ui_assets() {
            let path = path.as_str();
            let resp = get_local(path).await;
            assert_eq!(resp.status(), StatusCode::OK, "GET {path} → 200");
            let lc = body_string(resp).await.to_ascii_lowercase();
            assert!(!lc.contains("mock"), "{path} still contains \"mock\"");
            assert!(
                !lc.contains("nothing is written to disk"),
                "{path} still claims \"nothing is written to disk\""
            );
            assert!(
                !lc.contains("no secrets are stored"),
                "{path} still claims \"no secrets are stored\""
            );
            assert!(
                !lc.contains("any 6-digit code"),
                "{path} still claims \"any 6-digit code\""
            );
        }
    }

    /// The daemon ships and serves no fabricated data.
    ///
    /// The seed exists for the `file://` demo, and #300 made it INERT off
    /// `file://` — but inert is not absent: it was still compiled into every
    /// binary by `include_dir!` and still handed to any caller of `GET /`, which
    /// is unauthenticated on purpose (the shell draws its own login gate). So an
    /// unauthenticated stranger was served four fabricated `plan.md` documents.
    ///
    /// The seed now lives in `assets/ui-demo/`, a sibling of the embedded tree —
    /// the same trick `ui-tests/` already uses to stay out of `include_dir!`.
    /// This is the pin that keeps it there: the next fixture someone needs in a
    /// hurry belongs beside the others, not back in a served file.
    #[tokio::test]
    async fn the_served_ui_carries_no_seed() {
        // Identifiers, not prose: a comment may legitimately DISCUSS the seed
        // (several now do, explaining where it went), and a sweep that cannot
        // tell a mention from the data would make documenting the move fail.
        // The ASSIGNMENT, not the name: `app.js` legitimately READS
        // `window.WB_RUNS` in `initRuns()` — that consumer is production code
        // guarded by `seedAllowed()`, and it stays. What must not come back is a
        // served file that DEFINES the data.
        const SEED: &[&str] = &[
            "window.WB_RUNS = {",
            "window.WB_KANBAN = {",
            "window.WB_SEED_PROJECTS =",
            "window.WB_SEED_ROSTER =",
            "id=\"seed-plan-",
            "function fakeContent(",
            "function fakeMarkdown(",
        ];
        for path in swept_ui_assets() {
            let body = body_string(get_local(&path).await).await;
            for pin in SEED {
                assert!(!body.contains(pin), "{path} still carries the seed {pin}");
            }
        }
        // The other half of the claim: the daemon has no bytes to serve under
        // some other name either. Stated over the TREE rather than as five
        // guessed 404s — "no embedded path is a seed file" is strictly stronger
        // than "these five particular names 404", and it cannot be defeated by
        // naming the sixth one something else.
        for path in embedded_ui_paths() {
            assert!(
                !path.contains("wb-seed-"),
                "{path} is embedded, and the seed lives in assets/ui-demo/ (#300)"
            );
        }
        assert_eq!(
            get_local("/../ui-demo/wb-seed-runs.js").await.status(),
            StatusCode::NOT_FOUND,
            "the sibling seed directory must not be reachable by traversal"
        );
    }

    #[tokio::test]
    async fn translation_is_gone_from_the_served_ui() {
        let resp = get_local("/wb-translate.js").await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "GET /wb-translate.js must 404, the module is deleted"
        );
        for path in swept_ui_assets() {
            let path = path.as_str();
            let resp = get_local(path).await;
            assert_eq!(resp.status(), StatusCode::OK, "GET {path} → 200");
            let lc = body_string(resp).await.to_ascii_lowercase();
            assert!(!lc.contains("xlate"), "{path} still contains \"xlate\"");
            assert!(
                !lc.contains("wbtranslate"),
                "{path} still contains \"wbtranslate\""
            );
        }
    }

    #[tokio::test]
    async fn consoles_tab_is_fixed_and_named() {
        let body = body_string(get_local("/app.js").await).await;
        assert!(
            !body.contains(r#"title: "Agents""#),
            "app.js must not carry the old tab title \"Agents\""
        );
        let hit = body.lines().find(|line| line.contains(r#"id: "consoles""#));
        let line = hit.unwrap_or_else(|| panic!("no line in app.js sets id: \"consoles\""));
        assert!(
            line.contains(r#"title: "Consoles""#),
            "the line setting id: \"consoles\" must also set title: \"Consoles\"; got: {line}"
        );
        assert!(
            line.contains("closable: false"),
            "the line setting id: \"consoles\" must also set closable: false; got: {line}"
        );
    }

    #[tokio::test]
    async fn root_serves_wb_fail() {
        let resp = get_local("/wb-fail.js").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET wb-fail.js → 200");
        let fail = body_string(resp).await;
        assert!(
            fail.contains("function message"),
            "wb-fail.js must ship the message extractor"
        );

        let shell = body_string(get_local("/").await).await;
        assert!(
            shell.contains("wb-fail.js"),
            "the shell HTML must load the failure presenter"
        );
    }

    #[tokio::test]
    async fn session_serves_shell_but_gates_data() {
        // The shell bytes are served without a cookie…
        let resp = session_router("tok")
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "/ served pre-login (NOT redirected)"
        );

        // …but every DATA endpoint stays 401 under a no-cookie Session.
        for uri in [
            "/api/identity",
            "/ws/session?repo=x&agent=claude",
            "/ws/command",
        ] {
            let resp = session_router("tok")
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{uri} must be 401 with no cookie"
            );
        }
    }

    #[tokio::test]
    async fn session_state_reports_authed() {
        // Localhost is always authed.
        let body = body_string(get_local("/api/session").await).await;
        assert!(
            body.contains(r#""authed":true"#),
            "localhost authed: {body}"
        );

        // Session, no cookie → not authed.
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .uri("/api/session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(resp).await;
        assert!(
            body.contains(r#""authed":false"#),
            "no-cookie session not authed: {body}"
        );

        // Session + a valid minted cookie → authed.
        let now = now_unix();
        let code = rfc_seed().code_at(now / 30);
        let login = session_router("tok")
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!("code={code}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        let set_cookie = login
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("a Set-Cookie header")
            .to_string();
        let cookie_pair = set_cookie.split(';').next().unwrap().to_string();
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .uri("/api/session")
                    .header(header::COOKIE, &cookie_pair)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(resp).await;
        assert!(
            body.contains(r#""authed":true"#),
            "valid cookie authed: {body}"
        );
    }

    /// `GET /api/session` reports the wire name of the ACTIVE policy under all
    /// three binds (issue #205), so the Security modal can derive honest,
    /// bind-specific affordances instead of always assuming `Session`.
    #[tokio::test]
    async fn session_state_reports_policy() {
        // Localhost.
        let body = body_string(get_local("/api/session").await).await;
        assert!(
            body.contains(r#""policy":"localhost""#),
            "localhost: {body}"
        );

        // Session, no cookie — the route is allowlisted (200) even though
        // `authed` is false.
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .uri("/api/session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "allowlisted: {resp:?}");
        let body = body_string(resp).await;
        assert!(body.contains(r#""policy":"session""#), "session: {body}");

        // Bearer, with a matching Authorization header.
        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::fixed(
                auth::AuthPolicy::Bearer("tok".into()),
                epoch::SessionEpoch::in_memory_detached(),
            ),
        )
        .oneshot(
            Request::builder()
                .uri("/api/session")
                .header(header::AUTHORIZATION, "Bearer tok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        let body = body_string(resp).await;
        assert!(body.contains(r#""policy":"bearer""#), "bearer: {body}");
    }

    /// The served shell no longer claims every 6-digit code works (that was
    /// true of the pre-#205 mock login) and explains the login gate inline
    /// (issue #205, audit finding AC5; copy updated for the ADR-0032 amendment
    /// where the gate can also apply to a loopback bind).
    #[tokio::test]
    async fn login_gate_drops_mock_hint() {
        let shell = body_string(get_local("/").await).await;
        assert!(
            !shell.contains("any 6-digit code works"),
            "mock hint must be gone"
        );
        assert!(
            shell.contains("Needs 2FA first"),
            "require-login explanation must be present"
        );
    }

    /// `Secure` follows the request's scheme (audit F6, layer 2): a login that
    /// came through a TLS front (`X-Forwarded-Proto: https`, which dev tunnels
    /// forwards — measured 2026-09-21) mints a `Secure` cookie; a plain-http
    /// login does not; the idle-slide re-issue and the clear on logout carry the
    /// same answer, because a slide that dropped the attribute would silently
    /// downgrade the session and one that added it would strand a plain-http
    /// browser.
    #[tokio::test]
    async fn secure_cookie_follows_the_forwarded_scheme_on_login_slide_and_logout() {
        let now = now_unix();
        let code = rfc_seed().code_at(now / 30);
        let login = |forwarded: Option<&'static str>| {
            let mut req = Request::builder()
                .method("POST")
                .uri("/api/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            if let Some(proto) = forwarded {
                req = req.header("x-forwarded-proto", proto);
            }
            req.body(Body::from(format!("code={code}"))).unwrap()
        };
        let set_cookie = |resp: &Response| {
            resp.headers()
                .get(header::SET_COOKIE)
                .and_then(|v| v.to_str().ok())
                .expect("a Set-Cookie header")
                .to_string()
        };

        // Plain http: no `Secure` (the loopback default).
        let resp = session_router("tok").oneshot(login(None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            !set_cookie(&resp).contains("Secure"),
            "{}",
            set_cookie(&resp)
        );

        // Through a TLS front: `Secure`. (Different routers share the token and
        // the detached epoch's start value, and a step already consumed on one
        // is not consumed on another — each has its own last-step store.)
        let resp = session_router("tok")
            .oneshot(login(Some("https")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            set_cookie(&resp).ends_with("; Secure"),
            "{}",
            set_cookie(&resp)
        );
        // A front that says http, or a chain whose FIRST hop is http, is not https.
        let resp = session_router("tok")
            .oneshot(login(Some("http, https")))
            .await
            .unwrap();
        assert!(
            !set_cookie(&resp).contains("Secure"),
            "{}",
            set_cookie(&resp)
        );

        // The idle-slide re-issue: a cookie far enough into its idle window that
        // the guard re-issues it on this request, once over https and once over
        // http, on the SAME router (the slide signs under the router's epoch).
        let app = session_router("tok");
        let stale = cookie::sign(
            "tok",
            0,
            cookie::SessionKind::Standard,
            now - cookie::SLIDE_MIN_SECS - 10,
            now + cookie::SESSION_IDLE_TTL_SECS - cookie::SLIDE_MIN_SECS - 10,
        );
        for (forwarded, want_secure) in [(Some("https"), true), (None, false)] {
            let mut req = Request::builder()
                .uri("/api/identity")
                .header(header::COOKIE, format!("ralphy_session={stale}"));
            if let Some(proto) = forwarded {
                req = req.header("x-forwarded-proto", proto);
            }
            let resp = app
                .clone()
                .oneshot(req.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "the stale cookie still authorizes"
            );
            let slid = set_cookie(&resp);
            assert_eq!(
                slid.contains("Secure"),
                want_secure,
                "slide over {forwarded:?}: {slid}"
            );
        }

        // Logout: the clear carries the attribute of the request that asked.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/logout")
                    .header(header::COOKIE, format!("ralphy_session={stale}"))
                    .header("x-forwarded-proto", "https")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cleared = set_cookie(&resp);
        assert!(
            cleared.contains("Max-Age=0") && cleared.ends_with("; Secure"),
            "{cleared}"
        );
    }

    /// Logging off takes a session (audit F5): the route bumps the epoch for
    /// EVERY cookie, so a caller without one — who has nobody to log off — is
    /// refused before the bump, and the cookies that were valid stay valid.
    /// With a cookie it clears it server-side as before.
    #[tokio::test]
    async fn logout_clears_cookie_and_needs_a_session() {
        let cookie = login_set_cookie("").await;
        let cookie_pair = cookie.split(';').next().unwrap().to_string();
        // ONE router: the epoch lives in its `AuthState`, and the claim is that
        // an unauthenticated POST did not move it.
        let app = session_router("tok");

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/logout")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "no session → nobody to log off"
        );
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/identity")
                    .header(header::COOKIE, &cookie_pair)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "the live cookie survived the refused log-off"
        );

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/logout")
                    .header(header::COOKIE, &cookie_pair)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "POST /api/logout → 200");
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("a Set-Cookie header");
        assert!(
            set_cookie.contains("ralphy_session=;") && set_cookie.contains("Max-Age=0"),
            "cookie cleared: {set_cookie}"
        );
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/identity")
                    .header(header::COOKIE, &cookie_pair)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "and the epoch bump dropped it server-side (amendment §B)"
        );
    }

    /// A wrong TOTP code is rejected `401` by `POST /api/login` (the login handler
    /// checks the code VALUE, not merely form presence — a presence-only bug would
    /// pass the happy-path test above).
    #[tokio::test]
    async fn session_login_rejects_wrong_code() {
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("code=000000"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "wrong code → 401");
    }

    /// The auth layer covers the REMOTE-EXEC WS routes, not just `/api`: an
    /// unauthenticated `/ws/session` (PTY) and `/ws/command` (run dispatch)
    /// request is rejected `401` by the middleware BEFORE reaching the upgrade
    /// handler. A `401` here (not the handler's `400`) proves the layer fired, so
    /// a future route reordered past the layer would fail this test instead of
    /// silently serving an unauthenticated shell/run trigger.
    #[tokio::test]
    async fn bearer_policy_gates_the_remote_exec_ws_routes() {
        for uri in [
            "/ws/session?repo=x&agent=claude",
            "/ws/command",
            "/api/usage",
            "/api/peer/usage",
        ] {
            let resp = router(
                None,
                PathBuf::from("does-not-exist"),
                PathBuf::from("does-not-exist"),
                StorePaths::default(),
                Instant::now(),
                idle_shutdown(),
                auth::AuthState::fixed(
                    auth::AuthPolicy::Bearer("tok".into()),
                    epoch::SessionEpoch::in_memory_detached(),
                ),
            )
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{uri} must be gated by the auth layer, not reach its handler"
            );
        }
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

    /// Every document links every partial, in one order.
    ///
    /// The partials are ONE cascade cut into thirteen files, so this is not a
    /// convention — it is the thing that makes them equivalent to the file they
    /// came from. A document that links eleven of them is missing rules; a
    /// document that links them in a different order gets different winners for
    /// every selector declared in two sections. Neither failure is visible to a
    /// substring pin, and the second is not visible to a human reading a diff.
    #[test]
    fn every_shell_links_the_whole_cascade() {
        let mut expected: Vec<String> = UI
            .get_dir("styles")
            .expect("the stylesheet partials must be embedded")
            .files()
            .map(|f| f.path().to_string_lossy().replace('\\', "/"))
            // `.css` only, matching `served_css()`: anything else dropped into
            // `styles/` is not part of the cascade and must not be demanded as a
            // `<link>` in all three documents.
            .filter(|path| path.ends_with(".css"))
            .collect();
        expected.sort();

        for (shell, html) in [
            ("index.html", include_str!("../assets/ui/index.html")),
            ("detached.html", include_str!("../assets/ui/detached.html")),
            (
                "detached-fence.html",
                include_str!("../assets/ui/detached-fence.html"),
            ),
        ] {
            let linked: Vec<String> = tag_references(html)
                .into_iter()
                .filter(|r| r.starts_with("styles/"))
                .collect();
            assert_eq!(
                linked, expected,
                "{shell} must link every stylesheet partial in numeric order — \
                 the files are one cascade, and order decides which \
                 declaration wins"
            );
        }
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
            .map(|p| format!("/{p}"))
            .collect();
        out.sort();
        out
    }

    /// The sweep set is not allowed to quietly shrink.
    ///
    /// [`swept_ui_assets`] is derived, so nobody can forget to add a file to it —
    /// but somebody could narrow the filter, or move an asset under `vendor/`,
    /// and every sweep would go green over less. This states the floor: the
    /// three shells, and a count no smaller than the tree had when the sweeps
    /// stopped being hand-written lists.
    #[test]
    fn the_sweep_set_covers_every_asset_we_wrote() {
        let swept = swept_ui_assets();
        for shell in ["/index.html", "/detached.html", "/detached-fence.html"] {
            assert!(
                swept.iter().any(|p| p == shell),
                "{shell} must be swept — it is a served document this repo wrote"
            );
        }
        let js = swept.iter().filter(|p| p.ends_with(".js")).count();
        assert!(
            js >= 19,
            "expected at least the 19 non-vendor .js assets the tree held when \
             the sweeps became tree-driven, found {js} — did the filter narrow?"
        );
        // NEGATIVE CONTROL: a sweep set that swallowed the vendor bundles would
        // satisfy every count above while making each sweep meaningless (Monaco
        // says "mock" in its own test fixtures).
        assert!(
            !swept.iter().any(|p| p.starts_with("/vendor/")),
            "vendor bytes are not our copy and must stay out of the sweep set"
        );
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

    /// The three shells and the embedded tree agree, in both directions.
    ///
    /// This is the regression a file split actually causes, and until now
    /// NOTHING caught it: you move a fold out of `app.js` into `wb-foo.js`, and
    /// you forget the `<script>` tag — or you add it to `index.html` and not to
    /// `detached-fence.html`, which loads its own subset of the same modules.
    /// Every substring pin in this file still passes, every `node --test` still
    /// passes, and the workbench is broken in the browser.
    ///
    /// Both directions matter, and they fail differently:
    ///
    /// - A tag pointing at nothing is a 404 at boot — the module never defines
    ///   its global and the first caller dies on `undefined`.
    /// - An asset no shell references is dead weight shipped in the binary, and
    ///   more often it means the tag was DROPPED rather than the file added.
    #[test]
    fn every_shell_tag_resolves_and_every_asset_is_reachable() {
        const SHELLS: [(&str, &str); 3] = [
            ("index.html", include_str!("../assets/ui/index.html")),
            ("detached.html", include_str!("../assets/ui/detached.html")),
            (
                "detached-fence.html",
                include_str!("../assets/ui/detached-fence.html"),
            ),
        ];
        let embedded = embedded_ui_paths();
        let mut referenced: Vec<String> = Vec::new();

        for (shell, html) in SHELLS {
            for reference in tag_references(html) {
                // The seed loader is a template literal resolved at runtime, and
                // it points OUTSIDE the embedded tree on purpose (#300) — the
                // `the_served_ui_carries_no_seed` sweep owns that path.
                if reference.contains("${") || reference.starts_with("../") {
                    continue;
                }
                assert!(
                    embedded.iter().any(|p| p == &reference),
                    "{shell} references {reference}, which is not in the embedded \
                     tree — the browser gets a 404 for it at boot"
                );
                referenced.push(reference);
            }
        }

        // The other direction, and it must be PER SHELL. A union-wide "somebody
        // references it" is too weak to catch the split regression: drop
        // `wb-fleet.js` from index.html and the union is still satisfied by the
        // two popups, which is exactly the shape that ships a broken desk.
        //
        // `index.html` IS the desk, so its required set is derived and can never
        // go stale: every non-vendor asset this repo writes is loaded there. A
        // new module is protected the moment it is embedded, with no list to
        // remember.
        let index = SHELLS[0].1;
        let index_refs = tag_references(index);
        for path in embedded_ui_paths() {
            if path.starts_with("vendor/") {
                continue;
            }
            if !path.ends_with(".js") && !path.ends_with(".css") {
                continue;
            }
            assert!(
                index_refs.iter().any(|r| r == &path),
                "{path} is embedded but index.html has no tag for it — either the \
                 tag was dropped, or the asset is dead weight in the binary"
            );
        }

        // The two popups load SUBSETS, so theirs cannot be derived from the tree
        // — a module they legitimately do not need is not a defect. What each
        // one needs is stated here, and it is the floor: adding a module to a
        // popup never reds this, dropping one always does.
        for (shell, required) in [
            (
                "detached.html",
                &["wb-mode.js", "wb-fleet.js", "wb-monaco.js", "wb-viewer.js"][..],
            ),
            (
                "detached-fence.html",
                &[
                    "wb-mode.js",
                    "wb-fleet.js",
                    "wb-fail.js",
                    "wb-desk-sink.js",
                    "wb-detach-link.js",
                    "wb-session-route.js",
                    "wb-daemon.js",
                    // `wb-console.js` DESTRUCTURES `window.WBGeometry` and
                    // `window.WBWindowState` at module scope, so a popup without
                    // either tag throws on the console's first line rather than
                    // misbehaving later. Stated HERE because this set is a
                    // hardcoded floor: nothing derives the popup's needs from the
                    // tree, so a module added to the console and forgotten here
                    // breaks the second monitor with no other signal at all.
                    "wb-geometry.js",
                    "wb-window-state.js",
                    "wb-console.js",
                ][..],
            ),
        ] {
            let (_, html) = SHELLS
                .iter()
                .find(|(name, _)| *name == shell)
                .expect("the shell was just listed in SHELLS");
            let refs = tag_references(html);
            for module in required {
                assert!(
                    refs.iter().any(|r| r == module),
                    "{shell} must load {module} — the popup is inert without it"
                );
            }
            // The stylesheet is thirteen partials now, and "links all of them, in
            // order" is a stronger statement than this one was — so it is made
            // once, for all three documents, by
            // `every_shell_links_the_whole_cascade`. What stays here is the
            // floor it rests on: a popup that links NO stylesheet at all.
            assert!(
                refs.iter().any(|r| r.starts_with("styles/")),
                "{shell} must load the stylesheet — an unstyled popup is a broken one"
            );
        }

        // The script ORDERS that are a hard dependency rather than a habit:
        // `wb-console.js` destructures `window.WBGeometry` and
        // `window.WBWindowState` at module scope, so a later tag leaves it
        // destructuring `undefined` and the document dies at load. Asserted in
        // BOTH documents that carry the pair — the fence popup is a second boot
        // path and the reason a union-wide check is not enough.
        for (shell, html) in SHELLS {
            let refs = tag_references(html);
            let at = |name: &str| refs.iter().position(|r| r == name);
            for namespace in ["wb-geometry.js", "wb-window-state.js"] {
                if let (Some(before), Some(console)) = (at(namespace), at("wb-console.js")) {
                    assert!(
                        before < console,
                        "{shell} must load {namespace} BEFORE wb-console.js — the \
                         console destructures that namespace at module scope"
                    );
                }
            }
        }
    }

    /// A phone-width pane folds its chrome by the PANE's width, not the
    /// viewport's — a viewer is narrow in a desktop split and in a detached
    /// popup too. That criterion is a `@container` query on the pane, and it
    /// only works if the pane declares itself a container: drop the
    /// `container-type` and every rule inside the query goes silently inert,
    /// with nothing on the JS side to notice. Pinned here for that reason, and
    /// the threshold is pinned at ONE number because `wb-viewer.js` measures
    /// the same 560 to trim Monaco's gutter — two numbers would fold the
    /// captions and the gutter at different widths.
    #[test]
    fn the_narrow_pane_criterion_is_the_panes_own_width() {
        let css = served_css();
        // Anchored at a line start: `#viewers.split > .viewer {` (the slot's
        // two-column override) contains the same characters and must not be
        // the block this reads.
        for (pane, name) in [("\n.viewer {", "viewer"), ("\n.kanban {", "kanban")] {
            let at = css
                .find(pane)
                .unwrap_or_else(|| panic!("{pane} must be styled"));
            let block = &css[at..at + css[at..].find('}').expect("a rule block closes")];
            assert!(
                block.contains("container-type: inline-size") && block.contains(&format!("container-name: {name}")),
                "{pane} must declare itself the `{name}` container, or its @container rules never fire"
            );
            let query = format!("@container {name} (max-width: 560px)");
            assert!(
                css.contains(&query),
                "the stylesheet must fold `{name}` at 560px: `{query}`"
            );
        }
        let js = include_str!("../assets/ui/wb-viewer.js");
        assert!(
            js.contains("const NARROW_PX = 560;"),
            "wb-viewer.js must trim the gutter at the SAME 560px the stylesheet folds the captions"
        );
    }

    /// The UI suite's barrel cannot lie about what it runs.
    ///
    /// `node --test <dir>` given a BARE directory resolves `package.json#main`
    /// and runs that one file — it does not recurse. So `ui-tests/index.mjs` is
    /// the whole suite, and a `*.test.mjs` that nobody imports there is not a
    /// failing test: it is a file the runner never opens, silently. That is the
    /// worst shape a test can have, and it is invisible to the JS side by
    /// construction — the suite cannot notice a file it never loads. Only a
    /// reader of the DIRECTORY can, which is why this gate is in Rust.
    #[test]
    fn every_ui_test_file_is_imported_by_the_barrel() {
        // Comment-stripped: an import parked behind `//` does not run, and the
        // whole point of this gate is that a file the runner never opens is not
        // a passing test.
        let barrel: String = include_str!("../ui-tests/index.mjs")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui-tests");
        let mut found = 0;
        for entry in std::fs::read_dir(&dir).expect("the ui-tests directory ships with the crate") {
            let name = entry
                .expect("a directory entry the read_dir call just yielded")
                .file_name()
                .to_string_lossy()
                .into_owned();
            if !name.ends_with(".test.mjs") {
                continue;
            }
            found += 1;
            assert!(
                barrel.contains(&format!("import \"./{name}\";")),
                "ui-tests/{name} is never imported by ui-tests/index.mjs, so \
                 `node --test crates/ralphy-daemon/ui-tests` never runs it"
            );
        }
        assert!(
            found >= 10,
            "expected the UI suite to hold at least the ten test files it had \
             when this gate was written, found {found} — did the glob break?"
        );
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
            include_str!("../assets/ui/wb-monaco.js").contains("monaco.editor.create"),
            "wb-monaco.js must build the editor through Monaco's own factory"
        );

        let viewer = include_str!("../assets/ui/wb-viewer.js");
        assert!(
            viewer.contains("WBMonaco.create"),
            "wb-viewer.js must mount its editor through WBMonaco"
        );
        // The mirror pane (ADR-0037 §3c) is a SECOND editor over the pane's
        // model, never a second model: `wb_monaco_308.py` counts models per
        // open pane and a mirror must not move that count.
        assert!(
            include_str!("../assets/ui/wb-monaco.js").contains("function createOver("),
            "wb-monaco.js must offer an editor over an existing model"
        );
        assert!(
            viewer.contains("WBMonaco.createOver"),
            "wb-viewer.js must mount the mirror through WBMonaco.createOver"
        );
        // Built from parts so this pin cannot trip on its own source text.
        let outgoing = concat!("Code", "Mirror(");
        assert!(
            !viewer.contains(outgoing),
            "wb-viewer.js must not construct a {outgoing} editor"
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

        let js = include_str!("../assets/ui/wb-changes.js");
        assert!(
            js.contains("st-unknown"),
            "wb-changes.js must keep the st-unknown fallback"
        );
        for status in [
            "modified",
            "added",
            "deleted",
            "renamed",
            "untracked",
            "conflicted",
        ] {
            assert!(
                js.contains(status),
                "wb-changes.js must keep the {status} marker"
            );
        }

        // The diff tab (#311) is JS/HTML only, so no other Rust gate compiles it:
        // pin the row's click wiring and the diff editor's factory here.
        assert!(
            html.contains("openDiff(openSlug, c)"),
            "index.html must open a diff from a changes row"
        );
        assert!(
            include_str!("../assets/ui/wb-viewer.js").contains("WBMonaco.createDiff"),
            "wb-viewer.js must mount the diff through WBMonaco.createDiff"
        );
        assert!(
            js.contains("diffTarget"),
            "wb-changes.js must expose diffTarget"
        );

        // The staged/unstaged split (#315) is JS/HTML too: pin the row's two
        // halves, the group headline, and the per-group `:key` prefix without
        // which Alpine collides a staged-then-modified path's two rows.
        for pin in [
            r#"class="chg-name""#,
            r#"class="chg-dir""#,
            r#"class="chg-group-head""#,
            ">Staged Changes<",
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
        for pin in ["worktreeStatus", "indexStatus", "lastIndexOf"] {
            assert!(
                js.contains(pin),
                "wb-changes.js must keep the index-split pin {pin}"
            );
        }

        // The promotion to a rail view (#317): the view's root, the rail icon
        // that reaches it, and the per-project indicator that keeps the count
        // reachable with no navigation.
        for pin in [
            r#"class="changes-view""#,
            r#"data-lucide="git-compare""#,
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
        assert!(
            js.contains("function projectBadge("),
            "wb-changes.js must keep the per-project badge fold (#317)"
        );

        // The write controls (#318). The `node --test` suite CI runs never
        // renders markup and Playwright does not run there, so these substring
        // pins are the only CI-visible gate over this markup — every control the
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
        for pin in [
            "function groupPaths(",
            "function commitTarget(",
            "function writeLockReason(",
        ] {
            assert!(
                js.contains(pin),
                "wb-changes.js must keep the write-control helper {pin}"
            );
        }
    }

    /// The discard control (#319) — the same CI-visible substring gate the write
    /// controls get, plus the NEGATED pin that keeps a group-level "discard all"
    /// out: the issue asks for one file at a time, and a one-tap discard of every
    /// path is precisely the mis-tap PRD #297 refused to ship.
    #[test]
    fn the_discard_control_is_pinned_in_the_markup() {
        let html = include_str!("../assets/ui/index.html");
        let js = include_str!("../assets/ui/wb-changes.js");

        for pin in [
            r#"data-act="discard""#,
            r#"class="chg-group-note""#,
            "groupNote('unstaged')",
            "groupNote('staged')",
            "discardRow(openSlug, c)",
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
        for pin in ["function discardConfirm(", "function groupDiscardNote("] {
            assert!(js.contains(pin), "wb-changes.js must keep the fold {pin}");
        }
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
    /// Pinned here because no JS runs in CI and the failure is silent by
    /// construction: the binding is present, the class is spelled correctly, and
    /// the only symptom is an alarm that stays quiet.
    #[test]
    fn presence_staleness_is_derived_on_a_clock_not_inside_the_binding() {
        let html = include_str!("../assets/ui/index.html");
        assert!(
            html.contains(r#":class="{ stale: presenceStale }""#),
            "the uptime must bind the DERIVED flag"
        );
        assert!(
            !html.contains("Date.now() - _lastHeartbeat"),
            "staleness must not be computed inside a binding — it cannot re-fire"
        );
        let app = include_str!("../assets/ui/app.js");
        assert!(
            app.contains("presenceStale: false,"),
            "app.js must declare the flag as reactive state"
        );
        // Inside the tick, not merely somewhere in the file: a computation that
        // is not on a clock is the bug this test is named after.
        let tick = app
            .split_once("this._clockTick = setInterval(")
            .expect("app.js must keep the shell's clock tick")
            .1;
        let tick = &tick[..tick.find("}, 1000);").expect("the clock tick must close")];
        assert!(
            tick.contains("this.presenceStale ="),
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
    /// Pinned HERE because neither `wb_fleet_label.js` nor `wb_fleet_352.py`
    /// runs in CI: this is the only gate that fails when the fold is deleted or
    /// a surface is reverted to printing the ref.
    #[test]
    fn the_workbench_never_titles_a_repo_with_its_routing_head() {
        let fleet = include_str!("../assets/ui/wb-fleet.js");
        for pin in ["function refSlug(", "function refLabel("] {
            assert!(fleet.contains(pin), "wb-fleet.js must keep the fold {pin}");
        }
        // The popups load `wb-viewer.js`/`wb-console.js`, which now call the
        // fold — without the script tag the label silently falls back to the
        // ref in exactly the two windows nobody tests by hand.
        for page in [
            include_str!("../assets/ui/detached.html"),
            include_str!("../assets/ui/detached-fence.html"),
        ] {
            assert!(
                page.contains(r#"<script src="wb-fleet.js"></script>"#),
                "a detached popup must load the fold it calls"
            );
        }
        let app = include_str!("../assets/ui/app.js");
        assert!(
            app.contains("projectLabel(ref) {"),
            "app.js must keep the label helper the shell binds to"
        );
        // The crumb is the surface that STARTED this, and it no longer exists:
        // it went with the top bar, because the sidebar already names the open
        // project on its top row. So the pin moves to the surfaces that still
        // put the project's name on screen. It is deliberately NOT
        // `contains("projectLabel(openSlug)")` any more — ten call sites satisfy
        // that substring, so it would stay green with every one of these
        // surfaces reverted to the raw ref.
        let html = include_str!("../assets/ui/index.html");
        for pin in [
            r#"<span class="kanban-scope" x-text="openSlug ? projectLabel(openSlug) : 'no project'""#,
            r#"<span class="spend-project" x-text="projectLabel(openSlug)""#,
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
            r#"x-text="openSlug ?? 'no project'""#,
            r#"x-text="openSlug""#,
        ] {
            assert!(
                !html.contains(anti),
                "index.html must never print the routing head raw: {anti}"
            );
        }
        // The console title says the environment already; saying the ULID too
        // is what made it read `console · 01KY…/owner/repo · WSL: Ubuntu-22.04`.
        let console = include_str!("../assets/ui/wb-console.js");
        assert!(
            console.contains("WBFleet.refSlug(repo)"),
            "a session title must name the slug, not the ref (the environment follows it)"
        );
    }

    /// The fence floor, pinned where CI can see it — neither the node table nor
    /// the Playwright suite runs there, so this is the only gate that fails when
    /// the shell half of #340 is deleted or renamed.
    #[test]
    fn shell_draws_fences_below_the_windows() {
        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "function nextFenceSlot(",
            "function renderFences(",
            "function createFence(",
            "function renameFence(",
            "function removeFence(",
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #340 pin {pin}"
            );
        }
        // The spawn RULE is pure and moved to `wb-geometry.js` (ADR-0057); the
        // slot search that consumes it reads the plane and stayed. Pinning it in
        // its new home keeps #340's claim — "where a fence lands is a function,
        // not a placement" — stated somewhere.
        assert!(
            include_str!("../assets/ui/wb-geometry.js").contains("function fenceSpawnRect("),
            "the fence spawn rule must stay in wb-geometry.js (#340, ADR-0057)"
        );
        // The plane is sized to windows AND fences (ADR-0051 §2) AND note cards
        // (ADR-0064 §8). Reverting this ONE selector leaves every other test
        // green while a fence or a card past the last window becomes
        // unreachable — the stage never grows to hold it.
        assert!(
            js.contains(r#"querySelectorAll(".session-window, .fence, .note-card")"#),
            "applyExtent must fold the fences and the cards into the stage extent (#340, ADR-0064)"
        );
        let app = include_str!("../assets/ui/app.js");
        assert!(
            app.contains("newFence("),
            "app.js must wire the toolbar act"
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
            squeezed.contains(r#"name.addEventListener("mousedown", (e) => { if (name.readOnly) e.preventDefault(); });"#),
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
        // THE CAP: refused, not absorbed. `saveFences` prunes to `FENCE_MAX` by
        // dropping the oldest `ts`, so a 13th fence used to cost the operator a
        // DIFFERENT one — named, positioned, and merely the least recently touched
        // (`ts` is refreshed on every move, resize and rename). The prune stays as
        // the backstop for a desk that arrives over the cap; the gesture refuses.
        assert!(
            squeezed.contains("if (atFenceCap()) return false;"),
            "createFence must refuse at the cap instead of evicting a fence"
        );
        assert!(
            squeezed.contains("pruneDesk(next, FENCE_MAX)"),
            "the prune must remain the backstop for an over-cap desk from elsewhere"
        );
        // …and the name must not be counted. MEASURED against the running shell:
        // `Fence ${fences.length + 1}` froze at the cap, so the 13th fence and every
        // one after were all born "Fence 13" — duplicates in the list that IS the
        // plane's map (there is no minimap, and the Alt+Shift+F<n> rows are picked
        // out by name).
        assert!(
            squeezed.contains("name: nextFenceName(fences),"),
            "a new fence must be numbered from the names present, not from the count"
        );
        assert!(
            !squeezed.contains("name: `Fence ${fences.length + 1}`"),
            "numbering by the fence COUNT collides at the cap — measured"
        );

        let app_js = include_str!("../assets/ui/app.js");
        let app: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
        // The shell says why before the click and again if one gets through — the
        // #318 idiom, since `wb-console.js` reaches no shell and can only refuse.
        assert!(
            app.contains(
                "if (WBConsole.createFence() === false) this._flashAction(this.fenceCapMessage());"
            ),
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
            app.contains("fenceShortcutLabel(n) { return `F${n}`; }"),
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
    /// nothing. Neither the node table nor the Playwright suite runs in CI, so
    /// the four gesture handlers are pinned here on the predicate they consult.
    #[test]
    fn shell_drags_only_past_a_threshold() {
        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "const DRAG_THRESHOLD = { mouse: 4, touch: 10 }",
            "function dragThreshold(",
            "function dragBegins(",
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the threshold pin {pin}"
            );
        }
        let body = |name: &str| -> String {
            let after = js
                .split_once(name)
                .unwrap_or_else(|| panic!("wb-console.js must keep {name}"))
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
        // The console gestures persist only once armed: a bare tap must not
        // refresh `ts`, or the tap on one device out-folds a move on another.
        // `persist()` is the hook, not `persistWin` directly, since the note
        // card drops through the same gesture into its own collection
        // (ADR-0064 §8). BOTH halves are pinned: that only an armed gesture
        // persists, AND what the hook defaults to. Pinning the call alone
        // would let a regression bind the default to a no-op — every test
        // green while an armed window drag persists nothing and the layout is
        // lost on the next reload.
        for handler in ["function makeDraggable(", "function startResize("] {
            let b = body(handler);
            assert!(
                b.contains("if (armed) persist()"),
                "{handler} must persist only an armed gesture"
            );
            assert!(
                b.contains("opts?.onDrop || (() => persistWin(win))"),
                "{handler}'s drop hook must default to persisting the window"
            );
        }
        // The projection a card's lock is derived from. Pinned in Rust because
        // the module's fence array is not reachable from a ui-test: an
        // implementation that handed out the LIVE records, or that dropped
        // `rect`/`locked` from the copy, would break `lockedBy`'s "fence"
        // verdict with nothing on either side to catch it.
        assert!(
            body("function fenceRecords(").contains("fences.map((f) => ({ ...f }))"),
            "fenceRecords must answer copies of the whole record (ADR-0064 §8)"
        );
        // And the card must actually PASS both hooks: the default is correct
        // for a window and wrong for a card, whose node has no `_deskLocked`
        // and whose rect belongs to another collection.
        let notes_js = include_str!("../assets/ui/wb-notes.js");
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
        let console = include_str!("../assets/ui/wb-console.js").replace("\r\n", "\n");
        let notes = include_str!("../assets/ui/wb-notes.js");
        // The seam: a place in the tier WITHOUT focus, because a restore
        // focuses nothing and `focusWin` is the only other way to get one.
        assert!(
            console.contains("function stackWin(") && console.contains("\n    stackWin,\n"),
            "wb-console.js must export stackWin, the tier a restored surface enters by"
        );
        assert!(
            notes.contains("window.WBConsole.stackWin?.(el)"),
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
            notes.contains("tools.append(tone, index, veil, lock, close)"),
            "the head's cluster is the CARD's controls; the file's is not among them"
        );
        // `Hide this note` is gone (the operator's call, 2026-09-22): the eye
        // hides any card, so the entry was a second door to one place. The
        // UNMARK is not a duplicate of anything — the eye can mark and reveal
        // but never unmark — so it stays, shown only in the state it undoes.
        assert!(
            !notes.contains(r#""Hide this note""#),
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
            notes.contains("if (!marked && veiledNow(el))"),
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
        // The look is three closed sets in the FILE (§8 amendment), and the
        // two defaults are omitted so no note already on a plane is rewritten.
        assert!(
            notes.contains(r#"const FILLS = ["wash", "solid"]"#)
                && notes.contains("if (fill !== DEFAULT_FILL) lines.push")
                && notes.contains("if (ink !== DEFAULT_INK) lines.push"),
            "the palette's fields must be a closed set whose defaults stay out of the file"
        );
    }

    /// A console and a fence can be LOCKED in place (ADR-0050 / ADR-0051 lock
    /// amendment, 2026-09-20): the four gesture handlers and the tiler consult
    /// the lock and refuse, the chrome carries the toggle, and the stylesheet
    /// drops the bands. Every pin is an expression, as above.
    #[test]
    fn shell_locks_consoles_and_fences() {
        let geometry = include_str!("../assets/ui/wb-geometry.js");
        assert!(
            geometry.contains("function fenceOf("),
            "wb-geometry.js must keep fenceOf, the fold a gesture asks whose a window is"
        );
        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "function fenceLocked(",
            "function isLocked(",
            "function applyLock(",
            "function toggleLock(",
            "function paintFenceLock(",
            "function setFenceLock(",
            "function applyLocksFromMirror(",
            "actions.append(restartBtn, fullBtn, lockBtn, maxBtn, closeBtn)",
            "tools.append(tile, lock, detach, drop)",
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the lock pin {pin}"
            );
        }
        let body = |name: &str| -> String {
            let after = js
                .split_once(name)
                .unwrap_or_else(|| panic!("wb-console.js must keep {name}"))
                .1;
            after[..after.find("\n  }").expect("the function must close")].to_string()
        };
        // `heldFast()` defaults to `isLocked(win)` and is the note card's own
        // lock when a card is dragging (ADR-0064 §8); the refusal is the same.
        for handler in ["function makeDraggable(", "function startResize("] {
            let b = body(handler);
            assert!(
                b.contains("if (heldFast()) return")
                    && b.contains("opts?.locked || (() => isLocked(win))"),
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
            body("function persistWin(").contains("locked: !!win._deskLocked"),
            "persistWin must write the lock as a bool"
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
            body("function ingestDesk(").contains("applyLocksFromMirror()"),
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
            css_rule_body(&css, ".fence-head {").contains("8.5rem"),
            "the head's reserve must make room for the fourth tool"
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
    /// gestures that carry it. Same reason as the pin above — neither the node
    /// table nor the Playwright suite runs in CI, so this is the only gate that
    /// fails when this slice is deleted or renamed.
    #[test]
    fn shell_fences_are_a_group() {
        let js = include_str!("../assets/ui/wb-console.js");
        for pin in ["function startFenceMove(", "function startFenceResize("] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #341 pin {pin}"
            );
        }
        // The three folds #341 is really about are pure, and moved to
        // `wb-geometry.js` (ADR-0057). The gestures above stayed, because they
        // are DOM wiring. That division is the issue's own claim — "membership
        // is derived, never stored" is a property of a function over rects —
        // so pinning them in their new home states it better than before.
        let geometry = include_str!("../assets/ui/wb-geometry.js");
        for pin in [
            "function fenceMembership(",
            "function fenceFits(",
            "function fenceMoveDelta(",
        ] {
            assert!(
                geometry.contains(pin),
                "wb-geometry.js must keep the #341 fold {pin}"
            );
        }
        // Membership is DERIVED, never stored: the only fence id in the shell is
        // the fence element's OWN `data-fence-id`. A desk record that carried one
        // is exactly the state that can disagree with the geometry.
        let persist = js
            .split_once("function persistWin(")
            .expect("wb-console.js must keep persistWin")
            .1;
        assert!(
            !persist[..persist.find("\n  }").expect("persistWin must close")].contains("fence"),
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
    /// tiling is a per-fence act over a pure fold. Same reason as the pins
    /// above — this is the only gate that runs in CI, so a revert of either
    /// half (the retirement or the fence chrome) fails HERE or nowhere.
    #[test]
    fn shell_arranges_into_the_fence() {
        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "function arrangeFence(",
            "function fenceRepos(",
            "function refreshFenceChrome(",
            "fence-arrange",
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #342 pin {pin}"
            );
        }
        // The fold itself moved to `wb-geometry.js` (ADR-0057) — it is pure, and
        // that is the seam. Pinned where it now lives rather than dropped: the
        // claim #342 makes is that tiling IS a pure fold, and the file it lives
        // in is the evidence for that claim, not an incidental detail.
        assert!(
            include_str!("../assets/ui/wb-geometry.js").contains("function tileIntoRect("),
            "the tiling fold must stay in wb-geometry.js (#342, ADR-0057)"
        );
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
            .expect("wb-console.js must keep arrangeFence")
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
        let app = include_str!("../assets/ui/app.js");
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
        // The window floor lives in TWO files — the CSS declaration and the
        // constants `arrangeFence` relaxes it against. Nothing else notices when
        // they diverge: the tiles would silently render past the fence again.
        for (konst, decl) in [
            ("WIN_MIN_W = 240", "min-width: 240px"),
            ("WIN_MIN_H = 150", "min-height: 150px"),
        ] {
            assert!(
                js.contains(konst),
                "wb-console.js must mirror the window floor as {konst} (#342)"
            );
            assert!(
                rule("\n.session-window {").contains(decl),
                "styles.css's window floor must still be `{decl}` — wb-console.js mirrors it (#342)"
            );
        }
    }

    /// The fence list is the MAP (#343): the toolbar picker, the jump that
    /// reuses #337's arithmetic, and the birth of a console inside the focused
    /// fence. Same reason as the pins above — neither the node table nor the
    /// Playwright suite runs in CI, so a deletion fails HERE or nowhere. Every
    /// pin below is an EXPRESSION, not a bare noun: #342 measured that a
    /// function's own explanatory comment satisfies a noun pin, leaving it green
    /// over deleted code.
    #[test]
    fn shell_lists_the_fences() {
        let js = include_str!("../assets/ui/wb-console.js");
        let geometry = include_str!("../assets/ui/wb-geometry.js");
        assert!(
            geometry.contains("function rectHolds("),
            "the containment predicate must stay in wb-geometry.js (#343, ADR-0057)"
        );
        for pin in [
            "function fenceSummaries(",
            "function fenceList(",
            "function jumpToFence(",
            "function spawnRectIn(",
            "function focusFence(",
            "function clearFenceFocus(",
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #343 pin {pin}"
            );
        }
        let body = |name: &str| -> String {
            let after = js
                .split_once(name)
                .unwrap_or_else(|| panic!("wb-console.js must keep {name}"))
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
                !b.contains("anchorIntoView(restoreRect")
                    && !b.contains("bringIntoView(restoreRect"),
                "{jump} must not carry a second copy of the arithmetic"
            );
        }
        for (name, what) in [
            ("function bringIntoView(", "bring-into-view"),
            ("function anchorIntoView(", "anchor-into-view"),
        ] {
            assert_eq!(
                js.matches(name).count(),
                1,
                "there may be exactly ONE {what} implementation (#343)"
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
        assert!(
            body("function fenceCycle(").contains("fenceOrder("),
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
        assert!(
            body("function onFloorDown(").contains("clearFenceFocus()"),
            "a bare-floor press outside the focused fence must clear it (#343)"
        );
        // The containment predicate is REUSED, not re-spelled — the same rule
        // the issue states for `bringIntoView`. Pinning only the definition
        // lets an inlined comparison sit beside it as exported dead code.
        // The two owners now live in two files — `fenceMembership` went to
        // `wb-geometry.js` with the predicate it shares, the floor's hit test
        // stayed with the DOM it reads — so the slicer is told which source to
        // carve. That is the whole change: the invariant ("one containment
        // predicate, two callers") is exactly what it was, and it is now stated
        // ACROSS the seam, which is where it can actually break.
        let geometry_body = |name: &str| -> String {
            let after = geometry
                .split_once(name)
                .unwrap_or_else(|| panic!("wb-geometry.js must keep {name}"))
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
            body("function buildChrome(").contains("spawnRectIn("),
            "buildChrome must place a fence-born console through spawnRectIn (#343)"
        );
        // One fold feeds BOTH readouts: the fence's own chrome and the toolbar
        // row can never disagree about a count.
        assert!(
            body("function refreshFenceChrome(").contains("fenceSummaries("),
            "the fence chrome must read the same fold the list does (#343)"
        );
        let app = include_str!("../assets/ui/app.js");
        for pin in ["jumpFence(", "fenceList()"] {
            assert!(app.contains(pin), "app.js must keep the #343 pin {pin}");
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

    /// A fence detaches into its own window, and comes home (#346). Neither the
    /// node table nor the Playwright suite runs in CI, so a deletion fails HERE
    /// or nowhere. Every pin is an EXPRESSION, not a bare noun: #342 measured
    /// that a function's own explanatory comment satisfies a noun pin, leaving
    /// it green over deleted code.
    #[test]
    fn shell_detaches_a_fence() {
        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "function detachFold(",
            "const DETACH_MAX = 4",
            "function detachFence(",
            "function reattachFence(",
            "function mountDetached(",
            "fence-detach",
            "fence-detached",
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #346 pin {pin}"
            );
        }
        let body = |name: &str| -> String {
            let after = js
                .split_once(name)
                .unwrap_or_else(|| panic!("wb-console.js must keep {name}"))
                .1;
            after[..after.find("\n  }").expect("the function must close")].to_string()
        };
        // THE SINK SEAM. The persistence call sites must reach the desk through
        // the injected sink and carry NO `detached` branch of their own —
        // "incapable of writing", not "careful not to".
        assert!(
            body("function flushDesk(").contains("deskSink.put(body)"),
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
        assert!(
            !js.contains(r#""/api/desk", {"#),
            "the desk PUT must live only in wb-desk-sink.js (#346)"
        );
        let sink = include_str!("../assets/ui/wb-desk-sink.js");
        assert!(
            sink.contains(r#""/api/desk", {"#),
            "wb-desk-sink.js must still perform the desk PUT (#346)"
        );
        assert!(
            sink.contains("keepalive: true"),
            "the sink's putSync must outlive the closing document (#346)"
        );
        // Arrange is a no-op on a detached fence (ADR-0051 §7a): its consoles are
        // in another window, and tiling the empty box would rewrite the very
        // rects the re-attach restores from.
        assert!(
            body("function arrangeFence(").contains("if (detached.includes(id)) return;"),
            "arrangeFence must bail on a detached fence (#346, §7a)"
        );
        // Removing a detached fence would destroy the glyph that is the only way
        // home and keep its DETACH_MAX slot consumed — refuse it, do not silently
        // strand the popup.
        assert!(
            body("function removeFence(").contains("detached.includes(id)"),
            "removeFence must refuse a detached fence (#346, §7a)"
        );
        // The re-attach message must trust the SOURCE lookup, not the payload:
        // `m.fenceId` would let a popup for fence A re-attach fence B.
        assert!(
            !js.contains("reattachFence(m.fenceId"),
            "the re-attach message must use the proven owner, never its payload (#346)"
        );
        // The cap is the fold's, not a caller's: a second copy of the rule would
        // satisfy a bare-noun pin while the fold's own check was deleted.
        assert!(
            body("function detachFold(").contains("reg.length >= DETACH_MAX"),
            "the four-popup cap must be enforced inside the fold (#346)"
        );
        // The INVARIANT: either the popup exists and the members are torn down,
        // or neither. `window.open` must therefore be reached before a single
        // window is touched, and a null handle must bail.
        let detach = body("function detachFence(");
        assert!(
            detach.contains("window.open(\"detached-fence.html\""),
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
            teardown.contains("win._term?.dispose()") && teardown.contains("wins.delete(win)"),
            "tearDownMember must dispose the terminal and drop the window (#346, §9)"
        );
        assert!(
            !teardown.contains("forgetRecord") && !teardown.contains("sessions/close"),
            "a detached member must keep its desk record and its daemon session (#346)"
        );
        // THE POPUP'S DENIED CAPABILITIES. Each is what stops a window holding a
        // FRAGMENT of the plane from writing the whole desk or the shell's view.
        let html = include_str!("../assets/ui/detached-fence.html");
        for pin in [
            "window.WBDeskSink.none()",
            "autoBoot: false",
            "canLaunch: false",
            "read: () => null",
            "WBConsole.mountDetached(",
            "wb-fence-ready",
            "wb-fence-reattach",
        ] {
            assert!(
                html.contains(pin),
                "detached-fence.html must keep the #346 pin {pin}"
            );
        }
        assert!(
            !html.contains("WBConsole.open("),
            "the popup must expose no way to open a new console (#346)"
        );
        // The handshake's confidentiality control: a concrete targetOrigin, so a
        // page on any other origin never receives this fence's members. Pinned as
        // the ASSIGNMENT, not the bare noun — `window.location.origin` also
        // appears in the inbound guard below it, so rewriting `PEER` to an
        // unconditional `"*"` would leave a noun pin green (the file already
        // contains a literal `"*"` for the demo leg).
        assert!(
            html.contains(": window.location.origin;"),
            "the popup's PEER must resolve to a concrete origin (#346)"
        );
        assert!(
            !html.contains(r#"postMessage({ type: "wb-fence-ready" }, "*")"#),
            "the popup must never broadcast its handshake to \"*\" (#346)"
        );
        // The opener's reply is demo-aware for the same reason the popup's PEER
        // is: under `file://` an unconditional `location.origin` is dropped.
        assert!(
            body("window.addEventListener(\"message\"")
                .contains("isDemo() ? \"*\" : location.origin"),
            "the opener's handover must mirror the popup's demo-aware origin (#346)"
        );
        // The tree-wide sweep in `shell_stores_only_the_view_in_the_browser`
        // scans .html too; keep this document out of the browser's stores.
        assert!(
            !html.contains("localStorage") && !html.contains("sessionStorage"),
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
        // SCRIPT ORDER, the same hazard #339 pinned for wb-view.js. `wb-console.js`
        // hard-dereferences `window.WBDeskSink.daemon()` at module load, so a
        // reordered or dropped tag throws out of the whole IIFE and the Consoles
        // tab dies — while every source-text pin above stays green.
        let shell = include_str!("../assets/ui/index.html");
        for (doc, name) in [(shell, "index.html"), (html, "detached-fence.html")] {
            let sink_tag = doc
                .find(r#"<script src="wb-desk-sink.js"></script>"#)
                .unwrap_or_else(|| panic!("{name} must load wb-desk-sink.js (#346)"));
            let console_tag = doc
                .find(r#"<script src="wb-console.js"></script>"#)
                .unwrap_or_else(|| panic!("{name} must load wb-console.js (#346)"));
            assert!(
                sink_tag < console_tag,
                "{name}: wb-desk-sink.js must be script-tagged BEFORE wb-console.js (#346)"
            );
        }
    }

    /// The detach survives an F5, and dies with the tab that opened it (#347).
    /// Same bargain as `shell_detaches_a_fence`: neither the node table nor the
    /// Playwright suite runs in CI, so a deletion fails HERE or nowhere. Every
    /// pin is an EXPRESSION — #342 measured that a function's own explanatory
    /// comment satisfies a bare-noun pin over deleted code.
    #[test]
    fn shell_survives_a_reload_with_its_detach() {
        let js = include_str!("../assets/ui/wb-console.js");
        let link = include_str!("../assets/ui/wb-detach-link.js");
        let html = include_str!("../assets/ui/detached-fence.html");
        let shell = include_str!("../assets/ui/index.html");
        let body = |name: &str| -> String {
            let after = js
                .split_once(name)
                .unwrap_or_else(|| panic!("wb-console.js must keep {name}"))
                .1;
            after[..after.find("\n  }").expect("the function must close")].to_string()
        };

        // The rule is a PURE FOLD, and the shell reaches storage and channel
        // only through the injected link.
        for pin in [
            "function peerFold(",
            "link.readRegistry()",
            "link.writeRegistry(",
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #347 pin {pin}"
            );
        }
        // THE STORE SEAM, with its NEGATIVE CONTROL: the console module must name
        // no browser store at all, and the link module must be the one that does
        // — deleting the store wholesale has to be red, not green.
        assert!(
            !js.contains("sessionStorage"),
            "wb-console.js must reach the registry only through the injected link (#347)"
        );
        // The CALLS, not the bare nouns: this file's own prose names
        // `sessionStorage` three times, so a noun pin stays green over
        // no-op'd `readRecord`/`writeRecord` bodies — the exact defect the
        // doc comment above warns about.
        assert!(
            link.contains("sessionStorage.getItem(KEY)")
                && link.contains("sessionStorage.setItem(KEY,")
                && link.contains("new BroadcastChannel(CHANNEL)"),
            "wb-detach-link.js IS the registry and the channel — deleting it is not how #347 stays green"
        );
        assert!(
            link.contains(r#"const KEY = "wb.detach.v1""#)
                && link.contains(r#"const CHANNEL = "wb.detach.v1""#),
            "wb-detach-link.js must keep the one registry key and channel name (#347)"
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
            js.contains(r#"out.push({ record: null, session: s, action: "adopt" })"#),
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
            !js.contains("detached = out.registry"),
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
        // THE POPUP'S FIFTH DENIED CAPABILITY and its half of the lifecycle.
        for pin in [
            "detachLink: window.WBDetachLink.none()",
            // The channel-only factory: this document must reach no store, not
            // even to read the copy `window.open` handed it.
            "window.WBDetachLink.channel()",
            "\"popup-here\"",
            "\"popup-gone\"",
            // The BEHAVIOUR, not the class name: `detached-lost` alone is
            // satisfied by the CSS rule in this file's own <style> block, so
            // deleting `lost()` outright would keep a bare-noun pin green.
            r#"'<p class="detached-lost">"#,
            "window.close()",
            "WBConsole.peerFold(",
        ] {
            assert!(
                html.contains(pin),
                "detached-fence.html must keep the #347 pin {pin}"
            );
        }
        // #346's confidentiality control is UNCHANGED: the channel carries only
        // lifecycle chatter, the members still ride the concrete-origin handshake.
        assert!(
            html.contains(": window.location.origin;"),
            "the initial handover must keep its concrete targetOrigin (#346, #347)"
        );
        assert!(
            !html.contains("localStorage") && !html.contains("sessionStorage"),
            "the popup must still store nothing in the browser (#346)"
        );
        // SCRIPT ORDER, the same hazard #339/#346 pinned: `wb-console.js`
        // hard-dereferences `window.WBDetachLink` at module load, so a reordered
        // or dropped tag throws out of the whole IIFE while every pin above
        // stays green.
        for (doc, name) in [(shell, "index.html"), (html, "detached-fence.html")] {
            let link_tag = doc
                .find(r#"<script src="wb-detach-link.js"></script>"#)
                .unwrap_or_else(|| panic!("{name} must load wb-detach-link.js (#347)"));
            let console_tag = doc
                .find(r#"<script src="wb-console.js"></script>"#)
                .unwrap_or_else(|| panic!("{name} must load wb-console.js (#347)"));
            assert!(
                link_tag < console_tag,
                "{name}: wb-detach-link.js must be script-tagged BEFORE wb-console.js (#347)"
            );
        }
    }

    /// Peer session ownership must survive every browser reconnect and close
    /// path; exact repo equality keeps a local slug from lighting a peer row.
    #[test]
    fn workbench_session_assets_preserve_composite_repo_identity() {
        let console = include_str!("../assets/ui/wb-console.js");
        for pin in [
            r#"connect({ id: currentSessionId, repo: currentRepo"#,
            r#"c.verb === "session-open""#,
            "WBSessionRoute.url(",
            "WBSessionRoute.announcement(",
            "WBSessionRoute.closeUrl(",
        ] {
            assert!(console.contains(pin), "wb-console.js must keep {pin}");
        }
        assert!(include_str!("../assets/ui/app.js").contains("WBSessionRoute.matchesRepo("));
        let route = include_str!("../assets/ui/wb-session-route.js");
        for pin in [
            "function url(",
            "function closeUrl(",
            "function closeSucceeded(",
            "function announcement(",
            "function matchesRepo(",
        ] {
            assert!(route.contains(pin), "wb-session-route.js must keep {pin}");
        }
        // The console's vendor session name is END-TO-END or it is nothing: the
        // daemon announces it, the route folds it, the titlebar shows it. This
        // shell half has no Node coverage (`sessionPresentation` is exported onto
        // `window`, not `module`), so the pins are the guard — a refactor that
        // drops either one leaves a console whose address the operator cannot
        // read anywhere.
        assert!(
            route.contains("name: payload?.name"),
            "wb-session-route.js must fold the announced session name"
        );
        for pin in ["owner?.name", "tooltip: [repo || \"\", name]"] {
            assert!(
                console.contains(pin),
                "wb-console.js must surface the session name ({pin})"
            );
        }

        for html in [
            include_str!("../assets/ui/index.html"),
            include_str!("../assets/ui/detached-fence.html"),
        ] {
            let route_tag = html
                .find(r#"<script src="wb-session-route.js"></script>"#)
                .unwrap();
            let console_tag = html
                .find(r#"<script src="wb-console.js"></script>"#)
                .unwrap();
            assert!(
                route_tag < console_tag,
                "session routes must load before consoles"
            );
        }
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("wb_session_owner_351.js");
        let output = std::process::Command::new("node")
            .arg("--test")
            .arg(script)
            .output()
            .expect("Node.js must execute workbench session ownership coverage");
        assert!(
            output.status.success(),
            "workbench session ownership coverage failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// The stage/viewport shell (#336). A clamp lives in CSS and markup, which
    /// the `node --test` suite never renders and Playwright — which does — does
    /// not run in CI. So this is the only CI-visible gate that the deletion
    /// stays deleted: a re-added clamp would pass every unit test in the tree.
    #[test]
    fn shell_has_no_clamp_and_carries_the_stage() {
        let js = include_str!("../assets/ui/wb-console.js");
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
        // #336's claim is that the extent IS a pure function. It now lives in
        // the module that holds only pure functions, which is the same claim
        // made structurally rather than by assertion (ADR-0057).
        assert!(
            include_str!("../assets/ui/wb-geometry.js").contains("function stageExtent("),
            "the stage extent is a pure function, in wb-geometry.js (#336)"
        );

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

    /// The navigation layer over that plane (#337). Same reason as above: the
    /// node table and the Playwright pass both run out of CI, so this is the
    /// only gate that a gesture deleted here is a red test rather than a
    /// silently unreachable window.
    #[test]
    fn shell_navigates_the_plane() {
        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "function bringIntoView(",
            "function panNudge(",
            "function reveal(",
            "function onFloorDown(",
            "function onWheel(",
            // The REGISTRATION, not the phrase: pinning a bare `passive: false`
            // is satisfied by the comment that explains it, so the option could
            // be deleted with this gate still green.
            r#"addEventListener("wheel", onWheel, { passive: false })"#,
            // the auto-pan loop's teardown — an uncancelled rAF pans forever
            // after the button is released
            "cancelAnimationFrame",
            // …and its two lost-mouseup recoveries, which are the only reason
            // that teardown is reachable when the release never arrives
            "ev.buttons === 0",
            r#"window.addEventListener("blur", onUp)"#,
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #337 pin {pin}"
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

        // The picker's wiring lives in app.js; without this the `@click`
        // handlers in index.html can go dangling with every test still green.
        let app = include_str!("../assets/ui/app.js");
        for pin in ["toggleWindowMenu(", "revealWindow(", "windowList"] {
            assert!(app.contains(pin), "app.js must keep the #337 pin {pin}");
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

        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "function syncMaxPin(",
            // The REGISTRATION, not the function: without it the pin is only
            // re-derived on a maximize and a programmatic pan desyncs it.
            r#"ws.addEventListener("scroll", syncMaxPin)"#,
            "workbench:stage-extent",
            // The NEGATIVE control: the scroll freeze must SURVIVE. A blanket
            // deletion of the maximize machinery would satisfy every "no longer
            // contains" pin below and must be red, not green.
            "function syncMaxLock(",
            // The POSITIVE half of the `reveal()` change: the negative pin below
            // is one spelling and a requote would slip past it, and scenario 4
            // (the only behavioural gate) does not run in CI.
            r#"if (it.classList.contains("maximized")) return it;"#,
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #338 pin {pin}"
            );
        }
        assert!(
            !js.contains(r#"if (ws.classList.contains("maxlock")) return it;"#),
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
    /// None of this is reachable by the node table or Playwright in CI, so it
    /// fails here or nowhere.
    #[test]
    fn a_console_can_take_the_whole_screen() {
        let js = include_str!("../assets/ui/wb-console.js");
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
            // The two guards that keep the inline rect honest while the top
            // layer owns the geometry.
            r#"if (win.classList.contains("maximized") || isFull(win)) return;"#,
            r#"if (!win.classList.contains("maximized") && !isFull(win)) {"#,
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the fullscreen pin {pin}"
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
        // The two modules that own the state being persisted must reach it only
        // through `WBView` — a direct write from either is how a second store
        // starts.
        for (name, src) in [
            ("wb-console.js", include_str!("../assets/ui/wb-console.js")),
            ("app.js", include_str!("../assets/ui/app.js")),
        ] {
            assert!(
                !src.contains("localStorage"),
                "{name} must not touch localStorage — wb-view.js owns the store (#339)"
            );
        }

        let view = include_str!("../assets/ui/wb-view.js");
        // NEGATIVE CONTROL: deleting the store wholesale would satisfy every
        // "does not contain" assertion above. It must be red, not green.
        assert!(
            view.contains("localStorage"),
            "wb-view.js IS the browser store — deleting it is not how #339 stays green"
        );
        assert!(
            view.contains(r#"const KEY = "wb.view.v1""#),
            "wb-view.js must keep the one view key (#339)"
        );

        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "function viewLanding(",
            "function applyLanding(",
            // The REGISTRATION, not the function: without it the offset is never
            // persisted and the landing has nothing to restore.
            r#"ws.addEventListener("scroll", saveOffset)"#,
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #339 pin {pin}"
            );
        }

        // Tree-wide, not just the two modules above: any non-vendor asset that
        // starts naming `localStorage` is a second store by definition. `.html`
        // is swept too — both shells carry inline `<script>` blocks, so a store
        // could grow there without touching a single `.js`.
        for path in embedded_ui_paths() {
            let scanned = path.ends_with(".js") || path.ends_with(".html");
            if !scanned || path.starts_with("vendor/") || path == "wb-view.js" {
                continue;
            }
            let src = UI
                .get_file(&path)
                .and_then(|f| f.contents_utf8())
                .unwrap_or_else(|| panic!("{path} must be embedded as UTF-8"));
            assert!(
                !src.contains("localStorage"),
                "{path} must not touch localStorage — wb-view.js is the only store (#339)"
            );
        }

        // The store must be defined BEFORE its readers run: `wb-console.js`
        // reads `WBView` on its boot path, and a later tag would leave the
        // landing reading `undefined` on the very first paint.
        let html = include_str!("../assets/ui/index.html");
        let view_tag = html
            .find(r#"<script src="wb-view.js"></script>"#)
            .expect("index.html must load wb-view.js (#339)");
        let console_tag = html
            .find(r#"<script src="wb-console.js"></script>"#)
            .expect("index.html must load wb-console.js (#339)");
        assert!(
            view_tag < console_tag,
            "wb-view.js must be script-tagged BEFORE wb-console.js (#339)"
        );
    }

    /// Silence is not death. A browser throttles a hidden tab's timers to one
    /// tick per minute after about five minutes, so the six-second heartbeat
    /// window closed a working popup and pulled its consoles home mid-work. Both
    /// documents now demand a better witness before acting — a window handle,
    /// or a probe that message delivery (which is not timer-throttled) still
    /// answers. `wb_fence_347.py` scenario 5c drives it; this is CI's view.
    #[test]
    fn a_quiet_detach_peer_is_challenged_before_it_is_buried() {
        let js = include_str!("../assets/ui/wb-console.js");
        assert!(
            js.contains("function stillThere(id, entry) {"),
            "the origin must ask whether a quiet popup is really gone"
        );
        assert!(
            js.contains("&& !stillThere(id, entry)"),
            "the re-attach must be gated on that answer, not on the fold alone"
        );
        assert!(
            js.contains(r#"m.type === "popup-ping""#) && js.contains(r#"type: "origin-here""#),
            "the origin must answer a probe from its MESSAGE handler, not a timer"
        );
        let popup = include_str!("../assets/ui/detached-fence.html");
        assert!(
            popup.contains("!window.opener || window.opener.closed"),
            "the popup's verdict is the opener HANDLE, which owes nothing to a timer"
        );
        assert!(
            popup.contains("if (out.effects.some((x) => x.type === \"peer-lost\")) silent();"),
            "a lost peer must reach `silent`, which probes, and never `lost` directly"
        );
    }

    /// Three pieces of chrome that only a browser can really prove, pinned here
    /// because the browser pass — Playwright — does not run in CI.
    #[test]
    fn the_console_chrome_holds_its_three_rules() {
        let app = include_str!("../assets/ui/app.js");
        let html = include_str!("../assets/ui/index.html");
        // ONE dropdown at a time. Every toggler goes through `closeMenus`, which
        // enumerates the four in ONE place — the account menu and the toolbar's
        // pickers used to enumerate each other and left both open, overlapping.
        assert!(
            app.contains("closeMenus() {") && app.contains("this.avatarMenu = false;"),
            "app.js must close every menu from one place"
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
        let js = include_str!("../assets/ui/wb-console.js");
        assert!(
            js.contains("name.readOnly = true;")
                && js.contains(r#"name.addEventListener("dblclick""#),
            "the fence name must open on a double click and close on blur"
        );
    }

    /// The three clicks that ask first. Tiling a fence moves every console in
    /// it, removing a fence takes the region out from under them, and a
    /// console's × ends a live session — all one pixel from something harmless
    /// on the same title bar. The dialog is built in `wb-console.js` rather
    /// than borrowed from the shell's Alpine one, because the module also runs
    /// in the detached-fence popup, which has neither; `window.confirm` is
    /// pinned OUT because an automated browser dismisses it by default, which
    /// would turn every guarded click into a silently cancelled one.
    #[test]
    fn the_destructive_console_clicks_confirm_first() {
        let js = include_str!("../assets/ui/wb-console.js");
        assert!(
            js.contains("function askConfirm({"),
            "wb-console.js must own a confirmation dialog of its own"
        );
        assert!(
            !js.contains("window.confirm("),
            "the native confirm is not the seam — an automated browser dismisses it"
        );
        // The three call sites, each awaiting the answer before acting.
        for pin in [
            r#"title: "Tile this fence?""#,
            r#"title: "Remove this fence?""#,
            r#"title: "Close this console?""#,
        ] {
            assert!(js.contains(pin), "wb-console.js must keep the pin {pin}");
        }
        // The EXPORTED verbs stay unguarded: a caller that names `arrangeFence`
        // has already decided, and the dialog belongs to the accidental click.
        let verb = js
            .split_once("\n  function removeFence(id) {")
            .expect("wb-console.js must keep removeFence")
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
        let js = include_str!("../assets/ui/wb-console.js");
        assert!(
            js.contains(
                "record.kind === \"console\" || relaunchAgents ? \"relaunch\" : \"placeholder\""
            ),
            "the restore fold must relaunch an agent console ONLY under the opt-in"
        );
        // The DEFAULT is the whole guard: a caller that omits the option — the
        // fold's own tests, a later call site — must get the parked placeholder.
        assert!(
            js.contains("relaunchAgents = false }"),
            "an omitted `relaunchAgents` must default to false, never to launching"
        );
        // The relaunch verdict must ask for the record's OWN kind. `{ console:
        // true }` is the shell request, and reaching an agent record with it —
        // which the opt-in made possible — opened a plain shell in the agent's
        // box (measured in `wb_desk_303.py` scenario 10 before this line).
        // Since #411 the request is `relaunchRequest`'s, which also carries
        // the worktree the record was in; the fold is pinned by
        // `ui-tests/wb-console.test.mjs`, and what stays here is that the
        // restore path goes through it and nothing else.
        assert!(
            js.contains(
                r#"if (record.kind !== "agent") return { console: true, repo, command: consoleCommand(record.agent) };"#
            ),
            "a relaunched agent console must be requested by its vendor, not as a shell"
        );
        assert!(
            js.contains(
                "spawnOrMissing(relaunchRequest(record), record.agent, record.repo, record)"
            ),
            "the restore fold's relaunch must go through relaunchRequest"
        );
        // The popup holds a fragment of the plane and authors no session; its
        // injected `viewStore` reads nothing, and `canLaunch` refuses besides.
        assert!(
            js.contains("OPTS.canLaunch !== false && viewStore?.read()?.relaunch === true"),
            "the opt-in must be read through the injected view store, and never in the popup"
        );

        // The knob, and the store it writes to. `config.set` would put a
        // per-browser choice in a repo's settings.json for every client to obey.
        let settings = include_str!("../assets/ui/wb-settings.js");
        for pin in [
            r#"scope: "client""#,
            r#"key: "consoles.relaunch_on_load""#,
            r#"type: "toggle""#,
        ] {
            assert!(
                settings.contains(pin),
                "wb-settings.js must keep the pin {pin}"
            );
        }
        let app = include_str!("../assets/ui/app.js");
        assert!(
            app.contains("window.WBView.patch({ relaunch: value === true })"),
            "app.js must persist the toggle through the view store"
        );
        assert!(
            app.contains("if (this.CLIENT_KEYS.has(key)) {"),
            "a client-scoped key must return before the config.set path"
        );
        let html = include_str!("../assets/ui/index.html");
        assert!(
            html.contains(r#"it.type === 'toggle'"#),
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
        let schema = include_str!("../assets/ui/wb-settings.js");
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
        assert!(
            include_str!("../assets/ui/index.html").contains(r#":disabled="it.readonly === true""#),
            "index.html must disable the control a readonly item declares"
        );
        // Non-vacuous: a scan that stopped recognizing the schema's shape would
        // otherwise pass by checking nothing at all.
        assert!(
            checked > 10,
            "only {checked} project keys were cross-checked — the scan stopped seeing the schema"
        );
    }

    /// The plan viewer's prose is keyed to the issue the plan says it is for.
    /// Same CI bargain as the pins below: `node --test` covers the helpers and
    /// CI runs it, but the rendering is Playwright's, and that does not run.
    ///
    /// The defect this guards: the steps come from the run snapshot and are keyed
    /// by issue (ADR-0047 A1), but the prose is a `file.read` of `.ralphy/plan.md`
    /// — which holds the PREVIOUS issue's plan for the whole planning phase of the
    /// next one. Without the key the block renders that plan as the current one.
    #[test]
    fn the_plan_prose_is_keyed_to_the_issue_the_plan_names() {
        let runs_js = include_str!("../assets/ui/wb-runs.js");
        // The literal, cross-checked against its PRODUCER: the planner writes
        // `plan_trailer` (crates/ralphy-adapter-support/src/resume.rs). The daemon
        // does not depend on that crate (leaf-crate rule, ADR-0032 §10), so the
        // shared shape is pinned by literal here and named there.
        assert!(
            runs_js.contains("ralphy-plan:") && runs_js.contains("issue="),
            "wb-runs.js must read the plan trailer written by resume.rs `plan_trailer`"
        );
        for pin in ["planTrailerIssue(", "planBelongsTo("] {
            assert!(
                runs_js.contains(pin),
                "wb-runs.js must keep the helper {pin}"
            );
        }
        let app_js = include_str!("../assets/ui/app.js");
        let squeezed: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
        // Both readers of the prose go through the SAME gate — a picker that
        // offered a stale plan's headings would be the identical lie one level up.
        assert!(
            squeezed.contains("planHeadings(run) { if (!this.planProseIsCurrent(run)) return [];"),
            "planHeadings must withhold a stale plan's sections"
        );
        assert!(
            squeezed.contains("if (!run || !name || !this.planProseIsCurrent(run)) return \"\";"),
            "renderPlanSection must refuse prose that belongs to another issue"
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
        let runs_js = include_str!("../assets/ui/wb-runs.js");
        let squeezed: String = runs_js.split_whitespace().collect::<Vec<_>>().join(" ");
        // The mapper must CARRY the render facts. Bare-noun pins would pass on the
        // helpers alone while the mapper kept throwing the values away.
        for pin in [
            "model: i.model ?? null,",
            "effort: i.effort ?? null,",
            "budgetMin: i.budget_min ?? null,",
            "since: doc.phase?.since || \"\",",
        ] {
            assert!(
                squeezed.contains(pin),
                "wb-runs.js's fromSnapshot must carry {pin}"
            );
        }
        // One vocabulary with the console: `model / effort` (ui::render's
        // `model_effort_seg`) and `M:SS` (`fmt_clock`). A second spelling of the
        // same fact on the same run is the drift this pin exists to catch.
        assert!(
            squeezed.contains("return e ? `${m} / ${e}` : m;"),
            "modelEffort must mirror model_effort_seg's separator"
        );
        assert!(
            squeezed.contains(
                "return `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, \"0\")}`;"
            ),
            "fmtClock must mirror fmt_clock's M:SS"
        );
        // The three honesty rules of the clock, each a way it could lie instead.
        assert!(
            squeezed.contains("if (!run?.since) return \"\";"),
            "no anchor means NO clock — never a fabricated 0:00"
        );
        assert!(
            squeezed.contains("const elapsed = this.fmtClock(Math.max(0, (nowMs || 0) - since));"),
            "host/browser clock skew must clamp at zero, never render negative"
        );
        assert!(
            squeezed.contains("return budget > 0 ?"),
            "a budget of 0 is a DISABLED cap — no `/ 0:00` ceiling (mirrors render_active_line)"
        );
        // The fallback is the whole reason the title may name a vendor at all.
        assert!(
            squeezed.contains(
                "return this.modelEffort(this.activeIssue(run)?.model, this.activeIssue(run)?.effort) || run.agent || \"\";"
            ),
            "runTitle must degrade to the agent when the model is not known yet"
        );

        let shell = include_str!("../assets/ui/index.html");
        for pin in [
            r#"<span class="run-select-title" x-text="runTitle(currentRun())"></span>"#,
            r#"<span class="run-sub-phase" x-text="runIdentity(currentRun())"></span>"#,
            r#"<span class="run-clock" x-show="runClock(currentRun())""#,
        ] {
            assert!(
                shell.contains(pin),
                "index.html must keep the picker pin {pin}"
            );
        }
        // The vendor MOVED, it was not deleted: `runIdentity` is what carries it,
        // and the negative pin is what stops the title reverting to it.
        assert!(
            !shell.contains(r#"x-text="currentRun()?.agent""#),
            "the picker's headline must be the model, not the vendor"
        );

        let app_js = include_str!("../assets/ui/app.js");
        let app: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
        // The tick is what makes the clock live, and reading `nowMs` in the getter
        // is what subscribes the binding to it — a `Date.now()` inside `runClock`
        // would leave the clock frozen with every pin above still green.
        assert!(
            app.contains("return window.WBRun.phaseClock(run, this.nowMs);"),
            "runClock must read the reactive `nowMs`, not the clock directly"
        );
        // The GUARD and the period, not the whole interval body: the tick is a
        // shared seam (presence staleness rides it too), and quoting every
        // statement in it made this run-panel test fail for a change that had
        // nothing to do with the run panel.
        assert!(
            app.contains("this._clockTick = setInterval(() => {"),
            "the shell must own a one-second tick for the phase clock"
        );
        assert!(
            app.contains("if (this.runsOpen) this.nowMs = Date.now();"),
            "the phase clock must advance only while the panel is open"
        );
        assert!(
            app.contains("}, 1000);"),
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
            squeezed.contains(
                "* { scrollbar-width: thin; scrollbar-color: var(--border) transparent; }"
            ),
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

        let app_js = include_str!("../assets/ui/app.js");
        let app: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
        // ONE predicate, two subjects — the drift #318 avoided. A second
        // "does this repo have a live run" test is how the gate and the controls
        // beside it start disagreeing.
        assert!(
            app.contains(
                "return window.WBChanges.writeLockReason( this.runsByProject[this.openSlug], \"Labels are read-only while a run is active.\", );"
            ),
            "the label reason must reuse writeLockReason, not parallel it"
        );
        assert!(
            app.contains("if (this.labelsLocked()) return;"),
            "toggleLabel must refuse behind the disabled rows too"
        );
        let changes_js = include_str!("../assets/ui/wb-changes.js");
        assert!(
            changes_js.contains(r#"return `A run is active in this repo. ${tail}`;"#),
            "writeLockReason must compose one sentence around a named subject"
        );
    }

    /// Stopping a run — the one control in the Runs panel that throws away work
    /// in progress (ADR-0054, ADR-0032 §6) — asks through the shell's own dialog.
    /// `window.confirm` names the origin, ignores the theme and blocks the page,
    /// which is the wrong furniture for the panel's most consequential click.
    #[test]
    fn stopping_a_run_confirms_through_the_design_system_dialog() {
        let app_js = include_str!("../assets/ui/app.js");
        let squeezed: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            squeezed.contains(r#"const ok = await this.askConfirm({ title: "Stop this run?","#),
            "stopRun must confirm through askConfirm, not window.confirm"
        );
        // The remaining native calls are the DOCUMENTED fallback for an
        // unreachable shell (`getShell()` returning null), so they are counted
        // rather than forbidden: the count is what reds if a new one appears.
        assert_eq!(
            app_js.matches("window.confirm(").count(),
            1,
            "the only window.confirm left is the shell-unreachable fallback"
        );
    }

    /// The board can see, read and throw away the plan that the NEXT RUN will
    /// execute. Same CI bargain as the pins around it; the structural half of
    /// the slice, since Playwright — which renders it — does not run in CI.
    #[test]
    fn the_board_surfaces_the_plan_the_next_run_would_execute() {
        let runs_js = include_str!("../assets/ui/wb-runs.js");
        for pin in [
            "planSummary(",
            "planPillLabel(",
            "planPillWarns(",
            "isBundleReason(",
        ] {
            assert!(runs_js.contains(pin), "wb-runs.js must keep {pin}");
        }
        // The verdict must be the RUNNER's test — zero open steps
        // (ralphy-core `plan::count_open_steps`, read by runner/phases.rs) — and
        // never the `## Feasible:` heading's claim, which is the human's reason.
        // A heading-driven verdict would call a plan with nothing to do "ready".
        let squeezed: String = runs_js.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            squeezed.contains("infeasible: openSteps === 0,"),
            "infeasible must mean zero OPEN STEPS, mirroring count_open_steps"
        );

        let app_js = include_str!("../assets/ui/app.js");
        for pin in [
            r#"path: ".ralphy/plan.md","#,
            r#"window.WBDaemon.write("plan.discard", { repo: slug })"#,
        ] {
            assert!(app_js.contains(pin), "app.js must keep {pin}");
        }
        // The discard is confirmed, and the plan is only ever shown against the
        // issue its trailer names (`planFor`) — a plan offered on the wrong card
        // would invite a discard of the wrong work.
        let app_squeezed: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            app_squeezed
                .contains(r#"const ok = await this.askConfirm({ title: "Discard this plan?","#),
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
            html[discard..discard + 220].contains(":disabled=\"writeLocked()\""),
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
            html.contains(r#"class="plan-picker-caret" data-lucide="chevron-down""#),
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

        let app_js = include_str!("../assets/ui/app.js");
        // `rawFeedOpen: false` is the DEFAULT, not an incidental initialiser: the
        // feed can take 30vh of a panel whose job is the trail and the plan, so
        // the bytes are opt-in and only the head arrives with the output.
        for pin in ["dismissFeed()", "runVerbFailed(", "rawFeedOpen: false"] {
            assert!(app_js.contains(pin), "app.js must keep the #331 pin {pin}");
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

        let runs_js = include_str!("../assets/ui/wb-runs.js");
        for pin in ["verbLockTitle(", "exitNote("] {
            assert!(
                runs_js.contains(pin),
                "wb-runs.js must keep the #331 helper {pin}"
            );
        }
        assert!(
            include_str!("../assets/ui/wb-daemon.js").contains("runVerbFailed?.("),
            "wb-daemon.js must route a terminal verb frame to the panel (#331)"
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

        let app_js = include_str!("../assets/ui/app.js");
        // Every act in the panel routes its refusal here. Counted, not merely
        // present: a single surviving `_flashAction` on one of these paths is
        // one act that stays silent, and that is the whole bug.
        assert_eq!(
            app_js.matches("_changesRefused(").count(),
            15,
            "the 14 refusal sites in the Changes panel, plus the helper itself"
        );
        for pin in [
            "changesError: \"\"",
            "_changesRefused(msg) {",
            "this.changesError = msg || \"\";",
        ] {
            assert!(app_js.contains(pin), "app.js must keep the pin {pin}");
        }
        // The helper still flashes: with the Runs panel open, an answer that used
        // to appear there must not disappear because it gained a second home.
        let squeezed: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            squeezed.contains(
                "_changesRefused(msg) { this.changesError = msg || \"\"; this._flashAction(msg); }"
            ),
            "the panel note is added to the flash, never substituted for it"
        );

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
            r#"data-act="fetch" :disabled="!!syncBusy""#,
            r#"data-act="pull" :disabled="!!syncBusy""#,
            r#"data-act="push" :disabled="writeLocked() || !!syncBusy""#,
            r#":class="{ busy: syncBusy === 'fetch' }""#,
            r#":class="{ busy: syncBusy === 'pull' }""#,
            r#":class="{ busy: syncBusy === 'push' }""#,
        ] {
            assert!(html.contains(pin), "index.html must keep the pin {pin}");
        }
        // One ring per act, a sibling of the icon: Lucide replaces the `<i>`
        // after Alpine binds, so the ring cannot be a directive on the icon.
        assert_eq!(
            html.matches(r#"<span class="bar-spinner" aria-hidden="true"></span>"#)
                .count(),
            3,
            "each of the three remote acts carries its own ring"
        );

        let app_js = include_str!("../assets/ui/app.js");
        assert!(
            app_js.contains("syncBusy: null,"),
            "the slot is declared idle"
        );
        // Every act takes the slot on entry and releases it in `finally`: a
        // refusal or a transport throw must not leave the bar locked forever.
        for verb in ["fetch", "pull", "push"] {
            let take = format!("this.syncBusy = \"{verb}\";");
            assert!(
                app_js.contains(&take),
                "app.js must take the slot for {verb}"
            );
        }
        assert_eq!(
            app_js.matches("if (this.syncBusy) return;").count(),
            3,
            "each remote act refuses to start while another is out"
        );
        let squeezed: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(
            squeezed
                .matches("} finally { this.syncBusy = null; }")
                .count(),
            3,
            "each remote act releases the slot on every exit path"
        );

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
            r#"x-show="rowOpen(p) && branchError""#,
            r#"x-text="branchError""#,
        ] {
            assert!(html.contains(pin), "index.html must keep the pin {pin}");
        }
        // Under the bar that carries the chip, above the tree: the answer sits
        // where the question was asked, not below rows that can run off screen.
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

        let app_js = include_str!("../assets/ui/app.js");
        for pin in [
            "branchError: \"\"",
            "_branchRefused(msg) {",
            "this.branchError = msg || \"\";",
        ] {
            assert!(app_js.contains(pin), "app.js must keep the pin {pin}");
        }
        // Both `_mutateBranch` arms report: the refusal AND, in daemon mode, the
        // transport throw. The throw is the arm that used to be deliberately
        // silent, and it is the one that leaves the optimistic chip standing —
        // silence there is the chip claiming a switch nobody confirmed.
        // `createWorktree` (#405) and `removeWorktree` (#409) report through
        // the same helper, with the same two arms each. There is NO
        // client-side refusal under a selected worktree any more (#407): the
        // act is SENT with the checkout.
        assert_eq!(
            app_js.matches("_branchRefused(").count(),
            3,
            "the refusal arm and the daemon-mode throw arm of `_mutateBranch`, and the helper itself (a worktree CREATE reports into the console's own prompt, a REMOVE into a one-button notice — ADR-0063 amendment 2026-09-16 b)"
        );
        assert!(
            app_js.contains("WBDaemon.withCheckout({ repo: slug, name }, this.checkoutOf(slug))"),
            "a branch act under a selected worktree is sent WITH the checkout — the worktree's HEAD moves, never the primary's"
        );
        assert!(
            !app_js.contains("pick primary before switching branches"),
            "the #406 client-side refusal is gone"
        );
        assert!(
            app_js.contains(
                r#"_branchRefused("Could not reach the daemon. Check whether the branch changed.")"#
            ),
            "an unanswered branch change must not read as a completed one"
        );
        // The create lives in the console's prompt (wb-console.js): an
        // unanswered add re-opens it with the same honest line.
        assert!(
            include_str!("../assets/ui/wb-console.js").contains(
                r#"error = "Could not reach the daemon. Check whether the worktree was created.";"#
            ),
            "an unanswered worktree create must not read as a completed one"
        );
        assert!(
            app_js.contains(
                r#"refused("Could not reach the daemon. Check whether the worktree was removed.")"#
            ),
            "an unanswered worktree remove must not read as a completed one"
        );
        // The revert is on the REFUSAL arm only: a throw may have landed, and
        // reverting a switch that happened would put a lie in the chip.
        let squeezed: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            squeezed.contains(
                "revert(); this._branchRefused(window.WBFail.message(reply, \"branch change refused\"));"
            ),
            "only a refusal reverts the optimistic chip"
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
        // The rows are the checkout menu's (wb-console.js `checkoutMenu`),
        // opened from the Files bar's chip with `onRemove` (ADR-0063
        // amendment 2026-09-16 b): the trash is its own element whose click
        // stops before the row's own pick, and the primary row never has one.
        let console_js = include_str!("../assets/ui/wb-console.js");
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
                || include_str!("../assets/ui/app.js")
                    .contains("onRemove: (row) => this.removeWorktree(ref, row),"),
            "the Files chip's menu wires the remove action"
        );
        let app_js = include_str!("../assets/ui/app.js");
        assert!(
            app_js.contains(
                "window.WBProject.checkoutAfterListing(ck, this.worktreeListings[slug]) === null"
            ),
            "the selection resets from the re-read listing, never from the reply's status"
        );
        assert!(
            served_css().contains(".session-checkout-remove {"),
            "the remove action is styled in the served CSS"
        );
    }

    /// The runs chrome's own colour gate — plus the declarations that actually
    /// DO the bounding. The markup pins above prove the box exists; only these
    /// prove it is bounded, and `max-height` is a single line whose deletion
    /// restores the original defect with every other pin still green.
    #[test]
    fn the_runs_chrome_adds_no_colour_outside_the_token_set() {
        let css = served_css();
        let open = "/* #331 runs chrome */";
        let close = "/* #331 runs chrome end */";
        let start = css
            .find(open)
            .expect("styles.css must keep the #331 runs-chrome opening marker")
            + open.len();
        let end = css
            .find(close)
            .expect("styles.css must keep the #331 runs-chrome closing marker");
        let block = &css[start..end];
        // Every assertion below is a `contains`, so an emptied block would
        // satisfy only the negative one — check it has content first.
        assert!(
            !block.trim().is_empty(),
            "the #331 runs-chrome block must not be empty"
        );
        assert!(
            !block.contains('#'),
            "the #331 runs-chrome CSS must reference var(--…) tokens only, no hex literals"
        );
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

    /// The discard block's own colour + hover gate, reusing #318's scan. It also
    /// asserts the block still holds an `@media` rule: the touch de-emphasis IS
    /// the criterion, and a block that lost it would pass the rest vacuously.
    #[test]
    fn the_discard_controls_add_no_colour_outside_the_token_set() {
        let css = served_css();
        let open = "/* #319 discard */";
        let close = "/* #319 discard end */";
        let start = css
            .find(open)
            .expect("styles.css must keep the #319 discard opening marker")
            + open.len();
        let end = css
            .find(close)
            .expect("styles.css must keep the #319 discard closing marker");
        let block = &css[start..end];
        assert!(
            !block.contains('#'),
            "the #319 discard CSS must reference var(--…) tokens only, no hex literals"
        );
        assert!(
            block.contains("@media"),
            "the touch de-emphasis is the criterion — the block must keep its @media rule"
        );

        let declarations = strip_css_comments(block);
        let mut hover_rules = 0;
        for rule in declarations.split('}') {
            let Some((selector, body)) = rule.split_once('{') else {
                continue;
            };
            if !selector.contains(":hover") {
                continue;
            }
            hover_rules += 1;
            for banned in ["opacity", "visibility", "display", "max-height"] {
                assert!(
                    !body.contains(banned),
                    "a discard control must not be hover-gated on {banned}: {selector}"
                );
            }
        }
        assert!(
            hover_rules >= 2,
            "the discard block must still carry its :hover rules, found {hover_rules}"
        );
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

    /// The write controls' CSS must speak the shell's token language (ADR-0035)
    /// exactly as the rail view's does. Its own block, and its own marker pair:
    /// appending to #317's would silently widen a pin that names another issue.
    #[test]
    fn the_write_controls_add_no_colour_outside_the_token_set() {
        let css = served_css();
        let open = "/* #318 write controls */";
        let close = "/* #318 write controls end */";
        let start = css
            .find(open)
            .expect("styles.css must keep the #318 write-controls opening marker")
            + open.len();
        let end = css
            .find(close)
            .expect("styles.css must keep the #318 write-controls closing marker");
        let block = &css[start..end];
        assert!(
            !block.contains('#'),
            "the #318 write-control CSS must reference var(--…) tokens only, no hex literals"
        );
        // The touch criterion, pinned where CI can see it: a hover-gated
        // `opacity`/`visibility` is exactly the affordance a phone cannot find.
        // Split on `}` so each chunk is one rule — selector, then its body — and
        // judge the BODY of any rule whose selector mentions `:hover`. Comments
        // are stripped FIRST: one of them names `.branch-chip.disabled:hover` as
        // prior art, and a raw split would read that prose as a selector.
        let declarations = strip_css_comments(block);

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
                    "a write control must not be hover-gated on {banned}: {selector}"
                );
            }
        }
        // …and the scan must have had something to judge: a block that stopped
        // carrying `:hover` rules would satisfy the loop above vacuously.
        assert!(
            hover_rules >= 2,
            "the write-control block must still carry its :hover rules, found {hover_rules}"
        );
    }

    /// The rail view's CSS must speak the shell's token language (ADR-0035), not
    /// invent colours: a hex literal anywhere in the block is the failure this
    /// catches. `cargo test` is the only gate CI runs over these assets.
    #[test]
    fn the_changes_view_adds_no_colour_outside_the_token_set() {
        let css = served_css();
        let open = "/* #317 rail view */";
        let close = "/* #317 rail view end */";
        let start = css
            .find(open)
            .expect("styles.css must keep the #317 rail-view opening marker")
            + open.len();
        let end = css
            .find(close)
            .expect("styles.css must keep the #317 rail-view closing marker");
        let block = &css[start..end];
        assert!(
            !block.contains('#'),
            "the #317 rail-view CSS must reference var(--…) tokens only, no hex literals"
        );
    }

    /// The browser half of the run-completion nudge (#310) is exercised by
    /// `node --test`, which CI now runs, and by a Playwright pass, which it does
    /// not — and neither states the WIRING across the three assets. So the three
    /// symbols the push path hangs on are pinned from the Rust gate, the way
    /// #309 pinned the list's markup.
    #[test]
    fn the_run_completion_nudge_is_wired_through_the_ui_assets() {
        assert!(
            include_str!("../assets/ui/wb-changes.js").contains("function shouldReload("),
            "wb-changes.js must keep the shouldReload filter (#310)"
        );
        let daemon_js = include_str!("../assets/ui/wb-daemon.js");
        assert!(
            daemon_js.contains("function subscribeChanges("),
            "wb-daemon.js must keep the subscribeChanges socket (#310)"
        );
        assert!(
            daemon_js.contains("subscribeChanges,"),
            "wb-daemon.js must EXPORT subscribeChanges — app.js guards on it (#310)"
        );
        let app_js = include_str!("../assets/ui/app.js");
        for symbol in [
            "mountChangesSub()",
            "destroyChangesSub()",
            "shouldReload?.(",
        ] {
            assert!(
                app_js.contains(symbol),
                "app.js must keep {symbol} on the nudge path (#310)"
            );
        }
    }

    /// The wake affordance, pinned from CI. This is markup and a call site, not
    /// a module function, so the suite CI runs does not reach it and Playwright
    /// does not run there — these substrings are the only CI-visible gate over
    /// the one consumer `/api/fleet/nudge` has, and a route with no caller is a
    /// route that rots.
    #[test]
    fn the_peer_wake_is_wired_through_the_ui_assets() {
        assert!(
            include_str!("../assets/ui/wb-fleet.js").contains("function wakeable("),
            "wb-fleet.js must keep the pure wakeable predicate"
        );
        let app_js = include_str!("../assets/ui/app.js");
        for symbol in [
            "/api/fleet/nudge?daemon_id=",
            "async wakePeer(",
            "wakePeerFor(ref)",
            // Readiness, not the spawn: a caller that acted on `nudged` would be
            // back to reporting a peer as woken while it is still booting.
            "reply.ready",
        ] {
            assert!(
                app_js.contains(symbol),
                "app.js must keep {symbol} to wake a peer"
            );
        }
        let html = include_str!("../assets/ui/index.html");
        assert!(
            html.contains(r#"@click.stop="wakePeer(g.daemon)""#),
            "the group header must carry the wake control"
        );
        assert!(
            html.contains(r#"x-show="peerWakeable(g)""#),
            "the wake control must be withheld from states a nudge cannot fix"
        );
        // Alpine stringifies an object nested inside a `:class` ARRAY, so the
        // object form emits a literal `[object Object]` class and the state it
        // meant to set never applies. Measured in the browser, since no unit test
        // in this tree renders Alpine.
        assert!(
            html.contains(r#":class="[g.state, waking[g.daemon] ? 'waking' : '']""#),
            "the wake chip's :class must bind plain strings, never a nested object"
        );
        // `asleep` is the ordinary course of a day, not a fault. Without this the
        // blanket non-reachable rule paints it as an error on every visit.
        assert!(
            served_css().contains(":not(.asleep)"),
            "styles.css must exempt `asleep` from the danger colour"
        );
    }

    /// The tree's folder predicate must read Wunderbaum's `data` bag, never a
    /// bare `node.folder`. Wunderbaum copies source keys it does not itself
    /// define into `node.data`, so the `folder: true` the daemon-backed listing
    /// sets lands at `node.data.folder` and `node.folder` is always `undefined`
    /// — and `node.children` is `null` until a lazy folder expands. Reading
    /// either alone made EVERY collapsed folder answer "file", which silently
    /// took out five call sites at once: the context menu offered no create
    /// items, no subdirectory was ever added to the `/ws/tree` watch set,
    /// double-clicking a folder read it as bytes, `findFolderByRel` never
    /// resolved so subdirectory `tree.dirty` nudges were all dropped, and the
    /// reconcile lost descendant expansion. Only a browser sees that, and CI
    /// runs no browser — so the shape is pinned here.
    #[test]
    fn the_tree_folder_predicate_reads_wunderbaums_data_bag() {
        let js = include_str!("../assets/ui/app.js");
        let body = js
            .split_once("    isFolder(node) {")
            .expect("app.js no longer defines isFolder(node)")
            .1
            .split_once("\n    },")
            .expect("app.js's isFolder is never closed")
            .0;
        assert!(
            body.contains("node.data?.folder"),
            "isFolder must read node.data.folder (Wunderbaum's bag for unknown \
             source keys); found: {body:?}"
        );
        assert!(
            !body.contains("node.folder "),
            "isFolder must not read a bare node.folder — it is always undefined; \
             found: {body:?}"
        );
        assert!(
            body.contains("node.lazy"),
            "isFolder must accept a collapsed lazy folder, whose children are \
             still null; found: {body:?}"
        );
    }

    /// Creating must be reachable for every target the operator can point at:
    /// a folder, a file (meaning its parent), and the repo root. The root has
    /// no node — a right-click on empty tree space resolves to `null` — so the
    /// context handler must NOT bail on a missing node, or a top-level file is
    /// uncreatable. The Files header carries the same two actions, because
    /// right-clicking empty space is an affordance nothing on screen advertises.
    #[test]
    fn the_explorer_can_create_at_every_target_including_the_repo_root() {
        let js = include_str!("../assets/ui/app.js");
        assert!(
            js.contains("this.showMenu(ev.clientX, ev.clientY, node || null)"),
            "the tree's contextmenu handler must open the menu for a NULL node \
             (empty space = the repo root), not return early"
        );
        for symbol in [
            "emitCreate(node, kind) {",
            "createDir(node) {",
            "createHere(kind) {",
        ] {
            assert!(
                js.contains(symbol),
                "app.js must keep {symbol} — the create-target resolution"
            );
        }
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
    /// shell that cannot be reached.
    #[test]
    fn naming_a_new_entry_uses_the_design_system_prompt() {
        let js = include_str!("../assets/ui/app.js");
        for symbol in [
            "askPrompt(opts = {}) {",
            "promptSubmit() {",
            "promptRespond(name) {",
        ] {
            assert!(js.contains(symbol), "app.js must keep {symbol}");
        }
        assert!(
            js.contains("await c.askPrompt({"),
            "the create path must ask for the name through askPrompt"
        );
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

    /// The body of one Alpine method in `app.js`, sliced from its opener to the
    /// first four-space-indented `},` — the file's method terminator. Whole-file
    /// `contains` is useless for these pins: `createIcons()` alone appears at
    /// twenty-two sites, so a check that does not scope to the method it is
    /// about passes no matter which one regressed.
    fn js_method_body<'a>(js: &'a str, opener: &str) -> &'a str {
        js.split_once(opener)
            .unwrap_or_else(|| panic!("app.js must define `{opener}`"))
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

    /// The one unconditional `createIcons()` runs at `alpine:initialized`, which
    /// is BEFORE either of these reads resolves — so the rows and the panel body
    /// each contain `data-lucide` placeholders that global scan already passed
    /// over, and they stayed blank until an unrelated handler happened to
    /// re-scan the document (#332).
    ///
    /// The project list is bound at the LIST, not at its loader: filtering
    /// rebuilds the `x-for` and blanks every icon again, which a loader-side fix
    /// does not reach. The Runs panel has no such second route, so it converts
    /// from its own read.
    #[test]
    fn the_icons_are_converted_wherever_the_rows_are_built() {
        let html = include_str!("../assets/ui/index.html");
        let list = html
            .split_once("class=\"projects\"")
            .expect("index.html must carry the projects list")
            .1
            // To the first child, NOT to the first `>`: the effect contains an
            // arrow function, whose `=>` would cut the slice in half.
            .split_once("<template")
            .expect("the projects list must hold a row template")
            .0;
        assert!(
            list.contains("x-effect") && list.contains("createIcons()"),
            "`ul.projects` must convert its icons from an effect over its own \
             contents — a loader-side fix leaves the list blank after one \
             keystroke in the search box; found: {list:?}"
        );

        let js = include_str!("../assets/ui/app.js");
        let runs = js_method_body(js, "async hydrateRuns() {");
        assert!(
            runs.contains("createIcons()"),
            "`hydrateRuns` renders the panel body behind an x-if on the data it \
             fetches, so it must convert them itself; found: {runs:?}"
        );
    }

    /// A repo with no `origin` is keyed `path-<hash>` (ADR-0008 D7). That stays
    /// the identity; only the LABEL becomes the directory basename (#332).
    #[test]
    fn a_remoteless_project_is_labelled_by_its_directory() {
        let js = include_str!("../assets/ui/app.js");
        let load = js_method_body(js, "async loadRepos() {");
        assert!(
            load.contains("path: x.path"),
            "`loadRepos` must keep `/api/repos`'s path — the label reads it; \
             found: {load:?}"
        );

        // The fold moved to `wb-project.js` (ADR-0057) — it is a pure function
        // of a project record, and #332's whole point is that the label is
        // DERIVED rather than stored. The four needles below are what derives
        // it, so they follow the code; the `loadRepos` and `filteredProjects`
        // halves stay above and below, because those read component state.
        let project = include_str!("../assets/ui/wb-project.js");
        let label = project
            .split_once("function repoLabel(p) {")
            .expect("wb-project.js must define repoLabel")
            .1
            .split_once("\n  }")
            .expect("repoLabel must close at module indent")
            .0;
        for (needle, why) in [
            (
                r#"startsWith("path-")"#,
                "only a remoteless slug is relabelled",
            ),
            (
                r#"includes("/")"#,
                "a real GitHub repo named `owner/path-utils` must NOT be \
                 relabelled off disk",
            ),
            (
                r"split(/[\\/]/)",
                "both separators — this ships on Windows and Linux",
            ),
            (
                r"replace(/[\\/]+$/",
                "trailing separators go first, or `C:\\src\\widget\\` basenames \
                 to the empty string and the row loses its name",
            ),
        ] {
            assert!(
                label.contains(needle),
                "`repoLabel` must contain {needle:?} — {why}; found: {label:?}"
            );
        }

        let filter = js_method_body(js, "filteredProjects() {");
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
            html.contains(r#":title="p.slug""#),
            "the label is a view concern; `.project-slug`'s own title must stay \
             the canonical ADR-0008 D7 slug (the browser tests locate a row by it)"
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
                "the name is the row's ONE elastic child — it is what anchors \
                 .chg-badge now the branch chip's `margin-left: auto` has gone",
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

    /// The chip MOVED out of the row and into the Files bar (#332). Neither
    /// block nests a `<div>`, so slicing each to its first `</div>` is exact.
    #[test]
    fn the_branch_chip_lives_in_the_files_bar_not_the_project_row() {
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
        assert!(
            !row.contains("branch-chip"),
            "the branch chip must not sit in the row — capped at 48% of a 300px \
             column it cost the project name half its width; found: {row:?}"
        );
        assert!(
            row.contains("rowTitle(p)"),
            "a collapsed row shows no branch while `filteredProjects` still \
             matches on branch, so it must name the branch in its title; \
             found: {row:?}"
        );

        let files = slice(r#"class="side-head files-sec""#);
        for needle in [
            r#"class="branch-chip""#,
            "openBranchModal(p)",
            "createHere('file')",
        ] {
            assert!(
                files.contains(needle),
                "the Files bar must carry {needle:?} — the chip keeps its \
                 switcher behaviour and the bar keeps its own actions; \
                 found: {files:?}"
            );
        }
        assert_eq!(
            html.matches(r#"class="branch-chip""#).count(),
            1,
            "the chip was MOVED, not copied — two would let the row and the bar \
             disagree about the branch"
        );

        // `.side-head` uppercases and letter-spaces its label; a branch name is
        // case-sensitive, so `feat/UI` would render as a ref that does not exist.
        let css = served_css();
        let chip = css_rule_body(&css, ".files-sec .branch-chip {");
        for decl in [
            "text-transform: none",
            "letter-spacing: normal",
            "max-width: none",
        ] {
            assert!(
                chip.contains(decl),
                ".files-sec .branch-chip must declare {decl}; found: {chip:?}"
            );
        }

        let js = include_str!("../assets/ui/app.js");
        assert!(
            js.contains("rowTitle(p) {"),
            "app.js must define the row's composite title"
        );
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
}
