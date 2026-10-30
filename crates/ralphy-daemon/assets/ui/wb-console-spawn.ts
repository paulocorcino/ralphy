/* ---------------------------------------------------------------------------
   The console's window birth: a live console window and its desk record.

   `createSpawn(deps)` builds a live console window (the chrome, the terminal,
   the parked and peer strips, the restart and the close) and writes what its
   birth and its session change in its desk record. `open` is the operator's
   new console, under the desk cap. It reads the console only
   through `deps`. The rules are `birthDecision` and `peerOfflineView` in
   `wb-console-session.ts`; the key bar is `wb-console-keys.ts` (#621).
--------------------------------------------------------------------------- */
import * as WBConsoleSession from "./wb-console-session.ts";
import { WBWindowState } from "./wb-window-state.ts";
import { WBFleet } from "./wb-fleet.ts";
import { WBSessionRoute } from "./wb-session-route.ts";
import { DESK_MAX } from "./wb-desk-folds.ts";
import type { Group } from "./wb-fleet.ts";
import type { TerminalOpts } from "./wb-console-terminal-types.d.ts";
import type { ConfirmOptions } from "./wb-console-title.ts";
import type { ChromeRecord } from "./wb-stage-chrome.ts";
import type { ConsoleTerm, ConsoleWin, DeskRecord, DeskSessionFields, DeskWindowFields, ExtentOpts, Presentation, Rect, SpawnCarry, WindowSnapshot } from "./wb-types.d.ts";

const { birthDecision, peerOfflineView, peerActionLabel, consoleCommand } = WBConsoleSession;
const { sessionIdOf, watchingOf, windowCheckout } = WBWindowState;

// What the window birth reads from the console, and nothing else. Every dep
// is built before the spawn, except the placeholder group, built after it
// (`wb-console-placeholder.ts`): lazy arrows.
export type SpawnDeps = {
  // The page the strips are made in.
  document: Document;
  // Every console window.
  wins: Set<ConsoleWin>;
  // The fleet's last answer, grouped by peer: one map for the life of the console.
  peerGroups: Map<string, Group>;
  // The store `projects`' wake of a peer and its fleet read, read at each use.
  wakePeerNow: () => ((daemon: string) => Promise<boolean>) | null;
  readFleetNow: () => (() => void) | null;
  // The window chrome: the frame, the titlebar, the body and its buttons.
  buildChrome: (
    label: string | null | undefined,
    repo: string | null | undefined,
    desk: ChromeRecord,
    kind: string | null | undefined,
  ) => { win: ConsoleWin; body: HTMLElement; title: HTMLElement; restartBtn: HTMLButtonElement; closeBtn: HTMLButtonElement };
  // The title: what it says, and its painting.
  sessionPresentation: (
    label: string | null | undefined,
    repo: string | null | undefined,
    prior: { daemonId?: string | null; environment?: string | null } | null | undefined,
    owner: { daemon_id?: string; environment?: string; name?: string | null; checkout?: string | null } | null | undefined,
  ) => Presentation;
  renderTitle: (win: ConsoleWin, title: HTMLElement, presentation: Presentation) => void;
  // The worktree switcher: its menu, the restart, and the repo's listing.
  closeCheckoutMenu: () => void;
  restartWin: (win: ConsoleWin) => Promise<void>;
  ensureListing: (ref: string | null | undefined, force?: boolean) => void;
  // Builds the key bar of a window and gives back its latch buttons.
  buildKeyBar: (win: ConsoleWin) => { ctrlBtn: HTMLButtonElement; shiftBtn: HTMLButtonElement; selBtn: HTMLButtonElement };
  // Sleeps or wakes a window (`wb-console-dormancy.ts`).
  applyDormancy: (win: ConsoleWin) => unknown;
  // Builds the terminal of a window.
  attachTerminal: (body: HTMLElement, opts: TerminalOpts) => ConsoleTerm;
  // The GPU budget: the watch a window joins at birth and leaves at close.
  budget: {
    dormancyWatch: () => IntersectionObserver | null;
    trackDormancy: (win: ConsoleWin) => void;
    untrackDormancy: (win: ConsoleWin) => void;
  };
  // The door's confirm, and its toast.
  askConfirm: (opts: ConfirmOptions) => Promise<unknown>;
  toast: (opts: { text: string }) => void;
  // The desk record writes (`createDeskRecords`), and whether the desk is full.
  createRecord: (win: ConsoleWin) => void;
  atDeskCap: () => boolean;
  setWin: (win: ConsoleWin, fields: DeskWindowFields) => void;
  forgetRecord: (deskId: string) => void;
  // The desk view, a fresh desk read, and a window whose record left (`createDesk`).
  currentDesk: () => DeskRecord[];
  reloadDesk: () => Promise<unknown>;
  recordLeft: (win: ConsoleWin) => void;
  // The box of a window, and the stage's extent.
  restoreRect: (el: HTMLElement) => Rect;
  applyExtent: (opts?: ExtentOpts) => void;
  // Tells the console that its window set changed.
  changed: () => void;
  // The placeholder group (`wb-console-placeholder.ts`). Built later: lazy arrows.
  spawnPlaceholder: (
    record: SpawnCarry,
    missing?: string | null,
    refused?: { message: string | null } | null,
    held?: { unheard?: boolean },
  ) => ConsoleWin;
  spawnOrMissing: (
    req: TerminalOpts,
    label: string | null | undefined,
    repo: string | null | undefined,
    carry: SpawnCarry,
  ) => Promise<ConsoleWin>;
  markRelaunch: (deskId: string, takeDown: () => void) => void;
};

export function createSpawn(deps: SpawnDeps) {
  const {
    document,
    wins,
    peerGroups,
    wakePeerNow,
    readFleetNow,
    buildChrome,
    sessionPresentation,
    renderTitle,
    closeCheckoutMenu,
    restartWin,
    ensureListing,
    buildKeyBar,
    applyDormancy,
    attachTerminal,
    budget,
    askConfirm,
    toast,
    createRecord,
    atDeskCap,
    setWin,
    forgetRecord,
    currentDesk,
    reloadDesk,
    recordLeft,
    restoreRect,
    applyExtent,
    changed,
    spawnPlaceholder,
    spawnOrMissing,
    markRelaunch,
  } = deps;

  // Build the chrome and attach a live terminal. Shared by `open()` and the
  // load-time restore; `termOpts` is the `attachTerminal` opts, `desk` the
  // record this window continues (absent for a fresh launch).
  function spawnWindow(termOpts: TerminalOpts, label: string | null | undefined, repo: string | null | undefined, desk?: SpawnCarry) {
    const kind = termOpts.console ? "console" : "agent";
    const { win, body, title, restartBtn, closeBtn } = buildChrome(label, repo, desk, kind);
    // Read at launch, never later: a rename reaches the next restart and never
    // restarts the running session (ADR-0066 §6).
    if (termOpts.id == null && !termOpts.console) termOpts = { ...termOpts, name: win._deskConsoleName };
    termOpts = launchRecord(termOpts, win);

    // The terminal owns the Ctrl and Shift latches and the selection arming;
    // the key-bar buttons (assigned below) only REFLECT them.
    let ctrlBtn: HTMLButtonElement | null = null;
    let shiftBtn: HTMLButtonElement | null = null;
    let selBtn: HTMLButtonElement | null = null;

    // Debounced nudge for a keystroke typed into a parked window (#335):
    // repeated typing EXTENDS the pulse rather than stacking timers.
    let nudgeTimer: ReturnType<typeof setTimeout> | null = null;
    function clearNudge() {
      if (nudgeTimer) {
        clearTimeout(nudgeTimer);
        nudgeTimer = null;
      }
      const strip = win.querySelector(".session-parked");
      if (!strip) return;
      strip.classList.remove("is-nudged");
      const hintEl = strip.querySelector(".session-parked-hint");
      if (hintEl) hintEl.textContent = "";
    }

    // A held window's strip: the fleet's sentence for the peer, its action, and
    // the daemon's diagnosis under Details. Built once, then reworded on each
    // fleet read.
    const peerDaemon = WBFleet?.refDaemon(repo) || "";
    const showPeerDown = (group: Group | null) => {
      let strip: HTMLElement | null = win.querySelector<HTMLElement>(".session-peer-down");
      if (!strip) {
        strip = document.createElement("div") as HTMLElement;
        strip.className = "session-peer-down";
        const text = document.createElement("span");
        text.className = "session-peer-down-text";
        const detail = document.createElement("details");
        detail.className = "session-detail";
        const summary = document.createElement("summary");
        summary.textContent = "Details";
        detail.append(summary, document.createElement("p"));
        const btn = document.createElement("button");
        btn.className = "session-reconnect";
        btn.addEventListener("click", (e: MouseEvent) => {
          e.stopPropagation();
          if (btn.dataset.act === "wake") wakePeerNow()?.(peerDaemon);
          else readFleetNow()?.();
        });
        strip.append(text, detail, btn);
        win.insertBefore(strip, body);
      }
      const view = peerOfflineView(group, null, win._deskEnvironment);
      strip.querySelector(".session-peer-down-text")!.textContent = view.text;
      const detail = strip.querySelector<HTMLElement>(".session-detail")!;
      detail.querySelector("p")!.textContent = view.detail || "";
      detail.hidden = !view.detail;
      const btn = strip.querySelector<HTMLElement>(".session-reconnect")!;
      btn.dataset.act = view.action || "";
      const label = peerActionLabel(view.action);
      btn.hidden = !label;
      if (label) btn.textContent = label;
    };

    // The parked strip: this window watches a session another window drives.
    // Built once; returns the strip.
    const showParked = (): HTMLElement => {
      const shown = win.querySelector<HTMLElement>(".session-parked");
      if (shown) return shown;
      const strip = document.createElement("div");
      strip.className = "session-parked";
      const text = document.createElement("span");
      // The titlebar already names the agent and the repo; repeating them here
      // wraps the strip on a phone.
      text.textContent = "Read-only. Another window has control.";
      const hint = document.createElement("span");
      hint.className = "session-parked-hint";
      const btn = document.createElement("button");
      btn.className = "session-reconnect";
      btn.dataset.act = "take-over";
      btn.textContent = "Take over";
      btn.addEventListener("click", (e: MouseEvent) => {
        e.stopPropagation();
        win._term?.takeOver();
      });
      strip.append(text, hint, btn);
      // The body read now, not `body`: a dormant window gets a new body node
      // (`sleepWindow`), and the old one is no longer a child of `win`.
      win.insertBefore(strip, win.querySelector(".session-body"));
      return strip;
    };

    // NAMED, not inline: a dormant console rebuilds its terminal (`wakeWindow`)
    // and the rebuild must be wired to the same chrome. Everything closes over
    // `win`, never a particular terminal.
    const termWiring: TerminalOpts = {
      ...termOpts,
      // The size rule's inputs. `_covered` is the cover `refreshCover` keeps.
      covered: () => win._covered === true,
      focused: () => win.classList.contains("focused"),
      onCtrlLatch: (on) => {
        if (ctrlBtn) ctrlBtn.setAttribute("aria-pressed", on ? "true" : "false");
      },
      onShiftLatch: (on) => {
        if (shiftBtn) shiftBtn.setAttribute("aria-pressed", on ? "true" : "false");
      },
      onSelecting: (on) => {
        if (selBtn) selBtn.setAttribute("aria-pressed", on ? "true" : "false");
      },
      // The daemon assigned/echoed this window's session id: record it.
      onSession: (_id, owner) => {
        const presentation = sessionPresentation(label, repo, desk, owner);
        win._sessionCheckout = presentation.checkout;
        win._deskDaemonId = presentation.daemonId;
        win._deskEnvironment = presentation.environment;
        // The announcement wins over the request and the record.
        win._deskCheckout = presentation.checkout;
        renderTitle(win, title, presentation);
        title.title = presentation.tooltip;
        recordSession(win);
        // Without an id the window could not sleep (`dormancyDecision` D5).
        applyDormancy(win);
      },
      // Parked: watching a session another window drives. It KEEPS its window
      // and output; the strip's button is the only way `takeover` is ever sent
      // (#334, ADR-0051 §9).
      onPark: () => {
        showParked();
      },
      // A watcher's keystroke never reaches the child (gated in
      // `attachTerminal`); this pulses the strip so the refusal is SEEN (#335).
      // A watcher built with no park (a dormant watcher that wakes) has no
      // strip yet, so the first refused key builds it (#751).
      onWatchedInput: () => {
        const strip = showParked();
        clearTimeout(nudgeTimer);
        strip.classList.add("is-nudged");
        const hintEl = strip.querySelector(".session-parked-hint");
        if (hintEl) hintEl.textContent = "Input is read-only. Take over to type.";
        nudgeTimer = setTimeout(() => {
          nudgeTimer = null;
          strip.classList.remove("is-nudged");
          if (hintEl) hintEl.textContent = "";
        }, 2000);
      },
      onResume: () => {
        clearNudge();
        win.querySelector(".session-parked")?.remove();
      },
      // A session that ENDED: the parked strip's "take over" would only spin
      // at a dead id. The window stays (its scrollback is the last thing the
      // agent said) and the restart control is tinted as the next action.
      // A launch a PEER refused becomes that peer's placeholder, which says why
      // in words and comes back when the peer does.
      onEnded: (announced, refusal) => {
        clearNudge();
        win.querySelector(".session-parked")?.remove();
        if (announced === "refused" && WBFleet?.refDaemon(repo)) {
          const carry = deskOf(win);
          discard();
          spawnPlaceholder(carry, null, { message: refusal });
          return;
        }
        win.classList.add("ended");
      },
      // A peer project: the fleet's word on its peer gates every reattach.
      ...(peerDaemon
        ? {
            peerGroup: () => peerGroups.get(peerDaemon) || null,
            readFleet: () => readFleetNow()?.(),
          }
        : {}),
      onPeerHold: (group) => showPeerDown(group),
      onPeerBack: () => win.querySelector(".session-peer-down")?.remove(),
      onLink: (state) => {
        switch (state) {
          case "lost": {
            if (win.querySelector(".session-link-lost")) return;
            const strip = document.createElement("div");
            strip.className = "session-link-lost";
            strip.textContent = "Connection lost. Reconnecting…";
            win.insertBefore(strip, body);
            return;
          }
          case "back":
            win.querySelector(".session-link-lost")?.remove();
            return;
          default: {
            const unreachable: never = state;
            return unreachable;
          }
        }
      },
    };
    win._termWiring = termWiring;
    // The fleet reaches a live window here; `wakeWindow` replaces `_term`, so
    // it is read at each call. A dormant window has none, and its wake asks.
    win._peerRefresh = () => win._term?.peerRefresh();
    if (birthDecision({ id: termOpts.id, observed: !!budget.dormancyWatch() }) === "dormant") {
      // The state `sleepWindow` leaves. `_visible` is false until the observer
      // reports: a window not yet reported reads as visible, and any
      // `applyDormancy` before the report would wake it.
      win._dormantSession = termOpts.id!;
      win._dormantWatch = termOpts.watch;
      win._dormant = true;
      win._visible = false;
      win.classList.add("dormant");
    } else {
      // On the window, not in a local: after a sleep/wake cycle a captured
      // local would name a disposed terminal.
      win._term = attachTerminal(body, termWiring);
    }
    // The placeholder's relaunch path: carry this window's record, drop the
    // dead window, spawn a FRESH session — never the old `id`/`watch` opts,
    // which would reattach to a torn-down session. `checkout` CHOSEN by the
    // title's switcher (#412); `undefined` means "the recorded one".
    // Take this window off the stage without announcing a gap: a window with
    // the same desk id takes its place at once.
    const discard = () => {
      clearNudge();
      closeCheckoutMenu();
      win._term?.dispose("window-closed");
      win.remove();
      budget.untrackDormancy(win);
      wins.delete(win);
      applyExtent();
    };
    const relaunchIn = (checkout: string | null | undefined) => {
      // A restart is the operator's act: an adopted console gets its record.
      const carry = { ...deskOf(win), unrecorded: false };
      markRelaunch(carry.id, discard);
      // `win._deskKind`, not the local `kind`: a window reattached at load was
      // spawned with `{id, repo}` only. `~` is the daemon's repo-less label.
      const plain = win._deskKind === "console";
      const at = repo === "~" ? undefined : repo;
      // A window reattached at load has no `termOpts.checkout`; the recorded
      // announcement, else the desk record, keeps the restart in its tree (#411).
      const fresh = plain
        ? { console: true, repo: at, command: consoleCommand(label) }
        : {
            repo: at,
            agent: termOpts.agent ?? label,
            // Explicit target, else `windowCheckout`: announcement beats
            // request beats record.
            checkout:
              checkout !== undefined ? checkout : windowCheckout(win, termOpts.checkout),
          };
      spawnOrMissing(fresh, label, repo, carry);
      WB.emit("console-restart", { repo: at || null, agent: plain ? null : label });
    };
    win._relaunchIn = relaunchIn;
    restartBtn.addEventListener("click", (e: MouseEvent) => {
      e.stopPropagation();
      restartWin(win);
    });
    // The switcher needs the repo's listing; one read per ref, cached.
    if (kind === "agent") ensureListing(repo);
    // The id this window is attaching to, known before the terminal reports one.
    if (termOpts.id != null) win._wantsSession = termOpts.id;

    // THE KEY BAR is `wb-console-keys.ts`.
    ({ ctrlBtn, shiftBtn, selBtn } = buildKeyBar(win));

    closeBtn.onclick = async () => {
      // A dormant window has no handle; it carried both answers across the gap.
      const id = sessionIdOf(win);
      const watching = watchingOf(win);
      // A watcher's × closes only its own window; the writer's ends the session.
      const ok = await askConfirm({
        title: "Close this console?",
        message: watching
          ? `Closes this window only. ${label} keeps running.`
          : `Ends the ${label} session. You lose the text in this console.`,
        confirmLabel: "Close",
        danger: true,
      });
      if (!ok) return;
      const finish = () => {
        // A window closed mid-pulse must not leave `nudgeTimer` pending.
        clearNudge();
        win._term?.dispose("window-closed");
        // A watcher's × closes this window only: the console still runs, and
        // its record stays on the desk.
        if (!watching) forgetRecord(win._deskId);
        win.remove();
        budget.untrackDormancy(win);
        wins.delete(win);
        applyExtent();
        WB.emit("console-close", { repo: repo || null, agent: label });
        changed();
      };
      // End the daemon-owned session first, then drop the window. A WATCHER
      // closes only its own window: `/api/sessions/close` tree-kills the child
      // another operator is driving.
      if (id != null && !watching) {
        fetch(WBSessionRoute.closeUrl(id, win._deskRepo), {
          method: "POST",
        }).then(
          (response) => {
            if (WBSessionRoute.closeSucceeded(response.status)) finish();
            else
              win._term?.term.write(
                `\r\n[close failed — the daemon refused it]\r\n`,
              );
          },
          () => win._term?.term.write("\r\n[close failed — connection unavailable]\r\n"),
        );
      } else {
        finish();
      }
    };

    wins.add(win);
    budget.trackDormancy(win);
    changed();
    recordBirth(win, desk);
    return win;
  }

  // What a birth writes. A window whose record is in the view writes only the
  // fields its birth changed (a relaunch in the primary tree, a name the
  // record lacked) — never its rect, which came from the record. A window
  // carried from an adopted, unrecorded one stays unrecorded. Any other
  // window is a new console: its record is created.
  function recordBirth(win: ConsoleWin, carry: SpawnCarry | undefined) {
    const r = currentDesk().find((x) => x.id === win._deskId);
    if (r) {
      const fields = birthFields(r, win);
      if (Object.keys(fields).length) setWin(win, fields);
      return;
    }
    if (carry?.unrecorded) {
      win._deskUnrecorded = true;
      setTimeout(() => adoptOrphan(win), ADOPT_GRACE_MS);
      return;
    }
    createRecord(win);
  }

  // How long an adopted console waits for the record the page that launched
  // it writes. A launch and its create land within a second; a console whose
  // record no page will write (another device removed it) still gets one.
  const ADOPT_GRACE_MS = 10000;
  function adoptOrphan(win: ConsoleWin) {
    if (!win.isConnected || !win._deskUnrecorded) return;
    // A fresh read first: the record may have landed without a push here.
    reloadDesk().then(() => {
      if (!win.isConnected || !win._deskUnrecorded) return;
      if (currentDesk().some((r) => r.id === win._deskId)) return;
      recordLeft(win);
    });
  }

  // The session the daemon announced, and the worktree with it, written only
  // when they differ from the record: a reconnect changes nothing else.
  function recordSession(win: ConsoleWin) {
    const r = currentDesk().find((x) => x.id === win._deskId);
    if (!r) return;
    const fields: DeskWindowFields = {};
    const session = {
      sessionId: sessionIdOf(win),
      daemonId: win._deskDaemonId ?? null,
      environment: win._deskEnvironment ?? null,
    };
    if (sessionDiffers(r, session)) fields.session = session;
    if (checkoutDiffers(r, win)) fields.checkout = win._deskCheckout ?? null;
    if (Object.keys(fields).length) setWin(win, fields);
  }

  // The window's placement as a desk record, to carry identity and box across
  // a rebuild (takeover, placeholder → live console).
  function deskOf(win: ConsoleWin): WindowSnapshot {
    return {
      unrecorded: !!win._deskUnrecorded,
      id: win._deskId,
      repo: win._deskRepo,
      agent: win._deskAgent,
      kind: win._deskKind,
      daemonId: win._deskDaemonId,
      environment: win._deskEnvironment,
      checkout: win._deskCheckout ?? null,
      locked: !!win._deskLocked,
      consoleName: win._deskConsoleName ?? null,
      rect: restoreRect(win),
      max: win.classList.contains("maximized"),
    };
  }

  // `agent` names an adapter (claude/codex/opencode); when `plain` is set there
  // is no agent — a normal shell in the repo dir, labelled "console", or, with
  // `command`, a shell running that command and labelled by it (the same label
  // the daemon gives the session, so the desk record relaunches it as such).
  function open({ repo, agent, plain, checkout, command }: { repo?: string | null; agent?: string | null; plain?: boolean; checkout?: string | null; command?: string }) {
    if (atDeskCap()) {
      toast({ text: `You can have at most ${DESK_MAX} consoles. Close one first.` });
      return false;
    }
    const label = agent || (plain && command) || "console";
    // The plain console ignores the checkout: it rides the repo path (on a
    // peer, `wsl.exe --cd`) and stays on the primary.
    spawnWindow(plain ? { console: true, repo, command } : { repo, agent, checkout }, label, repo);
    WB.emit("console-open", { repo: repo || null, agent: agent || null, plain: !!plain });
    return true;
  }

  return { spawnWindow, recordBirth, recordSession, deskOf, open };
}

// The terminal options of a launch with the record of the window `win`, and
// the worktree intent written on the window.
function launchRecord(termOpts: TerminalOpts, win: ConsoleWin): TerminalOpts {
  // Every NEW launch names its record: the daemon keeps one session per
  // record, so a second page relaunching it joins this one (ADR-0050
  // amendment 2026-10-04).
  if (termOpts.id == null) termOpts = { ...termOpts, record: win._deskId };
  // A launch that names a worktree records the intent NOW, so a daemon that
  // dies mid-launch still leaves it behind.
  if (termOpts.checkout !== undefined) win._deskCheckout = termOpts.checkout ?? null;
  return termOpts;
}

// The fields of the record `r` that the birth of `win` changed: a relaunch in
// another tree, a name the record lacked.
function birthFields(r: DeskRecord, win: ConsoleWin): DeskWindowFields {
  const fields: DeskWindowFields = {};
  if (checkoutDiffers(r, win)) fields.checkout = win._deskCheckout ?? null;
  if (win._deskConsoleName && r.consoleName !== win._deskConsoleName) {
    fields.consoleName = win._deskConsoleName;
  }
  return fields;
}

// The window is in another checkout than its record names.
function checkoutDiffers(r: DeskRecord, win: ConsoleWin) {
  return (r.checkout ?? null) !== (win._deskCheckout ?? null);
}

// The session the daemon announced is not the one the record names.
function sessionDiffers(r: DeskRecord, session: DeskSessionFields) {
  return (
    (r.sessionId ?? null) !== session.sessionId ||
    (r.daemonId ?? null) !== session.daemonId ||
    (r.environment ?? null) !== session.environment
  );
}
