// The Spend tab's model, folded from the daemon's `/api/spend` summary document
// (PRD #355, tracer bullet #358). Pure: no DOM, no fetch — the fetch and the
// rendering live in app.js/index.html, exactly as `wb-changes.js` splits them.
//
// This module deliberately FORMATS NOTHING numeric. The total (`$2,350.59+`),
// the `token_meter` (`↑12.4k ⚡184k ❄8.1k ↓3.2k`) and every unpriced volume
// arrive already rendered by the daemon, so the `k`/`M` abbreviation and the
// money vocabulary have one implementation instead of one per client. What is
// folded here is which STATE the pane is in and which unpriced causes are worth
// showing — decisions, not typography.
(function (window) {
  "use strict";

  // The four states the pane can be in, named so the markup branches on a word
  // instead of on a combination of falsy fields.
  const EMPTY = "empty"; // no project open — the operator is told to open one
  const LOADING = "loading";
  const ERROR = "error";
  const READY = "ready";

  // The three causes an unpriced token can have, in the order the operator can
  // act on them: `recoverable` shrinks by running model recovery, `no_price` by
  // adding one line to `pricing.toml`, and `lost` never shrinks at all
  // (ADR-0053 D4 — the surface says *lost*, not *pending*).
  const CAUSES = [
    {
      key: "recoverable",
      label: "recoverable",
      hint: "the line carries a session id, so model recovery can still name its engine",
    },
    {
      key: "no_price",
      label: "no price",
      hint: "a real model the price table does not know — add it to pricing.toml",
    },
    {
      key: "lost",
      label: "lost",
      hint: "the line carries no session id, so there is no key to any store — this never comes back",
    },
  ];

  // The unpriced volume as rows, dropping every cause that is zero: a bucket
  // with nothing in it is not a finding, and three permanent zeroes would teach
  // the operator to stop reading the element that exists to be read.
  function causes(unpriced) {
    if (!unpriced) return [];
    return CAUSES.filter((c) => (unpriced[c.key] || 0) > 0).map((c) => ({
      ...c,
      // The daemon's pre-rendered volume for this cause (`500.0k`).
      value: unpriced[c.key + "_label"] || "",
    }));
  }

  // The unpriced share as a percentage string, or "" when nothing is unpriced.
  // One decimal: the figure this exists to shame (44%) and the figure it should
  // reach (0.2%) must both read, and an integer percent collapses the second.
  function sharePct(unpriced) {
    if (!unpriced || !(unpriced.tokens > 0)) return "";
    return (unpriced.share * 100).toFixed(1) + "%";
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
        message: "No project open.",
        hint: "Open a project in the sidebar to see what it cost.",
      };
    }
    if (error) return { kind: ERROR, project, message: error };
    if (loading || !doc) return { kind: LOADING, project };
    const unpriced = doc.unpriced || {};
    return {
      kind: READY,
      project,
      // Pre-rendered by the daemon — never recomputed here.
      total: doc.total || "~$?",
      // A floor is a claim about the number, so it is stated beside it rather
      // than left to the reader to infer from the trailing `+`.
      floor: !!doc.floor,
      meter: (doc.tokens && doc.tokens.meter) || "",
      tokens: (doc.tokens && doc.tokens.total) || 0,
      unpriced: {
        any: (unpriced.tokens || 0) > 0 || (unpriced.unmetered_sessions || 0) > 0,
        label: unpriced.label || "",
        share: sharePct(unpriced),
        causes: causes(unpriced),
        // Sessions whose vendor keeps no token count anywhere: real spend of
        // unmeasurable size, so it is reported as a count of sessions rather
        // than folded into a token figure it has no tokens for.
        unmetered: unpriced.unmetered_sessions || 0,
      },
    };
  }

  window.WBSpend = { EMPTY, LOADING, ERROR, READY, CAUSES, causes, sharePct, state };
})(window);
