//! The request facts the daemon reads itself, and the header text rules they share.

use axum::http::HeaderMap;
use serde::Serialize;

use super::MAX_HEADER_CHARS;

/// The request facts the daemon read itself (source `server`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct ServerFacts {
    /// The client's public address, as the front reported it in `X-Real-IP`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub real_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarded_for: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarded_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarded_proto: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accept_language: Option<String>,
    /// Every `Sec-CH-UA*` header, by name without the prefix.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub client_hints: Vec<(String, String)>,
    /// The address at the other end of the TCP connection. Through the dev
    /// tunnel it is the tunnel's loopback leg; on a direct connection it is
    /// the client. None on the Unix socket.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
}

impl ServerFacts {
    pub(crate) fn from_headers(headers: &HeaderMap) -> ServerFacts {
        let read = |name: &str| header_text(headers, name);
        let mut client_hints: Vec<(String, String)> = headers
            .iter()
            .filter_map(|(name, value)| {
                let hint = name.as_str().strip_prefix("sec-ch-ua")?;
                let hint = hint.strip_prefix('-').unwrap_or(hint);
                let value = clip(value.to_str().ok()?);
                Some((
                    if hint.is_empty() { "brands" } else { hint }.to_owned(),
                    value,
                ))
            })
            .collect();
        client_hints.sort();
        ServerFacts {
            real_ip: read("x-real-ip"),
            forwarded_for: read("x-forwarded-for"),
            forwarded_host: read("x-forwarded-host"),
            forwarded_proto: read("x-forwarded-proto"),
            user_agent: read("user-agent"),
            accept_language: read("accept-language"),
            client_hints,
            peer: None,
        }
    }

    /// The client's address: the front's `X-Real-IP`, else the TCP peer.
    pub(crate) fn address(&self) -> Option<String> {
        self.real_ip.clone().or_else(|| self.peer.clone())
    }
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(clip)
        .filter(|v| !v.is_empty())
}

/// `text` trimmed and cut to the longest value a line keeps.
pub(crate) fn clip(text: &str) -> String {
    text.trim().chars().take(MAX_HEADER_CHARS).collect()
}
