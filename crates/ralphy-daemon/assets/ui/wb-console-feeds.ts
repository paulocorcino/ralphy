/* ---------------------------------------------------------------------------
   The console's feeds: the last facts the shell handed it.

   `createFeeds(deps)` keeps the last session list, the project names, the
   fleet and the stale probe that the shell feeds in (a Shown fact has one
   owner; the console keeps only its last answer), the ids of new records and
   the Go-to list. Values that change are read through `lastSessions()`,
   `wakePeerNow()`, `readFleetNow()` and `staleProbeNow()`. It reads the console
   only through `deps` (#621).
--------------------------------------------------------------------------- */
import { WBConsoleName } from "./wb-console-name.ts";
import * as WBConsoleSession from "./wb-console-session.ts";
import { WBFleet } from "./wb-fleet.ts";
import { WBSessionRoute } from "./wb-session-route.ts";
import { apiFetch } from "./wb-api.ts";
import type { Group } from "./wb-fleet.ts";
import type { ConsoleOpts, ConsoleWin, Presentation } from "./wb-types.d.ts";

const { sessionRowFor } = WBConsoleSession;

// What the feeds read from the console, and nothing else.
export type FeedsDeps = {
  // The page's injected options: the stale probe the shell passes.
  OPTS: Pick<ConsoleOpts, "isStale">;
  // Every console window: a feed redraws what it shows.
  wins: Set<ConsoleWin>;
  // The stage element, or null before the page has one.
  stage: () => HTMLElement | null;
  // The tooltip of an agent-state dot.
  agentStateTitle: (state: string | null | undefined, detail?: string) => string;
  // Draws the title of a window again. Built later: a lazy arrow.
  renderTitle: (win: ConsoleWin, title: HTMLElement, presentation: Presentation) => void;
};

export function createFeeds(deps: FeedsDeps) {
  const { OPTS, wins, stage, agentStateTitle, renderTitle } = deps;

  function newId(prefix: string) {
    // `crypto.randomUUID` is undefined in a non-secure context and the daemon can
    // bind a plain-http LAN address (ADR-0032), so build the id by hand.
    return prefix + Date.now().toString(36) + "-" + Math.random().toString(36).slice(2, 10);
  }
  function newDeskId() {
    return newId("w-");
  }
  function newFenceId() {
    return newId("f-");
  }
  // `/api/sessions` and the peers it did not hear from (`unheard`, a Set of
  // daemon ids). Rejects when the list cannot be read. On this read a console
  // of an unheard peer is neither running nor gone: nothing relaunches, adopts
  // or forgets it (ADR-0050 amendment 2026-10-04).
  // Reads in flight at the same moment share one request: every held box
  // asks on the same fleet read, and each request asks every peer.
  let sessionsRead: Promise<{ sessions: HostedSession[]; unheard: ReadonlySet<string> }> | null = null;
  function readSessions() {
    if (!sessionsRead) {
      sessionsRead = (async () => {
        const r = await apiFetch("GET /api/sessions");
        if (!r.ok) throw new Error("sessions unavailable");
        const route = WBSessionRoute;
        return {
          sessions: await r.json(),
          unheard: route.unanswered(r.headers?.get?.(route.UNANSWERED_HEADER)),
        };
      })().finally(() => {
        sessionsRead = null;
      });
    }
    return sessionsRead;
  }
  // The project name and tooltip text per repo ref, fed by the shell
  // (`ingestProjects`) from the daemon's project list. A ref the shell has not
  // named yet (the detached popup, a desk read before the list) falls back to
  // its slug, never to the ref: a peer ref carries a `<daemon_id>/` routing
  // head (ADR-0052 §5).
  const projectNames = new Map<string | null | undefined, { name: string; title: string }>();
  function projectNameOf(ref: string | null | undefined) {
    const known = projectNames.get(ref);
    if (known) return known.name;
    return WBFleet ? WBFleet.refSlug(ref) : ref!;
  }
  function projectTitleOf(ref: string | null | undefined) {
    if (ref === "~") return ref;
    return projectNames.get(ref)?.title || projectNameOf(ref);
  }
  // `rows` is `[{ ref, name, title }]`. Titles and tooltips already drawn are
  // drawn again; a console NAME already given is the operator's and stays.
  function ingestProjects(rows: { ref: string; name: string; title?: string }[] | null | undefined) {
    projectNames.clear();
    for (const r of rows || []) {
      if (r && r.ref && r.name) projectNames.set(r.ref, { name: r.name, title: r.title || r.name });
    }
    for (const win of wins) {
      if (!win._title || !win._presentation) continue;
      const p = win._presentation;
      p.tooltip = WBConsoleName.tooltipLines(projectTitleOf(win._deskRepo), p.environment, p.name).join("\n");
      win._title.title = p.tooltip;
      renderTitle(win, win._title, p);
    }
  }

  // Each peer's fleet group by daemon id, and the shell's wake action, fed by
  // the shell (`ingestFleet`) after every fleet read. The shell owns the fleet;
  // this is only its last answer, for the placeholders of peer projects.
  // `readFleet` is the shell's fleet read on demand, for a window that saw
  // its peer fail.
  const peerGroups = new Map<string, Group>();
  let wakePeer: ((daemon: string) => Promise<boolean>) | null = null;
  let readFleet: (() => void) | null = null;
  function ingestFleet(groups: Group[] | null | undefined, hooks: { wake?: (daemon: string) => Promise<boolean>; read?: () => void } | null | undefined) {
    peerGroups.clear();
    for (const g of groups || []) if (g && g.daemon && !g.local) peerGroups.set(g.daemon, g);
    if (typeof hooks?.wake === "function") wakePeer = hooks.wake;
    if (typeof hooks?.read === "function") readFleet = hooks.read;
    for (const win of [...wins]) if (typeof win._peerRefresh === "function") win._peerRefresh();
  }

  // The shell's last `/api/sessions` poll, kept for the menus' state dots.
  let lastSessionsRead: HostedSession[] = [];

  // Every window on the plane, for the Go-to picker. Reads the DOM, not `wins`:
  // a snapshot at menu open, not reactive state.
  function list() {
    const st = stage();
    if (!st) return [];
    return [...st.querySelectorAll<ConsoleWin>(".session-window")].map((w) => ({
      id: w._deskId,
      agent: w._deskAgent,
      name: w._deskConsoleName ?? null,
      // The title's tooltip, so the Go-to row says what the title says.
      tooltip: w._title?.title || "",
      // `"~"` is the desk's "no repo".
      repo: w._deskRepo === "~" ? null : w._deskRepo,
      kind: w._deskKind,
      running: !w.classList.contains("placeholder"),
      state: w._agentState ?? null,
    }));
  }

  // The agent-state dot per window, from the shell's `/api/sessions` poll
  // (ADR-0059 §5): the state word as a class, hidden when the row says
  // nothing. A placeholder has no session and keeps no dot.
  function ingestSessions(sessions: HostedSession[] | null | undefined) {
    lastSessionsRead = Array.isArray(sessions) ? sessions : [];
    for (const win of wins) {
      const dot = win._stateDot;
      if (!dot) continue;
      const row = sessionRowFor(win, sessions);
      const state = row?.agent_state?.state ?? null;
      win._agentState = state;
      dot.className = "session-state" + (state ? ` ${state}` : "");
      dot.title = agentStateTitle(state, row?.agent_state?.detail);
      dot.hidden = !state;
    }
  }

  // `visibilitychange` and `online` both land on one iOS resume; without the
  // probe seam the popup would have no verdict at all.
  let staleProbe = OPTS.isStale || null;
  function setStaleProbe(fn: (() => boolean) | null) {
    staleProbe = typeof fn === "function" ? fn : null;
  }
  const lastSessions = () => lastSessionsRead;
  const wakePeerNow = () => wakePeer;
  const readFleetNow = () => readFleet;
  const staleProbeNow = () => staleProbe;

  return {
    newId,
    newDeskId,
    newFenceId,
    readSessions,
    projectNameOf,
    projectTitleOf,
    ingestProjects,
    peerGroups,
    ingestFleet,
    ingestSessions,
    list,
    setStaleProbe,
    lastSessions,
    wakePeerNow,
    readFleetNow,
    staleProbeNow,
  };
}
