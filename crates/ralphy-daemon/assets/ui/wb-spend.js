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

  // The five tiles, in the order PRD #355 fixes them. Every `value` is a string
  // the daemon rendered; the only thing decided here is which label sits above
  // it and which note sits below.
  function tiles(doc) {
    const k = doc.kpis || {};
    return [
      {
        key: "total",
        label: "total cost",
        value: doc.total || "~$?",
        note: "",
        primary: true,
        floor: !!doc.floor,
      },
      {
        key: "deliveries",
        label: "deliveries",
        // The ONE figure on this page that is a count rather than money, so it
        // is the one place a client-side `String()` is not an arithmetic.
        value: String(k.deliveries || 0),
        note: "issues this window's spend touched",
        floor: false,
      },
      {
        key: "cost_per_delivery",
        label: "cost per delivery",
        value: k.cost_per_delivery_median_label || "~$?",
        // The mean rides in the note, not in a sixth tile: the pair is one
        // reading — the typical issue, and how far the tail pulls the average.
        note: "median · mean " + (k.cost_per_delivery_mean_label || "~$?"),
        floor: !!k.cost_per_delivery_floor,
      },
      {
        key: "retry_burn",
        label: "retry burn",
        value: k.retry_burn_label || "—",
        note: "of ledger spend bought no delivery",
        floor: !!k.retry_burn_floor,
      },
      {
        key: "cache_hit",
        label: "cache hit",
        value: k.cache_hit_label || "—",
        note: "of prompt tokens served from cache",
        floor: false,
      },
    ];
  }

  // The deliveries grid. `issues` is whatever the board already holds — the
  // title is an ADORNMENT, so a cold board renders `#251` with no title and
  // NOTHING here reaches for one: the board fold spawns a CLI that makes tracker
  // calls, and a cost page must never pay it.
  function deliveryRows(doc, issues) {
    const titles = new Map();
    for (const i of issues || []) {
      if (i && i.number != null) titles.set(i.number, i.title || "");
    }
    const peak = (doc.deliveries || []).reduce((m, d) => Math.max(m, d.share || 0), 0);
    return (doc.deliveries || []).map((d) => ({
      issue: d.issue,
      label: "#" + d.issue,
      title: titles.get(d.issue) || "",
      value: d.total,
      floor: !!d.floor,
      attempts: d.attempts || 0,
      tokens: d.tokens_label || "",
      share: d.share_label || "",
      // Against the costliest row, so the smaller rows stay comparable instead
      // of collapsing into slivers — the same rule the meter rows use.
      width: peak > 0 ? pct((d.share || 0) / peak) : "0%",
    }));
  }

  // The models grid. A row that priced to nothing is styled as a GAP, not as a
  // cheap engine — the daemon already says which by carrying `priced`.
  function modelRows(doc) {
    const rows = doc.models || [];
    const peak = rows.reduce((m, r) => Math.max(m, r.share || 0), 0);
    return rows.map((r) => ({
      key: r.model,
      model: r.model,
      value: r.total,
      floor: !!r.floor,
      share: r.share_label || "",
      tokens: r.tokens_label || "",
      priced: !!r.priced,
      width: peak > 0 ? pct((r.share || 0) / peak) : "0%",
    }));
  }

  // The three lines PRD #355 sums into the total, each with the words that say
  // what it is. They render BESIDE the delivery rows, never among them: an
  // overhead line inside the grid would read as an issue that cost that much.
  function overheadLines(doc) {
    const o = doc.overhead || {};
    const sessions = o.interactive_sessions || 0;
    return [
      {
        key: "deliveries",
        label: "deliveries",
        value: o.deliveries_total || "~$?",
        floor: !!o.deliveries_floor,
        note: "",
      },
      {
        key: "interactive",
        label: "interactive",
        value: o.interactive_total || "~$?",
        floor: !!o.interactive_floor,
        note: sessions === 1 ? "1 session" : sessions + " sessions",
      },
      {
        key: "consolidation",
        label: "consolidation",
        value: o.consolidation_total || "~$?",
        floor: !!o.consolidation_floor,
        note: "run-level, no single issue",
      },
    ];
  }

  // The activity band: one column per day, two bars in it. Both heights are the
  // daemon's own share of its peak day — the two series have no common unit, so
  // each gets its own baseline rather than a shared axis that would lie.
  function band(doc) {
    const days = doc.activity || [];
    return {
      show: days.length > 0,
      days: days.map((d) => ({
        key: d.date,
        date: d.date,
        // `2026-07-30` → `07-30`: the year is the same on every column of a
        // 90-day window and costs a third of the label's width to repeat.
        short: d.date.length >= 10 ? d.date.slice(5) : d.date,
        value: d.usd_label || "~$?",
        usdHeight: pct(d.usd_share),
        deliveries: d.deliveries || 0,
        deliveriesHeight: pct(d.deliveries_share),
        quiet: !(d.usd > 0) && !(d.deliveries > 0),
      })),
    };
  }

  // The period control: the daemon's own vocabulary, so a key the client offers
  // is always a key the route accepts.
  const PERIODS = [
    { key: "all", label: "all time" },
    { key: "7d", label: "last 7 days" },
    { key: "30d", label: "last 30 days" },
    { key: "90d", label: "last 90 days" },
  ];

  // The whole pane, from the four things app.js knows: the open project, the
  // in-flight/failed state of the fetch, the document the daemon returned, and
  // whatever issues the board already holds.
  //
  // `project` is checked FIRST and on its own: with no project open there is
  // nothing to fetch, so "empty" is a fact about the workbench, never a verdict
  // on a request that was never made.
  function state({ project, loading, error, doc, issues, period } = {}) {
    const periods = { list: PERIODS, key: period || "all" };
    if (!project) {
      return {
        kind: EMPTY,
        periods,
        message: "No project open",
        hint: "Open a project in the sidebar to see what it cost.",
      };
    }
    if (error) return { kind: ERROR, project, periods, message: error };
    if (loading || !doc) return { kind: LOADING, project, periods };
    const unpriced = doc.unpriced || {};
    const tokens = doc.tokens || {};
    return {
      kind: READY,
      project,
      // The window the figures ACTUALLY carry, read off the document rather
      // than off the control — a label that led its own data would be the
      // misread the closed vocabulary exists to prevent.
      periods: { list: PERIODS, key: (doc.period || {}).key || "all" },
      periodLabel: (doc.period || {}).label || "all time",
      tiles: tiles(doc),
      deliveryRows: deliveryRows(doc, issues),
      deliveriesTruncated: doc.deliveries_truncated || 0,
      modelRows: modelRows(doc),
      overheadLines: overheadLines(doc),
      band: band(doc),
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

  // --- the Ledger pane -------------------------------------------------------

  // Every dimension the ledger record carries, plus the four token counts in the
  // canonical meter order (PRD #355 story 29). The four counts are RAW: they are
  // per-row, and the daemon's `k`/`M` abbreviation is a summary vocabulary — a
  // grid whose whole purpose is the detailed read must not round.
  const LEDGER_COLUMNS = [
    { key: "kind", label: "kind" },
    { key: "project", label: "project" },
    { key: "issue", label: "issue" },
    { key: "phase", label: "phase" },
    { key: "agent", label: "agent" },
    { key: "model", label: "model" },
    { key: "outcome", label: "outcome" },
    { key: "actor", label: "actor" },
    { key: "version", label: "version" },
    { key: "when", label: "when" },
    { key: "input", label: "↑ input" },
    { key: "cache_read", label: "⚡ cache read" },
    { key: "cache_creation", label: "❄ cache write" },
    { key: "output", label: "↓ output" },
  ];

  // The grid is one DOM row per ledger line and the ledger grows forever (626
  // lines on the operator's own today), so the visible LIST is bounded. No figure
  // is: the Overview's totals are folded server-side over every row.
  const LEDGER_CAP = 500;

  // A field that has no counterpart on this record's kind reads as `—`, never as
  // an empty cell: a blank is ambiguous between "no value" and "the grid dropped
  // a column".
  const NONE = "—";

  function text(value) {
    return value === undefined || value === null || value === "" ? NONE : String(value);
  }

  // ADR-0043 D10: a vendor that hides part of its usage (Gemini never writes its
  // router's tokens to disk) leaves counts that are a FLOOR, not the bill. The
  // caveat rides on the NUMBER itself — a figure that can be read without its
  // caveat will be — and the row says so in words beside it.
  function boundMark(value, lowerBound) {
    return value === NONE || !lowerBound ? value : "≥ " + value;
  }

  function boundNote(lowerBound) {
    return lowerBound ? " (lower bound)" : "";
  }

  // One row's four counts as strings. `tokens: null` is the scan's way of saying
  // the vendor keeps no count anywhere (ADR-0042 D11), which must never render as
  // `0` — that would claim a measurement nobody made.
  function counts(tokens, lowerBound) {
    if (!tokens) return { input: NONE, cache_read: NONE, cache_creation: NONE, output: NONE };
    const at = (key) =>
      boundMark(
        tokens[key] === undefined || tokens[key] === null ? NONE : String(tokens[key]),
        lowerBound,
      );
    return {
      input: at("input"),
      cache_read: at("cache_read"),
      cache_creation: at("cache_creation"),
      output: at("output"),
    };
  }

  function ledgerRow(rec) {
    return {
      kind: "ledger",
      project: text(rec.project),
      issue: rec.issue ? "#" + rec.issue : NONE,
      phase: text(rec.phase),
      agent: text(rec.agent),
      model: text(rec.model),
      outcome: text(rec.outcome),
      actor: text(rec.actor_name || rec.actor_email),
      version: text(rec.ralphy_version),
      when: text(rec.ts),
      tokens: counts(rec.tokens, !!rec.lower_bound),
      unpriced: rec.unpriced_cause || "",
      lowerBound: !!rec.lower_bound,
      boundNote: boundNote(!!rec.lower_bound),
    };
  }

  // An interactive session is a row too (PRD #355 story 16): it is real project
  // overhead, it can be unpriceable, and the modal this pane replaced showed it.
  // The four columns it has no field for read `—` rather than being hidden.
  function interactiveRow(rec) {
    return {
      kind: "interactive",
      project: text(rec.project),
      issue: NONE,
      phase: NONE,
      agent: text(rec.agent),
      model: text(rec.model),
      outcome: NONE,
      actor: text(rec.actor_name || rec.actor_email),
      version: NONE,
      when: text(rec.last_ts || rec.first_ts),
      tokens: counts(rec.tokens, !!rec.lower_bound),
      unpriced: rec.unpriced_cause || "",
      lowerBound: !!rec.lower_bound,
      boundNote: boundNote(!!rec.lower_bound),
    };
  }

  // The raw per-phase grid — what the removed Usage modal did, with columns and
  // with the daemon's unpriced verdict on each row. Pure, like everything else
  // here: `unpriced_cause` is READ, never re-derived, because `no_price` needs the
  // price table and this module has none.
  function ledger({ project, loading, error, records, interactive, missing, unpricedOnly } = {}) {
    const banner = missing || [];
    if (!project) {
      return {
        kind: EMPTY,
        columns: LEDGER_COLUMNS,
        rows: [],
        truncated: 0,
        missing: banner,
        anyLowerBound: false,
        unpricedOnly: !!unpricedOnly,
        message: "No project open",
        hint: "Open a project in the sidebar to read its ledger.",
      };
    }
    if (error) {
      return {
        kind: ERROR,
        columns: LEDGER_COLUMNS,
        rows: [],
        truncated: 0,
        missing: banner,
        anyLowerBound: false,
        unpricedOnly: !!unpricedOnly,
        message: error,
      };
    }
    if (loading) {
      return {
        kind: LOADING,
        columns: LEDGER_COLUMNS,
        rows: [],
        truncated: 0,
        missing: banner,
        anyLowerBound: false,
        unpricedOnly: !!unpricedOnly,
      };
    }
    let rows = (records || []).map(ledgerRow).concat((interactive || []).map(interactiveRow));
    if (unpricedOnly) rows = rows.filter((r) => !!r.unpriced);
    const anyLowerBound = rows.some((r) => r.lowerBound);
    const truncated = Math.max(0, rows.length - LEDGER_CAP);
    return {
      kind: READY,
      columns: LEDGER_COLUMNS,
      rows: rows.slice(0, LEDGER_CAP),
      truncated,
      missing: banner,
      anyLowerBound,
      unpricedOnly: !!unpricedOnly,
    };
  }

  window.WBSpend = {
    EMPTY,
    LOADING,
    ERROR,
    READY,
    CAUSE_COPY,
    PERIODS,
    LEDGER_COLUMNS,
    LEDGER_CAP,
    boundMark,
    ledger,
    causes,
    meterRows,
    tiles,
    deliveryRows,
    modelRows,
    overheadLines,
    band,
    floorNote,
    state,
  };
})(window);
