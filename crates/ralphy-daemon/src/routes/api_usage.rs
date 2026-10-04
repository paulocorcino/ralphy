//! The usage and spend resources: the token-usage ledger with the
//! interactive records, the Spend tab's summary, and this daemon's own
//! contribution that a peer reads.

use std::path::PathBuf;

use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Json;
use axum::Router;

use super::RouterShared;
use super::{encode_query_value, read_peer_store};
use crate::StorePaths;
use crate::{peer, registry, spend, usage};

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

/// The usage routes of the workbench: usage and spend.
pub(crate) fn usage_routes(s: &RouterShared) -> Router {
    Router::new()
        .route(
            "/api/usage",
            get({
                let dir = s.usage_dir.clone();
                let stores = s.stores.clone();
                let registry = s.registry_path.clone();
                let daemon_id = s.daemon_id.clone();
                let peers = s.peers_dir.clone();
                move |q: Query<UsageQuery>| {
                    usage_route(
                        dir,
                        stores,
                        registry,
                        peers,
                        daemon_id,
                        q.0.since,
                        q.0.project,
                        q.0.period,
                    )
                }
            }),
        )
        .route(
            "/api/spend",
            get({
                let dir = s.usage_dir.clone();
                let stores = s.stores.clone();
                let registry = s.registry_path.clone();
                move |q: Query<SpendQuery>| {
                    spend_route(
                        dir.clone(),
                        stores.clone(),
                        registry.clone(),
                        q.0.project,
                        q.0.period,
                    )
                }
            }),
        )
}
