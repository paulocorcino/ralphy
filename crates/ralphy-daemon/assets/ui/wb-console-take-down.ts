/* ---------------------------------------------------------------------------
   Taking a console window off the stage before its replacement is spawned: a
   restart (`wb-console-spawn.ts`) and a placeholder's Launch or Attach
   (`wb-console-placeholder.ts`) both do it the same way.
--------------------------------------------------------------------------- */
import { devError } from "./wb-log.ts";
import type { ConsoleWin } from "./wb-types.d.ts";

// What the take-down touches: the stage's windows, the GPU budget's watch,
// and the layout.
export type TakeDownDeps = {
  wins: Set<ConsoleWin>;
  budget: { untrackDormancy: (win: ConsoleWin) => void };
  applyExtent: () => void;
};

// Removes `win` and forgets it, then lays the stage out again. The layout wakes
// the siblings `win` covered, and a throw there must not reach the caller, which
// spawns the replacement next (#904, #925).
export function takeOffStage(deps: TakeDownDeps, win: ConsoleWin, what: string): void {
  win.remove();
  deps.budget.untrackDormancy(win);
  deps.wins.delete(win);
  try {
    deps.applyExtent();
  } catch (e) {
    devError(`${what}: the layout after the take-down failed`, e);
  }
}
