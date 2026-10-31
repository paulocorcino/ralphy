/* ---------------------------------------------------------------------------
   The size of one console's terminal.

   `createFit(deps)` fits the terminal to its window body, refits when the
   body changes size, and sends each new size of the terminal to the PTY.
   It resizes a TERMINAL, never a window rect (#336).

   The size rule: one session has one size, the size of the console the
   operator can see. A console that a full-bleed console covers, and that does
   not have the focus, does not fit and does not send a size; neither does a body
   that measures 0×0. Such a request is kept and runs when the cover goes.
   Every request waits for the next animation frame: a swap of the maximized
   console un-maximizes the old one BEFORE it maximizes the new one, so the
   cover is only true again at the end of the task.

   A console that watches takes the session size the daemon sends
   (`session-size`) and does not fit or send a size; when its body is smaller,
   the body scrolls (class `following`). It fits again when it stops watching.
   With no `session-size` (a peer of an older build) a watcher fits as before.
   --------------------------------------------------------------------------- */
import { encodeResize } from "./wb-console-session.ts";
import type { ConsoleChannel } from "./wb-console-terminal-types.d.ts";

export type FitDeps = {
  term: XtermTerminal;
  body: HTMLElement;
  fitAddon: XtermFitAddon;
  // The console's channel now, or null before the first one. Its
  // `readyState` takes the WebSocket numbers.
  socket: () => ConsoleChannel | null;
  // Whether a full-bleed console covers this one. Absent: never covered.
  covered?: () => boolean;
  // Whether this console has the focus. A focused console always sizes.
  focused?: () => boolean;
  // Whether this console only watches the session. Absent: never.
  watching?: () => boolean;
};

export function createFit({ term, body, fitAddon, socket, covered, focused, watching }: FitDeps) {
  let frame: number | null = null;
  // A request was skipped: the next allowed fit runs it.
  let pending = false;
  // The PTY has not been told a size since the channel opened.
  let owed = false;
  let disposed = false;
  // The last session size the daemon sent; null before the first one.
  let session: { rows: number; cols: number } | null = null;
  // The body carries the `following` class.
  let scrolls = false;

  function following() {
    return session !== null && watching?.() === true;
  }

  function setScrolls(on: boolean) {
    if (scrolls === on) return;
    scrolls = on;
    body.classList.toggle("following", on);
  }

  function canSize() {
    if (!(body.clientWidth > 0 && body.clientHeight > 0)) return false;
    return !(covered?.() === true && focused?.() !== true);
  }

  function send() {
    if (following()) return;
    const ws = socket();
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    ws.send(encodeResize(term.rows, term.cols));
    owed = false;
  }

  // Fits now if the rule allows it; returns whether it did.
  function fitNow() {
    if (following()) {
      pending = false;
      return false;
    }
    setScrolls(false);
    if (!canSize()) {
      pending = true;
      return false;
    }
    pending = false;
    try {
      fitAddon.fit();
    } catch {}
    // The grid did not change, so `onResize` did not send.
    if (owed) send();
    return true;
  }

  function request() {
    if (disposed || frame !== null) return;
    frame = requestAnimationFrame(() => {
      frame = null;
      if (!disposed) fitNow();
    });
  }

  // The handle's `fit`: the outside callers call `fit.fit()`.
  const fit = { fit: request };
  // The only ResizeObserver of the console's terminal. It calls through the
  // handle's door, so every refit request goes through one door.
  const ro = new ResizeObserver(() => fit.fit());
  ro.observe(body);
  term.onResize(() => send());

  return {
    fit,
    // The cover over this console went: run the request it kept.
    uncovered: () => {
      if (pending) request();
    },
    // The channel opened: tell the PTY the size now, or when the rule allows it.
    opened() {
      owed = true;
      fitNow();
    },
    // The daemon sent the session size. Only a watcher takes it: the writer's
    // own fit is the session size.
    sessionSize(rows: number, cols: number) {
      if (watching?.() !== true) return;
      session = { rows, cols };
      setScrolls(true);
      if (term.rows !== rows || term.cols !== cols) term.resize(cols, rows);
    },
    // Stops the refit of a window that is closed or ended.
    dispose: () => {
      disposed = true;
      ro.disconnect();
      if (frame !== null) cancelAnimationFrame(frame);
      frame = null;
    },
  };
}
