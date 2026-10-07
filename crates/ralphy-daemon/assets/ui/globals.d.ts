// The classic scripts a module may read on `window` (ADR-0075 D9), and the
// vendored Alpine. Each surface lists only what a module calls. `any` marks a
// value the classic script does not type yet; it narrows when that script
// moves to TypeScript.

/** A `/ws/command` reply: `status` is "ok" or the reason it is not. */
type DaemonReply = { status: string } & Record<string, any>;

/** One update of a streamed verb (`WBDaemon.spawn`). */
type SpawnStatus =
  | { status: "output"; chunk: string }
  | { status: "exited"; code: number | null }
  | { status: "error"; message?: string };

interface Window {
  /** The daemon door (`wb-daemon.ts`). `subscribeTree` is `any`: `app.ts` tests
   * `window.WBDaemon?.subscribeTree`, then calls it through the bare name, and a
   * typed function there is a TS2774 error until `app.ts` imports the module. */
  WBDaemon: Omit<import("./wb-daemon.ts").WBDaemonApi, "subscribeTree"> & { subscribeTree: any };
  Alpine: {
    data(name: string, factory: () => object): void;
    directive(name: string, handler: (el: any, directive: any, utilities: any) => void): void;
    start(): void;
    [member: string]: any;
  };
  /** The event bus of app.js. */
  WB: {
    emit(name: string, detail?: object): void;
  };
  /** The view store (`wb-view.js`). */
  WBView: {
    read(): any;
    patch(fields: object): void;
  };
  /** The consoles (`wb-console.js`). */
  WBConsole: {
    fontSize(): number;
    stepFont(px: number, step: number): number;
    setFont(px: number): number;
    reloadForRestoredDesk(): void;
    [member: string]: any;
  };
  /** app.js: `shell()`, and the live instance of it Alpine built. */
  shell: () => object;
  getShell(): any;
  WBRuns: { output(text: string): void };
  WBConsoleName: any;
  WBDeskSink: any;
  WBDetachLink: any;
  WBNotes: any;
  WBViewer: any;
  /** The vendored lucide (`vendor/lucide.js`). */
  lucide: any;
}

// Classic scripts and vendored libraries that app.js names bare.
declare var WBColumns: Window["WBColumns"];
declare var WBConsole: Window["WBConsole"];
declare var WBDaemon: Window["WBDaemon"];
declare var WBViewer: any;
declare var marked: any;
declare var DOMPurify: any;
/** The vendored Wunderbaum (`vendor/wunderbaum`). */
declare var mar10: any;

/** The vendored qrcode-generator (`vendor/qrcode.js`). */
declare function qrcode(
  typeNumber: number,
  errorCorrection: string,
): { addData(data: string): void; make(): void; createImgTag(cellSize: number, margin: number): string };
