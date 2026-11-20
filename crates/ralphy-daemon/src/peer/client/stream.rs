//! A streamed GET from a peer (ADR-0091 D6): the body is passed on as it
//! arrives, never collected, so a long video is not held in memory.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use axum::body::{Body, Bytes, HttpBody};
use axum::http::{header, HeaderMap, Request, Response};
use futures_util::Stream;
use http_body_util::{BodyExt, Full};

use super::{classify_address, pool};
use crate::peer::PeerDescriptor;

/// The deadlines of a streamed answer. There is no total deadline: a video
/// may stream for an hour, and only a silent peer is cut.
#[derive(Debug, Clone, Copy)]
pub struct StreamDeadlines {
    /// How long the peer has to send the head of its answer.
    pub first_byte: Duration,
    /// How long the peer may send nothing once the body has started.
    pub silence: Duration,
}

/// The deadlines of the file bytes route.
pub const STREAM_DEADLINES: StreamDeadlines = StreamDeadlines {
    first_byte: Duration::from_secs(10),
    silence: Duration::from_secs(30),
};

/// `GET <path>` from `d` with `headers` added, the body passed on in chunks.
pub async fn get_stream(
    d: &PeerDescriptor,
    path: &str,
    headers: HeaderMap,
    deadlines: StreamDeadlines,
) -> Result<Response<Body>> {
    if let Some(refused) = classify_address(&d.address) {
        bail!("{}", refused.diagnosis(&d.environment));
    }
    let authority = format!("{}:{}", d.address, d.port);
    let mut req = Request::builder()
        .method("GET")
        .uri(format!("http://{authority}{path}"))
        .header(header::AUTHORIZATION, format!("Bearer {}", d.token))
        .body(Full::new(Bytes::new()))
        .context("building the peer request")?;
    req.headers_mut().extend(headers);
    let resp = tokio::time::timeout(deadlines.first_byte, pool().request(req))
        .await
        .with_context(|| format!("request to {authority}{path} timed out"))?
        .with_context(|| format!("requesting {authority}{path}"))?;
    let (parts, body) = resp.into_parts();
    let body = Body::from_stream(chunks(body, deadlines.silence));
    Ok(Response::from_parts(parts, body))
}

/// The data frames of `body`, ended by an error when `silence` passes with
/// no frame.
fn chunks<B>(body: B, silence: Duration) -> impl Stream<Item = std::io::Result<Bytes>>
where
    B: HttpBody<Data = Bytes> + Unpin + Send + 'static,
    B::Error: std::fmt::Display,
{
    futures_util::stream::unfold(Some(body), move |state| async move {
        let mut body = state?;
        match next_data(&mut body, silence).await? {
            Ok(data) => Some((Ok(data), Some(body))),
            Err(e) => Some((Err(e), None)),
        }
    })
}

/// The next data frame of `body`, skipping trailers; `None` at its end.
async fn next_data<B>(body: &mut B, silence: Duration) -> Option<std::io::Result<Bytes>>
where
    B: HttpBody<Data = Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    loop {
        let frame = match tokio::time::timeout(silence, body.frame()).await {
            Err(_) => return Some(Err(silent(silence))),
            Ok(None) => return None,
            Ok(Some(Err(e))) => return Some(Err(std::io::Error::other(e.to_string()))),
            Ok(Some(Ok(frame))) => frame,
        };
        if let Ok(data) = frame.into_data() {
            return Some(Ok(data));
        }
    }
}

fn silent(silence: Duration) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!("the peer sent no byte for {} ms", silence.as_millis()),
    )
}
