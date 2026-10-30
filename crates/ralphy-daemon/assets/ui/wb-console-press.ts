/* ---------------------------------------------------------------------------
   The keyboard and mouse rules of one console's terminal: the copy key, the
   keys the page keeps for itself, and the press that is held until it is a
   click or a drag.

   `wirePress(deps)` adds them to the terminal and its window body.
   `wb-console-terminal.ts` calls it once per terminal; the fold it applies is
   `pressRoute` in `wb-console-input.ts`.
   --------------------------------------------------------------------------- */
import * as WBConsoleInput from "./wb-console-input.ts";

const { dragThreshold, dragBegins, pressRoute, forceSelectionKeys, holdMoveReport } =
  WBConsoleInput;

// What the press rules read from the terminal, and nothing else.
export type PressDeps = {
  term: XtermTerminal;
  body: HTMLElement;
  // The page: `getShell` tells whether the shell's key listener exists.
  window: Pick<Window, "getShell">;
  writeClipboard: (text: string, term: XtermTerminal) => void;
};

// Alt+Shift with no Ctrl and no Meta.
function altShiftOnly(e: KeyboardEvent) {
  return e.altKey && e.shiftKey && !e.ctrlKey && !e.metaKey;
}

// Whether the page, not the child, takes this chord.
function pageTakesChord(e: KeyboardEvent, body: HTMLElement, win: PressDeps["window"]) {
  if (!altShiftOnly(e)) return false;
  // Alt+Shift+arrows in a column walk the columns and their rows (ADR-0051
  // §5), and so in the grid of a console tab (ADR-0037 §3d): xterm must not
  // send them to the child, and the document listener of the columns
  // (wb-consoles-tab.ts) takes them.
  if (
    /^Arrow(Left|Right|Up|Down)$/.test(e.code) &&
    !!body.closest(".session-window.column, .session-window.tab-grid")
  ) {
    return true;
  }
  // Alt+Shift+R and Alt+Shift+<digit> open a console from inside a
  // terminal too. Only where the console menus' document listener exists
  // (wb-consoles-tab-menus.ts, on the page with a shell): a
  // detached popup has none, so its terminal keeps the key.
  return /^(?:Digit\d|KeyR)$/.test(e.code) && typeof win.getShell === "function";
}

export function wirePress({ term, body, window, writeClipboard }: PressDeps) {
  // Ctrl+Insert copies the selection. NOT Ctrl+Shift+C: on Chrome/Edge that
  // is the DevTools accelerator and a page cannot take it back. Ctrl+C
  // belongs to the child.
  term.attachCustomKeyEventHandler((e: KeyboardEvent) => {
    if (pageTakesChord(e, body, window)) return false;
    if (e.type !== "keydown" || !e.ctrlKey || e.shiftKey || e.altKey) return true;
    if (e.key !== "Insert" || !term.hasSelection()) return true;
    writeClipboard(term.getSelection(), term);
    return false;
  });
  // `pressRoute`, applied. The held press is REPLAYED to xterm as a
  // synthetic event: with the force-selection key once it turns into a
  // drag, or as itself (then the release) once it ends in place. The
  // replays are marked so this listener lets them through. The real release
  // is stopped: xterm adds its `mouseup` listener to the document during the
  // replayed press, and the real release would report a second time.
  const replayed = new WeakSet<Event>();
  const replay = (
    target: EventTarget | null,
    type: string,
    from: MouseEvent,
    keys: { altKey?: boolean; shiftKey?: boolean },
  ) => {
    const ev = new MouseEvent(type, {
      bubbles: true,
      cancelable: true,
      view: from.view,
      clientX: from.clientX,
      clientY: from.clientY,
      screenX: from.screenX,
      screenY: from.screenY,
      button: 0,
      buttons: type === "mousedown" ? 1 : 0,
      // xterm's selection starts only on `detail === 1` (a single click).
      detail: 1,
      ...keys,
    });
    replayed.add(ev);
    // The press came from a node inside the terminal (the `contains` test).
    target?.dispatchEvent(ev);
  };
  body.addEventListener(
    "mousedown",
    (e: MouseEvent) => {
      if (replayed.has(e) || !term.element?.contains(e.target as Node)) return;
      const modified = e.shiftKey || e.altKey || e.ctrlKey || e.metaKey;
      if (pressRoute(term.modes.mouseTrackingMode, e.button, modified) !== "hold") return;
      e.stopPropagation();
      e.preventDefault();
      term.focus();
      const doc = body.ownerDocument;
      const start = { x: e.clientX, y: e.clientY };
      const end = () => {
        doc.removeEventListener("mousemove", onMove, true);
        doc.removeEventListener("mouseup", onUp, true);
      };
      const onMove = (m: MouseEvent) => {
        if (!dragBegins(start, { x: m.clientX, y: m.clientY }, dragThreshold("mouse"))) return;
        end();
        // The selection service is disabled under tracking, so it does not
        // extend an old selection; a new drag replaces it.
        term.clearSelection();
        replay(
          e.target,
          "mousedown",
          e,
          forceSelectionKeys(navigator.platform || navigator.userAgent),
        );
      };
      const onUp = (u: MouseEvent) => {
        end();
        u.stopPropagation();
        replay(e.target, "mousedown", e, {});
        replay(e.target, "mouseup", u, {});
      };
      doc.addEventListener("mousemove", onMove, true);
      doc.addEventListener("mouseup", onUp, true);
    },
    true,
  );
  body.addEventListener(
    "mousemove",
    (e: MouseEvent) => {
      if (holdMoveReport(term.modes.mouseTrackingMode, term.hasSelection(), e.buttons)) {
        e.stopPropagation();
      }
    },
    true,
  );
}
