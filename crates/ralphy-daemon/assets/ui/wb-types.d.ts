// The shapes more than one workbench module reads: a rect on the stage, the
// desk layout's records and changes, and a console window. Types only: the
// build skips a `.d.ts` file, so a module imports from here with
// `import type`, on one line (#613).
import type { createTerminal } from "./wb-console-terminal.ts";
import type { createTitle } from "./wb-console-title.ts";

// ---- the plane ---------------------------------------------------------------

/** A box in stage pixels: the daemon's `DeskRect`. */
export type Rect = { left: number; top: number; width: number; height: number };

/** A point in stage or client pixels. */
export type Point = { x: number; y: number };

/** A scroll offset, or the top-left corner of a box. */
export type Offset = { left: number; top: number };

/** The size of a viewport or of the stage. */
export type Size = { width: number; height: number };

/** A pointer move, in pixels. */
export type Delta = { dx: number; dy: number };

// ---- the desk layout ---------------------------------------------------------
// The JSON of `crates/ralphy-daemon/src/desk.rs`. A field with a serde default
// is optional, because a page may leave it out; an `Option` the daemon does
// not serialise when it is `None` is optional too, and a page may send `null`.

/** One console window's record. */
export type DeskRecord = {
  id: string;
  repo?: string;
  agent?: string;
  kind?: string;
  rect: Rect;
  max?: boolean;
  /** Nullable and never the key: a session does not outlive its daemon. */
  sessionId?: number | null;
  daemonId?: string | null;
  environment?: string | null;
  /** The worktree name; `null` is the primary tree. */
  checkout?: string | null;
  locked?: boolean;
  consoleName?: string | null;
  ts?: number;
};

/** One fence's record. */
export type DeskFence = {
  id: string;
  name?: string;
  rect: Rect;
  locked?: boolean;
  ts?: number;
};

/** One note card's record: where the card sits, not the note itself. */
export type DeskNote = {
  id: string;
  repo?: string;
  path?: string;
  checkout?: string | null;
  rect: Rect;
  locked?: boolean;
  ts?: number;
};

/** The desk the daemon serves (`DeskStore`). */
export type Desk = {
  rev?: number;
  generation?: number;
  windows: DeskRecord[];
  fences: DeskFence[];
  notes: DeskNote[];
  /** The selected worktree per repo ref. Not sent when it is empty. */
  checkouts?: Record<string, string>;
};

/** The three record types a desk change names. */
export type DeskRecordType = "window" | "fence" | "note";

/** The session of a window in a `set`: a reconnect changes all three. */
export type DeskSessionFields = {
  sessionId?: number | null;
  daemonId?: string | null;
  environment?: string | null;
};

/** The fields a `set` may change on a window. */
export type DeskWindowFields = {
  rect?: Rect;
  max?: boolean;
  locked?: boolean;
  consoleName?: string;
  /** Present with `null` means "the primary tree"; absent says nothing. */
  checkout?: string | null;
  session?: DeskSessionFields;
};

/** The fields a `set` may change on a fence. */
export type DeskFenceFields = { rect?: Rect; name?: string; locked?: boolean };

/** The file of a note card, as one unit: `(repo, checkout, path)`. */
export type DeskFileFields = { repo: string; path?: string; checkout?: string | null };

/** The fields a `set` may change on a note card. */
export type DeskNoteFields = { rect?: Rect; locked?: boolean; file?: DeskFileFields };

/** One desk change (`desk::apply::Change`), as a page sends it. */
export type DeskChange =
  | { op: "create"; type: "window"; record: DeskRecord }
  | { op: "create"; type: "fence"; record: DeskFence }
  | { op: "create"; type: "note"; record: DeskNote }
  | { op: "set"; type: "window"; id: string; fields: DeskWindowFields }
  | { op: "set"; type: "fence"; id: string; fields: DeskFenceFields }
  | { op: "set"; type: "note"; id: string; fields: DeskNoteFields }
  | { op: "remove"; type: DeskRecordType; id: string }
  | { op: "checkout"; repo: string; name: string }
  | { op: "checkout-clear"; repo: string; ifName: string };

// ---- a console window --------------------------------------------------------

type Terminals = ReturnType<typeof createTerminal>;

/** The live terminal of a console window (`attachTerminal`). */
export type ConsoleTerm = ReturnType<Terminals["attachTerminal"]>;

/** What a window's terminal is built with, kept for a rebuild after sleep. */
export type TermWiring = Parameters<Terminals["attachTerminal"]>[1];

/** What the title shows for a window's session (`sessionPresentation`). */
export type Presentation = ReturnType<ReturnType<typeof createTitle>["sessionPresentation"]>;

/** A tap on the titlebar, kept to find a double tap. */
export type Tap = { t: number; x: number; y: number };

/**
 * The fields `wb-window-state.ts` declares in `FIELDS`: every window is born
 * with all of them (`initWindow`).
 */
export type ConsoleWinFields = {
  /** The desk record this window is. `buildChrome` seeds it on every window. */
  _deskId: string;
  /** The repo ref; `"~"` is the home directory. */
  _deskRepo: string;
  _deskAgent: string | null;
  _deskKind: string | null;
  _deskDaemonId: string | null;
  _deskEnvironment: string | null;
  /** The worktree, from the record, the launch request or the switcher. */
  _deskCheckout: string | null;
  _deskLocked: boolean;
  /** The console name: a label, not an identity. */
  _deskConsoleName: string | null;
  /** Adopted from a session with no record: the first act creates it. */
  _deskUnrecorded: boolean;
  /** The worktree the daemon announced on `session-open`. */
  _sessionCheckout: string | null;
  _presentation: Presentation | null;
  /** The title span. */
  _title: HTMLElement | null;
  /** The agent's state dot before the title. */
  _stateDot: HTMLElement | null;
  _maxBtn: HTMLButtonElement | null;
  _colBtn: HTMLButtonElement | null;
  /** Wires a new terminal into the chrome (the key bar). */
  _rewire: ((t: ConsoleTerm) => void) | null;
  /** Restarts the console, in `checkout` when it is not `undefined`. */
  _relaunchIn: ((checkout: string | null | undefined) => void) | null;
  _termWiring: TermWiring | null;
  /** `null` while the window is a placeholder or asleep. */
  _term: ConsoleTerm | null;
  /** The agent state its session last reported. */
  _agentState: string | null;
  /** The session id this window was spawned to attach to. */
  _wantsSession: number | null;
  /** Whether the window is in the viewport. */
  _visible: boolean;
  /** Asleep: its renderer is gone and the fields below carry its session. */
  _dormant: boolean;
  _dormantSession: number | null;
  _dormantWatch: boolean;
  _dormantTimer: ReturnType<typeof setTimeout> | null;
};

/** A `.session-window` element and the fields the console modules set on it. */
export type ConsoleWin = HTMLElement &
  ConsoleWinFields & {
    /** Asks the placeholder's peer box again. */
    _peerRefresh?: () => void;
    /** Sets the terminal's `inputmode` from the key bar and the keyboard. */
    _applyInputMode?: () => void;
    /** Whether another window fills the viewport over this one. */
    _covered?: boolean;
    /** Attaches a placeholder to its session when the session runs. */
    _revive?: () => Promise<void>;
    /** The pointer type of the last press on the titlebar. */
    _lastPointerType?: string;
    _lastTap?: Tap | null;
  };
