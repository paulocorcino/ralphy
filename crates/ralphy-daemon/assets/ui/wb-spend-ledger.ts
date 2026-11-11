// The Ledger pane of the Spend tab: the raw per-phase grid, folded from the records the
// daemon sent. Pure, as `wb-spend.ts`, which re-exports `ledger` in `WBSpend`.

import type { MissingUsage, SpendRecord } from "./wb-spend-types.d.ts";
import { CAUSE_COPY, EMPTY, ERROR, LOADING, READY } from "./wb-spend-copy.ts";
import { textOr } from "./wb-fleet.ts";

// The words a Ledger row shows for its kind and its unpriced cause. The row
// keeps the keys (`kind`, `unpriced`) for filters and classes.
const KIND_WORD = { ledger: "run", interactive: "interactive" };
function causeWord(key: string | null | undefined): string {
  if (key == null || key === "") return "";
  return textOr(CAUSE_COPY[key]?.title, key);
}

// An ISO time as the browser's local date and time; a value that does not
// parse stays as it is.
function localTime(ts: string | null | undefined): string {
  if (ts == null || ts === "" || ts === NONE) return NONE;
  const d = new Date(ts);
  return Number.isNaN(d.getTime()) ? ts : d.toLocaleString();
}

// --- the Ledger pane -------------------------------------------------------

// Every dimension the ledger record carries, plus the four token counts in the
// canonical meter order (PRD #355 story 29). Not `project`: the daemon serves
// only the open project's rows, and the pane head already names it. The four
// counts are RAW: they are per-row, and the daemon's `k`/`M` abbreviation is a
// summary vocabulary — a grid whose whole purpose is the detailed read must
// not round.
export const LEDGER_COLUMNS = [
  { key: "kind", label: "Kind" },
  { key: "issue", label: "Issue" },
  { key: "phase", label: "Phase" },
  { key: "agent", label: "Agent" },
  { key: "model", label: "Model" },
  { key: "outcome", label: "Outcome" },
  { key: "actor", label: "Actor" },
  { key: "version", label: "Version" },
  { key: "when", label: "When" },
  { key: "input", label: "↑ Input" },
  { key: "cache_read", label: "⚡ Cache read" },
  { key: "cache_creation", label: "❄ Cache write" },
  { key: "output", label: "↓ Output" },
];

// The grid is one DOM row per ledger line and the ledger grows forever (626
// lines on the operator's own today), so the visible LIST is bounded. No figure
// is: the Overview's totals are folded server-side over every row.
export const LEDGER_CAP = 500;

// A field that has no counterpart on this record's kind reads as `—`, never as
// an empty cell: a blank is ambiguous between "no value" and "the grid dropped
// a column".
const NONE = "—";

function text(value: string | number | boolean | null | undefined): string {
  return value === undefined || value === null || value === "" ? NONE : String(value);
}

// ADR-0043 D10: a vendor that hides part of its usage (Gemini never writes its
// router's tokens to disk) leaves counts that are a FLOOR, not the bill. The
// caveat rides on the NUMBER itself — a figure that can be read without its
// caveat will be — and the row says so in words beside it.
export function boundMark(value: string, lowerBound: boolean): string {
  return value === NONE || !lowerBound ? value : "≥ " + value;
}

function boundNote(lowerBound: boolean): string {
  return lowerBound ? " (lower bound)" : "";
}

// One row's four counts as strings. `tokens: null` is the scan's way of saying
// the vendor keeps no count anywhere (ADR-0042 D11), which must never render as
// `0` — that would claim a measurement nobody made.
function counts(tokens: SpendRecord["tokens"], lowerBound: boolean) {
  if (tokens == null) return { input: NONE, cache_read: NONE, cache_creation: NONE, output: NONE };
  const at = (key: string) =>
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

// A row this daemon did not write. `daemon_id` is stamped on every row the
// fleet fold touches, so an id that differs from ours came from a peer — and a
// peer's row is in this grid but in NONE of the Overview's figures.
function fromPeer(rec: SpendRecord, daemonId: string | null | undefined): boolean {
  return (
    daemonId != null &&
    daemonId !== "" &&
    rec.daemon_id !== undefined &&
    rec.daemon_id !== "" &&
    rec.daemon_id !== daemonId
  );
}

// Whether a value is a non-empty string.
function isText(value: string | null | undefined): value is string {
  return value != null && value !== "";
}

// Who ran the record: the name, else the email.
function actorOf(rec: SpendRecord): string | undefined {
  return rec.actor_name === undefined || rec.actor_name === "" ? rec.actor_email : rec.actor_name;
}

function ledgerRow(rec: SpendRecord, daemonId: string | null | undefined) {
  return {
    kind: "ledger",
    kindLabel: KIND_WORD.ledger,
    peer: fromPeer(rec, daemonId),
    issue: rec.issue !== undefined && rec.issue !== 0 ? "#" + rec.issue : NONE,
    phase: text(rec.phase),
    agent: text(rec.agent),
    model: text(rec.model),
    outcome: text(rec.outcome),
    actor: text(actorOf(rec)),
    version: text(rec.ralphy_version),
    when: text(rec.ts),
    whenLabel: localTime(text(rec.ts)),
    tokens: counts(rec.tokens, rec.lower_bound === true),
    unpriced: rec.unpriced_cause ?? "",
    unpricedLabel: causeWord(rec.unpriced_cause),
    lowerBound: rec.lower_bound === true,
    boundNote: boundNote(rec.lower_bound === true),
  };
}

// An interactive session is a row too (PRD #355 story 16): it is real project
// overhead, it can be unpriceable, and the modal this pane replaced showed it.
// The four columns it has no field for read `—` rather than being hidden.
// When the session was last seen, else when it began.
function lastTs(rec: SpendRecord): string | undefined {
  return rec.last_ts === undefined || rec.last_ts === "" ? rec.first_ts : rec.last_ts;
}

function interactiveRow(rec: SpendRecord, daemonId: string | null | undefined) {
  return {
    kind: "interactive",
    kindLabel: KIND_WORD.interactive,
    peer: fromPeer(rec, daemonId),
    issue: NONE,
    phase: NONE,
    agent: text(rec.agent),
    model: text(rec.model),
    outcome: NONE,
    actor: text(actorOf(rec)),
    version: NONE,
    when: text(lastTs(rec)),
    whenLabel: localTime(text(lastTs(rec))),
    tokens: counts(rec.tokens, rec.lower_bound === true),
    unpriced: rec.unpriced_cause ?? "",
    unpricedLabel: causeWord(rec.unpriced_cause),
    lowerBound: rec.lower_bound === true,
    boundNote: boundNote(rec.lower_bound === true),
  };
}

// The raw per-phase grid — what the removed Usage modal did, with columns and
// with the daemon's unpriced verdict on each row. Pure, like everything else
// here: `unpriced_cause` is READ, never re-derived, because `no_price` needs the
// price table and this module has none.
//
// `daemonId` is THIS daemon's id, carried so the pane can say when it is
// showing rows the Overview beside it does not total: `/api/usage` folds the
// fleet, `/api/spend` is local only (PRD #355, Out of Scope). A difference the
// page cannot remove is one it must NAME.
export function ledger({
  project,
  loading,
  error,
  records,
  interactive,
  missing,
  unpricedOnly,
  daemonId,
}: {
  project?: string | null;
  loading?: boolean;
  error?: string;
  records?: SpendRecord[];
  interactive?: SpendRecord[];
  missing?: MissingUsage[];
  unpricedOnly?: boolean;
  daemonId?: string | null;
} = {}) {
  const base = {
    columns: LEDGER_COLUMNS,
    rows: [],
    truncated: 0,
    missing: missing ?? [],
    anyLowerBound: false,
    peers: 0,
    unpricedOnly: unpricedOnly === true,
  };
  if (!isText(project)) {
    return {
      ...base,
      kind: EMPTY,
      message: "No project open",
      hint: "Open a project in the sidebar to read its ledger.",
    };
  }
  if (isText(error)) return { ...base, kind: ERROR, message: error };
  if (loading === true) return { ...base, kind: LOADING };

  let rows = (records ?? []).map((r) => ledgerRow(r, daemonId));
  rows = rows.concat((interactive ?? []).map((r) => interactiveRow(r, daemonId)));
  if (unpricedOnly === true) rows = rows.filter((r) => r.unpriced !== "");
  const truncated = Math.max(0, rows.length - LEDGER_CAP);
  // The TAIL, not the head: the ledger is written and served oldest-first, so
  // slicing the front would cap a busy project to its very first phase lines
  // and hide everything recent — including every interactive session, which
  // the daemon appends after the run records.
  const visible = rows.slice(rows.length - Math.min(rows.length, LEDGER_CAP));
  return {
    ...base,
    kind: READY,
    rows: visible,
    truncated,
    anyLowerBound: visible.some((r) => r.lowerBound),
    peers: visible.filter((r) => r.peer).length,
  };
}
