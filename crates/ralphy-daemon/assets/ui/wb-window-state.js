/* ---------------------------------------------------------------------------
   ralphy workbench — what a console window IS, and what it HOLDS

   A console window is a `.session-window` element, and every fact about it is
   an underscore-prefixed property hung off that element. Twenty-three of them.
   Before this module nothing declared the set, and the cost was measurable:

     - `_sessionOwner` and `_sessionEnvironment` were written on every session
       announcement and read nowhere in the repo. Nothing noticed, because an
       unused field cannot look wrong where the field set is never stated.

     - "what session is this window holding?" had FOUR formulas across six
       sites, split into two camps that never consulted each other: three read
       the id carried across dormancy, three read the id the window was spawned
       to attach to, none read both. Two of the three defects found while
       building dormant consoles were exactly that — a sleeping window whose
       desk record demoted itself to a placeholder, and a sleeping window
       "go to session" could not find, so it opened a second one.

   So this module holds two things and nothing else: the INVENTORY (`FIELDS`,
   plus `initWindow` as the one place a window is born with it) and the
   ACCESSORS for the three questions that had more than one source. It is
   deliberately NOT a lifecycle state machine: of 45 fix commits on
   `wb-console.js` in twelve months, 11 were layout-state and 10 were
   gesture/renderer, so a lifecycle fold would target the minority and the
   guards that closed those are already in the code.

   NOTHING here may reach for the browser's own per-origin store — "window
   state" is a name that invites exactly that mistake, and that store belongs
   to `wb-view.js` alone (ADR-0050: the desk layout is daemon state, and #339
   sweeps the whole tree for a second one).
--------------------------------------------------------------------------- */
window.WBWindowState = (function () {
  // THE INVENTORY. Every property a `.session-window` may carry, with the value
  // it is born holding. Grouped by who owns the write, because that is the
  // question a reader actually arrives with.
  const FIELDS = {
    // The desk record this window IS (ADR-0050). Seeded from a restored record
    // so a window carries its identity before any socket answers.
    _deskId: null,
    _deskRepo: "~",
    _deskAgent: null,
    _deskKind: null,
    _deskDaemonId: null,
    _deskEnvironment: null,
    _deskCheckout: null,
    _deskLocked: false,

    // What the DAEMON announced, on `session-open`. Distinct from the `_desk*`
    // mirrors above on purpose: `_deskCheckout` is also written at spawn, by
    // the launch request and by the checkout switcher, so only
    // `_sessionCheckout` means "the daemon said so" — which is why
    // `checkoutOf` can give it precedence.
    _sessionCheckout: null,
    _presentation: null,

    // Chrome handles: nodes and closures built once, reused by every terminal
    // this window will ever hold (a dormant console rebuilds its terminal, not
    // its chrome).
    _title: null,
    _stateDot: null,
    _rewire: null,
    _relaunchIn: null,
    _termWiring: null,

    // The live terminal, and the agent state its session last reported
    // (ADR-0059). `_term` is null whenever the window is a placeholder or
    // asleep — every read of it must tolerate that.
    _term: null,
    _agentState: null,

    // INTENT: the id this window was spawned to attach to, known before any
    // terminal reports one. Without it `reach` misses its own window.
    _wantsSession: null,

    // Dormancy: a console scrolled off the viewport disposes its renderer and
    // carries its answers across the gap here.
    _visible: true,
    _dormant: false,
    _dormantSession: null,
    _dormantWatch: false,
    _dormantTimer: null,
  };

  const NAMES = Object.keys(FIELDS);

  // The one place a console window is born. Writes EVERY field, so a reader of
  // any window sees the whole set rather than whichever subset a code path
  // happened to reach — and a `seed` key that is not in the inventory throws
  // rather than quietly adding a twenty-fourth field nothing declares.
  function initWindow(win, seed) {
    for (const name of NAMES) win[name] = FIELDS[name];
    if (seed) {
      for (const name of Object.keys(seed)) {
        if (!(name in FIELDS)) {
          throw new Error(`wb-window-state: unknown window field ${name}`);
        }
        win[name] = seed[name];
      }
    }
    return win;
  }

  // THE question, with one answer. In precedence order:
  //   1. the live terminal — the only source that is certainly current;
  //   2. the id carried across dormancy — the window still holds that session,
  //      it just has no renderer to ask;
  //   3. the id it was spawned to attach to — `_term.sessionId` lands only on
  //      the first terminal frame, and the daemon skips the replay frame for a
  //      session that has printed nothing, so a brand-new console reads null
  //      from (1) for as long as it stays silent.
  function sessionIdOf(win) {
    if (!win) return null;
    return win._term?.sessionId ?? win._dormantSession ?? win._wantsSession ?? null;
  }

  // Is this window a WATCHER — parked on a session another window drives, with
  // no writer slot (ADR-0051 §9)? Same shape as `sessionIdOf`: the live handle
  // first, the flag carried across dormancy second. A window with neither has
  // no session to watch, so `false` is the honest answer, not `undefined`.
  function watchingOf(win) {
    if (!win) return false;
    if (win._term) return !!win._term.watching;
    return !!win._dormantWatch;
  }

  // Which worktree this WINDOW's console lives in (ADR-0063). The daemon's
  // announcement is the truth about where a console RUNS, so it beats what the
  // caller asked for, which beats what the desk recorded. An explicit switch
  // target is NOT part of this chain — that is the caller overriding the
  // question, not answering it, and it stays at the call site.
  //
  // Named for the window on purpose: `wb-console.js` has long had a
  // `checkoutOf(ref)` that answers a DIFFERENT question — which checkout a
  // repo ref is currently pointed at, which is a project-wide selection rather
  // than a fact about one console.
  function windowCheckout(win, asked) {
    if (!win) return asked ?? null;
    return win._sessionCheckout ?? asked ?? win._deskCheckout ?? null;
  }

  return { FIELDS, initWindow, sessionIdOf, watchingOf, windowCheckout };
})();
