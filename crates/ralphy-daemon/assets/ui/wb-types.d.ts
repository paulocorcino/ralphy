// The shapes more than one workbench module reads: a rect on the stage, the
// desk layout's records and changes, and a console window. It also holds the
// shell's own types and the re-exports `app.ts` imports. They live here
// because `app.ts` has a line ratchet and must not grow. Types only: the
// build skips a `.d.ts` file, so a module imports from here with
// `import type`, on one line (#613).
import type { createTerminal } from "./wb-console-terminal.ts";
import type { createTitle } from "./wb-console-title.ts";
import type { DeskDeps } from "./wb-console-desk.ts";
import type { DetachDeps } from "./wb-console-detach.ts";
import type { PopupRegistryDeps } from "./wb-console-popups.ts";
import type { ViewDeps } from "./wb-console-view.ts";
import type { FleetPeer } from "./wb-fleet.ts";

// Types the shell names, re-exported so `app.ts` has one import line for them.
export type { RosterRow } from "./wb-agents.ts";
export type { ChangeEntry, Sync } from "./wb-changes.ts";
export type { CheckoutRow } from "./wb-console-title.ts";
export type { Read } from "./wb-fail.ts";
export type { FleetPeer, Group } from "./wb-fleet.ts";
export type { Listing, Project } from "./wb-project.ts";
export type { BoardRow, Issue as BoardIssue } from "./wb-kanban.ts";
export type { Run, Issue as RunIssue } from "./wb-runs.ts";
export type { Slot } from "./wb-split.ts";

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

/** What `applyExtent` may be asked: `grow` never shrinks the stage mid-gesture. */
export type ExtentOpts = { grow?: boolean };

// ---- what a window is spawned from -------------------------------------------

/**
 * The record a console window continues when it is spawned: a desk record, a
 * popup's member, or a live session no record claims (`unrecorded`). Every
 * field may be missing, because each source fills a different part.
 */
export type SpawnCarry = {
  id?: string;
  repo?: string | null;
  agent?: string | null;
  kind?: string | null;
  rect?: Partial<Rect>;
  max?: boolean;
  locked?: boolean;
  consoleName?: string | null;
  checkout?: string | null;
  daemonId?: string | null;
  environment?: string | null;
  unrecorded?: boolean;
  /** The live session id; `null` when the window has none. */
  session?: number | null;
};

/** A window as the desk and a popup's snapshot read it (`deskOf`). */
export type WindowSnapshot = SpawnCarry & { id: string; rect: Rect };

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

/**
 * The reply to a desk `PUT`: the desk after the changes, the changes it
 * refused one by one, or why it refused the whole batch (`state`, `error`).
 */
export type DeskReply = Partial<Desk> & { refused?: { error: string }[]; state?: string; error?: string };

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
  _dormantWatch: boolean | undefined;
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

/**
 * What an entry module hands `createConsole`. The shell passes nothing; the
 * detached fence page passes all but `isStale`, so it cannot write the desk,
 * the stored view or the detach registry.
 */
export type ConsoleOpts = {
  /** Where the desk changes go (`wb-desk-sink.ts`). */
  deskSink?: DeskDeps["deskSink"];
  /** The per-client view (`WBView`). */
  viewStore?: ViewDeps["viewStore"];
  /** `false`: the page restores no desk and has no fences of its own. */
  autoBoot?: boolean;
  /** `false`: the page launches no session by itself. */
  canLaunch?: boolean;
  /** The detach registry and the lifecycle channel (`wb-detach-link.ts`). */
  detachLink?: DetachDeps["link"] & PopupRegistryDeps["link"];
  /** Whether the sockets are stale after a resume. */
  isStale?: () => boolean;
};

/**
 * An element whose `style.zIndex` is written as a number: the DOM turns the
 * number into a string, and a read gives the string.
 */
export type Stacked = Omit<HTMLElement, "style"> & {
  style: Omit<CSSStyleDeclaration, "zIndex"> & { get zIndex(): string; set zIndex(value: string | number) };
};

/** The five closed-set fields of a note card's look (`applyLook`, `lookOf`). */
export type NoteLook = { tone: string; fill: string; ink: string; font: string; size: string };

/** The editor a note card mounts (`window.CrepeLean.create` resolves with it). */
export type NoteEditor = {
  getMarkdown(): string;
  setReadonly(value: boolean): void;
  destroy(): void;
  /** The ProseMirror root, or `null` when the view is gone. */
  dom(): HTMLElement | null;
  redrawDiagrams(): void;
};

/**
 * The record a note card is built from: a desk record, or a detached fence's
 * member. A never-saved note in a popup carries its text in `draft` and the
 * name it chose in `claim`.
 */
export type NoteSource = DeskNote & { draft?: string; claim?: string | null };

/**
 * A `.note-card` element, with the fields the card keeps on it. `buildCard`
 * sets the first group on every card; the others appear as the card is used.
 */
export type NoteCard = HTMLElement & {
  _noteTone: string;
  _noteFill: string;
  _noteInk: string;
  _noteFont: string;
  _noteSize: string;
  /** The text of the document, with its front matter. */
  _noteMarkdown: string;
  /** The card holds text not yet saved. */
  _noteDirty: boolean;
  /** A write is in flight. */
  _noteInFlight: boolean;
  _noteTimer: ReturnType<typeof setTimeout> | null;
  /** When the file last landed, in Unix milliseconds. */
  _noteSavedAt: number | null;
  /** The session half of the veil: shown since the card opened. */
  _noteRevealed: boolean;
  /** The card's desk record. */
  _noteRecord?: DeskNote;
  /** The record of a card whose record left the desk, or the popup's snapshot. */
  _noteOrphan?: NoteSource;
  _noteLocked?: boolean;
  _noteEditor?: NoteEditor | null;
  _noteAsleep?: boolean;
  /** Set on teardown, so a late editor is destroyed. */
  _noteGone?: boolean;
  _noteFocusOnMount?: boolean;
  /** A draft was handed to a detach popup: that card is the only writer. */
  _noteHandedOff?: boolean;
  /** The name this card chose and has not written yet. */
  _noteClaim?: string | null;
  _noteClaimSent?: string;
  /** The naming probe in flight. */
  _noteNaming?: Promise<string | null> | null;
  /** The write chain of this card. */
  _noteWrite?: Promise<void>;
  /** The rename field aims the card at another file. */
  _notePointing?: boolean;
  /** Built from the stage's records by `render`. */
  _noteListed?: boolean;
  /** The floating box while the card is on top, in viewport pixels. */
  _noteOnTop?: Rect | null;
  /** The card's place on the plane while it floats. */
  _noteShadow?: HTMLElement | null;
};

// ---- the shell (`app.ts`) ------------------------------------------------------

/** A timer id kept in a field, or `null` while none runs. */
export type Timer = ReturnType<typeof setInterval> | null;

/** The read state of a shown fact (`WBFail.readFold`); `null` before the first read. */
export type ReadState = import("./wb-fail.ts").Read | null;

/** An open modal: the flag path it answers to, and the element that gets focus back. */
export type ModalEntry = { path: string; opener: HTMLElement | null };

/** What `askConfirm` takes; each field has a default. */
export type ConfirmAsk = {
  title?: string;
  message?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
};

/** What `askPrompt` takes; each field has a default. */
export type PromptAsk = {
  title?: string;
  message?: string;
  value?: string;
  placeholder?: string;
  confirmLabel?: string;
};

/** A repo of `/api/repos`. */
export type RepoRow = {
  slug: string;
  name?: string;
  path?: string;
  root?: string;
  branch?: string | null;
  head?: { kind: string; name?: string; sha?: string } | null;
  dirty?: boolean;
  reachable: boolean;
  remote?: string | null;
};

/** A repo of `/api/fleet`: the local ones (`local`) and each peer's. */
export type FleetRepoRow = {
  key: string;
  slug: string;
  name?: string;
  path?: string;
  branch?: string | null;
  dirty?: boolean | null;
  reachable: boolean;
  remote?: string | null;
  local: boolean;
  daemon_id: string;
  daemon_name?: string;
  environment?: string;
  os?: string;
  peer_state?: string;
};

/** The reply of `/api/fleet`. */
export type FleetReply = { peers?: FleetPeer[]; repos?: FleetRepoRow[] };

/** A persistent socket the shell keeps: `resume` re-opens it after a suspend. */
export type Subscription = ReturnType<import("./wb-daemon.ts").WBDaemonApi["subscribePresence"]>;

// ---- the shell's own types (read by pp.ts only) ---------------------------

/** The shell fields its methods set on first use: none is in the literal. */
export type ShellLate = {
  _clockTick?: Timer;
  _actionTimer?: Timer;
  /** Counts the `worktree.list` reads; only the newest one is kept. */
  _listingSeq?: number;
  /** Counts the `issue.show` reads; only the newest one is kept. */
  _issueDetailGen?: number;
};

/** The repo's ready plan on the board: its text and `WBRun.planSummary` of it. */
export type ReadyPlan = { md: string; summary: ReturnType<typeof import("./wb-runs.ts").WBRun.planSummary> };

/** The run pill of a board card (`WBKanban.runningFor`), or `null`. */
export type RunPill = ReturnType<typeof import("./wb-kanban.ts").WBKanban.runningFor>;

type SpendInput = NonNullable<Parameters<typeof import("./wb-spend.ts").WBSpend.state>[0]>;
type LedgerInput = NonNullable<Parameters<typeof import("./wb-spend.ts").WBSpend.ledger>[0]>;

/** The `/api/spend` document, or `null` before a read. */
export type SpendDoc = SpendInput["doc"];

/** A record of the `/api/usage` ledger. */
export type LedgerRecord = NonNullable<LedgerInput["records"]>[number];

/** A peer whose usage did not arrive (`/api/usage` `missing`). */
export type LedgerMissing = NonNullable<LedgerInput["missing"]>[number];

/** The daemon's auth model as the shell holds it; `probeSession` fills it. */
export type SecurityFact = {
  tokenSet: boolean;
  passwordSet: boolean;
  totpEnrolled: boolean;
  requireLogin: boolean;
  remoteImages: boolean;
  policy: string;
};

/** A tab of the canvas. A file or diff tab names its file and its checkout. */
export type CanvasTab = {
  id: string;
  kind: string;
  title: string;
  icon: string;
  closable: boolean;
  path?: string;
  project?: string;
  checkout?: string | null;
};

/** What `openTab` takes. `content` is the bytes a re-attached popup brings back. */
export type TabOpen = {
  project: string;
  path: string;
  title: string;
  ftype: string;
  content?: string;
  fragment?: string;
  find?: string | null;
  checkout?: string | null;
  encoding?: string;
  bom?: boolean;
};

/** What a file tab opens with: the bytes, or why the daemon served none.
 * `null`: there is nothing to show, and the tab is closed. */
export type TabBody = { content?: string; encoding?: string; bom?: boolean; refused?: string } | null;

/** A diff tab's two sides (`WBChanges.diffTarget`). */
export type DiffTarget = ReturnType<typeof import("./wb-changes.ts").WBChanges.diffTarget>;

/** A row of the context menu: a separator, or an action. */
export type MenuItem =
  | { sep: true }
  | { sep?: false; label: string; icon: string; run: () => void; danger?: boolean; disabled?: boolean; title?: string };

/** A message a detached file window sends the shell (`wb-detached.ts`). */
export type FilePopupMessage =
  | { type: "wb-detach-ready" }
  | { type: "wb-emit"; action: string; detail?: object }
  | { type: "wb-open-request"; detail?: OpenRequest }
  | { type: "wb-reattach"; desc?: FileDescriptor };

/** A `file.write` payload: the pane's text, its encoding and its byte order mark. */
export type SavePayload = CommandPayload & { encoding?: string; bom?: boolean };
