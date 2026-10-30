/* ---------------------------------------------------------------------------
   The console's placeholder: a console the daemon no longer runs.

   `createPlaceholders(deps)` builds the box of a console that does not run
   (same chrome, same box, no session), relaunches it or attaches it to the
   session that runs it now, brings placeholders back on a resume, and keeps
   the relaunch marks the columns wait for. It reads the console only through
   `deps`. The rules are `placeholderSession` in `wb-desk-folds.ts` and the
   peer rules in `wb-console-session.ts`; a live window is
   `wb-console-spawn.ts` (#621).
--------------------------------------------------------------------------- */
import * as WBConsoleSession from "./wb-console-session.ts";
import * as WBDeskFolds from "./wb-desk-folds.ts";
import { WBWindowState } from "./wb-window-state.ts";
import { WBFleet } from "./wb-fleet.ts";
import type { Group } from "./wb-fleet.ts";
import type { TerminalOpts } from "./wb-console-terminal-types.d.ts";
import type { ConfirmOptions } from "./wb-console-title.ts";
import type { ChromeRecord } from "./wb-stage-chrome.ts";
import type { ConsoleWin, DeskRecord, ExtentOpts, SpawnCarry, WindowSnapshot } from "./wb-types.d.ts";

const { peerReturnDecision, peerHost, peerOfflineView, peerActionLabel, heldReturnDecision, unheardRef, relaunchRequest } = WBConsoleSession;
const { placeholderSession, isUnknownCheckout } = WBDeskFolds;
const { sessionIdOf } = WBWindowState;

// What the placeholder reads from the console, and nothing else. Every dep is
// built before it; the spawn, the desk, the detach and the dormancy take its
// functions as lazy arrows.
export type PlaceholderDeps = {
  // The console's page: the daemon's Observe read.
  window: Window;
  // The page the box is made in.
  document: Document;
  // `canLaunch: false` keeps every automatic relaunch out of the popup.
  OPTS: { canLaunch?: boolean };
  // Every console window.
  wins: Set<ConsoleWin>;
  // The GPU budget: the watch a box joins at birth and leaves when it drops.
  budget: {
    trackDormancy: (win: ConsoleWin) => void;
    untrackDormancy: (win: ConsoleWin) => void;
  };
  // The window chrome: the frame, the titlebar, the body and its buttons.
  buildChrome: (
    label: string | null | undefined,
    repo: string | null | undefined,
    desk: ChromeRecord,
    kind: string | null | undefined,
  ) => { win: ConsoleWin; body: HTMLElement; title: HTMLElement; restartBtn: HTMLButtonElement; closeBtn: HTMLButtonElement };
  // A live window, a window's record, and what a birth writes (`wb-console-spawn.ts`).
  spawnWindow: (termOpts: TerminalOpts, label: string | null | undefined, repo: string | null | undefined, desk?: SpawnCarry) => ConsoleWin;
  deskOf: (win: ConsoleWin) => WindowSnapshot;
  recordBirth: (win: ConsoleWin, carry: SpawnCarry | undefined) => void;
  // The session list, read now.
  readSessions: () => Promise<{ sessions: HostedSession[]; unheard: ReadonlySet<string> }>;
  // The fleet's last answer, grouped by peer: one map for the life of the console.
  peerGroups: Map<string, Group>;
  // The store `projects`' wake of a peer, read at each use.
  wakePeerNow: () => ((daemon: string) => Promise<boolean>) | null;
  // A fresh desk read (`createDesk`).
  reloadDesk: () => Promise<unknown>;
  // The desk records, and the removal of one (`createDeskRecords`).
  loadDesk: () => DeskRecord[];
  forgetRecord: (deskId: string) => void;
  // The door's confirm.
  askConfirm: (opts: ConfirmOptions) => Promise<unknown>;
  // The stage's extent.
  applyExtent: (opts?: ExtentOpts) => void;
  // Tells the console that its window set changed.
  changed: () => void;
};

export function createPlaceholders(deps: PlaceholderDeps) {
  const {
    window,
    document,
    OPTS,
    wins,
    budget,
    buildChrome,
    spawnWindow,
    deskOf,
    recordBirth,
    readSessions,
    peerGroups,
    wakePeerNow,
    reloadDesk,
    loadDesk,
    forgetRecord,
    askConfirm,
    applyExtent,
    changed,
  } = deps;

  // Whether the worktree a relaunch asks for is still there (#411). The daemon
  // refuses a launch into a missing worktree with a `400` BEFORE the socket
  // upgrades, and a browser cannot read that status. So ask first, through the
  // cheapest Observe read that takes a checkout (no spawn): the ONE reply that
  // means the tree is gone is `unknown checkout`. Anything else — an
  // unreachable daemon included — lets the launch decide.
  async function checkoutStillThere(repo: string | null | undefined, checkout: string | null | undefined) {
    const daemon = window.WBDaemon;
    if (!checkout || !repo || typeof daemon?.observe !== "function") return true;
    let reply;
    try {
      reply = await daemon.observe("tree.list", { repo, path: "", checkout });
    } catch {
      return true;
    }
    return !isUnknownCheckout(reply);
  }

  // Spawn an agent console — unless the worktree it asks for is gone, in which
  // case a placeholder SAYS so (#411). A console must never silently land on
  // the primary tree because its own vanished (the #409 gates).
  // Desk ids whose window a relaunch took off the stage and has not put back
  // yet: the columns (`wbColumns`) wait for them instead of dropping them.
  const relaunching = new Set<string | undefined>();
  function isRelaunching(deskId: string) {
    return relaunching.has(deskId);
  }
  // Marks `deskId`, then takes its window off the stage. `spawnOrMissing`
  // clears the mark; a take-down that throws clears it here.
  function markRelaunch(deskId: string, takeDown: () => void) {
    relaunching.add(deskId);
    try {
      takeDown();
    } catch (e) {
      relaunching.delete(deskId);
      throw e;
    }
  }

  async function spawnOrMissing(req: TerminalOpts, label: string | null | undefined, repo: string | null | undefined, carry: SpawnCarry) {
    try {
      if (req.checkout && !(await checkoutStillThere(repo, req.checkout))) {
        return spawnPlaceholder({ ...carry, checkout: req.checkout }, req.checkout);
      }
      return spawnWindow(req, label, repo, carry);
    } finally {
      relaunching.delete(carry?.id);
    }
  }

  // The live session a placeholder should attach to, read NOW: the page was
  // loaded before another device started this console, so neither the mirror
  // nor the load-time session list knows it. `null` when the list says it does
  // not run; `undefined` when that is not known — the list cannot be read, or
  // it did not hear from the record's peer.
  async function liveSessionFor(win: ConsoleWin, record: SpawnCarry) {
    await reloadDesk();
    let sessions;
    try {
      const read = await readSessions();
      if (unheardRef(record.repo, read.unheard)) return undefined;
      sessions = read.sessions;
    } catch {
      return undefined;
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

  // Bring every placeholder whose console now runs somewhere back as an attached
  // window. ATTACH only — a resume must never launch a vendor CLI. One at a
  // time, so an attach counts as `held` for the next placeholder; one pass at a
  // time, because `visibilitychange` and `online` land together on an iOS resume.
  let reviving: Promise<void> | null = null;
  function revivePlaceholders() {
    if (reviving) return reviving;
    reviving = (async () => {
      for (const w of [...wins]) {
        if (typeof w._revive === "function" && w.isConnected) await w._revive();
      }
    })().finally(() => {
      reviving = null;
    });
    return reviving;
  }

  // An agent console the daemon no longer runs: same chrome, same box, no
  // session — one click relaunches into this very record, unless the console
  // runs by now (another device started it), in which case it attaches.
  // `missing` names a worktree that no longer exists (#411): the button
  // relaunches on the PRIMARY tree, explicitly by its label. `refused` (a
  // `{ message }`) is a launch a peer refused. A box for a peer project says
  // what its peer's fleet state is, and redraws on every fleet read.
  // `held.unheard`: restored while the session list did not hear from its
  // peer, so whether it runs is not known; it asks again on each fleet read.
  function spawnPlaceholder(record: SpawnCarry, missing?: string | null, refused?: { message: string | null } | null, held?: { unheard?: boolean }) {
    const { win, body, restartBtn, closeBtn } = buildChrome(
      record.agent,
      record.repo,
      record,
      record.kind,
    );
    win.classList.add("placeholder");
    // Nothing runs here to restart: Relaunch is the one action.
    restartBtn.hidden = true;

    const note = document.createElement("div");
    note.className = "session-offline";
    const text = document.createElement("p");
    text.textContent =
      record.kind === "console" ? "This console is not running." : "This agent console is not running.";
    const btn = document.createElement("button");
    btn.className = "session-reconnect";
    btn.textContent = "Relaunch";
    // The popup offers it too: a relaunch reuses this record's id, so the
    // popup's members stay the fence's snapshot (ADR-0051 §8, amended
    // 2026-10-05). Only a click launches there; `canLaunch` keeps every
    // automatic relaunch out of the popup.
    note.append(text, btn);
    body.append(note);

    // The peer's words. `peerAction` is the button's action ("wake", "retry",
    // "relaunch"); the box starts as "not running" until the fleet says more.
    const daemon = WBFleet?.refDaemon(record.repo) || "";
    const canLaunch = OPTS.canLaunch !== false;
    type BoxAction = WBConsoleSession.PeerAction | "relaunch";
    let peerAction: BoxAction = "relaunch";
    let wasOffline = false;
    let shown: Parameters<typeof show>[0] | null = null;
    let detail: HTMLDetailsElement | null = null;
    let detailText: HTMLParagraphElement | null = null;
    const show = (view: { text: string; detail: string; action: BoxAction }) => {
      shown = view;
      text.textContent = view.text;
      peerAction = view.action;
      const label = view.action === "relaunch" ? "Relaunch" : peerActionLabel(view.action);
      btn.hidden = !label;
      if (label) btn.textContent = label;
      if (!detail) {
        detail = document.createElement("details");
        detail.className = "session-detail";
        const summary = document.createElement("summary");
        summary.textContent = "Details";
        detailText = document.createElement("p");
        detail.append(summary, detailText);
        note.append(detail);
      }
      detailText!.textContent = view.detail || "";
      detail.hidden = !view.detail;
    };
    const showPeer = () => {
      // A click in flight owns the box; a missing worktree says something else.
      if (!daemon || missing || btn.disabled || !win.isConnected) return;
      const group = peerGroups.get(daemon);
      if (!group) return;
      const available = WBFleet.available(group);
      // The fleet may call the peer available while its session list timed
      // out: a held box asks the list itself, and never waits for `wasOffline`.
      if (unheard && available) {
        askHeld();
        return;
      }
      if (!available) wasOffline = true;
      showPeerTurn(group, available);
    };
    // What the box does with the fleet's latest word on the peer: relaunch,
    // offer a relaunch, or say that the peer is offline or refused.
    function showPeerTurn(group: Group, available: boolean) {
      const turn = peerReturnDecision({ kind: record.kind, canLaunch, available, wasOffline });
      if (turn === "relaunch") {
        btn.click();
        return;
      }
      if (turn === "offer") {
        show({ text: `${peerHost(group, record.environment)} is back.`, detail: "", action: "relaunch" });
        return;
      }
      if (available && !refused) return;
      show(peerOfflineView(available ? null : group, refused?.message, peerHost(group, record.environment)));
    }
    if (refused) show(peerOfflineView(null, refused.message, peerHost(peerGroups.get(daemon), record.environment)));
    let unheard = !!(daemon && held?.unheard);
    // The box says why it waits: the list did not hear from the peer. "Try
    // again" asks the list again, and launches only when the list heard from
    // the peer and the console does not run there.
    const showUnheard = () =>
      show({
        text: `${peerHost(peerGroups.get(daemon), record.environment)} does not answer.`,
        detail: "",
        action: "retry",
      });
    if (unheard) showUnheard();
    // One question at a time: fleet reads arrive every 30 s and on every
    // `peers.dirty`, and an answer must be acted on once.
    let asking = false;
    const askHeld = async () => {
      if (asking) return;
      asking = true;
      try {
        const session = await check();
        if (!win.isConnected || btn.disabled || !unheard) return;
        const turn = heldReturnDecision({ kind: record.kind, canLaunch, session });
        if (turn === "stay") {
          showUnheard();
          return;
        }
        unheard = false;
        if (turn === "attach") attach(session!);
        else if (turn === "relaunch") btn.click();
        else show({ text: `${peerHost(peerGroups.get(daemon), record.environment)} is back.`, detail: "", action: "relaunch" });
      } finally {
        asking = false;
      }
    };
    win._peerRefresh = showPeer;

    const markMissing = (name: string | null | undefined) => {
      missing = name;
      win.classList.add("missing-checkout");
      text.textContent = `Worktree ${name} no longer exists.`;
      btn.textContent = "Relaunch in primary";
    };
    if (missing) markMissing(missing);
    // A placeholder restored for a recorded worktree asks whether that tree is
    // still there (no spawn), so the box says "gone" on load, not on the click.
    else if (record.checkout) {
      checkoutStillThere(record.repo, record.checkout).then((there) => {
        if (!there && win.isConnected) markMissing(record.checkout);
      });
    }

    // `respawn`: the same desk id comes back at once, and the spawn announces
    // it. Announcing the gap would take the console out of its column
    // (ADR-0051 §5) and promote the next one to the maximize.
    const drop = (respawn?: boolean) => {
      win.remove();
      budget.untrackDormancy(win);
      wins.delete(win);
      applyExtent();
      if (!respawn) changed();
    };
    // The attach `restoreDesk` makes, into this record's id and rect. A session
    // another window drives parks this one as a watcher (`reconnectDecision`).
    const attach = (session: HostedSession) => {
      const carry = deskOf(win);
      drop(true);
      spawnWindow(
        { id: session.id, repo: session.repo },
        session.agent || "console",
        session.repo,
        carry,
      );
    };
    // One check at a time: a click and a resume can arrive together.
    let checking: Promise<HostedSession | null | undefined> | null = null;
    const check = () => {
      if (!checking) {
        checking = liveSessionFor(win, record)
          .catch(() => undefined)
          .finally(() => {
            checking = null;
          });
      }
      return checking;
    };
    win._revive = async () => {
      const session = await check();
      if (session && win.isConnected) attach(session);
    };
    btn.addEventListener("click", async (e: MouseEvent) => {
      e.stopPropagation();
      if (btn.disabled) return;
      btn.disabled = true;
      if (peerAction === "wake" && !(await wakeFirst())) return;
      const session = await check();
      if (!win.isConnected) return;
      if (session) {
        attach(session);
        return;
      }
      // A peer console whose list is not known may still run there: a launch
      // would start a second one on a peer without the record join. The box
      // says so and waits for the next read or click.
      if (session === undefined && daemon) {
        btn.disabled = false;
        unheard = true;
        showUnheard();
        return;
      }
      relaunchHere();
    });
    // The store's wake answers when the peer is usable, and reports its own
    // failure; the box then says again what it said. False when the click
    // stops here.
    async function wakeFirst() {
      text.textContent = `Waking ${peerHost(peerGroups.get(daemon), record.environment)}…`;
      const woke = typeof wakePeerNow() === "function" && (await wakePeerNow()!(daemon));
      if (!win.isConnected) return false;
      if (!woke) {
        btn.disabled = false;
        show(shown!);
        showPeer();
        return false;
      }
      return true;
    }
    // The agent menu's launch path, reusing this record's id, rect and
    // maximized state — in the recorded worktree unless that is the one that
    // is gone, in which case the button said "primary". For a local record
    // an unreadable session list launches too: the launch then fails as it
    // always did.
    function relaunchHere() {
      const carry = deskOf(win);
      markRelaunch(carry.id, () => drop(true));
      if (missing) carry.checkout = null;
      spawnOrMissing(
        relaunchRequest({ ...record, checkout: missing ? null : record.checkout }),
        record.agent,
        record.repo,
        carry,
      );
    }
    closeBtn.onclick = async () => {
      // Nothing is running here; the question says so rather than borrowing the
      // live console's warning.
      const ok = await askConfirm({
        title: "Close this console?",
        message: `This ${record.agent} console is not running. Close removes its window.`,
        confirmLabel: "Close",
        danger: true,
      });
      if (!ok) return;
      forgetRecord(win._deskId);
      drop();
      WB.emit("console-close", { repo: record.repo || null, agent: record.agent });
    };

    wins.add(win);
    budget.trackDormancy(win);
    changed();
    recordBirth(win, record);
    showPeer();
    return win;
  }

  return { isRelaunching, markRelaunch, spawnOrMissing, revivePlaceholders, spawnPlaceholder };
}
