// The names a module reads on `window` or bare (ADR-0075 D9): the instances
// an entry module creates for its page, the event bus, and the vendored
// libraries. Each surface lists only what a module calls. The vendored
// libraries ship no types: each interface below is written by hand from the
// members the modules call, the way `qrcode` is typed.

/** A value JSON can carry. */
type JsonValue = string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue | undefined };

/** The `payload` of a `/ws/command` command: the verb's arguments. */
type CommandPayload = { repo?: string | null; checkout?: string; [field: string]: JsonValue | undefined };

// `filter(Boolean)` keeps only the truthy items, but the lib's own type for it
// keeps the falsy members in the item type.
interface Array<T> {
  filter(predicate: BooleanConstructor): Exclude<T, false | 0 | "" | null | undefined>[];
}

/** A `/ws/command` reply: `status` is "ok" or the reason it is not. A refusal
 * says why in `message` or `reason`. */
type DaemonReply = { status: string; message?: string; reason?: string };

/** A `tree.list` entry: a child of the listed directory. */
type TreeEntry = { name: string; dir: boolean; ignored?: boolean };

/** A `file.read` reply: the text, the encoding it was decoded with, and
 * whether the file starts with a byte order mark; or why the daemon read
 * nothing. The daemon sends no other `status` for this verb. */
type FileReadReply = FileReadOk | (DaemonReply & { status: "error" });

/** A `file.read` reply with the file's text. */
type FileReadOk = { status: "ok"; content: string; encoding: string; bom: boolean };

/** A file Write verb's reply: a refused encoding names the first character it
 * could not take. */
type WriteReply = DaemonReply & { char_index?: number };

/** The reply each verb adds to `DaemonReply` (the shapes in `ui-tests/fixtures`
 * and the daemon's reply structs). A Query verb nests the CLI's JSON under one
 * field. A verb not listed here answers only `DaemonReply`. */
interface DaemonReplies {
  "tree.list": DaemonReply & { entries?: TreeEntry[] };
  "tree.find": DaemonReply & { hits?: { path: string; dir: boolean }[]; truncated?: boolean };
  "tree.grep": DaemonReply & { hits?: { path: string; count: number }[]; truncated?: boolean };
  "file.read": FileReadReply;
  "file.image": DaemonReply & { mediaType?: string; base64?: string };
  "file.write": WriteReply;
  "file.create": WriteReply;
  "file.rename": WriteReply;
  "file.copy": WriteReply;
  /** `modified`: when the note was last saved, in Unix milliseconds. */
  "note.read": DaemonReply & { markdown?: string; modified?: number | null };
  "note.write": WriteReply;
  /** The path the daemon saved the pasted image at. */
  "image.write": DaemonReply & { path?: string };
  "runs.list": DaemonReply & {
    runs?: import("./wb-runs.ts").RunSnapshot[];
    unreadable?: { runid: string; reason: string }[];
  };
  "branch.list": DaemonReply & { branches?: { current?: string; branches?: string[] } };
  "worktree.list": DaemonReply & { checkouts?: import("./wb-project.ts").Listing };
  "changes.list": DaemonReply & { changes?: { changes?: import("./wb-changes.ts").ChangeRow[] } };
  "sync.status": DaemonReply & { sync?: { sync?: import("./wb-changes.ts").SyncBody } };
  /** `blob.status`: `present`, `absent` or `refused`. */
  "blob.read": DaemonReply & { blob?: { status?: string; content?: string | null; reason?: string | null } };
  "board.list": DaemonReply & {
    board?: { issues?: import("./wb-kanban.ts").BoardRow[]; labels?: import("./wb-kanban.ts").BoardLabel[] };
  };
  "issue.show": DaemonReply & { issue?: import("./wb-kanban.ts").IssueDetail };
  /** The repo's resolved settings. */
  "config.get": DaemonReply & { config?: { [key: string]: JsonValue | undefined } };
  "host.aliases": DaemonReply & { aliases?: import("./wb-hosts.ts").Alias[] };
  "host.key": DaemonReply & { key?: import("./wb-hosts.ts").KeyReply };
  "project.add": DaemonReply & { slug?: string; name?: string; path?: string };
  "dir.list": DaemonReply & {
    entries?: { name: string; repo: boolean; added: boolean; error?: string }[];
    /** How many folders the list left out. */
    more?: number;
    /** The folder a first list starts at. */
    start?: string;
    dir?: { path: string; root: string | null; added: boolean };
  };
}

/** A live session of `/api/sessions` (the daemon's `HostedSessionInfo`). */
type HostedSession = {
  id: number;
  repo: string;
  agent: string;
  /** `agent` or `console`. */
  kind: string;
  started_at: number;
  daemon_id: string;
  environment: string;
  name?: string;
  /** The worktree; absent on the primary tree. */
  checkout?: string;
  /** The desk record the session serves. */
  record?: string;
  /** The agent's own state, from its hooks. */
  agent_state?: { state: string; since: string; detail?: string };
};

/** The reply to `verb`. */
type ReplyOf<V extends string> = V extends keyof DaemonReplies ? DaemonReplies[V] : DaemonReply;

/** One update of a streamed verb (`WBDaemon.spawn`). */
type SpawnStatus =
  | { status: "output"; chunk: string }
  | { status: "exited"; code: number | null }
  | { status: "error"; message?: string };

/** A `[0x02][JSON]` frame the daemon pushes on a subscription socket. */
type PushFrame = { verb: string; payload?: PushPayload };

/** What a push names: the repo, and the dir or the reason for a tree push, or
 * the `tab` that wrote the desk. */
type PushPayload = { repo?: string; path?: string; checkout?: string | null; reason?: string; tab?: string };

/** The `/ws` heartbeat: who the daemon is and how long it has been up. */
type Presence = { name: string | null; avatar: string | null; uptime_secs: number; build?: string };

/** The vendored xterm.js terminal (`vendor/xterm.js`). */
interface XtermTerminal {
  /** Set by `open`, which every terminal calls first. */
  element: HTMLElement;
  textarea: HTMLTextAreaElement;
  rows: number;
  cols: number;
  options: { fontSize: number; macOptionClickForcesSelection: boolean };
  modes: { mouseTrackingMode: string; applicationCursorKeysMode: boolean };
  buffer: { active: { type: string; viewportY: number } };
  parser: { registerOscHandler(ident: number, handler: (data: string) => boolean): XtermDisposable };
  open(parent: HTMLElement): void;
  loadAddon(addon: XtermAddon): void;
  write(data: string | Uint8Array, callback?: () => void): void;
  paste(data: string): void;
  reset(): void;
  focus(): void;
  dispose(): void;
  refresh(start: number, end: number): void;
  scrollLines(amount: number): void;
  selectLines(start: number, end: number): void;
  hasSelection(): boolean;
  getSelection(): string;
  clearSelection(): void;
  attachCustomKeyEventHandler(handler: (event: KeyboardEvent) => boolean): void;
  onData(listener: (data: string) => void): XtermDisposable;
  onResize(listener: (size: { rows: number; cols: number }) => void): XtermDisposable;
  onSelectionChange(listener: () => void): XtermDisposable;
}

/** What an xterm.js `on…` call returns. */
interface XtermDisposable {
  dispose(): void;
}

/** An xterm.js addon. */
interface XtermAddon {
  dispose(): void;
}

/** The fit addon (`vendor/xterm-addon-fit.js`). */
interface XtermFitAddon extends XtermAddon {
  fit(): void;
}

/** The WebGL renderer addon (`vendor/xterm-addon-webgl.js`). */
interface XtermWebglAddon extends XtermAddon {
  onContextLoss(listener: () => void): XtermDisposable;
}

/** A lucide icon node: the tag, its attributes, and the child nodes. */
type LucideNode = [tag: string, attrs: Record<string, string | number>, children?: LucideNode[]];

/** An icon of `lucide.icons`: an `<svg>` node with its children. */
type LucideIcon = [tag: string, attrs: Record<string, string | number>, children: LucideNode[]];

/** What an Alpine directive handler gets with its element. */
interface AlpineDirective {
  expression: string;
}

/** The Alpine helpers a directive handler uses. */
interface AlpineDirectiveUtilities {
  evaluateLater(expression: string): (receive: (value: string) => void) => void;
  effect(callback: () => void): void;
}

/** A Wunderbaum tree (`vendor/wunderbaum`), as `wb-files.ts` builds it. */
interface WunderbaumTree {
  root: WunderbaumNode;
  element: HTMLElement;
  findFirst(match: (node: WunderbaumNode) => boolean): WunderbaumNode | undefined;
  getActiveNode(): WunderbaumNode | null;
  filterNodes(
    match: (node: WunderbaumNode) => boolean,
    options: { mode: "hide" | "dim"; autoExpand: boolean; matchBranch: boolean; noData: boolean },
  ): void;
  isFilterActive(): boolean;
  updateFilter(): void;
  clearFilter(): void;
  enableUpdate(flag: boolean): void;
  updatePendingModifications(): void;
  destroy(): void;
}

/** A row of a Wunderbaum tree. Wunderbaum copies a source key it does not
 * know into `data`: `folder` is read there. */
interface WunderbaumNode {
  title: string;
  /** `null` for the root's parent. */
  parent: WunderbaumNode | null;
  /** `null` on a lazy or empty folder. */
  children: WunderbaumNode[] | null;
  data: { folder?: boolean };
  lazy: boolean;
  expanded: boolean;
  classes: string;
  /** `null` once a reload of an ancestor has unregistered the node. */
  tree: WunderbaumTree | null;
  setExpanded(flag: boolean): Promise<void>;
  setActive(): void;
  visit(callback: (node: WunderbaumNode) => void): void;
  hasClass(name: string): boolean;
  removeChildren(): void;
  load(source: WunderbaumSource[]): Promise<void>;
  isLoading(): boolean;
  startEditTitle(): void;
}

/** One row a tree level is loaded from. */
type WunderbaumSource = { title: string; folder?: boolean; lazy?: boolean; icon?: string; classes?: string };

/** What a Wunderbaum event handler gets. */
interface WunderbaumEvent {
  tree: WunderbaumTree;
  node: WunderbaumNode;
  /** The row's element (`render`). */
  nodeElem: HTMLElement;
  /** The new expanded state (`expand`, `beforeExpand`). */
  flag: boolean;
  /** Set when the first load failed (`init`). */
  error?: Error;
  /** The title before and after an edit (`edit.apply`). */
  oldValue: string;
  newValue: string;
}

/** The options `wb-files.ts` builds a tree with. */
interface WunderbaumOptions {
  element: Element;
  header: boolean;
  filter: { autoApply: boolean; mode: "hide" | "dim" };
  source: WunderbaumSource[] | Promise<WunderbaumSource[]>;
  /** `false`: the level stays unloaded. */
  lazyLoad(e: WunderbaumEvent): Promise<WunderbaumSource[] | false>;
  /** `false` keeps the folder closed. */
  beforeExpand(e: WunderbaumEvent): false | undefined;
  load(e: WunderbaumEvent): void;
  render(e: WunderbaumEvent): void;
  init(e: WunderbaumEvent): void;
  edit: { trigger: string[]; apply(e: WunderbaumEvent): boolean };
  expand(e: WunderbaumEvent): void;
  /** `false` stops the default action. */
  dblclick(e: WunderbaumEvent): boolean;
}

interface Window {
  /** The daemon door (`wb-daemon.ts`). `subscribeTree` may be absent:
   * `wb-files.ts` tests `window.WBDaemon?.subscribeTree` and then calls it
   * through the bare name, and a function that is always there is a TS2774
   * error at that test. The bare `WBDaemon` below has it. */
  WBDaemon: Omit<import("./wb-daemon.ts").WBDaemonApi, "subscribeTree"> & {
    subscribeTree: import("./wb-daemon.ts").WBDaemonApi["subscribeTree"] | undefined;
  };
  Alpine: {
    data(name: string, factory: () => object): void;
    directive(
      name: string,
      handler: (el: HTMLElement, directive: AlpineDirective, utilities: AlpineDirectiveUtilities) => void,
    ): void;
    store(name: "projects"): import("./wb-projects-store.ts").ProjectsStore;
    store(name: string, value: object): void;
    raw<T>(value: T): T;
    start(): void;
  };
  /** The event bus of `app.ts`, or the opener bridge of a torn-off page. */
  WB: {
    emit<K extends WorkbenchActionName>(action: K, ...detail: WorkbenchEmitArgs<K>): void;
  };
  /** The consoles (`wb-console.ts`). */
  WBConsole: ReturnType<typeof import("./wb-console.ts").createConsole>;
  /** `app.ts`: `shell()`, and the live instance of it Alpine built. */
  shell: () => object;
  getShell(): import("./app.ts").Shell | null;
  WBRuns: { output(text: string): void };
  /** The note cards (`wb-notes.ts`). */
  WBNotes: ReturnType<typeof import("./wb-notes.ts").createNotes>;
  /** The file pane (`wb-viewer.ts`). */
  WBViewer: ReturnType<typeof import("./wb-viewer.ts").createViewer>;
  /** The vendored lucide (`vendor/lucide.js`). */
  lucide: {
    icons: Record<string, LucideIcon | undefined>;
    createElement(node: LucideNode): SVGElement;
  };
}

/** The fields of a file-tree gesture on one node (`wb-files.ts`). */
type WorkbenchNodeDetail = { project: string | null; path: string; title: string; isFolder: boolean };

/** Every gesture `WB.emit` sends, and the detail its senders give. `null`: the
 * gesture has no detail. `wb-events.ts` lists the same names at run time, for
 * the popup bridges. `wb-daemon.ts` forwards `run-start` and `command` to the
 * daemon; the page itself handles the others. */
interface WorkbenchActions {
  "kanban-toggle": { open: boolean };
  /** `checkout`: the checkout the gesture aims at; `null` is the primary tree. */
  "branch-switch": { project: string | null; branch: string; checkout: string | null };
  /** `from`: the branch the new one starts from. */
  "branch-create": { project: string | null; name: string; from: string; checkout: string | null };
  "run-issue-focus": { project: string | null; runid: string | undefined; issue: number };
  /** The daemon refuses a missing `agent` or `branchMode`. A `null`
   * `planAgent` means the agent also plans. */
  "run-start": { project: string | null; agent: string; planAgent: string | null; branchMode: string; command: string };
  /** A run verb with no parameter (triage, push). */
  command: { project: string | null; verb: string };
  "issue-label-change": { project: string | null; number: number; label: string; op: "add" | "remove" };
  login: null;
  logoff: null;
  "open-refused": { project: string | null; path: string; reason: string };
  "open-diff": { project: string; path: string; checkout: string | null };
  "detach-blocked": { project: string; path: string };
  detach: { project: string; path: string };
  /** `agent`: `null` for a plain console. */
  "console-open": { repo: string | null; agent: string | null; plain: boolean };
  "console-close": { repo: string | null; agent: string | null | undefined };
  "console-restart": { repo: string | null; agent: string | null | undefined };
  /** `from`, `to`: `null` is the primary tree. */
  "console-switch-checkout": { repo: string | null; from: string | null; to: string | null };
  "worktree-created": { project: string; name: string; message: string };
  "fence-reattach": { fence: string };
  "fence-focus": { fence: string };
  "fence-detach": { fence: string };
  "fence-detach-blocked": { fence: string };
  "fence-detach-refused": { fence: string; reason: string };
  "fence-remove-refused": { fence: string; reason: string };
  /** `from`, `to`: the full rel paths before and after. */
  rename: WorkbenchNodeDetail & { from: string; to: string };
  open: WorkbenchNodeDetail & { ftype: string };
  delete: WorkbenchNodeDetail;
  "copy-path": WorkbenchNodeDetail;
  /** `path`: the directory the new entry goes in. */
  create: { project: string | null; path: string; kind: string; isFolder: boolean };
  /** `project`: `null` for a setting of this browser. */
  "setting-change": { project: string | null; key: string; value: unknown };
  /** The pane's text, its encoding and its byte order mark. */
  save: {
    project: string;
    path: string;
    bytes: number;
    content: string;
    checkout: string | null;
    encoding: string | undefined;
    bom: boolean;
  };
  reload: { project: string; path: string };
}

type WorkbenchActionName = keyof WorkbenchActions;

/** The arguments of `WB.emit` after the action name. A popup bridge adds the
 * popup as `fromWindow`. */
type WorkbenchEmitArgs<K extends WorkbenchActionName> = [WorkbenchActions[K]] extends [null]
  ? []
  : [detail: WorkbenchActions[K] & { fromWindow?: Window }];

/** The detail of `workbench:action`: the gesture (`action`), the fields its
 * sender gave, and when it was sent (`at`). A gesture a popup sent carries the
 * popup in `fromWindow`. */
type WorkbenchAction = {
  [K in WorkbenchActionName]: { action: K; at: string; fromWindow?: Window } & (WorkbenchActions[K] extends null
    ? unknown
    : WorkbenchActions[K]);
}[WorkbenchActionName];

/** A file a pane can reopen anywhere, a tab or a detached popup, with its
 * current content (`descOf` in `wb-viewer.ts`). */
type FileDescriptor = {
  project: string;
  label: string;
  path: string;
  ftype: string;
  content: string;
  checkout: string | null;
  encoding?: string;
  bom: boolean;
};

/** A repo file a rendered markdown link asks to open. `as: "bytes"`: the file
 * is not a note, and opens as a file. */
type OpenRequest = {
  project: string | null;
  path: string;
  checkout: string | null;
  fragment?: string;
  as?: "bytes";
};

// The `workbench:*` events (#633): every name a module sends or hears, and its
// `detail`. An event with no detail has `null`, as `new CustomEvent(name)`
// gives. `wb-events.ts` sends them; `src/tests.rs` fails on a `workbench:`
// name in a module or in `index.html` that is not listed here.

/** The `workbench:*` events sent on `window`. The `-open` events that
 * `index.html` sends with `$dispatch` reach the window, where a dialog hears
 * them with `.window`. */
interface WindowEventMap {
  /** The open project changed: the project now open and the one that was. */
  "workbench:project-changed": CustomEvent<{ slug: string | null; previous: string | null }>;
  /** The open checkout's HEAD moved. */
  "workbench:head-moved": CustomEvent<{ ref: string }>;
  "workbench:menus-close": CustomEvent<null>;
  /** Re-open the sockets; `stale` is the heartbeat verdict. */
  "workbench:sockets-resume": CustomEvent<{ stale: boolean }>;
  "workbench:fleet-read": CustomEvent<null>;
  /** A sleeping peer woke. */
  "workbench:peer-woken": CustomEvent<{ daemon: string }>;
  "workbench:checkout-changed": CustomEvent<null>;
  "workbench:panels-reread": CustomEvent<null>;
  "workbench:log-off": CustomEvent<null>;
  /** Re-list the directory `rel`, then show `reveal`. */
  "workbench:tree-dirty": CustomEvent<{ rel: string; reveal?: string }>;
  "workbench:file-search-open": CustomEvent<null>;
  /** Ask where to move the row at the full rel path `from`. */
  "workbench:move-open": CustomEvent<{ from: string }>;
  "workbench:move-confirmed": CustomEvent<{ from: string; to: string }>;
  "workbench:whats-new-open": CustomEvent<null>;
  "workbench:security-open": CustomEvent<null>;
  "workbench:about-open": CustomEvent<null>;
  "workbench:settings-open": CustomEvent<null>;
  "workbench:add-project-open": CustomEvent<null>;
  "workbench:hosts-open": CustomEvent<null>;
}

/** The `workbench:*` events sent on `document`. */
interface DocumentEventMap {
  /** A gesture, for the daemon door and the shell. */
  "workbench:action": CustomEvent<WorkbenchAction>;
  "workbench:open-request": CustomEvent<OpenRequest>;
  "workbench:detach-request": CustomEvent<FileDescriptor>;
  "workbench:reattach-request": CustomEvent<FileDescriptor>;
  /** The divider between the two panes moved. */
  "workbench:split-ratio": CustomEvent<{ ratio: number }>;
  "workbench:canvas-resize": CustomEvent<{ width: number }>;
  /** How many consoles are open. */
  "workbench:consoles-changed": CustomEvent<{ count: number }>;
  "workbench:stage-extent": CustomEvent<import("./wb-types.d.ts").Size>;
  "workbench:columns-stale": CustomEvent<null>;
  /** A console's title bar asks to restore its column. */
  "workbench:column-restore": CustomEvent<{ id: string }>;
  /** A console's title bar asks for the columns list, under `rect`. */
  "workbench:column-open": CustomEvent<{ id: string; rect: DOMRect }>;
  /** A fence asks to open its consoles as columns. */
  "workbench:fence-columns": CustomEvent<{ items: { id: string; rect: import("./wb-types.d.ts").Rect }[] }>;
  /** These consoles leave the columns. */
  "workbench:columns-leave": CustomEvent<{ ids: string[] }>;
  "workbench:desk-restored": CustomEvent<null>;
  /** A card chose the file name of a never-saved note, before its first write. */
  "workbench:note-claimed": CustomEvent<{ id: string; claim: string }>;
  /** A card gave a never-saved note its file. */
  "workbench:note-named": CustomEvent<{ id: string; path: string }>;
}

// The page instances and vendored libraries that modules name bare.
declare var WBConsole: Window["WBConsole"];
// `const`, not `var`: a `var` is also a member of `window`, and the two types
// would merge into one `subscribeTree` that is always there.
declare const WBDaemon: import("./wb-daemon.ts").WBDaemonApi;
declare var WBViewer: Window["WBViewer"];
declare var WB: Window["WB"];
/** The vendored marked (`vendor/marked.min.js`). */
declare var marked: { parse(markdown: string): string };
/** The vendored DOMPurify (`vendor/dompurify.min.js`). */
declare var DOMPurify: {
  sanitize(dirty: string, config?: { USE_PROFILES?: { svg?: boolean; svgFilters?: boolean; html?: boolean } }): string;
};
/** The vendored xterm.js and its addons (`vendor/xterm*.js`). */
declare var Terminal: new (options: { convertEol: boolean; theme: { [color: string]: string } }) => XtermTerminal;
declare var FitAddon: { FitAddon: new () => XtermFitAddon };
declare var WebglAddon: { WebglAddon: new () => XtermWebglAddon };
declare var WebLinksAddon: { WebLinksAddon: new () => XtermAddon };
/** The vendored Wunderbaum (`vendor/wunderbaum`). */
declare var mar10: {
  Wunderbaum: {
    new (options: WunderbaumOptions): WunderbaumTree;
    /** The row an event happened on, or `null` outside the rows. */
    getNode(event: Event): WunderbaumNode | null;
  };
};

// A timer the page may not have set yet is `null`, and clearing it does
// nothing: the HTML timer API turns the id into a number, `null` into 0.
declare function clearTimeout(id: number | null | undefined): void;

/** The vendored qrcode-generator (`vendor/qrcode.js`). */
declare function qrcode(
  typeNumber: number,
  errorCorrection: string,
): { addData(data: string): void; make(): void; createImgTag(cellSize: number, margin: number): string };
