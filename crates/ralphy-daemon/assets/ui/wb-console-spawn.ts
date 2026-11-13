/* ---------------------------------------------------------------------------
   The console's window birth: a live console window and its desk record.

   `createSpawn(deps)` builds a live console window (the chrome, the terminal,
   the parked and peer strips, the restart and the close) and writes what its
   birth and its session change in its desk record. `open` is the operator's
   new console, under the desk cap. It reads the console only through `deps`.
   The rule is `birthDecision` in `wb-console-session.ts`; the key bar is
   `wb-console-keys.ts`; the strips are `wb-console-strips.ts`; the desk
   record is `wb-console-spawn-records.ts` (#621).
--------------------------------------------------------------------------- */
import * as WBConsoleSession from "./wb-console-session.ts";
import { WBWindowState } from "./wb-window-state.ts";
import { WBFleet } from "./wb-fleet.ts";
import { WBSessionRoute } from "./wb-session-route.ts";
import { DESK_MAX } from "./wb-desk-folds.ts";
import { createSpawnRecords } from "./wb-console-spawn-records.ts";
import { showLinkLost, showParked, showPeerDown } from "./wb-console-strips.ts";
import { takeOffStage } from "./wb-console-take-down.ts";
import type { Group } from "./wb-fleet.ts";
import type { TerminalOpts } from "./wb-console-terminal-types.d.ts";
import type { ConfirmOptions } from "./wb-console-title.ts";
import type { ChromeRecord } from "./wb-stage-chrome.ts";
import type {
  ConsoleTerm,
  ConsoleWin,
  DeskRecord,
  DeskWindowFields,
  ExtentOpts,
  Presentation,
  Rect,
  SpawnCarry,
} from "./wb-types.d.ts";

const { birthDecision, consoleCommand } = WBConsoleSession;
const { sessionIdOf, watchingOf, windowCheckout } = WBWindowState;

// A blank text is no value: the events carry `null` for it.
function orNull(text: string | null | undefined) {
  return text === null || text === undefined || text === "" ? null : text;
}

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
  ) => {
    win: ConsoleWin;
    body: HTMLElement;
    title: HTMLElement;
    restartBtn: HTMLButtonElement;
    closeBtn: HTMLButtonElement;
  };
  // The title: what it says, and its painting.
  sessionPresentation: (
    label: string | null | undefined,
    repo: string | null | undefined,
    prior: { daemonId?: string | null; environment?: string | null } | null | undefined,
    owner:
      | { daemon_id?: string; environment?: string; name?: string | null; checkout?: string | null }
      | null
      | undefined,
  ) => Presentation;
  renderTitle: (win: ConsoleWin, title: HTMLElement, presentation: Presentation) => void;
  // The worktree switcher: its menu, the restart, and the request for the
  // repo's listing.
  closeCheckoutMenu: () => void;
  restartWin: (win: ConsoleWin) => Promise<void>;
  ensureListing: (ref: string | null | undefined, force?: boolean) => void;
  // Builds the key bar of a window and gives back its latch buttons.
  buildKeyBar: (win: ConsoleWin) => {
    ctrlBtn: HTMLButtonElement;
    shiftBtn: HTMLButtonElement;
    selBtn: HTMLButtonElement;
  };
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

// The state `sleepWindow` leaves. `_visible` is false until the observer
// reports: a window not yet reported reads as visible, and any
// `applyDormancy` before the report would wake it.
function markDormant(win: ConsoleWin, termOpts: TerminalOpts) {
  win._dormantSession = termOpts.id ?? null;
  win._dormantWatch = termOpts.watch;
  win._dormant = true;
  win._visible = false;
  win.classList.add("dormant");
}

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
  const { recordBirth, recordSession, deskOf } = createSpawnRecords({
    createRecord,
    setWin,
    currentDesk,
    reloadDesk,
    recordLeft,
    restoreRect,
  });

  // Build the chrome and attach a live terminal. Shared by `open()` and the
  // load-time restore; `termOpts` is the `attachTerminal` opts, `desk` the
  // record this window continues (absent for a fresh launch).
  function spawnWindow(
    asked: TerminalOpts,
    label: string | null | undefined,
    repo: string | null | undefined,
    desk?: SpawnCarry,
  ) {
    const kind = asked.console === true ? "console" : "agent";
    const { win, body, title, restartBtn, closeBtn } = buildChrome(label, repo, desk, kind);
    // Read at launch, never later: a rename reaches the next restart and never
    // restarts the running session (ADR-0066 §6).
    const named =
      asked.id == null && asked.console !== true ? { ...asked, name: win._deskConsoleName } : asked;
    const termOpts = launchRecord(named, win);

    // The terminal owns the Ctrl and Shift latches and the selection arming;
    // the key-bar buttons (assigned below) only REFLECT them.
    let ctrlBtn: HTMLButtonElement | null = null;
    let shiftBtn: HTMLButtonElement | null = null;
    let selBtn: HTMLButtonElement | null = null;

    // Debounced nudge for a keystroke typed into a parked window (#335):
    // repeated typing EXTENDS the pulse rather than stacking timers.
    let nudgeTimer: ReturnType<typeof setTimeout> | null = null;
    function clearNudge() {
      if (nudgeTimer !== null) {
        clearTimeout(nudgeTimer);
        nudgeTimer = null;
      }
      const strip = win.querySelector(".session-parked");
      if (!strip) return;
      strip.classList.remove("is-nudged");
      const hintEl = strip.querySelector(".session-parked-hint");
      if (hintEl) hintEl.textContent = "";
    }

    const peerDaemon = WBFleet.refDaemon(repo);
    const peerStrip = { document, peerDaemon, wakePeerNow, readFleetNow };
    const parked = () => showParked(document, win);

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
        parked();
      },
      // A watcher's keystroke never reaches the child (gated in
      // `attachTerminal`); this pulses the strip so the refusal is SEEN (#335).
      // A watcher built with no park (a dormant watcher that wakes) has no
      // strip yet, so the first refused key builds it (#751).
      onWatchedInput: () => {
        const strip = parked();
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
        if (announced === "refused" && WBFleet.refDaemon(repo) !== "") {
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
            peerGroup: () => peerGroups.get(peerDaemon) ?? null,
            readFleet: () => readFleetNow()?.(),
          }
        : {}),
      onPeerHold: (group) => {
        showPeerDown(peerStrip, win, group);
      },
      onPeerBack: () => win.querySelector(".session-peer-down")?.remove(),
      onLink: (state) => {
        switch (state) {
          case "lost": {
            showLinkLost(document, win);
            return;
          }
          case "back":
            win.querySelector(".session-link-lost")?.remove();
            return;
          default: {
            const unreachable: never = state;
            void unreachable;
          }
        }
      },
    };
    win._termWiring = termWiring;
    // The fleet reaches a live window here; `wakeWindow` replaces `_term`, so
    // it is read at each call. A dormant window has none, and its wake asks.
    win._peerRefresh = () => win._term?.peerRefresh();
    if (
      birthDecision({ id: termOpts.id, observed: budget.dormancyWatch() !== null }) === "dormant"
    ) {
      markDormant(win, termOpts);
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
      takeOffStage({ wins, budget, applyExtent }, win, "console restart");
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
            checkout: checkout === undefined ? windowCheckout(win, termOpts.checkout) : checkout,
          };
      void spawnOrMissing(fresh, label, repo, carry);
      WB.emit("console-restart", { repo: orNull(at), agent: plain ? null : label });
    };
    win._relaunchIn = relaunchIn;
    restartBtn.addEventListener("click", (e: MouseEvent) => {
      e.stopPropagation();
      void restartWin(win);
    });
    // The switcher needs the repo's listing; the store reads it once per ref.
    if (kind === "agent") ensureListing(repo);
    // The id this window is attaching to, known before the terminal reports one.
    if (termOpts.id != null) win._wantsSession = termOpts.id;

    // THE KEY BAR is `wb-console-keys.ts`.
    ({ ctrlBtn, shiftBtn, selBtn } = buildKeyBar(win));

    const closeClicked = async () => {
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
      if (ok !== true) return;
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
        WB.emit("console-close", { repo: orNull(repo), agent: label });
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
            else win._term?.term.write(`\r\n[close failed — the daemon refused it]\r\n`);
            return null;
          },
          () => win._term?.term.write("\r\n[close failed — connection unavailable]\r\n"),
        );
      } else {
        finish();
      }
    };
    closeBtn.addEventListener("click", () => {
      void closeClicked();
    });

    wins.add(win);
    budget.trackDormancy(win);
    changed();
    recordBirth(win, desk);
    return win;
  }

  // `agent` names an adapter (claude/codex/opencode); when `plain` is set there
  // is no agent — a normal shell in the repo dir, labelled "console", or, with
  // `command`, a shell running that command and labelled by it (the same label
  // the daemon gives the session, so the desk record relaunches it as such).
  function open({
    repo,
    agent,
    plain = false,
    checkout,
    command,
  }: {
    repo?: string | null;
    agent?: string | null;
    plain?: boolean;
    checkout?: string | null;
    command?: string | undefined;
  }) {
    if (atDeskCap()) {
      toast({ text: `You can have at most ${DESK_MAX} consoles. Close one first.` });
      return false;
    }
    const label = orNull(agent) ?? (plain ? orNull(command) : null) ?? "console";
    // The plain console ignores the checkout: it rides the repo path (on a
    // peer, `wsl.exe --cd`) and stays on the primary.
    spawnWindow(plain ? { console: true, repo, command } : { repo, agent, checkout }, label, repo);
    WB.emit("console-open", { repo: orNull(repo), agent: orNull(agent), plain });
    return true;
  }

  return { spawnWindow, recordBirth, recordSession, deskOf, open };
}

// The terminal options of a launch with the record of the window `win`, and
// the worktree intent written on the window.
function launchRecord(asked: TerminalOpts, win: ConsoleWin): TerminalOpts {
  // Every NEW launch names its record: the daemon keeps one session per
  // record, so a second page relaunching it joins this one (ADR-0050
  // amendment 2026-10-04).
  const termOpts = asked.id == null ? { ...asked, record: win._deskId } : asked;
  // A launch that names a worktree records the intent NOW, so a daemon that
  // dies mid-launch still leaves it behind.
  if (termOpts.checkout !== undefined) win._deskCheckout = termOpts.checkout ?? null;
  return termOpts;
}
