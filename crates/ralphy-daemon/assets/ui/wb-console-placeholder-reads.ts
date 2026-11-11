/* ---------------------------------------------------------------------------
   What a placeholder asks the page before it relaunches: whether its worktree
   is still there, and which live session it should attach to.
   `createPlaceholders` (`wb-console-placeholder.ts`) asks both.
--------------------------------------------------------------------------- */
import * as WBConsoleSession from "./wb-console-session.ts";
import * as WBDeskFolds from "./wb-desk-folds.ts";
import { WBWindowState } from "./wb-window-state.ts";
import type { ConsoleWin, SpawnCarry } from "./wb-types.d.ts";

const { unheardRef } = WBConsoleSession;
const { placeholderSession, isUnknownCheckout } = WBDeskFolds;
const { sessionIdOf } = WBWindowState;

// What the live-session read takes from the placeholders' deps.
export type LiveReads = {
  wins: Set<ConsoleWin>;
  readSessions: () => Promise<{ sessions: HostedSession[]; unheard: ReadonlySet<string> }>;
  reloadDesk: () => Promise<unknown>;
  loadDesk: () => WBDeskFolds.FoldRecord[];
};

// Whether the worktree a relaunch asks for is still there (#411). The daemon
// refuses a launch into a missing worktree with a `400` BEFORE the socket
// upgrades, and a browser cannot read that status. So ask first, through the
// cheapest Observe read that takes a checkout (no spawn): the ONE reply that
// means the tree is gone is `unknown checkout`. Anything else — an
// unreachable daemon included — lets the launch decide.
export async function checkoutStillThere(
  window: Window,
  repo: string | null | undefined,
  checkout: string | null | undefined,
) {
  const daemon = window.WBDaemon;
  if (checkout === null || checkout === undefined || checkout === "") return true;
  if (repo === null || repo === undefined || repo === "") return true;
  if (typeof daemon.observe !== "function") return true;
  let reply;
  try {
    reply = await daemon.observe("tree.list", { repo, path: "", checkout });
  } catch {
    return true;
  }
  return !isUnknownCheckout(reply);
}

// The live session a placeholder should attach to, read NOW: the page was
// loaded before another device started this console, so neither the mirror
// nor the load-time session list knows it. `null` when the list says it does
// not run; `undefined` when that is not known — the list cannot be read, or
// it did not hear from the record's peer.
export async function liveSessionFor(reads: LiveReads, win: ConsoleWin, record: SpawnCarry) {
  const { wins, readSessions, reloadDesk, loadDesk } = reads;
  await reloadDesk();
  let sessions;
  try {
    const read = await readSessions();
    if (unheardRef(record.repo, read.unheard)) {
      return;
    }
    sessions = read.sessions;
  } catch {
    return;
  }
  const layout: WBDeskFolds.FoldRecord[] = loadDesk();
  // Deleted by another page: this window still stands for it.
  if (!layout.some((rec) => rec.id === win._deskId)) {
    layout.push({ ...record, id: win._deskId, sessionId: null });
  }
  const held = [...wins]
    .filter((w) => w !== win && !w.classList.contains("placeholder"))
    .map((w) => ({ id: sessionIdOf(w), repo: w._deskRepo }))
    .filter((h) => h.id != null);
  return placeholderSession({ layout, sessions, recordId: win._deskId, held });
}
