/* ---------------------------------------------------------------------------
   The size of one console's terminal.

   `createFit(deps)` fits the terminal to its window body, refits when the
   body changes size, and sends each new size of the terminal to the PTY.
   It resizes a TERMINAL, never a window rect (#336).
   --------------------------------------------------------------------------- */
import { encodeResize } from "./wb-console-session.ts";

export type FitDeps = {
  term: XtermTerminal;
  body: HTMLElement;
  fitAddon: XtermFitAddon;
  // The socket now, or null before the first one.
  socket: () => WebSocket | null;
};

export function createFit({ term, body, fitAddon, socket }: FitDeps) {
  // The only ResizeObserver of the console's terminal.
  const ro = new ResizeObserver(() => {
    try {
      fitAddon.fit();
    } catch {}
  });
  ro.observe(body);
  term.onResize(({ rows, cols }) => {
    const ws = socket();
    if (ws && ws.readyState === WebSocket.OPEN) ws.send(encodeResize(rows, cols));
  });
  return {
    // The handle's `fit`: the outside callers call `fit.fit()`.
    fit: { fit: () => fitAddon.fit() },
    // Stops the refit of a window that is closed or ended.
    dispose: () => ro.disconnect(),
  };
}
