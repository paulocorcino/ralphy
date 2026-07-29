// The federated sidebar's grouping fold (ADR-0052 §5, issue #349).
//
// PURE: rows + peers in, groups out. No DOM, no fetch, no Alpine — so the one
// rule that matters (the same `owner/repo` on two daemons is two rows in two
// groups, never one row that overwrote the other) is testable in node.
//
// Named `WBFleet`/`fleetGroups` on purpose: `peer` is already taken in this UI
// by the detached-fence link (`peerFold`, `peer-lost`), which is a different
// thing entirely.
(function (window) {
  "use strict";

  // A row this daemon owns. Peer rows carry `daemon` (the peer's daemon_id);
  // local rows never do, which is what makes "local" a property of the data
  // rather than a flag someone has to remember to set.
  function isLocal(row) {
    return !row.daemon;
  }

  function groupKey(row) {
    return isLocal(row) ? "local" : row.daemon;
  }

  function repoRef(row) {
    return row.key || row.slug;
  }

  function isPeerRef(ref) {
    const head = String(ref || "").split("/")[0];
    return /^[0-9A-HJKMNP-TV-Z]{26}$/.test(head);
  }

  // Group `repos` by the daemon that owns them, attaching each peer's name and
  // state from `peers` (the `/api/fleet` peer list). A peer that contributed no
  // rows still gets a group: an unreachable environment must be visible as an
  // environment, not vanish because it could not be asked for repos.
  //
  // Order: local first, then peers by environment then name — stable, so the
  // sidebar does not reshuffle between polls.
  function fleetGroups(repos, peers) {
    const rows = Array.isArray(repos) ? repos : [];
    const peerList = Array.isArray(peers) ? peers : [];

    const groups = new Map();
    const ensure = (key, seed) => {
      if (!groups.has(key)) groups.set(key, Object.assign({ key: key, rows: [] }, seed));
      return groups.get(key);
    };

    for (const row of rows) {
      const key = groupKey(row);
      const g = ensure(key, {
        environment: row.env || "",
        daemon: isLocal(row) ? "" : row.daemon,
        // A local group has no entry in the peer list below, so its name has to
        // come off the row — which `loadFleet` stamps from `/api/fleet`.
        name: isLocal(row) ? row.daemonName || "" : "",
        state: isLocal(row) ? "local" : row.peerState || "",
        diagnosis: "",
        nudgeable: false,
        local: isLocal(row),
      });
      g.rows.push(row);
    }

    for (const p of peerList) {
      const g = ensure(p.daemon_id, {
        environment: p.environment || "",
        daemon: p.daemon_id,
        name: "",
        state: "",
        diagnosis: "",
        nudgeable: false,
        local: false,
      });
      // The peer list is authoritative about the peer; the rows only carry what
      // a row needs. A row-derived state is a fallback, never an override.
      g.environment = p.environment || g.environment;
      g.name = p.name || "";
      g.state = p.state || g.state;
      g.diagnosis = p.diagnosis || "";
      g.nudgeable = !!p.nudgeable;
    }

    const out = Array.from(groups.values());
    out.sort((a, b) => {
      if (a.local !== b.local) return a.local ? -1 : 1;
      const byEnv = String(a.environment).localeCompare(String(b.environment));
      if (byEnv !== 0) return byEnv;
      return String(a.daemon).localeCompare(String(b.daemon));
    });
    // A fleet of one needs no environment headers — they would only name the
    // machine the operator is already sitting at.
    const header = out.length > 1;
    for (const g of out) g.header = header;
    return out;
  }

  window.WBFleet = {
    fleetGroups: fleetGroups,
    repoRef: repoRef,
    isPeerRef: isPeerRef,
  };
})(window);
