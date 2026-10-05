//! The traffic summary of one console socket: the bytes it sent, how many of
//! them were a scrollback replay, and the round trip of its pings. It is logged
//! once, when the socket closes, so that a session over a slow or metered link
//! can be measured.

use std::collections::VecDeque;
use std::time::Instant;

/// Which loop owns the socket: the bridge to a session this daemon hosts, or
/// the relay to a session a peer hosts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Channel {
    Local,
    Peer,
}

impl Channel {
    fn as_str(self) -> &'static str {
        match self {
            Channel::Local => "local",
            Channel::Peer => "peer",
        }
    }
}

/// The `end` of a socket that closed without a deliberate end: the client went
/// away, the link failed, or a send failed.
pub(crate) const DROPPED: &str = "dropped";

/// Pings waiting for their pong. A client answers each ping, so more than a
/// few waiting means the pongs are lost, and the oldest are dropped.
const OUTSTANDING_CAP: usize = 4;

/// Round-trip samples kept per socket: about 5 hours of 20 s pings.
const SAMPLE_CAP: usize = 1024;

struct Outstanding {
    payload: Vec<u8>,
    sent: Instant,
    busy: bool,
}

/// The keys that join a summary to other records: the session this daemon
/// hosts, or the peer that hosts it; and the browser tab (the holder).
struct Keys {
    session: Option<u64>,
    peer_id: Option<String>,
    peer: Option<String>,
    holder: Option<String>,
}

pub(crate) struct Traffic {
    channel: Channel,
    keys: Keys,
    started: Instant,
    replay_bytes: u64,
    live_bytes: u64,
    live_frames: u64,
    in_bytes: u64,
    lagged_events: u64,
    lagged_skipped: u64,
    live_since_ping: bool,
    outstanding: VecDeque<Outstanding>,
    busy_ms: Vec<u32>,
    idle_ms: Vec<u32>,
}

/// The numbers one log line carries. Round trips are in milliseconds; `None`
/// when the socket earned no sample of that kind.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Summary {
    pub(crate) replay_bytes: u64,
    pub(crate) live_bytes: u64,
    pub(crate) live_frames: u64,
    pub(crate) in_bytes: u64,
    pub(crate) lagged_events: u64,
    pub(crate) lagged_skipped: u64,
    pub(crate) rtt_samples: usize,
    pub(crate) rtt_min_ms: Option<u32>,
    pub(crate) rtt_p50_ms: Option<u32>,
    pub(crate) rtt_max_ms: Option<u32>,
    pub(crate) rtt_busy_p50_ms: Option<u32>,
    pub(crate) rtt_idle_p50_ms: Option<u32>,
}

impl Traffic {
    /// A socket bridged to session `session` of this daemon.
    pub(crate) fn local(session: u64, holder: Option<String>, now: Instant) -> Traffic {
        let keys = Keys {
            session: Some(session),
            peer_id: None,
            peer: None,
            holder,
        };
        Traffic::new(Channel::Local, keys, now)
    }

    /// A socket relayed to a session the peer `peer_id` hosts; `peer` is its
    /// environment label, for people.
    pub(crate) fn peer(
        peer_id: String,
        peer: String,
        holder: Option<String>,
        now: Instant,
    ) -> Traffic {
        let keys = Keys {
            session: None,
            peer_id: Some(peer_id),
            peer: Some(peer),
            holder,
        };
        Traffic::new(Channel::Peer, keys, now)
    }

    fn new(channel: Channel, keys: Keys, now: Instant) -> Traffic {
        Traffic {
            channel,
            keys,
            started: now,
            replay_bytes: 0,
            live_bytes: 0,
            live_frames: 0,
            in_bytes: 0,
            lagged_events: 0,
            lagged_skipped: 0,
            live_since_ping: false,
            outstanding: VecDeque::new(),
            busy_ms: Vec::new(),
            idle_ms: Vec::new(),
        }
    }

    pub(crate) fn replay(&mut self, bytes: usize) {
        self.replay_bytes += bytes as u64;
    }

    pub(crate) fn live(&mut self, bytes: usize) {
        self.live_bytes += bytes as u64;
        self.live_frames += 1;
        self.live_since_ping = true;
    }

    pub(crate) fn inbound(&mut self, bytes: usize) {
        self.in_bytes += bytes as u64;
    }

    /// A burst outran this socket and `skipped` messages never reached it.
    pub(crate) fn lagged(&mut self, skipped: u64) {
        self.lagged_events += 1;
        self.lagged_skipped += skipped;
    }

    /// A ping is "busy" when live output went out since the previous ping, so
    /// its round trip shares the link with that output.
    pub(crate) fn ping_sent(&mut self, payload: &[u8], now: Instant) {
        if self.outstanding.len() == OUTSTANDING_CAP {
            self.outstanding.pop_front();
        }
        self.outstanding.push_back(Outstanding {
            payload: payload.to_vec(),
            sent: now,
            busy: self.live_since_ping,
        });
        self.live_since_ping = false;
    }

    /// A pong echoes the payload of its ping (RFC 6455 §5.5.3); a pong that
    /// matches no waiting ping records nothing.
    pub(crate) fn pong_received(&mut self, payload: &[u8], now: Instant) {
        let Some(at) = self.outstanding.iter().position(|o| o.payload == payload) else {
            return;
        };
        let ping = self
            .outstanding
            .remove(at)
            .expect("`position` returned an index inside the queue");
        if self.busy_ms.len() + self.idle_ms.len() >= SAMPLE_CAP {
            return;
        }
        let ms = u32::try_from(now.duration_since(ping.sent).as_millis()).unwrap_or(u32::MAX);
        if ping.busy {
            self.busy_ms.push(ms);
        } else {
            self.idle_ms.push(ms);
        }
    }

    pub(crate) fn summary(&self) -> Summary {
        let mut all: Vec<u32> = self.busy_ms.iter().chain(&self.idle_ms).copied().collect();
        all.sort_unstable();
        Summary {
            replay_bytes: self.replay_bytes,
            live_bytes: self.live_bytes,
            live_frames: self.live_frames,
            in_bytes: self.in_bytes,
            lagged_events: self.lagged_events,
            lagged_skipped: self.lagged_skipped,
            rtt_samples: all.len(),
            rtt_min_ms: all.first().copied(),
            rtt_p50_ms: median(&all),
            rtt_max_ms: all.last().copied(),
            rtt_busy_p50_ms: median_of(&self.busy_ms),
            rtt_idle_p50_ms: median_of(&self.idle_ms),
        }
    }

    /// `end` is how the socket ended: a session end reason, or `dropped`.
    pub(crate) fn log(&self, end: &str, now: Instant) {
        let s = self.summary();
        tracing::info!(
            channel = self.channel.as_str(),
            session = self.keys.session,
            peer_id = self.keys.peer_id.as_deref(),
            peer = self.keys.peer.as_deref(),
            holder = self.keys.holder.as_deref(),
            end,
            secs = now.duration_since(self.started).as_secs(),
            replay_bytes = s.replay_bytes,
            live_bytes = s.live_bytes,
            live_frames = s.live_frames,
            in_bytes = s.in_bytes,
            lagged_events = s.lagged_events,
            lagged_skipped = s.lagged_skipped,
            rtt_samples = s.rtt_samples,
            rtt_min_ms = s.rtt_min_ms,
            rtt_p50_ms = s.rtt_p50_ms,
            rtt_max_ms = s.rtt_max_ms,
            rtt_busy_p50_ms = s.rtt_busy_p50_ms,
            rtt_idle_p50_ms = s.rtt_idle_p50_ms,
            "console socket traffic"
        );
    }
}

fn median_of(samples: &[u32]) -> Option<u32> {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    median(&sorted)
}

/// Nearest-rank median of sorted samples: the lower middle one for an even count.
fn median(sorted: &[u32]) -> Option<u32> {
    if sorted.is_empty() {
        return None;
    }
    Some(sorted[(sorted.len() - 1) / 2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn traffic(now: Instant) -> Traffic {
        Traffic::local(1, Some("tab-1".to_string()), now)
    }

    #[test]
    fn a_pong_is_timed_against_the_ping_with_its_payload() {
        let t0 = Instant::now();
        let mut t = traffic(t0);
        t.ping_sent(&[1], t0);
        t.ping_sent(&[2], t0 + Duration::from_millis(100));
        t.pong_received(&[9], t0 + Duration::from_millis(150));
        t.pong_received(&[2], t0 + Duration::from_millis(130));
        let s = t.summary();
        assert_eq!(s.rtt_samples, 1, "an unknown payload records no sample");
        assert_eq!(s.rtt_p50_ms, Some(30));
    }

    #[test]
    fn replay_and_live_bytes_are_kept_apart() {
        let mut t = traffic(Instant::now());
        t.replay(1000);
        t.live(10);
        t.live(5);
        t.inbound(3);
        let s = t.summary();
        assert_eq!((s.replay_bytes, s.live_bytes, s.live_frames), (1000, 15, 2));
        assert_eq!(s.in_bytes, 3);
    }

    #[test]
    fn a_ping_after_live_output_is_busy_and_a_quiet_one_is_idle() {
        let t0 = Instant::now();
        let mut t = traffic(t0);
        t.ping_sent(&[1], t0);
        t.pong_received(&[1], t0 + Duration::from_millis(40));
        t.live(10);
        t.ping_sent(&[2], t0);
        t.pong_received(&[2], t0 + Duration::from_millis(400));
        let s = t.summary();
        assert_eq!(s.rtt_idle_p50_ms, Some(40));
        assert_eq!(s.rtt_busy_p50_ms, Some(400));
    }

    #[test]
    fn the_median_is_the_lower_middle_sample() {
        assert_eq!(median(&[]), None);
        assert_eq!(median_of(&[30, 10, 20]), Some(20));
        assert_eq!(median_of(&[40, 10, 30, 20]), Some(20));
    }

    #[test]
    fn lost_pongs_do_not_grow_the_waiting_queue() {
        let t0 = Instant::now();
        let mut t = traffic(t0);
        for seq in 0..=OUTSTANDING_CAP as u8 {
            t.ping_sent(&[seq], t0);
        }
        assert_eq!(t.outstanding.len(), OUTSTANDING_CAP);
        t.pong_received(&[0], t0 + Duration::from_millis(10));
        assert_eq!(t.summary().rtt_samples, 0, "the oldest ping was dropped");
    }

    #[test]
    fn the_keys_name_the_session_or_the_peer_and_the_tab() {
        let now = Instant::now();
        let local = Traffic::local(7, Some("tab-a".to_string()), now);
        assert_eq!(local.channel, Channel::Local);
        assert_eq!(local.keys.session, Some(7));
        assert_eq!(local.keys.holder.as_deref(), Some("tab-a"));
        let relayed = Traffic::peer("01PEER".to_string(), "Ubuntu".to_string(), None, now);
        assert_eq!(relayed.channel, Channel::Peer);
        assert_eq!(relayed.keys.peer_id.as_deref(), Some("01PEER"));
        assert_eq!(relayed.keys.peer.as_deref(), Some("Ubuntu"));
        assert_eq!(relayed.keys.session, None);
    }

    #[test]
    fn a_burst_that_outran_the_socket_is_counted() {
        let mut t = traffic(Instant::now());
        t.lagged(7);
        t.lagged(3);
        let s = t.summary();
        assert_eq!((s.lagged_events, s.lagged_skipped), (2, 10));
    }
}
