/* ---------------------------------------------------------------------------
   What the console's terminal (`wb-console-terminal.ts`) reads from the
   console, and what `attachTerminal` is built with. The console modules that
   build or wire a terminal import these types from here. Types only: the
   build skips a `.d.ts` file, so a module imports from here with
   `import type`.
   --------------------------------------------------------------------------- */
import type { SessionRequest } from "./wb-session-route.ts";
import type { FramePayload } from "./wb-console-frames.ts";
import type { ScrollProbe } from "./wb-console-probe.ts";
import type { Group } from "./wb-fleet.ts";

// What the terminal reads from the console, and nothing else.
export type TerminalDeps = {
  // The console's page: `WBDaemon` (the image paste) and `getShell` (whether
  // the shell's key listener exists).
  window: Pick<Window, "WBDaemon" | "getShell">;
  // The session socket's origin, scheme-matched to the page.
  WS_ORIGIN: string;
  // The stored font size, read when a terminal is made.
  fontSize: () => number;
  // The plane and the viewport; null before the page has them.
  stage: () => HTMLElement | null;
  workspace: () => HTMLElement | null;
  // Stops a jump of the viewport in flight: a two-finger pan outranks it.
  cancelSlide: () => void;
  // The system clipboard. They are made in `wb-console-keys.ts`: the key bar uses them too.
  writeClipboard: (text: string, term: XtermTerminal) => void;
  readClipboard: () => Promise<ClipboardRead>;
  // The page's scroll probe; absent where the page has none (ADR-0084).
  scrollProbe?: ScrollProbe;
};

// What a clipboard read gives: an image, or else text (`clipboardContent`).
type ClipboardRead = { image?: Blob; text?: string };

// The link of a window that had a socket: "lost" while it reconnects, "back"
// when a socket opens again or the window stops trying.
export type LinkState = "lost" | "back";

// What `attachTerminal` is built with, and what the chrome hands back to
// rebuild a terminal after sleep (`TermWiring`).
export type TerminalOpts = SessionRequest & {
  // Arms or disarms the key bar's line-selection button.
  onSelecting?: (on: boolean) => void;
  // A key or a paste was refused because this window only watches.
  onWatchedInput?: () => void;
  onCtrlLatch?: (on: boolean) => void;
  onShiftLatch?: (on: boolean) => void;
  // The session's id, and the daemon's announcement when it came with one.
  onSession?: (id: number | null, announced?: FramePayload) => void;
  // This window parked as a watcher: `announced` is the daemon's reason.
  onPark?: (announced: string | null) => void;
  onEnded?: (announced: string | null, refusal: string | null) => void;
  // The baton came back to this window.
  onResume?: () => void;
  // Whether a full-bleed console covers this window, and whether it has the
  // focus: the size rule of wb-console-fit.ts.
  covered?: () => boolean;
  focused?: () => boolean;
  // The peer's fleet group, or null for a local project.
  peerGroup?: () => Group | null;
  onPeerHold?: (group: Group | null) => void;
  onPeerBack?: () => void;
  // Shown beside the terminal, never in it: a notice in the terminal forgets
  // the byte count, and the reconnect could not resume (ADR-0079 D8).
  onLink?: (state: LinkState) => void;
  readFleet?: () => void;
};
