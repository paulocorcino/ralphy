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

/// An authenticated WebSocket to another daemon in the local fleet.
pub type PeerSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub enum SocketError {
    Peer(PeerStatus),
    Http { status: u16, body: String },
}

/// How long a peer has to answer a request. Short on purpose: `/api/fleet`
/// probes every peer on every request, so an absent peer must cost the operator
/// a beat, not a page load.
pub const PEER_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a peer has to accept the TCP connection. Shorter than
/// [`PEER_TIMEOUT`], so a peer that is down fails at connect, with the same
/// reason on every platform. Windows refuses a closed loopback port only after
/// about 2 s, while a listening port accepts in under 1 ms (measured with
/// `TcpClient`, Windows 11, 2026-10-04).
pub const PEER_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

// #536: with equal limits, the connect error and the request timeout raced on
// Windows, and the operator saw one of two reasons for the same stopped peer.
const _: () = assert!(PEER_CONNECT_TIMEOUT.as_millis() < PEER_TIMEOUT.as_millis());

/// Ceiling on a peer's response body. A maximum-size accepted image expands by
/// 4/3 in base64, then rides in a small JSON envelope; derive the cap from that
/// user-visible boundary while still bounding any descriptor-named port.
const MAX_PEER_BODY: usize = (crate::tree::MAX_IMAGE_BYTES as usize).div_ceil(3) * 4 + 1024;

/// What a probe learned about a peer. Computed fresh on every request and never
/// persisted — a descriptor is a claim, liveness is an observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerStatus {
    Reachable,
    /// The peer refused the token. `tunnel` is true when the peer is reached
    /// through a tunnel, where the `--peer-store` advice for a WSL distro does
    /// not apply.
    Unauthorized {
        tunnel: bool,
    },
    VersionMismatch {
        theirs: u32,
        ours: u32,
    },
    /// The peer's WSL distro is not running, so neither is the daemon inside it.
    /// Distinct from [`PeerStatus::Unreachable`] because it is the ordinary case,
    /// not a fault: WSL terminates an idle distro and the daemon goes with it.
    Asleep {
        distro: String,
    },
    Unreachable {
        why: String,
    },
    Refused {
        why: String,
    },
    /// The `ssh` that carries this peer (ADR-0067 §2) is not running; the
    /// daemon is opening it again. Not a fault: a VPN drop ends up here.
    TunnelClosed {
        host: String,
        cause: Option<String>,
        /// The last line the `ssh` that exited wrote on its stderr.
        said: Option<String>,
    },
    /// The tunnel is open, but the daemon on the other end does not answer.
    TunnelSilent {
        host: String,
        why: String,
    },
}

impl PeerStatus {
    /// The short state string the workbench renders and groups by.
    pub fn state(&self) -> &'static str {
        match self {
            PeerStatus::Reachable => "reachable",
            PeerStatus::Unauthorized { .. } => "unauthorized",
            PeerStatus::VersionMismatch { .. } => "version-mismatch",
            PeerStatus::Asleep { .. } => "asleep",
            PeerStatus::Unreachable { .. } => "unreachable",
            PeerStatus::Refused { .. } => "refused",
            PeerStatus::TunnelClosed { .. } => "tunnel-closed",
            PeerStatus::TunnelSilent { .. } => "tunnel-silent",
        }
    }

    /// Operator-facing sentences naming WHICH environment is in this state and
    /// what to do about it. The environment label is what makes a fleet
    /// diagnosis legible — "unauthorized" alone names no machine. The workbench
    /// shows them as they are, so they follow ADR-0065 (#432): a capital letter,
    /// a final period, and the remedy as a sentence of its own.
    pub fn diagnosis(&self, environment: &str) -> String {
        match self {
            PeerStatus::Reachable => format!("{environment} is connected."),
            PeerStatus::Unauthorized { tunnel: false } => {
                format!("{environment} refused the token. Restart its daemon with --peer-store.")
            }
            PeerStatus::Unauthorized { tunnel: true } => format!(
                "{environment} refused the token. Another daemon may answer on this port, or the token changed. Add the host again."
            ),
            PeerStatus::VersionMismatch { theirs, ours } => format!(
                "{environment} uses protocol {theirs}, not {ours}. Upgrade the older Ralphy."
            ),
            PeerStatus::Asleep { distro } => {
                format!("{environment} is asleep. Wake the distro {distro}.")
            }
            PeerStatus::Unreachable { why } => {
                format!("{environment} did not answer: {why}. Start its daemon.")
            }
            PeerStatus::Refused { why } => {
                format!("Ralphy did not connect to {environment}: {why}.")
            }
            PeerStatus::TunnelClosed {
                host,
                cause: None,
                said: None,
            } => format!("The tunnel to {host} is closed. Ralphy is opening it again."),
            PeerStatus::TunnelClosed {
                host,
                cause: None,
                said: Some(said),
            } => format!(
                "The tunnel to {host} is closed: ssh said \"{said}\". Ralphy is opening it again."
            ),
            PeerStatus::TunnelClosed {
                host,
                cause: Some(cause),
                ..
            } => format!("The tunnel to {host} is closed. Ralphy could not open it: {cause}."),
            PeerStatus::TunnelSilent { host, why } => format!(
                "The tunnel to {host} is open, but its daemon does not answer: {why}. Start it."
            ),
        }
    }
}

/// Who this daemon is, for the checks a probe makes before and after dialling.
/// Borrowed: it is read once per probe and never stored.
#[derive(Debug, Clone, Copy)]
pub struct SelfRef<'a> {
    /// The port this daemon's own listener is bound to.
    pub port: u16,
    /// This daemon's `daemon_id`, empty when un-baptized.
    pub daemon_id: &'a str,
}

/// The self-dial gate: `Some(Refused { .. })` when the descriptor points at this
/// daemon's own loopback listener.
///
/// Both daemons default to [`crate::DEFAULT_PORT`], and WSL2's
/// `localhostForwarding` — the relay ADR-0052 §2 depends on — publishes the
/// peer's listener on that same port on the Windows loopback. Whichever daemon
/// starts second loses: if it is the local one it fails to bind and says so, but
/// if it is the peer's relay the loss is silent, and the local daemon then dials
/// `127.0.0.1:<its own port>` and reaches ITSELF.
///
/// That is worth its own refusal because of how it looked without one: the local
/// daemon presented the peer's token to its own auth, was correctly rejected,
/// and reported the peer's credential as rotated — sending the operator to fix a
/// token that was never broken (measured in the #353 capstone). Refusing to dial
/// names the collision instead.
pub fn classify_self_dial(address: &str, port: u16, me: SelfRef<'_>) -> Option<PeerStatus> {
    let loopback = address
        .parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false);
    (loopback && port == me.port).then(|| PeerStatus::Refused {
        why: format!(
            "it announces {address}:{port}, the port of this daemon. \
             A connection there comes back here, so the two cannot work together. \
             Give one of them a different `--port`"
        ),
    })
}

/// The loopback gate: `None` when `address` is a loopback IP and may be dialled,
/// `Some(Refused { .. })` for anything else, including an unparseable address.
pub fn classify_address(address: &str) -> Option<PeerStatus> {
    match address.parse::<IpAddr>() {
        Ok(ip) if ip.is_loopback() => None,
        Ok(ip) => Some(PeerStatus::Refused {
            why: format!("{ip} is not a loopback address"),
        }),
        Err(e) => Some(PeerStatus::Refused {
            why: format!("`{address}` is not an IP address ({e})"),
        }),
    }
}

/// Which "unreachable" a failed dial actually was, given what the host knows
/// about the peer's own distro.
///
/// One state covered two opposite situations, and they want opposite sentences:
/// the environment is not running at all (normal — WSL idles a distro out, and a
/// nudge pays a cold start), or the environment is running and its daemon is not
/// (a fault — the unit died, and a nudge fixes it at once). `running` is `None`
/// when the host could not be asked, and an unasked host must not be made to
/// invent either verdict: the answer degrades to the diagnosis that assumes
/// nothing.
///
/// Pure on purpose — the process that produces `running` lives in `peer::nudge`,
/// the one seam allowed to invoke `wsl.exe`.
pub fn classify_unreachable(
    distro: Option<&str>,
    running: Option<bool>,
    why: String,
) -> PeerStatus {
    match (distro, running) {
        (Some(distro), Some(false)) => PeerStatus::Asleep {
            distro: distro.to_string(),
        },
        (Some(distro), Some(true)) => PeerStatus::Unreachable {
            why: format!("the distro {distro} is running, but its daemon is not: {why}"),
        },
        _ => PeerStatus::Unreachable { why },
    }
}

/// Which tunnel state a failed dial was, from the answer of the ensure that
/// followed it: `Ok(true)` means it had to start a new `ssh`, so the tunnel was
/// closed; `Ok(false)` means the held `ssh` still runs, so the tunnel is open and
/// the daemon behind it is silent. `said` is the last stderr line of the `ssh`
/// that exited. Pure: the ensure lives in `peer::tunnel`.
pub fn classify_tunnel(
    host: &str,
    ensured: std::result::Result<bool, String>,
    why: String,
    said: Option<String>,
) -> PeerStatus {
    let host = host.to_string();
    match ensured {
        Ok(true) => PeerStatus::TunnelClosed {
            host,
            cause: None,
            said,
        },
        Ok(false) => PeerStatus::TunnelSilent { host, why },
        Err(cause) => PeerStatus::TunnelClosed {
            host,
            cause: Some(cause),
            said,
        },
    }
}

/// The failed-dial path: ask the host about the peer's distro, then classify.
///
/// The question is asked ONLY here — after a dial has already failed and only for
/// a peer that announced a distro — so a fleet whose peers all answer never
/// spawns a process to find that out.
async fn diagnose_failed_dial(d: &PeerDescriptor, why: String) -> PeerStatus {
    // A tunnel peer is ensured here too (ADR-0067 §2: "after a probe fails"),
    // and the ensure's answer is what separates its two states.
    if let Some(spec) = d.tunnel.clone() {
        let host = if d.name.is_empty() {
            &d.environment
        } else {
            &d.name
        };
        let (ensured, said) = match super::tunnel::hold_open(d.daemon_id.clone(), spec).await {
            Ok((started, said)) => (Ok(started), said),
            Err(e) => (Err(format!("{e:#}")), None),
        };
        return classify_tunnel(host, ensured, why, said);
    }
    let Some(distro) = d.nudge.as_ref().map(|spec| spec.distro.clone()) else {
        return PeerStatus::Unreachable { why };
    };
    let asked = {
        let distro = distro.clone();
        tokio::task::spawn_blocking(move || super::nudge::is_distro_running(&distro)).await
    };
    let running = match asked {
        Ok(Ok(running)) => Some(running),
        // Both arms degrade to "unknown" rather than to a verdict: a host that
        // could not be asked is not a host that said "stopped". Logged, never
        // swallowed — this runs behind an already-failing probe, so a wrong
        // answer here would be invisible without it.
        Ok(Err(e)) => {
            tracing::debug!(%distro, error = %format!("{e:#}"), "could not read distro liveness");
            None
        }
        Err(e) => {
            tracing::debug!(%distro, error = %e, "the distro liveness query did not complete");
            None
        }
    };
    classify_unreachable(Some(&distro), running, why)
}

/// The sentence for a request to `d` that failed before the peer answered.
/// It is the diagnosis a failed handshake gets, so a tunnel whose daemon is
/// silent or a distro that sleeps is named as such, the same way `/api/fleet`
/// names it.
pub(crate) async fn transport_failed(d: &PeerDescriptor, why: String) -> String {
    diagnose_failed_dial(d, why).await.diagnosis(&d.environment)
}

/// How long an idle pooled connection is kept for the next request. Comfortably
/// longer than the tree poll's 25 s window, so a poller reuses ONE connection
/// instead of leaving a four-minute `TIME_WAIT` behind every cycle.
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// Idle connections kept per peer. Enough for the concurrent shapes this daemon
/// actually has — a tree poller, a runs poller, a fleet probe and a couple of
/// reads — without holding sockets a peer has to remember.
const POOL_MAX_IDLE_PER_HOST: usize = 8;

/// The ONE pooled HTTP client behind every peer dial.
///
/// It exists because the alternative was measured: a fresh `TcpStream` and HTTP
/// handshake per request meant every `tree.list`, every probe and every poll cost
/// the DIALLING host an ephemeral port held in `TIME_WAIT` for four minutes. On
/// Windows that pool is 16 384 ports wide and is rejected by 4-tuple, so a live
/// workbench against a WSL peer drained it against that one peer and every later
/// `connect` failed with "address already in use" — the file tree went blank
/// while the peer itself was healthy and answering in 3 ms (2026-09-01).
///
/// Process-wide rather than threaded through the call sites: there is exactly one
/// peer transport per daemon (ADR-0052 §2 — "one dependency, at one seam"), and
/// hyper keys its connections by authority, so the pool IS the seam rather than a
/// dependency of it. Threading it through would put a parameter on fifteen call
/// sites to express a singleton.
fn pool() -> &'static hyper_util::client::legacy::Client<
    hyper_util::client::legacy::connect::HttpConnector,
    Full<Bytes>,
> {
    static POOL: std::sync::OnceLock<
        hyper_util::client::legacy::Client<
            hyper_util::client::legacy::connect::HttpConnector,
            Full<Bytes>,
        >,
    > = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let mut connector = hyper_util::client::legacy::connect::HttpConnector::new();
        // The connect bound that used to be an explicit `timeout` around
        // `TcpStream::connect`. Same promise, now enforced where the pool dials.
        connector.set_connect_timeout(Some(PEER_CONNECT_TIMEOUT));
        connector.set_nodelay(true);
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .pool_idle_timeout(POOL_IDLE_TIMEOUT)
            .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
            .build(connector)
    })
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
/// Connection establishment remains bounded by [`PEER_CONNECT_TIMEOUT`].
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
    me: SelfRef<'_>,
) -> std::result::Result<PeerSocket, SocketError> {
    websocket(d, &format!("/ws/session?{query}"), "session", me).await
}

/// Probe the peer protocol, then open its authenticated command socket.
pub async fn command(
    d: &PeerDescriptor,
    me: SelfRef<'_>,
) -> std::result::Result<PeerSocket, SocketError> {
    websocket(d, "/ws/command", "command", me).await
}

async fn websocket(
    d: &PeerDescriptor,
    path: &str,
    purpose: &str,
    me: SelfRef<'_>,
) -> std::result::Result<PeerSocket, SocketError> {
    let status = probe(d, me).await;
    if status != PeerStatus::Reachable {
        return Err(SocketError::Peer(status));
    }
    if let Some(refused) = classify_address(&d.address) {
        return Err(SocketError::Peer(refused));
    }

    let authority = format!("{}:{}", d.address, d.port);
    let stream = tokio::time::timeout(
        PEER_CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect((d.address.as_str(), d.port)),
    )
    .await
    .map_err(|_| {
        SocketError::Peer(PeerStatus::Unreachable {
            why: format!("connecting to {authority} timed out"),
        })
    })?
    .map_err(|e| {
        SocketError::Peer(PeerStatus::Unreachable {
            why: format!("connecting to {authority}: {e}"),
        })
    })?;

    let uri = format!("ws://{authority}{path}");
    let mut request = uri.into_client_request().map_err(|e| {
        SocketError::Peer(PeerStatus::Unreachable {
            why: format!("building the {purpose} request: {e}"),
        })
    })?;
    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {}", d.token).parse().map_err(|e| {
            SocketError::Peer(PeerStatus::Unreachable {
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
        SocketError::Peer(PeerStatus::Unreachable {
            why: format!("{purpose} handshake with {authority} timed out"),
        })
    })?
    .map_err(|error| match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => SocketError::Http {
            status: response.status().as_u16(),
            body: response
                .body()
                .as_deref()
                .map(|body| String::from_utf8_lossy(body).into_owned())
                .unwrap_or_default(),
        },
        error => SocketError::Peer(PeerStatus::Unreachable {
            why: format!("opening the peer {purpose} at {authority}: {error}"),
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
    let bytes = match body {
        Some(body) => serde_json::to_vec(body).context("encoding the peer request body")?,
        None => Vec::new(),
    };
    // ABSOLUTE uri, unlike the origin-form a raw connection took: the pool picks
    // (and reuses) the connection from the authority, so the authority has to be
    // in the request. `Host` follows from it and is no longer set by hand.
    let mut builder = Request::builder()
        .method(method)
        .uri(format!("http://{authority}{path}"))
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

    let resp = tokio::time::timeout(response_timeout, pool().request(req))
        .await
        .with_context(|| format!("request to {authority}{path} timed out"))?
        // A peer that is not there is the common case, and it deserves the plain
        // sentence the operator sees in the fleet panel — "connecting to X" — not
        // "requesting X/api/…: client error (Connect)". The pool knows which half
        // failed, so keep saying so.
        .map_err(|e| {
            let what = if e.is_connect() {
                format!("connecting to {authority}")
            } else {
                format!("requesting {authority}{path}")
            };
            anyhow::Error::new(e).context(what)
        })?;
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
pub async fn probe(d: &PeerDescriptor, me: SelfRef<'_>) -> PeerStatus {
    probe_hello(d, me).await.0
}

/// [`probe`], plus the environment label the peer gave in its handshake when
/// it is reachable. `None` when the peer is not reachable or sent no label.
pub async fn probe_hello(d: &PeerDescriptor, me: SelfRef<'_>) -> (PeerStatus, Option<String>) {
    let mut environment = None;
    let status = handshake(d, me, &mut environment).await;
    if status != PeerStatus::Reachable {
        environment = None;
    }
    (status, environment)
}

async fn handshake(
    d: &PeerDescriptor,
    me: SelfRef<'_>,
    environment: &mut Option<String>,
) -> PeerStatus {
    if let Some(refused) = classify_address(&d.address) {
        return refused;
    }
    if let Some(refused) = classify_self_dial(&d.address, d.port, me) {
        return refused;
    }
    let (status, body) = match get(d, "/api/peer/hello").await {
        Ok(pair) => pair,
        Err(e) => return diagnose_failed_dial(d, format!("{e:#}")).await,
    };
    if status == 401 || status == 403 {
        return PeerStatus::Unauthorized {
            tunnel: d.tunnel.is_some(),
        };
    }
    if status != 200 {
        return PeerStatus::Unreachable {
            why: format!("peer answered HTTP {status} to the handshake"),
        };
    }
    let hello = serde_json::from_slice::<serde_json::Value>(&body).ok();
    *environment = hello
        .as_ref()
        .and_then(|v| v.get("environment").and_then(|e| e.as_str()))
        .filter(|e| !e.is_empty())
        .map(str::to_string);
    // Belt and braces for the gate above: a handshake that answers with THIS
    // daemon's own id came back to us by some route the port check did not
    // model, and treating it as a peer would federate this daemon with itself.
    // Skipped when un-baptized, where the empty id is not an identity.
    let answered_id = hello
        .as_ref()
        .and_then(|v| v.get("daemon_id").and_then(|id| id.as_str()));
    if !me.daemon_id.is_empty() && answered_id == Some(me.daemon_id) {
        return PeerStatus::Refused {
            why: format!(
                "the handshake at {}:{} answered with this daemon's own identity — \
                 the connection loops back here rather than reaching another environment",
                d.address, d.port
            ),
        };
    }
    // ADR-0067 amendment M5: several daemons can share one host, so a port can
    // answer with another daemon than the one this descriptor names. Skipped
    // when either side has no id.
    if let Some(answered) = answered_id {
        if !d.daemon_id.is_empty() && answered != d.daemon_id {
            // Only `host add` writes a tunnel descriptor, and running it again
            // reads the identity and the socket anew.
            let remedy = if d.tunnel.is_some() {
                ". Add the host again"
            } else {
                ""
            };
            return PeerStatus::Refused {
                why: format!(
                    "another daemon answers at {}:{}: it is {answered}, not {}{remedy}",
                    d.address, d.port, d.daemon_id
                ),
            };
        }
    }
    let theirs = hello
        .as_ref()
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
