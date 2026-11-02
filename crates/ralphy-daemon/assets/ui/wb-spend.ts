// The Spend tab's model, folded from the daemon's `/api/spend` summary document
// (PRD #355, tracer bullet #358). Pure: no DOM, no fetch — the fetch and the
// rendering live in app.ts/index.html, exactly as `wb-changes.ts` splits them.
//
// This module COMPUTES NOTHING NUMERIC. Every figure on screen — the total
// (`$2,350.59+`), the `token_meter` (`↑12.4k ⚡184k ❄8.1k ↓3.2k`), each volume
// and each percentage — arrives already rendered by the daemon, so the `k`/`M`
// abbreviation and the money vocabulary have one implementation instead of one
// per client. What lives here is which STATE the pane is in and the explanatory
// COPY for each unpriced cause: decisions and words, never arithmetic.

import type { SpendDoc, TitledIssue, Tokens, Unpriced } from "./wb-spend-types.d.ts";
import { CAUSE_COPY, EMPTY, ERROR, LOADING, READY } from "./wb-spend-copy.ts";
import { LEDGER_CAP, LEDGER_COLUMNS, boundMark, ledger } from "./wb-spend-ledger.ts";

// A daemon cause row plus its copy. An unknown key (a future fourth cause)
// renders under its own name rather than vanishing — the gap must never get
// quieter than it is.
function causes(unpriced: Unpriced) {
  const rows = (unpriced && unpriced.causes) || [];
  return rows.map((c) => ({
    key: c.key,
    title: CAUSE_COPY[c.key]?.title || c.key,
    hint: CAUSE_COPY[c.key]?.hint || "",
    value: c.label,
    share: c.share_label,
    // The daemon's `0.0..=1.0`, as the CSS width it is drawn at. This is a
    // unit conversion for a bar, not a figure the operator reads — every
    // number they READ is a string the daemon rendered.
    width: pct(c.share),
  }));
}

function pct(share: number | null | undefined): string {
  return Math.max(0, Math.min(100, (share || 0) * 100)) + "%";
}

// The token split as bar rows, in the daemon's canonical order. Each row's bar
// is drawn against the LARGEST part, not against the total: with one kind at
// 90% the other three collapse to invisible slivers, and "how do these four
// compare" is the question the rows exist to answer.
// The daemon names each kind in lowercase (meter.rs); the row shows the
// label in sentence case (ADR-0065 §2). The daemon's name stays the fallback.
const PART_NAME: Record<string, string> = {
  input: "Input",
  cache_read: "Cache read",
  cache_creation: "Cache write",
  output: "Output",
};

function meterRows(tokens: Tokens) {
  const parts = (tokens && tokens.parts) || [];
  const peak = parts.reduce((m: number, p) => Math.max(m, p.tokens || 0), 0);
  return parts.map((p) => ({
    key: p.key,
    glyph: p.glyph,
    name: PART_NAME[p.key] || p.name,
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
function tiles(doc: SpendDoc) {
  const k = doc.kpis || {};
  return [
    {
      key: "total",
      label: "Total cost",
      value: doc.total || "~$?",
      note: "",
      primary: true,
      floor: !!doc.floor,
    },
    {
      key: "deliveries",
      label: "Deliveries",
      // The ONE figure on this page that is a count rather than money, so it
      // is the one place a client-side `String()` is not an arithmetic.
      value: String(k.deliveries || 0),
      note: "Issues worked on in this period",
      floor: false,
    },
    {
      key: "cost_per_delivery",
      label: "Cost per delivery",
      value: k.cost_per_delivery_median_label || "~$?",
      // The mean rides in the note, not in a sixth tile: the pair is one
      // reading — the typical issue, and how far the tail pulls the average.
      note: "Median · mean " + (k.cost_per_delivery_mean_label || "~$?"),
      floor: !!k.cost_per_delivery_floor,
    },
    {
      key: "retry_burn",
      label: "Retry burn",
      value: k.retry_burn_label || "—",
      note: "Share of spend on issues not delivered",
      floor: !!k.retry_burn_floor,
    },
    {
      key: "cache_hit",
      label: "Cache hit",
      value: k.cache_hit_label || "—",
      note: "Share of prompt tokens read from cache",
      floor: false,
    },
  ];
}

// The deliveries grid. `issues` is whatever the board already holds — the
// title is an ADORNMENT, so a cold board renders `#251` with no title and
// NOTHING here reaches for one: the board fold spawns a CLI that makes tracker
// calls, and a cost page must never pay it.
function deliveryRows(doc: SpendDoc, issues: TitledIssue[] | null | undefined) {
  const titles = new Map<number, string>();
  for (const i of issues || []) {
    if (i && i.number != null) titles.set(i.number, i.title || "");
  }
  const peak = (doc.deliveries || []).reduce((m: number, d) => Math.max(m, d.share || 0), 0);
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
function modelRows(doc: SpendDoc) {
  const rows = doc.models || [];
  const peak = rows.reduce((m: number, r) => Math.max(m, r.share || 0), 0);
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
function overheadLines(doc: SpendDoc) {
  const o = doc.overhead || {};
  const sessions = o.interactive_sessions || 0;
  return [
    {
      key: "deliveries",
      label: "Deliveries",
      value: o.deliveries_total || "~$?",
      floor: !!o.deliveries_floor,
      note: "",
    },
    {
      key: "interactive",
      label: "Interactive",
      value: o.interactive_total || "~$?",
      floor: !!o.interactive_floor,
      note: sessions === 1 ? "1 session" : sessions + " sessions",
    },
    {
      key: "consolidation",
      label: "Consolidation",
      value: o.consolidation_total || "~$?",
      floor: !!o.consolidation_floor,
      note: "Cost of the run itself, not of one issue",
    },
  ];
}

// The activity band: one column per day, two bars in it. Both heights are the
// daemon's own share of its peak day — the two series have no common unit, so
// each gets its own baseline rather than a shared axis that would lie.
function band(doc: SpendDoc) {
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
      title:
        `${d.date} · ${d.usd_label || "~$?"} · ` +
        (d.deliveries === 1 ? "1 delivery" : (d.deliveries || 0) + " deliveries"),
      deliveriesHeight: pct(d.deliveries_share),
      quiet: !(d.usd > 0) && !(d.deliveries > 0),
    })),
  };
}

// The period control: the daemon's own vocabulary, so a key the client offers
// is always a key the route accepts.
const PERIODS = [
  { key: "all", label: "All time" },
  { key: "7d", label: "Last 7 days" },
  { key: "30d", label: "Last 30 days" },
  { key: "90d", label: "Last 90 days" },
];

// The whole pane, from the four things app.ts knows: the open project, the
// in-flight/failed state of the fetch, the document the daemon returned, and
// whatever issues the board already holds.
//
// `project` is checked FIRST and on its own: with no project open there is
// nothing to fetch, so "empty" is a fact about the workbench, never a verdict
// on a request that was never made.
function state({
  project,
  loading,
  error,
  doc,
  issues,
  period,
}: {
  project?: string | null;
  loading?: boolean;
  error?: string;
  doc?: SpendDoc | null;
  issues?: TitledIssue[] | null;
  period?: string;
} = {}) {
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
  return readyState(project, doc, issues);
}

// The pane of a document the daemon returned for the open project.
function readyState(project: string, doc: SpendDoc, issues: TitledIssue[] | null | undefined) {
  const unpriced = doc.unpriced || {};
  const tokens = doc.tokens || {};
  return {
    kind: READY,
    project,
    // The window the figures ACTUALLY carry, read off the document rather
    // than off the control — a label that led its own data would be the
    // misread the closed vocabulary exists to prevent.
    periods: { list: PERIODS, key: (doc.period || {}).key || "all" },
    periodLabel: docPeriodLabel(doc),
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
    ...tokenFields(tokens),
    coverage: coverageOf(unpriced),
    unpriced: unpricedOf(unpriced),
  };
}

// The words come from the control's own list, keyed by the document's
// key, so the note and the select read the same.
function docPeriodLabel(doc: SpendDoc) {
  return (
    (PERIODS.find((p) => p.key === (doc.period || {}).key) || {}).label ||
    (doc.period || {}).label ||
    "All time"
  );
}

// The token meter of the pane, with its defaults.
function tokenFields(tokens: Tokens) {
  return {
    meter: tokens.meter || "",
    tokensTotal: tokens.label || "0",
    tokensRaw: tokens.total || 0,
    meterRows: meterRows(tokens),
  };
}

// The priced and unpriced shares, as the coverage bar draws them.
function coverageOf(unpriced: Unpriced) {
  return {
    // Only worth drawing once something is missing: a full-width bar at
    // 100% priced is a decoration that says what the absent floor marker
    // already said.
    show: (unpriced.tokens || 0) > 0,
    priced: pct(unpriced.priced_share),
    pricedLabel: unpriced.priced_share_label || "",
    pricedVolume: unpriced.priced_label || "",
    unpriced: pct(unpriced.share),
    unpricedLabel: unpriced.share_label || "",
  };
}

// The spend the pane could not price, and why.
function unpricedOf(unpriced: Unpriced) {
  return {
    any: (unpriced.tokens || 0) > 0 || (unpriced.unmetered_sessions || 0) > 0,
    label: unpriced.label || "",
    share: unpriced.share_label || "",
    causes: causes(unpriced),
    // Sessions whose vendor keeps no token count anywhere: real spend of
    // unmeasurable size, so it is reported as a count of sessions rather
    // than folded into a token figure it has no tokens for.
    unmetered: unpriced.unmetered_sessions || 0,
  };
}

// Why the figure is a floor, in words. Two distinct reasons can raise the flag
// and they call for different sentences: volume that could not be PRICED, and
// sessions that were never COUNTED (`tokens: null`, ADR-0042 D11) — the second
// leaves no tokens to name, so a note about "unpriced volume" would be false.
function floorNote(doc: SpendDoc, unpriced: Unpriced): string {
  if (!doc.floor) return "";
  const volume = unpriced.tokens || 0;
  const sessions = unpriced.unmetered_sessions || 0;
  if (volume > 0) {
    return (
      "The real cost can be higher. " +
      unpriced.label +
      " tokens (" +
      unpriced.share_label +
      ") could not be priced." +
      (sessions > 0 ? " Some sessions have no token count." : "")
    );
  }
  if (sessions > 0) return "The real cost can be higher. Some sessions have no token count.";
  return "The real cost can be higher. Part of this spend could not be priced.";
}

export const WBSpend = {
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

// `app.ts` and a browser check read this name (ADR-0075 D9).
if (typeof window !== "undefined") window.WBSpend = WBSpend;

declare global {
  interface Window {
    WBSpend: typeof WBSpend;
  }
}
