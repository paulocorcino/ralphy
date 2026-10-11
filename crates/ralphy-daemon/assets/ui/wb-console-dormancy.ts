/* ---------------------------------------------------------------------------
   The console's dormancy: sleep, wake, the cover rule and the resume.

   `createDormancy(deps)` sleeps a window off the viewport and wakes it on
   return, keeps the covered rule, and resumes every socket when the page
   comes back from a suspend. Building it registers no listener: `listen()`
   registers the page listeners, and the console calls it once. It reads the
   console only through `deps`. The rules are `dormancyDecision` in
   `wb-console-session.ts`; the watch and the GPU budget are
   `wb-console-gpu.ts` (#621).
--------------------------------------------------------------------------- */
import * as WBConsoleInput from "./wb-console-input.ts";
import * as WBConsoleSession from "./wb-console-session.ts";
import { DORMANT_AFTER_MS } from "./wb-console-gpu.ts";
import type { WBDeskSync } from "./wb-desk-sync.ts";
import type { TerminalOpts } from "./wb-console-terminal.ts";
import type { ConsoleTerm, ConsoleWin } from "./wb-types.d.ts";

const { keyboardInset } = WBConsoleInput;
const { dormancyDecision, RESUME_HIDDEN_MS } = WBConsoleSession;

// What the dormancy reads from the console, and nothing else.
export type DormancyDeps = {
  // The console's page: the `online` listener and the visual viewport.
  window: Window;
  // The page the listeners and the fresh session bodies are made in.
  document: Document;
  // Every console window.
  wins: Set<ConsoleWin>;
  // The desk changes not sent yet: the `online` listener sends them.
  sync: Pick<ReturnType<typeof WBDeskSync.createSync>, "hasPending">;
  // The stale probe the shell fed, read at each resume.
  staleProbeNow: () => (() => boolean) | null;
  // Shows or hides the key bar of a window. Built later: a lazy arrow.
  applyKeyBar: (win: ConsoleWin) => void;
  // The GPU budget. Built later: a lazy arrow.
  budget: () => { scheduleGpu: () => void };
  // Whether a window fills the viewport. Built later: a lazy arrow.
  fillsViewport: (win: ConsoleWin) => boolean;
  // Whether a window is in full screen. Built later: a lazy arrow.
  isFull: (win: HTMLElement) => boolean;
  // Builds the terminal of a window. Built later: a lazy arrow.
  attachTerminal: (body: HTMLElement, opts: TerminalOpts) => ConsoleTerm;
  // Sends the desk changes after `ms`. Built later: a lazy arrow.
  scheduleDeskFlush: (ms?: number) => void;
  // Brings every placeholder whose console runs back as a window. Built later: a lazy arrow.
  revivePlaceholders: () => Promise<void>;
};

export function createDormancy(deps: DormancyDeps) {
  const {
    window,
    document,
    wins,
    sync,
    staleProbeNow,
    applyKeyBar,
    budget,
    fillsViewport,
    isFull,
    attachTerminal,
    scheduleDeskFlush,
    revivePlaceholders,
  } = deps;
  // Coming back from a suspend. Registered in EVERY document that runs this
  // module (shell and each popup): each owns the sockets of the windows it
  // paints, and nothing else would revive them.
  let hiddenAt = 0;

  // The verdict the probe gives, or — with no probe, which is the popup — how
  // long this document was hidden. `Infinity` for the network events: `online`
  // fires precisely because the link the sockets ran over is a different link now.
  function isStale(hiddenMs: number) {
    const staleProbe = staleProbeNow();
    if (staleProbe) {
      try {
        return staleProbe() === true;
      } catch {
        return true;
      }
    }
    return hiddenMs > RESUME_HIDDEN_MS;
  }

  function resumeAll(stale: boolean) {
    let woke = 0;
    for (const w of wins) {
      // A placeholder has no terminal, and a window whose session ended latches
      // itself — both decline from inside `resume`.
      if (w._term?.resume(stale)) woke += 1;
    }
    return woke;
  }

  // Dispose the renderer, keep the window: frame, title, state dot (fed by the
  // `/api/sessions` poll, not this socket) and desk record are untouched.
  //
  // The `.session-body` is REPLACED, not reused: `attachTerminal` registers its
  // touch handlers on the body node and `term.dispose()` does not remove them,
  // so attaching twice into one div would double every gesture.
  function sleepWindow(win: ConsoleWin) {
    const t = win._term;
    if (!t) return false;
    // Carried across the gap, because the handle that knows them is about to go.
    win._dormantSession = t.sessionId;
    win._dormantWatch = t.watching;
    // `reach` finds a window by `_term.sessionId` OR `_wantsSession`. Without
    // this a "go to session" on a sleeping console would miss its own window
    // and spawn a SECOND one against the same id, which the daemon would park.
    if (t.sessionId != null) win._wantsSession = t.sessionId;
    t.dispose("dormant");
    win._term = null;
    win._dormant = true;
    win.classList.add("dormant");
    const stale = win.querySelector(".session-body");
    if (stale) {
      const fresh = document.createElement("div");
      fresh.className = "session-body";
      stale.replaceWith(fresh);
    }
    return true;
  }

  // Rebuild through the shipped factory with the wiring this window was born
  // with. NOT a takeover: the reattach is the ordinary one, so a session another
  // client claimed while this one slept parks visibly instead of being stolen
  // back (ADR-0051 §9).
  function wakeWindow(win: ConsoleWin) {
    if (!win._dormant) return false;
    const body = win.querySelector<HTMLElement>(".session-body");
    const wiring = win._termWiring;
    if (!body || !wiring || win._dormantSession == null) return false;
    win._dormant = false;
    win.classList.remove("dormant");
    win._term = attachTerminal(body, {
      ...wiring,
      id: win._dormantSession,
      watch: win._dormantWatch,
      takeover: false,
    });
    win._dormantSession = null;
    win._rewire?.(win._term);
    return true;
  }

  // Carry out `dormancyDecision` for one window. The fold owns the rule; this
  // owns the clock, and re-asks when the timer fires — the window may have been
  // focused, maximized or closed meanwhile.
  function applyDormancy(win: ConsoleWin) {
    const verdict = dormancyDecision(dormancyInputs(win));
    if (win._dormantTimer) {
      clearTimeout(win._dormantTimer);
      win._dormantTimer = null;
    }
    if (verdict === "wake") wakeWindow(win);
    budget().scheduleGpu();
    if (verdict === "sleep") {
      win._dormantTimer = setTimeout(() => {
        win._dormantTimer = null;
        if (dormancyDecision(dormancyInputs(win)) === "sleep") sleepWindow(win);
      }, DORMANT_AFTER_MS);
    }
    return verdict;
  }

  function isCovered(win: ConsoleWin) {
    if (fillsViewport(win)) return false;
    for (const other of wins) if (other !== win && fillsViewport(other)) return true;
    return false;
  }

  // Re-ask only the windows whose cover changed: `applyDormancy` restarts the
  // grace period, and the callers run on every columns repaint.
  function refreshCover() {
    for (const win of wins) {
      const covered = isCovered(win);
      if (covered === !!win._covered) continue;
      win._covered = covered;
      applyDormancy(win);
    }
  }

  // The live reading of one window, handed to the pure fold.
  function dormancyInputs(win: ConsoleWin) {
    return {
      // Unobserved windows have never been told; treat them as visible, which
      // is the reading that changes nothing.
      intersecting: win._visible !== false,
      covered: isCovered(win),
      dormant: !!win._dormant,
      maximized: win.classList.contains("maximized") || win.classList.contains("column"),
      fullscreen: isFull(win),
      focused: win.classList.contains("focused"),
      hasTerminal: !!win._term,
      ended: win.classList.contains("ended"),
      // NOT `sessionIdOf`: only these two sources survive `sleepWindow`. A
      // spawned-but-silent console with only `_wantsSession` would sleep with
      // nothing for `wakeWindow` to reattach to, and never come back.
      sessionId: win._term?.sessionId ?? win._dormantSession ?? null,
    };
  }

  // The page listeners. Registered once, by the console, after the desk
  // read has started.
  function listen() {
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState !== "visible") {
        hiddenAt = Date.now();
        return;
      }
      const hiddenMs = hiddenAt ? Date.now() - hiddenAt : 0;
      hiddenAt = 0;
      resumeAll(isStale(hiddenMs));
      revivePlaceholders();
    });
    // No `pageshow`: a document holding an open WebSocket is not bfcache-eligible,
    // so the restore this would catch cannot happen here.
    window.addEventListener("online", () => {
      resumeAll(true);
      revivePlaceholders();
      // Desk changes kept while the link was down go now.
      if (sync.hasPending()) scheduleDeskFlush(0);
    });

    // The key-bar setting changed. The shell re-emits every save on
    // `workbench:action`. A detached popup never receives it (its `WB.emit` posts
    // to the opener) and reads no store, so its bar stays on auto.
    document.addEventListener("workbench:action", (e: DocumentEventMap["workbench:action"]) => {
      if (e.detail?.action !== "setting-change" || e.detail.key !== "consoles.key_bar") return;
      for (const w of wins) applyKeyBar(w);
    });

    // Publish the keyboard inset. The popup and the node harness load this module
    // without `visualViewport`; absent, `--kb-inset` keeps the stylesheet's
    // `:root` default of 0px.
    const vv = window.visualViewport;
    if (vv) {
      const publishInset = () => {
        // iOS PANS rather than resizes: with the visual viewport scrolled down, a
        // window sized to the remaining height starts above the visible region,
        // titlebar included. Scroll back first, then measure; `offsetTop` is still
        // subtracted because the scroll lands a frame later.
        if (vv.offsetTop > 0) window.scrollTo(0, 0);
        const px = keyboardInset({
          innerHeight: window.innerHeight,
          height: vv.height,
          offsetTop: vv.offsetTop,
          scale: vv.scale,
        });
        document.documentElement.style.setProperty("--kb-inset", px + "px");
      };
      vv.addEventListener("resize", publishInset);
      vv.addEventListener("scroll", publishInset);
      // The inset must HEAL: on an iPad the keyboard rising kicks the document
      // out of fullscreen, a transition the visual viewport does not always
      // report, and a stale `--kb-inset` keeps every maximized console short.
      document.addEventListener("fullscreenchange", publishInset);
      window.addEventListener("orientationchange", publishInset);
      window.addEventListener("resize", publishInset);
      // A terminal that has LOST focus cannot be the reason a keyboard is up.
      document.addEventListener("focusout", () => setTimeout(publishInset, 250));
    }
  }

  return { resumeAll, applyDormancy, isCovered, refreshCover, listen };
}
