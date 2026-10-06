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
  WBDaemon: {
    observe(verb: string, payload: object): Promise<DaemonReply>;
    spawn(verb: string, payload: object, onStatus: (st: SpawnStatus) => void): number;
  };
  WBFail: {
    failed(reply: unknown, fallback: string): string;
    cause(reply: unknown, fallback: string): string;
  };
  WBFleet: {
    groupTitle(group: { state: string; local: boolean }): string;
  };
  Alpine: {
    data(name: string, factory: () => object): void;
  };
}
