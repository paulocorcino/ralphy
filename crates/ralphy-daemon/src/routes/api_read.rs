//! The read-only resources: repos, usage and spend, the desk, identity,
//! about, release, agents, and the embedded UI bytes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::extract::Query;
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::AgentLocator;
use super::{encode_query_value, read_peer_store};
use crate::{assets, StorePaths, UI};
use crate::{
    checkout, desk, dispatch, fleet, identity, peer, registry, rekey, release, roster, spend, usage,
};

/// Query for `GET /api/usage`: an optional `since` (RFC3339 UTC) lower bound.
/// Callers MUST URL-encode `+` as `%2B` — axum/`serde_urlencoded` decode a raw
/// `+` as a space, corrupting the `+00:00` offset.
#[derive(serde::Deserialize)]
pub(crate) struct UsageQuery {
    pub(crate) since: Option<String>,
    /// The open project's `owner/repo` slug. OPTIONAL, and its presence is what
    /// turns the fleet dump into the Spend tab's Ledger feed: rows outside the
    /// project leave, and each survivor gains its `unpriced_cause` verdict.
    /// Absent, the response is byte-identical to the pre-#360 one.
    pub(crate) project: Option<String>,
    /// The window, in `/api/spend`'s own vocabulary (`all` | `7d` | `30d` |
    /// `90d`). Read only alongside `project`, and derived through the SAME
    /// helper `spend_route` uses, so the Ledger grid and the Overview's figures
    /// are scoped to one window rather than two that happen to agree.
    pub(crate) period: Option<String>,
}

/// Query for `GET /api/spend`: the open project's `owner/repo` slug. REQUIRED —
/// the view is scoped to one project (PRD #355), so a missing slug is a caller
/// bug, and axum's `Query` extractor refuses it with a `400` before the handler
/// runs. The empty state for "no project open" is the client's, not this route's.
#[derive(serde::Deserialize)]
pub(crate) struct SpendQuery {
    pub(crate) project: String,
    /// The window: `all` (the default) | `7d` | `30d` | `90d`. An unrecognized
    /// value is a REFUSAL, never a silent fallback — see [`spend_route`].
    pub(crate) period: Option<String>,
}

#[derive(serde::Deserialize)]
pub(crate) struct AgentsQuery {
    pub(crate) repo: Option<String>,
}

/// `GET /api/repos`: the registered repos as JSON, each with its live
/// reachability. Read FRESH from disk on every request so a separate `ralphy
/// run` process's write shows up on the next page refresh. A load error yields
/// an empty list with `200` (logged) rather than failing the page. `branch` is
/// likewise read fresh from `<path>/.git/HEAD`, `None` when it cannot be
/// determined (detached HEAD, unreachable repo, worktree gitdir pointer).
///
/// The one place the registry self-heals (ADR-0036 amendment 2026-09-16): a
/// `path-<hash>` entry whose repo now HAS an origin is handed to `ralphy daemon
/// add` — synchronously, inside this request's blocking section — and the
/// registry is re-read, so the first page that could see the remote already
/// sees `owner/repo`. Once per `(slug, remote)` for this router's lifetime.
pub(crate) async fn repos_route(registry_path: PathBuf, memo: rekey::HealMemo) -> Response {
    #[derive(serde::Serialize)]
    struct RepoView {
        slug: String,
        path: String,
        reachable: bool,
        branch: Option<String>,
        // Additive (#204): the real working-tree state and origin URL. Both spawn
        // `git`, so the whole `Vec` is built inside `spawn_blocking` below.
        dirty: bool,
        remote: Option<String>,
        // Additive (#362): the canonical ABSOLUTE native root, for the explorer's
        // "Copy full path". `path` is left byte-identical — peers parse it
        // (`fleet::store_from_repos_json`).
        root: Option<String>,
    }
    let store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry; serving empty list");
            registry::RegistryStore::default()
        }
    };
    fn build_views(store: &registry::RegistryStore) -> Vec<RepoView> {
        store
            .repos
            .iter()
            .map(|(slug, entry)| RepoView {
                slug: slug.clone(),
                path: entry.path.clone(),
                reachable: entry.reachable(),
                branch: entry.head_branch(),
                dirty: entry.dirty(),
                remote: entry.remote(),
                root: entry.root(),
            })
            .collect()
    }
    // `dirty`/`remote` each spawn a `git` subprocess per repo — that must not
    // block the async reactor, so the whole map runs on a blocking thread.
    let views = tokio::task::spawn_blocking(move || {
        let views = build_views(&store);
        let todo = rekey::claim_candidates(
            &memo,
            views
                .iter()
                .map(|v| (v.slug.as_str(), v.path.as_str(), v.remote.as_deref())),
        );
        if todo.is_empty() {
            return views;
        }
        rekey::heal(&dispatch::ProcessSpawner, &dispatch::ralphy_exe(), &todo);
        match registry::load_from(&registry_path) {
            Ok(healed) => build_views(&healed),
            Err(e) => {
                tracing::warn!(error = %e, "repo registry unreadable after a re-key; serving the pre-heal list");
                views
            }
        }
    })
    .await
    .unwrap_or_default();
    Json(views).into_response()
}

/// `GET /api/usage[?since=<RFC3339 UTC, with `+` encoded as `%2B`>]`: the
/// token-usage ledger's run records PLUS the interactive records scanned from the
/// Claude and Codex stores, as `{ daemon_id, records: [...], interactive: [...] }` (ADR-0033
/// §2/§3). Both read FRESH from disk on every request, same as `/api/repos`.
/// `since` keeps run records whose `ts` is lexically `>=` it and interactive
/// records whose `last_ts` is `>=` it. The interactive scan excludes any session
/// the ledger already owns (its `session_id` in `records`) and writes nothing.
///
/// `project` narrows the folded reading to one project's rows and annotates each
/// survivor with its `unpriced_cause` — the Spend tab's Ledger grid (#360). The
/// verdict is computed HERE rather than in JavaScript because `no_price` (a real
/// model absent from `pricing.toml`) is undecidable without the price table, and
/// PRD #355 fixes that the client computes nothing numeric.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn usage_route(
    usage_dir: PathBuf,
    stores: StorePaths,
    registry_path: PathBuf,
    peers_dir: PathBuf,
    daemon_id: Option<String>,
    since: Option<String>,
    project: Option<String>,
    period: Option<String>,
) -> Response {
    // Refused, never silently widened: a Ledger grid showing all time under a
    // "last 7 days" label is the misread the closed vocabulary exists to
    // prevent — the same stance `spend_route` takes.
    let window = match project.as_deref() {
        None => spend::Window::All,
        Some(_) => match spend::Window::parse(period.as_deref().unwrap_or("all")) {
            Some(window) => window,
            None => return (StatusCode::BAD_REQUEST, "unknown period").into_response(),
        },
    };
    let local = local_usage_contribution(
        usage_dir.clone(),
        stores,
        registry_path,
        daemon_id,
        since.as_deref(),
    );
    let (descriptors, rejected) = read_peer_store(peers_dir).await;
    let path = since
        .as_deref()
        .map(|value| format!("/api/peer/usage?since={}", encode_query_value(value)))
        .unwrap_or_else(|| "/api/peer/usage".to_string());
    let requests = descriptors.iter().cloned().map(|descriptor| {
        let path = path.clone();
        async move {
            let result = match peer::client::get(&descriptor, &path).await {
                Ok((200, body)) => serde_json::from_slice::<usage::UsageContribution>(&body)
                    .map_err(|error| format!("invalid peer usage data: {error}")),
                Ok((status, _)) => Err(format!("peer usage request answered HTTP {status}")),
                Err(error) => Err(format!("{error:#}")),
            };
            (descriptor, result)
        }
    });
    let peers = futures_util::future::join_all(requests).await;
    let mut fleet = usage::fold_fleet_usage(local, peers, rejected);
    let Some(project) = project else {
        return Json(fleet).into_response();
    };
    usage::scope_to_project(&mut fleet, &project);
    let window_start = window_since(window);
    // `PriceTable::load()` is a SYNCHRONOUS fetch (ADR-0034 A6) and the recovery
    // map is a file read — neither may run on the async executor, the same stance
    // `spend_route` takes.
    let annotated = tokio::task::spawn_blocking(move || {
        spend::scope_to_window(
            &mut fleet.records,
            &mut fleet.interactive,
            window_start.as_deref(),
        );
        let recovered = usage::recovered_models(&usage_dir);
        let prices = ralphy_pricing::PriceTable::load();
        spend::annotate_unpriced(
            &mut fleet.records,
            &mut fleet.interactive,
            &recovered,
            &prices,
        );
        fleet
    })
    .await;
    match annotated {
        Ok(fleet) => Json(fleet).into_response(),
        // Answering `200` with unannotated rows would show an EMPTY unpriced
        // filter — a gap reported as closed is the lie this surface exists to
        // prevent.
        Err(error) => {
            tracing::warn!(%error, "the usage classification failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to classify usage",
            )
                .into_response()
        }
    }
}

/// The RFC3339 lower bound a window means right now. The clock lives HERE, not
/// in the fold, and this is the ONE derivation of it — `/api/spend` and
/// `/api/usage?project=` both call it, so the Overview's figures and the Ledger
/// grid beside them can never be scoped to two different weeks.
///
/// Snapped to the START of a civil day, n-1 days back: "last 7 days" is today
/// plus the six before it — exactly seven whole days. An instant-based
/// `now - 7 days` would instead span EIGHT dates, the first of them a partial
/// day that under-reads against full-day peaks, and would make the activity
/// band's zero-fill emit a column for a day it never seeded.
pub(crate) fn window_since(window: spend::Window) -> Option<String> {
    window.days().map(|days| {
        (chrono::Utc::now() - chrono::Duration::days(i64::from(days) - 1))
            .date_naive()
            .and_time(chrono::NaiveTime::MIN)
            .and_utc()
            .to_rfc3339()
    })
}

/// `GET /api/spend?project=<owner/repo>&period=<all|7d|30d|90d>`: that project's
/// priced spend summary — the total, the KPI tiles, the deliveries and models
/// grids, the activity band and the unpriced volume (PRD #355, #358, #359). The
/// response is **bounded**: opening the Spend tab must not transfer the ledger,
/// so the deliveries grid is capped and nothing else grows with the row count.
///
/// The clock lives HERE, not in the fold: an unrecognized `period` is refused
/// with `400`, and a recognized one becomes the RFC3339 `since` that both the
/// I/O scoping and `spend::summarize` are given.
///
/// Local only. The fleet-wide view is deliberately deferred (PRD #355, Out of
/// Scope); this reads the same local contribution `/api/usage` builds, and folds
/// it through the shared price table (ADR-0034 D6).
pub(crate) async fn spend_route(
    usage_dir: PathBuf,
    stores: StorePaths,
    registry_path: PathBuf,
    project: String,
    period: Option<String>,
) -> Response {
    // A silent fallback to `all` would show all-time figures under a window
    // label — exactly the misread this surface exists to prevent.
    let Some(window) = spend::Window::parse(period.as_deref().unwrap_or("all")) else {
        return (StatusCode::BAD_REQUEST, "unknown period").into_response();
    };
    let since = window_since(window);
    // Reading the ledger, scanning the vendor stores and loading the price table
    // are all SYNCHRONOUS file I/O (the price fetch is sync by ADR-0034 A6), and
    // the scans walk whole session trees — none of it may run on the async
    // executor. Same stance as `repos_route`'s per-repo `git` spawns.
    let summary = tokio::task::spawn_blocking(move || {
        // The window is applied by the FOLD alone, deliberately. Pre-filtering
        // the read would scope the I/O too, but `usage::run_records` reads a
        // missing `ts` as `""` and drops the row, while the fold KEEPS a row
        // with no timestamp (`spend::period::in_window`) — so a ts-less ledger
        // line would silently leave the total the moment an operator picked a
        // period. Two filters that disagree about a row is worse than one pass
        // over a file this route already reads whole for `all`.
        let contribution =
            local_usage_contribution(usage_dir.clone(), stores, registry_path, None, None);
        let recovered = usage::recovered_models(&usage_dir);
        let prices = ralphy_pricing::PriceTable::load();
        spend::summarize(&spend::SpendInput {
            records: &contribution.records,
            interactive: &contribution.interactive,
            recovered: &recovered,
            prices: &prices,
            project: &project,
            window,
            since: since.as_deref(),
        })
    })
    .await;
    match summary {
        Ok(summary) => Json(summary).into_response(),
        // A panicked blocking task must not read as an empty project: `$0` and
        // "nothing spent" are exactly the lies this surface exists to avoid.
        Err(error) => {
            tracing::warn!(%error, "the spend fold failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to summarize spend",
            )
                .into_response()
        }
    }
}

pub(crate) async fn usage_local_route(
    usage_dir: PathBuf,
    stores: StorePaths,
    registry_path: PathBuf,
    daemon_id: Option<String>,
    since: Option<String>,
) -> Response {
    Json(local_usage_contribution(
        usage_dir,
        stores,
        registry_path,
        daemon_id,
        since.as_deref(),
    ))
    .into_response()
}

pub(crate) fn local_usage_contribution(
    usage_dir: PathBuf,
    stores: StorePaths,
    registry_path: PathBuf,
    daemon_id: Option<String>,
    since: Option<&str>,
) -> usage::UsageContribution {
    // A registry load error must not fail the page — serve interactive records
    // with no project/actor attribution, like `repos_route` (logged).
    let store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry for the usage scan; serving unattributed");
            registry::RegistryStore::default()
        }
    };
    usage::local_contribution(&usage_dir, &stores, &store, daemon_id, since)
}

/// `GET /api/desk`: the saved desk — windows and fences together, each in layout
/// order (ADR-0050, ADR-0051 §10), plus `checkouts` (the selected worktree per
/// repo ref, ADR-0063 §4) when any is set. An absent or corrupt `desk.toml`
/// answers `200 {"windows":[],"fences":[]}` — a lost layout costs a cascaded
/// stage, never an error the shell has to handle.
///
/// Served under the registry's CANONICAL keys: a record saved under a slug the
/// registry has since re-keyed (`former_slugs`) is rewritten on the way out,
/// so a migrated project's consoles come back to it (ADR-0036 amendment
/// 2026-09-16). The registry is read fresh, like `/api/repos`.
pub(crate) async fn desk_get_route(path: PathBuf, registry_path: PathBuf) -> Response {
    let aliases = former_slug_aliases(&registry_path);
    Json(rekey::rekey_desk(desk::load_from(&path), &aliases)).into_response()
}

/// The registry's former-slug → canonical-key map, or empty when the registry
/// is unreadable (warned; the desk is still served — a lost alias costs a
/// cascaded stage, never the layout).
pub(crate) fn former_slug_aliases(
    registry_path: &Path,
) -> std::collections::BTreeMap<String, String> {
    match registry::load_from(registry_path) {
        Ok(store) => store.former_slug_map(),
        Err(e) => {
            tracing::warn!(error = %e, "repo registry unreadable; desk served without slug aliases");
            Default::default()
        }
    }
}

/// `PUT /api/desk`: replace the desk wholesale, each record type pruned to its
/// own cap ([`desk::DESK_MAX`], [`desk::FENCE_MAX`]) newest by `ts`, answering
/// `200` with the pruned store — the client needs the daemon's post-prune truth
/// in one round trip (last-write-wins, no ETag).
///
/// A body that is not a `{ windows, fences, checkouts? }` object — including
/// the pre-#340 bare array — is rejected by the `Json` extractor as `422` and
/// never reaches here, so `desk.toml` is untouched; a rect that is out of frame
/// — non-finite, or an origin off the stage's pinned 0,0 — and a checkout
/// value that is not one path component (`checkout::lexical`) are rejected
/// here as `400`. Every rejection returns BEFORE any write, so a refused upload
/// leaves `desk.toml` byte-identical on every path. A well-shaped checkout name
/// is stored unvalidated: whether the worktree still exists is the verb's call
/// (`unknown checkout`), not a spawn per desk write.
///
/// Non-overlap between fences is deliberately NOT validated: refusing a whole
/// desk upload would cost the operator their layout and the daemon has no repair
/// path, so that invariant belongs to the client (ADR-0051 §6).
///
/// Stored under the registry's canonical keys: an upload from a tab that read
/// the desk before a re-key still names the former slug, and is normalized
/// through `former_slugs` before anything is written — so `desk.toml`
/// converges on the first save after a migration, whichever tab saves.
pub(crate) async fn desk_put_route(
    path: PathBuf,
    registry_path: PathBuf,
    up: desk::DeskUpload,
) -> Response {
    if let Some(bad) = up.windows.iter().find(|r| !desk::rect_is_sane(&r.rect)) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("record {} has an out-of-frame rect", bad.id) }),
            ),
        )
            .into_response();
    }
    if let Some(bad) = up.fences.iter().find(|f| !desk::rect_is_sane(&f.rect)) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("fence {} has an out-of-frame rect", bad.id) }),
            ),
        )
            .into_response();
    }
    if let Some(bad) = up.notes.iter().find(|n| !desk::rect_is_sane(&n.rect)) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("note {} has an out-of-frame rect", bad.id) }),
            ),
        )
            .into_response();
    }
    if let Some((repo, name)) = up
        .checkouts
        .iter()
        .find(|(_, n)| checkout::lexical(n).is_none())
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("checkout {name} for {repo} is not a valid name") }),
            ),
        )
            .into_response();
    }
    // #411: a per-record checkout is the same kind of name as the per-repo
    // selection, gated the same way before anything is written. A note card
    // carries the same key (ADR-0064 §4, identity is `(checkout, path)`).
    let record_checkouts = up
        .windows
        .iter()
        .map(|r| (r.id.as_str(), r.checkout.as_deref()))
        .chain(
            up.notes
                .iter()
                .map(|n| (n.id.as_str(), n.checkout.as_deref())),
        );
    if let Some((id, name)) = record_checkouts
        .filter_map(|(id, c)| c.map(|n| (id, n)))
        .find(|(_, n)| checkout::lexical(n).is_none())
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("checkout {name} on record {id} is not a valid name") }),
            ),
        )
            .into_response();
    }
    // Read, fold, write — as ONE step. Two pages flushing at once would
    // otherwise both read the same desk and the second write would drop the
    // first fold; the lock is process-wide because the store is (one
    // `desk.toml` per daemon). Nothing awaits under it: `load_from` and
    // `save_to` are synchronous file reads and writes.
    static DESK_WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _held = DESK_WRITE.lock().await;
    let merged = desk::merge(desk::load_from(&path), up);
    let store = rekey::rekey_desk(
        desk::DeskStore {
            windows: desk::prune(merged.windows),
            fences: desk::prune_fences(merged.fences),
            notes: desk::prune_notes(merged.notes),
            checkouts: merged.checkouts,
        },
        &former_slug_aliases(&registry_path),
    );
    match desk::save_to(&store, &path) {
        Ok(()) => Json(store).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}

/// `GET /api/identity`: the loaded identity's `name`/`avatar` as JSON, or 404
/// when the daemon has not been baptized yet.
pub(crate) async fn identity_route(identity: Option<identity::Identity>) -> Response {
    #[derive(serde::Serialize)]
    struct IdentityView {
        name: String,
        avatar: String,
    }
    match identity {
        Some(id) => Json(IdentityView {
            name: id.name,
            avatar: id.avatar,
        })
        .into_response(),
        None => (StatusCode::NOT_FOUND, "no identity").into_response(),
    }
}

/// `GET /api/about`: the daemon's static product facts for the workbench About
/// panel — the git-published version (embedded at build time, so it tracks the
/// release tag) and the license/creator/source facts pulled straight from the
/// workspace manifest. Read-only, no secrets.
///
/// It carries no description. The one that was here was
/// `CARGO_PKG_DESCRIPTION`, written for whoever opens the manifest — it cites
/// an ADR path and the rule confining tokio to this crate — and an About card
/// is not that reader. The crate keeps its description; the card says nothing
/// rather than saying the wrong thing to the wrong audience.
pub(crate) async fn about_route() -> Response {
    #[derive(serde::Serialize)]
    struct AboutView {
        name: &'static str,
        version: &'static str,
        license: &'static str,
        repository: &'static str,
        creator: &'static str,
    }
    Json(AboutView {
        name: "ralphy",
        // Embedded by build.rs from `git describe --tags` (falls back to the
        // Cargo manifest version off a tarball).
        version: env!("RALPHY_VERSION"),
        // From the workspace manifest (`license`/`repository` are inherited).
        license: env!("CARGO_PKG_LICENSE"),
        repository: env!("CARGO_PKG_REPOSITORY"),
        creator: "Paulo Corcino",
    })
    .into_response()
}

/// How often the daemon asks what has been published. Four reads a day notices a
/// release cut this morning and cannot contribute to exhausting the
/// unauthenticated rate limit (ADR-0056 §6).
pub(crate) const RELEASE_POLL_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// One pass of the release watch. Blocking (it is `ureq`), silent on failure by
/// construction, and a no-op when the operator turned the watch off.
pub(crate) fn poll_releases(store: &Path) {
    if release::watch_disabled_in(store) {
        return;
    }
    let cache = release::cache_path_in(store);
    ralphy_release::fetch::refresh_if_stale(&ralphy_release::RefreshOpts::new(&cache));
}

/// `GET /api/release`: where this build stands against what has been published,
/// and the whole gap between the two.
///
/// Reads the cache only — the fetch is the background watch's job, so a page
/// load never waits on the network and never triggers a request of its own.
/// A store the daemon could not resolve answers the same shape with nothing in
/// it, because "we do not know" is a normal state, not an error.
pub(crate) async fn release_route(store: Option<PathBuf>) -> Response {
    let (releases, disabled) = match store.as_deref() {
        Some(dir) => (
            ralphy_release::fetch::load(&release::cache_path_in(dir)),
            release::watch_disabled_in(dir),
        ),
        None => (Vec::new(), false),
    };
    Json(release::view(
        env!("RALPHY_VERSION"),
        &releases,
        ralphy_release::Channel::Rc,
        disabled,
    ))
    .into_response()
}

/// `GET /api/agents[?repo=<routed-ref>]`: roster and presence snapshot from the
/// environment that owns `repo`. A peer request deliberately omits `repo`, so
/// the owning daemon computes locally and federation cannot recurse.
pub(crate) async fn agents_route(
    Query(query): Query<AgentsQuery>,
    peers_dir: PathBuf,
    daemon_id: String,
    locator: AgentLocator,
    bound_port: u16,
) -> Response {
    let Some(repo_ref) = query.repo.as_deref() else {
        return Json(roster::roster_with(locator.as_ref())).into_response();
    };
    let (descriptors, rejects) = read_peer_store(peers_dir).await;
    match fleet::route(repo_ref, &daemon_id, &descriptors) {
        fleet::Route::Local { .. } => Json(roster::roster_with(locator.as_ref())).into_response(),
        fleet::Route::Peer { peer, .. } => {
            let status = peer::client::probe(
                peer,
                peer::client::SelfRef {
                    port: bound_port,
                    daemon_id: &daemon_id,
                },
            )
            .await;
            if status != peer::client::PeerStatus::Reachable {
                return (StatusCode::BAD_GATEWAY, status.diagnosis(&peer.environment))
                    .into_response();
            }
            match peer::client::get(peer, "/api/agents").await {
                Ok((200, body)) => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "application/json")],
                    body,
                )
                    .into_response(),
                Ok((status, _)) => (
                    StatusCode::BAD_GATEWAY,
                    fleet::peer_unreachable(
                        peer,
                        &format!("the peer answered HTTP {status} to the roster request"),
                    ),
                )
                    .into_response(),
                Err(error) => (
                    StatusCode::BAD_GATEWAY,
                    fleet::peer_unreachable(peer, &format!("{error:#}")),
                )
                    .into_response(),
            }
        }
        fleet::Route::UnknownDaemon { daemon_id } => {
            if let Some((environment, theirs)) = rejects
                .iter()
                .find_map(|reject| reject.version_mismatch_for(daemon_id))
            {
                let status = peer::client::PeerStatus::VersionMismatch {
                    theirs,
                    ours: peer::PEER_PROTOCOL_VERSION,
                };
                return (StatusCode::BAD_GATEWAY, status.diagnosis(environment)).into_response();
            }
            (
                StatusCode::BAD_GATEWAY,
                format!("unknown peer daemon {daemon_id}"),
            )
                .into_response()
        }
    }
}

/// Serve a file from the embedded UI tree; `/` means `index.html`. Every asset
/// carries a content `ETag` under `Cache-Control: no-cache`, so a reload is a
/// round of `304`s, and a text asset goes out gzipped when the browser admits
/// it (see [`crate::assets`]).
pub(crate) async fn ui_asset(headers: HeaderMap, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let Some(file) = UI.get_file(path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let content_type = content_type(path);
    let prepared =
        assets::prepared(path, file.contents(), assets::compressible(content_type)).await;
    let header_str = |name: header::HeaderName| headers.get(name).and_then(|v| v.to_str().ok());

    let mut resp = Response::builder()
        .header(header::ETAG, prepared.etag.as_str())
        .header(header::CACHE_CONTROL, assets::CACHE_CONTROL)
        .header(header::VARY, "Accept-Encoding");
    if assets::not_modified(header_str(header::IF_NONE_MATCH), &prepared.etag) {
        resp = resp.status(StatusCode::NOT_MODIFIED);
        return finish_asset(resp.body(axum::body::Body::empty()));
    }
    resp = resp.header(header::CONTENT_TYPE, content_type);
    let body = match &prepared.gzip {
        Some(gz) if assets::accepts_gzip(header_str(header::ACCEPT_ENCODING)) => {
            resp = resp.header(header::CONTENT_ENCODING, "gzip");
            axum::body::Body::from(gz.clone())
        }
        _ => axum::body::Body::from(file.contents()),
    };
    finish_asset(resp.body(body))
}

/// The builder's only failure is an invalid header value, and every value
/// above is a literal or a hex string — a failure is a bug, answered with a
/// 500 rather than a panic on the request path.
fn finish_asset(built: Result<Response, axum::http::Error>) -> Response {
    match built {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!(error = %e, "building an asset response");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub(crate) fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("eot") => "application/vnd.ms-fontobject",
        Some("json") => "application/json",
        // A manifest served as octet-stream is ignored by every browser, which
        // is a silent failure: the shell renders, "add to home screen" just
        // never offers a standalone launch.
        Some("webmanifest") => "application/manifest+json",
        _ => "application/octet-stream",
    }
}
