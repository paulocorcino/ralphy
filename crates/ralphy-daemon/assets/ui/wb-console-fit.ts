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
   --------------------------------------------------------------------------- */
import { encodeResize } from "./wb-console-session.ts";

export type FitDeps = {
  term: XtermTerminal;
  body: HTMLElement;
  fitAddon: XtermFitAddon;
  // The socket now, or null before the first one.
  socket: () => WebSocket | null;
  // Whether a full-bleed console covers this one. Absent: never covered.
  covered?: () => boolean;
  // Whether this console has the focus. A focused console always sizes.
  focused?: () => boolean;
};

export function createFit({ term, body, fitAddon, socket, covered, focused }: FitDeps) {
  let frame: number | null = null;
  // A request was skipped: the next allowed fit runs it.
  let pending = false;
  // The PTY has not been told a size since the socket opened.
  let owed = false;
  let disposed = false;

  function canSize() {
    if (!(body.clientWidth > 0 && body.clientHeight > 0)) return false;
    return !(covered?.() === true && focused?.() !== true);
  }

  function send() {
    const ws = socket();
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    ws.send(encodeResize(term.rows, term.cols));
    owed = false;
  }

  // Fits now if the rule allows it; returns whether it did.
  function fitNow() {
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

  // The only ResizeObserver of the console's terminal.
  const ro = new ResizeObserver(() => request());
  ro.observe(body);
  term.onResize(() => send());

  return {
    // The handle's `fit`: the outside callers call `fit.fit()`.
    fit: { fit: request },
    // The cover over this console went: run the request it kept.
    uncovered() {
      if (pending) request();
    },
    // The socket opened: tell the PTY the size now, or when the rule allows it.
    opened() {
      owed = true;
      fitNow();
    },
    // Stops the refit of a window that is closed or ended.
    dispose() {
      disposed = true;
      ro.disconnect();
      if (frame !== null) cancelAnimationFrame(frame);
      frame = null;
    },
  };
}
