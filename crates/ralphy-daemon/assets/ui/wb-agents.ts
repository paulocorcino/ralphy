// The console menu's rows, folded from the daemon's adapter roster
// (GET /api/agents), the live session list and the open repo. Pure: no DOM, no
// fetch, no mutation of its inputs — unit-tested in ui-tests/wb-agents.test.mjs.

// Verbatim in the menu's title attribute AND pinned by both test layers.
/** One adapter of `GET /api/agents` (roster.rs). */
export type RosterRow = { id: string; label?: string; accelerator?: string; available?: boolean; reason?: string | null };
/** A live session, as the menu counts it. */
export type Session = { agent?: string; repo?: string };

const NEEDS_REPO = "Open a project before you start an agent.";
const NOT_INSTALLED = "Not installed here.";

// The roster sends the code `not installed here` (roster.rs); the menu
// shows the sentence (ADR-0065 §6).
function shownReason(reason: string | null | undefined): string {
  return !reason || reason === "not installed here" ? NOT_INSTALLED : reason;
}

function rowFor(
  kind: string,
  label: string,
  plain: boolean,
  digit: string | undefined,
  available: boolean | undefined,
  reason: string | null | undefined,
  sessions: Session[] | null | undefined,
  openSlug: string | null | undefined,
) {
  // A count drawn from another repo would offer to reach a session with a
  // different working directory — a row that lies about what its click does.
  const scope = plain ? openSlug || "~" : openSlug;
  const mine = (sessions || []).filter(
    (s) => s && s.agent === kind && s.repo === scope,
  );
  const needsRepo = !plain && !openSlug;
  const unavailable = !plain && available === false;
  const disabled = needsRepo || unavailable;
  return {
    kind,
    label,
    plain,
    digit,
    disabled,
    needsRepo,
    unavailable,
    title: needsRepo ? NEEDS_REPO : unavailable ? shownReason(reason) : "",
    tryAnyway: unavailable && !needsRepo,
    // How many of this row's consoles are already open in this repo. A
    // READOUT only: the menu is "New console", so every row's click launches
    // — reaching a live window is the Go-to menu's job. (Until 2026-09-22 a
    // live agent row attached instead and carried a "+" to launch anyway;
    // the click that did not do what the menu's name promised confused more
    // than the duplicate it prevented.)
    live: mine.length,
  };
}

function menuRows({
  roster,
  sessions,
  openSlug,
}: { roster?: RosterRow[] | null; sessions?: Session[] | null; openSlug?: string | null } = {}) {
  const rows = (roster || []).map((r) =>
    rowFor(
      r.id,
      r.label || r.id,
      false,
      r.accelerator,
      r.available,
      r.reason,
      sessions,
      openSlug,
    ),
  );
  // The plain shell is not a vendor adapter, so it never enters the daemon's
  // roster; the menu appends it last on digit 0.
  rows.push(rowFor("console", "console", true, "0", true, null, sessions, openSlug));
  return rows;
}

// The console row's "Run…" field → the command line for one free console,
// or `null` when there is nothing to run. The daemon labels that session
// with the command, so its live count and its relaunch find it by the label,
// and the plain row's count (kind "console") never includes it.
function runCommand(text: unknown): string | null {
  const command = typeof text === "string" ? text.trim() : "";
  return command || null;
}

export type MenuRow = ReturnType<typeof rowFor>;

function canLaunch(row: MenuRow | null | undefined, tryAnyway = false): boolean {
  return !!row && !row.needsRepo && (!row.unavailable || tryAnyway);
}

// Whether a click on `row` launches, or is refused (`null`). The one intent
// a row has: a launch.
function consoleIntent(row: MenuRow | null | undefined, { tryAnyway = false }: { tryAnyway?: boolean } = {}) {
  return canLaunch(row, tryAnyway) ? "launch" : null;
}

function rosterUrl(repo: string | null | undefined): string {
  return repo ? `/api/agents?repo=${encodeURIComponent(repo)}` : "/api/agents";
}

function runRows(roster: RosterRow[] | null | undefined) {
  return (roster || []).map((row) => ({
    id: row.id,
    label: row.label || row.id,
    available: row.available !== false,
    title: row.available === false ? shownReason(row.reason) : "",
  }));
}

function rosterState(roster: unknown, repo: string | null | undefined) {
  const rows: RosterRow[] = Array.isArray(roster) ? roster.slice() : [];
  return { repo: repo || null, roster: rows, agents: runRows(rows) };
}

export const WBAgents = {
  menuRows,
  runCommand,
  canLaunch,
  consoleIntent,
  rosterUrl,
  rosterState,
  runRows,
  NEEDS_REPO,
  NOT_INSTALLED,
};
