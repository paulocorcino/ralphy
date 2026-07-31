// The Spend tab's model, folded from the daemon's `/api/spend` summary document
// (PRD #355, tracer bullet #358). Pure: no DOM, no fetch — the fetch and the
// rendering live in app.js/index.html, exactly as `wb-changes.js` splits them.
//
// This module COMPUTES NOTHING NUMERIC. Every figure on screen — the total
// (`$2,350.59+`), the `token_meter` (`↑12.4k ⚡184k ❄8.1k ↓3.2k`), each volume
// and each percentage — arrives already rendered by the daemon, so the `k`/`M`
// abbreviation and the money vocabulary have one implementation instead of one
// per client. What lives here is which STATE the pane is in and the explanatory
// COPY for each unpriced cause: decisions and words, never arithmetic.
(function (window) {
  "use strict";

  // The four states the pane can be in, named so the markup branches on a word
  // instead of on a combination of falsy fields.
  const EMPTY = "empty"; // no project open — the operator is told to open one
  const LOADING = "loading";
  const ERROR = "error";
  const READY = "ready";

  // What each unpriced cause MEANS, keyed by the daemon's closed vocabulary. The
  // split exists because one bucket shrinks with work and another never will
  // (ADR-0053 D4) — so each row states which it is, rather than leaving three
  // numbers to be told apart by name alone.
  const CAUSE_COPY = {
    recoverable: {
      title: "recoverable",
      hint: "the line recorded a session id — model recovery can still name its engine, until that vendor store is pruned",
    },
    no_price: {
      title: "no price",
      hint: "a real model the price table does not know — one line in pricing.toml closes it",
    },
    lost: {
      title: "lost",
      hint: "the line recorded no session id, so there is no key to any store — no amount of work brings this back",
    },
  };

  // A daemon cause row plus its copy. An unknown key (a future fourth cause)
  // renders under its own name rather than vanishing — the gap must never get
  // quieter than it is.
  function causes(unpriced) {
    const rows = (unpriced && unpriced.causes) || [];
    return rows.map((c) => ({
      key: c.key,
      title: (CAUSE_COPY[c.key] || {}).title || c.key,
      hint: (CAUSE_COPY[c.key] || {}).hint || "",
      value: c.label,
      share: c.share_label,
      // The daemon's `0.0..=1.0`, as the CSS width it is drawn at. This is a
      // unit conversion for a bar, not a figure the operator reads — every
      // number they READ is a string the daemon rendered.
      width: pct(c.share),
    }));
  }

  function pct(share) {
    return Math.max(0, Math.min(100, (share || 0) * 100)) + "%";
  }

  // The token split as bar rows, in the daemon's canonical order. Each row's bar
  // is drawn against the LARGEST part, not against the total: with one kind at
  // 90% the other three collapse to invisible slivers, and "how do these four
  // compare" is the question the rows exist to answer.
  function meterRows(tokens) {
    const parts = (tokens && tokens.parts) || [];
    const peak = parts.reduce((m, p) => Math.max(m, p.tokens || 0), 0);
    return parts.map((p) => ({
      key: p.key,
      glyph: p.glyph,
      name: p.name,
      value: p.label,
      share: p.share_label,
      width: peak > 0 ? pct((p.tokens || 0) / peak) : "0%",
      // A kind with nothing in it is dimmed, never dropped: an absent `⚡` would
      // read as "there is no cache column" rather than "nothing was reused".
      empty: !(p.tokens > 0),
    }));
  }

  // The whole pane, from the three things app.js knows: the open project, the
  // in-flight/failed state of the fetch, and the document the daemon returned.
  //
  // `project` is checked FIRST and on its own: with no project open there is
  // nothing to fetch, so "empty" is a fact about the workbench, never a verdict
  // on a request that was never made.
  function state({ project, loading, error, doc } = {}) {
    if (!project) {
      return {
        kind: EMPTY,
        message: "No project open",
        hint: "Open a project in the sidebar to see what it cost.",
      };
    }
    if (error) return { kind: ERROR, project, message: error };
    if (loading || !doc) return { kind: LOADING, project };
    const unpriced = doc.unpriced || {};
    const tokens = doc.tokens || {};
    return {
      kind: READY,
      project,
      // Pre-rendered by the daemon — never recomputed here.
      total: doc.total || "~$?",
      // A floor is a claim about the number, so it is stated beside it rather
      // than left to the reader to infer from the trailing `+`.
      floor: !!doc.floor,
      // The caveat names the volume that made it a floor, because "some of it"
      // is not something the operator can act on.
      floorNote: floorNote(doc, unpriced),
      meter: tokens.meter || "",
      tokensTotal: tokens.label || "0",
      tokensRaw: tokens.total || 0,
      meterRows: meterRows(tokens),
      coverage: {
        // Only worth drawing once something is missing: a full-width bar at
        // 100% priced is a decoration that says what the absent floor marker
        // already said.
        show: (unpriced.tokens || 0) > 0,
        priced: pct(unpriced.priced_share),
        pricedLabel: unpriced.priced_share_label || "",
        pricedVolume: unpriced.priced_label || "",
        unpriced: pct(unpriced.share),
        unpricedLabel: unpriced.share_label || "",
      },
      unpriced: {
        any: (unpriced.tokens || 0) > 0 || (unpriced.unmetered_sessions || 0) > 0,
        label: unpriced.label || "",
        share: unpriced.share_label || "",
        causes: causes(unpriced),
        // Sessions whose vendor keeps no token count anywhere: real spend of
        // unmeasurable size, so it is reported as a count of sessions rather
        // than folded into a token figure it has no tokens for.
        unmetered: unpriced.unmetered_sessions || 0,
      },
    };
  }

  // Why the figure is a floor, in words. Two distinct reasons can raise the flag
  // and they call for different sentences: volume that could not be PRICED, and
  // sessions that were never COUNTED (`tokens: null`, ADR-0042 D11) — the second
  // leaves no tokens to name, so a note about "unpriced volume" would be false.
  function floorNote(doc, unpriced) {
    if (!doc.floor) return "";
    const volume = unpriced.tokens || 0;
    const sessions = unpriced.unmetered_sessions || 0;
    if (volume > 0) {
      return (
        "a floor — " +
        unpriced.label +
        " tokens (" +
        unpriced.share_label +
        ") could not be priced" +
        (sessions > 0 ? ", and some sessions were never counted" : "")
      );
    }
    if (sessions > 0) return "a floor — some sessions carry no token count at all";
    return "a floor — some of this spend is a lower bound";
  }

  window.WBSpend = {
    EMPTY,
    LOADING,
    ERROR,
    READY,
    CAUSE_COPY,
    causes,
    meterRows,
    floorNote,
    state,
  };
})(window);
