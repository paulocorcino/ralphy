/* ---------------------------------------------------------------------------
   What one console's terminal takes from the clipboard and gives to it: the
   image paste, the OSC 52 copy of an agent, and the right button.

   `wirePaste(deps)` adds the paste listener, the OSC 52 handler and the right
   button's listeners, and returns `dropImage` (the paste key uses it too).
   The terminal's state that changes (`watching`, `replaying`, the repo) is
   read through getters. `wb-console-terminal.ts` calls it once per terminal
   (#621).
--------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import * as WBConsoleInput from "./wb-console-input.ts";
import type { TerminalDeps, TerminalOpts } from "./wb-console-terminal-types.d.ts";

const { pasteOffered, rightClickAction, pasteDecision, scrubClipboard } = WBConsoleInput;

// The largest OSC 52 payload accepted, measured on the BASE64 so an oversized
// string is never materialised. xterm's own OSC limit is 10MB.
const OSC52_MAX_B64 = 128 * 1024;

// What the paste, the copy and the right button read from the terminal, and nothing else.
export type PasteDeps = {
  // The terminal the paste and the copy act on.
  term: XtermTerminal;
  // The window body the right button's listeners go on.
  body: HTMLElement;
  // The console's page: `WBDaemon` takes the image.
  window: Pick<Window, "WBDaemon">;
  // The terminal's opts: `onWatchedInput` shows a refused paste.
  opts: Pick<TerminalOpts, "onWatchedInput">;
  // The system clipboard, from the console.
  writeClipboard: TerminalDeps["writeClipboard"];
  readClipboard: TerminalDeps["readClipboard"];
  // The terminal's state now: a watcher, a replay in progress, the repo.
  watchingNow: () => boolean;
  replayingNow: () => boolean;
  currentRepoNow: () => string | null;
};

export function wirePaste(deps: PasteDeps) {
  const { term, body, window, opts, writeClipboard, readClipboard, watchingNow, replayingNow, currentRepoNow } = deps;

  // A pasted IMAGE is not text (ADR-0055): xterm forwards only `text/plain`.
  // The bytes become a clipboard drop (`image.write`) and the drop's PATH is
  // pasted through `term.paste` — bracketed when the child asked, NO trailing
  // newline either way. Text falls through untouched. The gate mirrors
  // `onData`: a watcher SEES the refusal, and nothing leaves its window.
  // Returns whether the paste was taken (false = let the text through).
  const dropImage = (types: string[], file: Blob | null) => {
    const decision = pasteDecision({ types, size: file ? file.size : -1, watching: watchingNow() });
    if (decision === "passthrough") return false;
    if (decision === "watched") {
      if (typeof opts.onWatchedInput === "function") opts.onWatchedInput();
      return true;
    }
    if (decision === "too-large") {
      term.write("\r\n[paste refused — too large]\r\n");
      return true;
    }
    const daemon = window.WBDaemon;
    if (!daemon) {
      // The popup forgot its bridge: say so rather than swallow the paste.
      term.write("\r\n[paste refused — daemon not connected]\r\n");
      return true;
    }
    const reader = new FileReader();
    reader.onerror = () => term.write("\r\n[paste refused — unreadable]\r\n");
    reader.onload = () => {
      const base64 = (typeof reader.result === "string" ? reader.result : "").replace(/^data:[^,]*,/, "");
      daemon
        .write("image.write", { repo: currentRepoNow(), base64 })
        .then((reply) => {
          if (WBFail.isError(reply) || !reply.path) {
            const why = WBFail.why(reply, "the daemon refused it");
            term.write(`\r\n[paste refused — ${why}]\r\n`);
            return;
          }
          term.paste(reply.path);
          term.focus();
        })
        // The socket closed with no reply: say what the browser saw.
        .catch((err: unknown) => {
          const why = (err instanceof Error && err.message) || "connection unavailable";
          term.write(`\r\n[paste refused — ${why}]\r\n`);
        });
    };
    // `file` is set: a missing one has size -1, and `pasteDecision` refuses it.
    reader.readAsDataURL(file!);
    return true;
  };
  term.textarea.addEventListener("paste", (e: ClipboardEvent) => {
    const items: DataTransferItem[] = Array.from(e.clipboardData?.items ?? []);
    const image = items.find((i) => i.type.startsWith("image/"));
    const file = image ? image.getAsFile() : null;
    if (dropImage(items.map((i) => i.type), file)) e.preventDefault();
  });

  // OSC 52 — "put this on the clipboard". xterm's core does not implement
  // it, and OSC 52 has no reply, so an agent would announce a copy it never
  // got. WRITE ONLY:
  // - The READ form (`52;c;?`) is never answered: it would let an agent read
  //   the operator's clipboard.
  // - Refused while `replaying`: the scrollback replays as raw bytes, and a
  //   copy from an hour ago would rewrite the clipboard on every reattach.
  // - Refused in a watcher: the same bytes reach EVERY attached window, and
  //   the window whose operator asked owns the clipboard. (Copying BY HAND
  //   in a watcher stays allowed — the #335 gate is about writing to the
  //   child.)
  term.parser.registerOscHandler(52, (data: string) => {
    // NEVER return the clipboard promise: `OscHandler.end` PAUSES the parser
    // on a promise, and a rejected write (unfocused document) would stall
    // the terminal.
    const replaying = replayingNow(), watching = watchingNow();
    if (replaying || watching) return true;
    const semi = data.indexOf(";");
    if (semi < 0) return true;
    const payload = data.slice(semi + 1);
    // `?` reads and `!` clears; both are no-ops.
    if (payload === "?" || payload === "!") return true;
    if (payload.length > OSC52_MAX_B64) return true;
    let text: string;
    try {
      const bin = atob(payload);
      text = new TextDecoder().decode(Uint8Array.from(bin, (c) => c.charCodeAt(0)));
    } catch {
      // base64url or bad padding: `atob` THROWS into xterm's parser.
      return true;
    }
    writeClipboard(scrubClipboard(text), term);
    return true;
  });
  // `rightClickAction` and `holdMoveReport`, applied. CAPTURE phase on
  // `body`: xterm binds its listeners on `term.element`, a child, so a stop
  // here means neither xterm nor the child gets the event. The decision is
  // made on `mousedown`, while the selection still exists. Both clipboard
  // calls run inside the press, a user gesture.
  body.addEventListener(
    "mousedown",
    (e: MouseEvent) => {
      if (e.button !== 2) return;
      const rightTaken = rightClickAction(term.hasSelection(), pasteOffered(navigator.clipboard));
      e.stopPropagation();
      // xterm's own mousedown focused the terminal; it no longer runs.
      e.preventDefault();
      term.focus();
      if (rightTaken === "copy") {
        writeClipboard(term.getSelection(), term);
        term.clearSelection();
      } else if (rightTaken === "paste") {
        readClipboard()
          .then(({ image, text }) => {
            if (image) dropImage([image.type], image);
            else if (text) term.paste(text);
          })
          .catch(() => {});
      }
    },
    true,
  );
  body.addEventListener(
    "contextmenu",
    (e: Event) => {
      e.preventDefault();
      e.stopPropagation();
    },
    true,
  );

  return { dropImage };
}
