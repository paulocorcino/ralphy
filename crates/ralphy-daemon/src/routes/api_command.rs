//! `POST /api/command`: a one-reply command over HTTP (ADR-0082 D1). The body
//! is the `Command` the command socket takes, and the answer is the one reply
//! payload the socket would send. A verb whose output streams is refused
//! here: the workbench sends it on the command socket. Same guard, body cap
//! and audit record as the socket (ADR-0072 D3, D11).

use axum::extract::DefaultBodyLimit;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Extension, Json, Router};

use super::audit_layer::{Caller, SocketAudit};
use super::RouterShared;
use super::{answer_one, host_refusal, request_may_carry_a_secret, Answer, CommandCtx};
use crate::protocol::Command;

/// The answer to a verb whose output streams.
fn streams_refusal() -> serde_json::Value {
    serde_json::json!({
        "status": "error",
        "message": "This command sends its output while it runs, so the workbench sends it on the command socket.",
    })
}

/// The one reply to `cmd`, or the refusal of a verb that streams. A host
/// verb that its own checks refuse gets that refusal, as on the socket.
async fn reply(ctx: &CommandCtx, cmd: &Command, who: &SocketAudit) -> serde_json::Value {
    match answer_one(ctx, cmd, who).await {
        Answer::Reply(reply) => reply,
        Answer::Stream(verb) if verb.is_host() => {
            host_refusal(cmd, verb, ctx.secret_ok).unwrap_or_else(streams_refusal)
        }
        Answer::Stream(_) => streams_refusal(),
    }
}

pub(crate) fn command_http_routes(s: &RouterShared) -> Router {
    let registry_path = s.registry_path.clone();
    let peers_dir = s.peers_dir.clone();
    let daemon_id = s.daemon_id.clone();
    let sessions = s.sessions.clone();
    let audit = s.audit.clone();
    Router::new()
        .route(
            "/api/command",
            post(
                move |headers: HeaderMap,
                      caller: Option<Extension<Caller>>,
                      Json(cmd): Json<Command>| {
                    let ctx = CommandCtx {
                        registry_path: registry_path.clone(),
                        peers_dir: peers_dir.clone(),
                        daemon_id: daemon_id.clone(),
                        sessions: sessions.clone(),
                        secret_ok: request_may_carry_a_secret(&headers),
                    };
                    let who = SocketAudit::of(audit.clone(), caller);
                    async move { Json(reply(&ctx, &cmd, &who).await) }
                },
            ),
        )
        .layer(DefaultBodyLimit::max(crate::tree::MAX_COMMAND_BYTES))
}
