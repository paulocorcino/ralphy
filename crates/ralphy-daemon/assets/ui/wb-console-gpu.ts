/* ---------------------------------------------------------------------------
   The console's dormancy watch and GPU budget (ADR-0075 phase 5).

   `createGpuBudget(deps)` returns one budget with new state: its own observer
   and its own queued rebalance (ADR-0075 D7). It reads the console only
   through `deps`, and `GpuBudgetDeps` lists every read, so `tsc` refuses a
   read outside it. `wb-console.ts` creates one budget per console and keeps
   what reaches the terminals: sleeping and waking a window, and the covered
   rule.

   `gpuHolders` and the constants are members of `window.WBConsole` under
   their own name.
   --------------------------------------------------------------------------- */
import type { ConsoleWin } from "./wb-types.d.ts";

// ---- dormant consoles ----------------------------------------------------
// Every console costs an xterm buffer, a ResizeObserver, a WebGL context
// while it holds one (`rebalanceGpu`), and the parse+paint of every byte the
// daemon sends, visible or not.
//
// So a window off the viewport long enough disposes its terminal and closes
// its socket, and rebuilds on return. A window under columns, a maximize or
// the physical screen counts as off the viewport (ADR-0051 §9, covered
// amendment). The SESSION is untouched — child, PTY and
// scrollback are the daemon's (session.rs) and the reattach replays them; same
// "dispose the terminal, keep the record" as `tearDownMember`, releasing the
// writer slot the same way. A dormant console wakes by the ORDINARY attach and
// never sends `takeover`, so a session claimed meanwhile lands in the parked
// state of ADR-0051 §9.
//
// Dormancy is runtime state of THIS client only: never persisted, never on the
// desk record (ADR-0050), never told to the daemon.
//
// One-sided on purpose: slow to sleep, instant to wake, and the margin brings a
// window back a screenful before it could be seen — panning stays free.
export const DORMANT_AFTER_MS = 15000;
export const DORMANT_MARGIN_PX = 300;

// ---- the GPU budget ------------------------------------------------------
// LIMIT: Chrome keeps 16 live WebGL contexts per renderer process and drops
// the oldest past it. A desk restored as a cascade has every console seen
// and uncovered at once (measured: 20 consoles, 4 contexts lost, Chrome on
// Windows, 2026-10-04). So the page hands out at most GPU_BUDGET contexts,
// to the windows on top; the others draw with the DOM renderer. The budget
// is under 16 because the detached-fence popup has its own budget and can
// share the renderer process.
export const GPU_BUDGET = 12;

// The indexes of the windows that hold a context, pure and tabled. Each
// window is {seen, covered, hasTerminal, z}: only a seen, uncovered window
// with a terminal is a candidate, and the highest `z` win (focus raises a
// window to the top). Ties keep the input order.
export function gpuHolders(windows: GpuCandidate[], budget: number) {
  return windows
    .map((w, i) => ({ ...w, i }))
    .filter((w) => w.seen && !w.covered && w.hasTerminal)
    .sort((a, b) => b.z - a.z)
    .slice(0, budget)
    .map((w) => w.i);
}

// What `gpuHolders` reads of one window.
export type GpuCandidate = { seen: boolean; covered: boolean; hasTerminal: boolean; z: number };

// What the budget reads from the console, and nothing else.
export type GpuBudgetDeps = {
  // The live windows on the plane.
  wins: ReadonlySet<ConsoleWin>;
  // The viewport, the observer's root; null before the page has it.
  workspace: () => HTMLElement | null;
  isCovered: (win: ConsoleWin) => boolean;
  // Stays in the console: it reaches `wakeWindow` and the terminal factory.
  applyDormancy: (win: ConsoleWin) => unknown;
  // The page's; absent in a page without it, and then the watch is inert.
  IntersectionObserver: typeof IntersectionObserver | undefined;
  queueMicrotask: (task: () => void) => void;
};

export function createGpuBudget(deps: GpuBudgetDeps) {
  const { wins, workspace, isCovered, applyDormancy } = deps;

  // Built on first use: `#workspace` is not in the document when this module
  // evaluates. Without `IntersectionObserver` the feature is inert.
  let dormancyObserver: IntersectionObserver | null = null;
  function dormancyWatch() {
    if (dormancyObserver) return dormancyObserver;
    const IntersectionObserver = deps.IntersectionObserver;
    if (typeof IntersectionObserver !== "function") return null;
    const root = workspace();
    if (!root) return null;
    dormancyObserver = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          (entry.target as ConsoleWin)._visible = entry.isIntersecting;
          applyDormancy(entry.target as ConsoleWin);
        }
      },
      { root, rootMargin: `${DORMANT_MARGIN_PX}px`, threshold: 0 },
    );
    return dormancyObserver;
  }
  function trackDormancy(win: ConsoleWin) {
    const watch = dormancyWatch();
    if (watch) watch.observe(win);
    else {
      // Nothing will ever report this window seen.
      win._visible = true;
      scheduleGpu();
    }
  }
  // Paired with every `wins.delete`: the observer holds its targets, so a window
  // taken off the plane without this stays reachable for the life of the page.
  function untrackDormancy(win: ConsoleWin) {
    if (win._dormantTimer) {
      clearTimeout(win._dormantTimer);
      win._dormantTimer = null;
    }
    dormancyObserver?.unobserve(win);
    // Its context, if it had one, goes to the next window in line.
    scheduleGpu();
  }

  // Coalesced: a restore asks once per window, and the drops must run before
  // the loads so the page never holds more than the budget.
  let gpuQueued = false;
  function scheduleGpu() {
    if (gpuQueued) return;
    gpuQueued = true;
    deps.queueMicrotask(() => {
      gpuQueued = false;
      rebalanceGpu();
    });
  }
  function rebalanceGpu() {
    const list = [...wins];
    const keep = new Set<ConsoleWin>(
      gpuHolders(
        list.map((w) => ({
          // `=== true`, not the dormancy fold's reading: an unobserved window
          // is not yet seen.
          seen: w._visible === true,
          covered: isCovered(w),
          hasTerminal: !!w._term,
          z: parseInt(w.style.zIndex, 10) || 0,
        })),
        GPU_BUDGET,
      ).map((i) => list[i]),
    );
    for (const w of list) if (!keep.has(w)) w._term?.dropGpu();
    for (const w of keep) w._term!.useGpu();
  }

  return { trackDormancy, untrackDormancy, scheduleGpu, dormancyWatch };
}
