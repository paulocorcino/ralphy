// The types only the shell (`app.ts`) reads. It also re-exports the types of
// other modules that the shell names, so `app.ts` imports all of them on one
// line: `app.ts` has an exact line ratchet (`APP_TS_LINES` in
// `crates/xtask/tests/ratchets.rs`). Only `app.ts` imports from here. A type
// that a second module reads goes in `wb-types.d.ts`, and any other module
// imports a type from the module that declares it. Types only: the build
// skips a `.d.ts` file, so `app.ts` imports from here with `import type`
// (#653).

// Types of other modules that the shell names.
export type { RosterRow } from "./wb-agents.ts";
export type { ChangeEntry, Sync } from "./wb-changes.ts";
export type { CheckoutRow } from "./wb-console-title.ts";
export type { Read } from "./wb-fail.ts";
export type { FleetPeer, Group } from "./wb-fleet.ts";
export type { Listing, Project } from "./wb-project.ts";
export type { BoardRow, Issue as BoardIssue } from "./wb-kanban.ts";
export type { Run, Issue as RunIssue } from "./wb-runs.ts";
export type { Slot } from "./wb-split.ts";

// ---- the shell's own types --------------------------------------------------

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

/** A persistent socket the shell keeps: `resume` re-opens it after a suspend. */
export type Subscription = ReturnType<import("./wb-daemon.ts").WBDaemonApi["subscribePresence"]>;

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

/** A `file.write` payload: the pane's text, its encoding and its byte order mark. */
export type SavePayload = CommandPayload & { encoding?: string; bom?: boolean };
