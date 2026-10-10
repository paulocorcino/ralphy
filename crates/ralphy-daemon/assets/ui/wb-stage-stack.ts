/* ---------------------------------------------------------------------------
   The stage's z stack and the gestures, one of each per document.

   Console windows and note cards share one z counter: `focusWin` orders
   `.session-window` and `.note-card` together (ADR-0073, the 2026-10-10
   amendment, decision 2). `createStack(document)` holds the counter,
   `focusWin` and `stackWin`. The console's own work on a focus change goes
   in through `setFocusHook`.

   `createGestures()` owns the elements under a gesture of the operator. The
   chrome and the fence gestures both begin and end gestures, and the desk and
   the cards ask it, so the set has its own owner that each takes through its
   `deps`, and not one of them.

   Importing this module does nothing. Each entry module calls both once per
   page, before the console and the note cards, and passes them to both
   (ADR-0075 D7). The detached-fence popup has its own document, so it builds
   its own.
   --------------------------------------------------------------------------- */
import type { ConsoleWin, Stacked } from "./wb-types.d.ts";

// The console's work on a focus change. A page with no console sets none.
export type FocusHook = {
  // A window or a card that lost the focus to another one.
  blurred: (w: ConsoleWin) => void;
  // Runs once the focus has moved.
  focused: (win: Stacked) => void;
};

export function createStack(document: Document) {
  // The viewport (the scrolling box); the windows and the cards are inside it.
  const workspace = () => document.getElementById("workspace");
  let hook: FocusHook | null = null;

  // Focus stacking. `z` climbs each time a window is raised; when it reaches the
  // ceiling the whole stack is renormalized back down (preserving order) so the
  // console z-index never overtakes the runs overlay (z 150) or the tabbar.
  const Z_BASE = 60;
  const Z_CEIL = 120;
  let z = Z_BASE;

  // Give a surface a place in the window tier WITHOUT focusing it. A restore
  // builds its consoles through `buildChrome`, which ends in `focusWin` and so
  // hands every window a z; a note card is built by `WBNotes.render` and had
  // none, which put it at `auto` — BELOW every console (z ≥ 61). MEASURED: a
  // card restored beside a console was visible where nothing overlapped and
  // deaf where something did, because the click landed on the terminal's
  // canvas and the keystrokes went to the shell. A surface on the plane is in
  // the tier or it is under it; there is no third state.
  function stackWin(win: Stacked) {
    if (win.style.zIndex) return;
    // At the ceiling the counter stops and the newcomers tie: a tie among
    // cards is a stacking order, while a number past the ceiling would put a
    // card over the tab bar. The next `focusWin` renormalises the lot.
    if (z < Z_CEIL) z += 1;
    win.style.zIndex = z;
  }

  function focusWin(win: Stacked) {
    z += 1;
    if (z > Z_CEIL) {
      // Renormalize: re-stack the existing windows by their current z, resetting
      // the counter so focus never pushes a console over the overlay/tabbar tier.
      const ordered = [...workspace()!.querySelectorAll<Stacked>(".session-window, .note-card")].sort(
        (a, b) => (parseInt(a.style.zIndex, 10) || 0) - (parseInt(b.style.zIndex, 10) || 0),
      );
      z = Z_BASE;
      for (const w of ordered) {
        if (w === win) continue;
        z += 1;
        w.style.zIndex = z;
      }
      z += 1;
    }
    win.style.zIndex = z;
    for (const w of workspace()!.querySelectorAll<ConsoleWin>(".session-window.focused, .note-card.focused")) {
      if (w === win) continue;
      w.classList.remove("focused");
      hook?.blurred(w);
    }
    win.classList.add("focused");
    hook?.focused(win);
  }

  // The console sets its hook once, when it is built.
  function setFocusHook(next: FocusHook) {
    hook = next;
  }

  return { stackWin, focusWin, setFocusHook };
}

export type Stack = ReturnType<typeof createStack>;

// The elements under a gesture of the operator, from the press to the
// release: windows, fences, cards, and every member a fence move carries.
// `begin` at the press, `end` at the release; `active` is the question a desk
// this page takes asks before it moves an element.
export function createGestures() {
  const gestures = new Set<HTMLElement>();
  return {
    begin: (el: HTMLElement) => {
      gestures.add(el);
    },
    end: (el: HTMLElement) => {
      gestures.delete(el);
    },
    active: (el: HTMLElement) => gestures.has(el),
  };
}

export type Gestures = ReturnType<typeof createGestures>;
