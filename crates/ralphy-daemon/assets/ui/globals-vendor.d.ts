// The ambient types of the vendored libraries (xterm.js, Wunderbaum, Alpine,
// lucide, marked, DOMPurify, qrcode) and the names they put on the page.
// The vendored libraries ship no types: each interface below is written by
// hand from the members the modules call, the way `qrcode` is typed. The
// page instances, the event maps and the daemon replies are in `globals.d.ts`.

/** The vendored xterm.js terminal (`vendor/xterm.js`). */
interface XtermTerminal {
  /** Set by `open`, which every terminal calls first. */
  element: HTMLElement;
  textarea: HTMLTextAreaElement;
  rows: number;
  cols: number;
  options: { fontSize: number; macOptionClickForcesSelection: boolean; allowProposedApi: boolean };
  unicode: { activeVersion: string };
  modes: { mouseTrackingMode: string; applicationCursorKeysMode: boolean };
  buffer: { active: { type: string; viewportY: number; baseY: number } };
  parser: { registerOscHandler(ident: number, handler: (data: string) => boolean): XtermDisposable };
  /** xterm's private core: only the scrollbar's visibility is read through it
   *  (`showScrollbarWhileNeeded`, wb-console-terminal.ts). */
  _core?: { _viewport?: { _scrollableElement?: { updateOptions(options: { vertical: number }): void } } };
  open(parent: HTMLElement): void;
  loadAddon(addon: XtermAddon): void;
  write(data: string | Uint8Array, callback?: () => void): void;
  paste(data: string): void;
  reset(): void;
  focus(): void;
  dispose(): void;
  refresh(start: number, end: number): void;
  resize(cols: number, rows: number): void;
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
  evaluateLater: (expression: string) => (receive: (value: string) => void) => void;
  effect: (callback: () => void) => void;
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

/** What every Wunderbaum event handler gets. */
interface WunderbaumEvent {
  tree: WunderbaumTree;
  node: WunderbaumNode;
}
/** `render`: the row's element. */
interface WunderbaumRenderEvent extends WunderbaumEvent {
  nodeElem: HTMLElement;
}
/** `expand` and `beforeExpand`: the new expanded state. */
/** A key pressed in the tree. `node` is the row that has the focus, if any. */
interface WunderbaumKeyEvent extends WunderbaumEvent {
  event: KeyboardEvent;
}

interface WunderbaumExpandEvent extends WunderbaumEvent {
  flag: boolean;
}
/** `init`: set when the first load failed. */
interface WunderbaumInitEvent extends WunderbaumEvent {
  error?: Error;
}
/** `edit.apply`: the title before and after the edit. */
interface WunderbaumEditEvent extends WunderbaumEvent {
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
  beforeExpand(e: WunderbaumExpandEvent): false | undefined;
  load(e: WunderbaumEvent): void;
  render(e: WunderbaumRenderEvent): void;
  init(e: WunderbaumInitEvent): void;
  edit: { trigger: string[]; apply(e: WunderbaumEditEvent): boolean };
  expand(e: WunderbaumExpandEvent): void;
  /** `false` stops the default action. */
  dblclick(e: WunderbaumEvent): boolean;
  /** `false` stops the tree's own handling of the key. */
  keydown(e: WunderbaumKeyEvent): false | undefined;
  /** Files dropped from the operator's computer; no `dragStart`, so no row
   * of the tree can be dragged. */
  dnd: {
    /** `false` lets a folder that is not loaded yet take a drop. */
    preventLazyParents: boolean;
    /** The drop effect when the drag names none. */
    dropEffectDefault: "copy" | "move" | "link";
    /** How long the pointer rests on a closed folder before it opens. */
    autoExpandMS: number;
    /** The drop regions a row takes; `false` refuses the drop. */
    dragEnter(e: WunderbaumDndEvent): "over" | false;
    /** Every `dragover` on a row. */
    dragOver(e: WunderbaumDndEvent): void;
    /** `false` keeps a closed folder closed this time. */
    dragExpand(e: WunderbaumDndEvent): boolean;
    drop(e: WunderbaumDndEvent): void;
  };
}

/** `dnd.dragEnter` and `dnd.drop`: the row under the pointer, and the
 * browser's event. */
interface WunderbaumDndEvent extends WunderbaumEvent {
  event: DragEvent;
}

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
declare var Unicode11Addon: { Unicode11Addon: new () => XtermAddon };
/** The vendored Wunderbaum (`vendor/wunderbaum`). */
declare var mar10: {
  Wunderbaum: {
    new (options: WunderbaumOptions): WunderbaumTree;
    /** The row an event happened on, or `null` outside the rows. */
    getNode(event: Event): WunderbaumNode | null;
  };
};

/** The vendored qrcode-generator (`vendor/qrcode.js`). */
declare function qrcode(
  typeNumber: number,
  errorCorrection: string,
): { addData(data: string): void; make(): void; createImgTag(cellSize: number, margin: number): string };
