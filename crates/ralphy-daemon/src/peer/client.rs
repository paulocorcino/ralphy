//! Dialling a peer (docs/adr/0052 §2): the daemon's first — and only — HTTP
//! client, confined to this one seam.
//!
//! Loopback only, always. A descriptor advertising a routable address is refused
//! before a socket is opened: the whole transport rests on WSL2's
//! `localhostForwarding` relay, so anything that is not `127.0.0.1` is either a
//! mistake or an attempt to make this daemon dial the network for someone.
//!
//! Every dial is bounded by a timeout and its connection task is aborted on
//! EVERY return path — a peer that stops answering mid-body must cost one failed
//! request, not one leaked task per probe.

use std::net::IpAddr;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use axum::body::Bytes;
use axum::http::{header, Request};
use http_body_util::{BodyExt, Full};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::{PeerDescriptor, PEER_PROTOCOL_VERSION};

/// An authenticated session socket to the daemon that owns the repo.
pub type SessionSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub enum SessionError {
    Peer(PeerStatus),
    Http { status: u16, body: String },
}

/// How long a peer has to accept a connection and answer. Short on purpose:
/// `/api/fleet` probes every peer on every request, so an absent peer must cost
/// the operator a beat, not a page load.
pub const PEER_TIMEOUT: Duration = Duration::from_secs(2);

/// Ceiling on a peer's response body. A maximum-size accepted image expands by
/// 4/3 in base64, then rides in a small JSON envelope; derive the cap from that
/// user-visible boundary while still bounding any descriptor-named port.
const MAX_PEER_BODY: usize = (crate::tree::MAX_IMAGE_BYTES as usize).div_ceil(3) * 4 + 1024;

/// What a probe learned about a peer. Computed fresh on every request and never
/// persisted — a descriptor is a claim, liveness is an observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerStatus {
    Reachable,
    Unauthorized,
    VersionMismatch { theirs: u32, ours: u32 },
    Unreachable { why: String },
    Refused { why: String },
}

impl PeerStatus {
    /// The short state string the workbench renders and groups by.
    pub fn state(&self) -> &'static str {
        match self {
            PeerStatus::Reachable => "reachable",
            PeerStatus::Unauthorized => "unauthorized",
            PeerStatus::VersionMismatch { .. } => "version-mismatch",
            PeerStatus::Unreachable { .. } => "unreachable",
            PeerStatus::Refused { .. } => "refused",
        }
    }

    /// An operator-facing sentence naming WHICH environment is in this state and
    /// what to do about it. The environment label is what makes a fleet
    /// diagnosis legible — "unauthorized" alone names no machine.
    pub fn diagnosis(&self, environment: &str) -> String {
        match self {
            PeerStatus::Reachable => format!("peer {environment} answered the handshake"),
            PeerStatus::Unauthorized => format!(
                "peer {environment} refused the credential — its token was rotated; restart that daemon with --peer-store to re-announce"
            ),
            PeerStatus::VersionMismatch { theirs, ours } => format!(
                "peer {environment} speaks peer protocol {theirs}, this daemon speaks {ours} — upgrade the older Ralphy"
            ),
            PeerStatus::Unreachable { why } => format!(
                "peer {environment} did not answer ({why}) — start it, or nudge it if it is a WSL distro"
            ),
            PeerStatus::Refused { why } => {
                format!("peer {environment} was not dialled: {why}")
            }
        }
    }
}

/// The loopback gate: `None` when `address` is a loopback IP and may be dialled,
/// `Some(Refused { .. })` for anything else, including an unparseable address.
pub fn classify_address(address: &str) -> Option<PeerStatus> {
    match address.parse::<IpAddr>() {
        Ok(ip) if ip.is_loopback() => None,
        Ok(ip) => Some(PeerStatus::Refused {
            why: format!("{ip} is not a loopback address; a peer is reached over loopback only"),
        }),
        Err(e) => Some(PeerStatus::Refused {
            why: format!("`{address}` is not an IP address ({e})"),
        }),
    }
}

/// Aborts the hyper connection task on every return path — success, HTTP error,
/// timeout, or an early `?`. Without this a peer that stalls mid-body leaks one
/// task per probe, and `/api/fleet` probes on every request.
struct ConnGuard(tokio::task::JoinHandle<()>);

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// `GET <path>` from `d` over loopback with `d.token` as a bearer credential.
/// Returns the status code and the collected body.
pub async fn get(d: &PeerDescriptor, path: &str) -> Result<(u16, Vec<u8>)> {
    send(d, "GET", path, None, PEER_TIMEOUT).await
}

/// `POST <path>` with a JSON body and `d.token` as a bearer credential.
pub async fn post_json(
    d: &PeerDescriptor,
    path: &str,
    body: &serde_json::Value,
) -> Result<(u16, Vec<u8>)> {
    send(d, "POST", path, Some(body), PEER_TIMEOUT).await
}

/// JSON POST whose response may legitimately outlive the ordinary peer timeout.
/// Connection establishment remains bounded by [`PEER_TIMEOUT`].
pub async fn post_json_timeout(
    d: &PeerDescriptor,
    path: &str,
    body: &serde_json::Value,
    response_timeout: Duration,
) -> Result<(u16, Vec<u8>)> {
    send(d, "POST", path, Some(body), response_timeout).await
}

/// Probe the peer protocol, then open an authenticated `/ws/session` socket.
///
/// The socket is established before the browser is upgraded, so every refusal
/// remains an ordinary HTTP diagnosis on the local daemon.
pub async fn session(
    d: &PeerDescriptor,
    query: &str,
) -> std::result::Result<SessionSocket, SessionError> {
    let status = probe(d).await;
    if status != PeerStatus::Reachable {
        return Err(SessionError::Peer(status));
    }
    if let Some(refused) = classify_address(&d.address) {
        return Err(SessionError::Peer(refused));
    }

    let authority = format!("{}:{}", d.address, d.port);
    let stream = tokio::time::timeout(
        PEER_TIMEOUT,
        tokio::net::TcpStream::connect((d.address.as_str(), d.port)),
    )
    .await
    .map_err(|_| {
        SessionError::Peer(PeerStatus::Unreachable {
            why: format!("connecting to {authority} timed out"),
        })
    })?
    .map_err(|e| {
        SessionError::Peer(PeerStatus::Unreachable {
            why: format!("connecting to {authority}: {e}"),
        })
    })?;

    let uri = format!("ws://{authority}/ws/session?{query}");
    let mut request = uri.into_client_request().map_err(|e| {
        SessionError::Peer(PeerStatus::Unreachable {
            why: format!("building the session request: {e}"),
        })
    })?;
    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {}", d.token).parse().map_err(|e| {
            SessionError::Peer(PeerStatus::Unreachable {
                why: format!("building the peer credential: {e}"),
            })
        })?,
    );

    let (socket, _) = tokio::time::timeout(
        PEER_TIMEOUT,
        tokio_tungstenite::client_async(request, tokio_tungstenite::MaybeTlsStream::Plain(stream)),
    )
    .await
    .map_err(|_| {
        SessionError::Peer(PeerStatus::Unreachable {
            why: format!("session handshake with {authority} timed out"),
        })
    })?
    .map_err(|error| match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => SessionError::Http {
            status: response.status().as_u16(),
            body: response
                .body()
                .as_deref()
                .map(|body| String::from_utf8_lossy(body).into_owned())
                .unwrap_or_default(),
        },
        error => SessionError::Peer(PeerStatus::Unreachable {
            why: format!("opening the peer session at {authority}: {error}"),
        }),
    })?;
    Ok(socket)
}

async fn send(
    d: &PeerDescriptor,
    method: &str,
    path: &str,
    body: Option<&serde_json::Value>,
    response_timeout: Duration,
) -> Result<(u16, Vec<u8>)> {
    // Any refusal at all stops the dial — matching only the `Refused` shape would
    // let a future variant fall through and open the socket. The gate fails CLOSED.
    if let Some(refused) = classify_address(&d.address) {
        bail!("{}", refused.diagnosis(&d.environment));
    }
    let authority = format!("{}:{}", d.address, d.port);
    let stream = tokio::time::timeout(
        PEER_TIMEOUT,
        tokio::net::TcpStream::connect((d.address.as_str(), d.port)),
    )
    .await
    .with_context(|| format!("connecting to {authority} timed out"))?
    .with_context(|| format!("connecting to {authority}"))?;

    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
            .await
            .with_context(|| format!("HTTP handshake with {authority}"))?;
    let _guard = ConnGuard(tokio::spawn(async move {
        // A dropped/errored connection is the peer going away; the request future
        // reports it, so there is nothing to propagate from here.
        if let Err(e) = conn.await {
            tracing::debug!(error = %e, "peer connection ended");
        }
    }));

    let bytes = match body {
        Some(body) => serde_json::to_vec(body).context("encoding the peer request body")?,
        None => Vec::new(),
    };
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(axum::http::header::HOST, &authority)
        .header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {}", d.token),
        );
    if body.is_some() {
        builder = builder.header(axum::http::header::CONTENT_TYPE, "application/json");
    }
    let req = builder
        .body(Full::new(Bytes::from(bytes)))
        .context("building the peer request")?;

    let resp = tokio::time::timeout(response_timeout, sender.send_request(req))
        .await
        .with_context(|| format!("request to {authority}{path} timed out"))?
        .with_context(|| format!("requesting {authority}{path}"))?;
    let status = resp.status().as_u16();
    // Cap the body: `probe` collects from any loopback port a descriptor names,
    // before anything about the answer has been validated.
    let body = tokio::time::timeout(
        response_timeout,
        http_body_util::Limited::new(resp.into_body(), MAX_PEER_BODY).collect(),
    )
    .await
    .with_context(|| format!("reading the body from {authority}{path} timed out"))?
    // `Limited`'s error is a boxed `StdError`, which `anyhow` cannot take a
    // context on directly — carry its message instead of dropping it.
    .map_err(|e| anyhow::anyhow!("{e}"))
    .with_context(|| {
        format!("reading the body from {authority}{path} (cap {MAX_PEER_BODY} bytes)")
    })?
    .to_bytes()
    .to_vec();
    Ok((status, body))
}

/// Probe `d` with the version handshake and classify what came back.
pub async fn probe(d: &PeerDescriptor) -> PeerStatus {
    if let Some(refused) = classify_address(&d.address) {
        return refused;
    }
    let (status, body) = match get(d, "/api/peer/hello").await {
        Ok(pair) => pair,
        Err(e) => {
            return PeerStatus::Unreachable {
                why: format!("{e:#}"),
            }
        }
    };
    if status == 401 || status == 403 {
        return PeerStatus::Unauthorized;
    }
    if status != 200 {
        return PeerStatus::Unreachable {
            why: format!("peer answered HTTP {status} to the handshake"),
        };
    }
    let theirs = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("protocol_version").and_then(|p| p.as_u64()));
    match theirs {
        Some(v) if v as u32 == PEER_PROTOCOL_VERSION => PeerStatus::Reachable,
        Some(v) => PeerStatus::VersionMismatch {
            theirs: v as u32,
            ours: PEER_PROTOCOL_VERSION,
        },
        None => PeerStatus::Unreachable {
            why: "the handshake answered without a protocol_version".to_string(),
        },
    }
}
