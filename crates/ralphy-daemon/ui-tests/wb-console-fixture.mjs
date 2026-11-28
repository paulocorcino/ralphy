// The shared loaders of the wb-console*.test.mjs files: the console module
// over a stub page, and a page over a fake desk daemon.
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { createConsole } from "../assets/ui/wb-console.ts";
import { WBDeskSink } from "../assets/ui/wb-desk-sink.ts";
import { idLookups } from "./fake-document.mjs";
import { fakeClassList, load as loadModule, withoutBroadcast } from "./harness.mjs";

export const UI = join(import.meta.dirname, "../assets/ui");
// The console family's source text, for the few pins on how it is written.
// It is every module named `wb-console.ts`, `wb-console-*.ts`,
// `wb-stage-*.ts`, `wb-desk.ts` or `wb-desk-*.ts`, joined in the order of
// `CONSOLE_FAMILY` in `src/tests/`, so a move between family modules does
// not move a pin (#621).
function familyGroup(name) {
  if (!name.endsWith(".ts") || name.endsWith(".d.ts")) return -1;
  if (name === "wb-console.ts") return 0;
  if (name.startsWith("wb-console-")) return 1;
  if (name.startsWith("wb-stage-")) return 2;
  if (name === "wb-desk.ts") return 3;
  if (name.startsWith("wb-desk-")) return 4;
  return -1;
}
function compareNames(a, b) {
  if (a < b) return -1;
  return a > b ? 1 : 0;
}
export const SRC = readdirSync(UI)
  .filter((name) => familyGroup(name) >= 0)
  .toSorted((a, b) => familyGroup(a) - familyGroup(b) || compareNames(a, b))
  .map((name) => readFileSync(join(UI, name), "utf8"))
  .join("");

// `extras` is merged into the stub `window` BEFORE the console is created, so
// a test can supply a browser global (`matchMedia`) or a page instance that
// the entry module creates. The default has no `WBNotes` and no `WBDaemon`,
// and several tests pin the fallback that produces. `opts` is what an entry module passes `createConsole`.
export function load(extras = {}, docExtras = {}, opts = {}) { // own: adds the sink reset and the page state to the harness load
  // `boot` is not called: the stub document has no stage. No `ResizeObserver`
  // is injected ON PURPOSE: the surviving one lives inside `attachTerminal`,
  // which this harness never reaches, so an observer created with the console
  // alongside a clamp fails LOUDLY here.
  // The sink's hold lives once per document (the module), and each `load()` is
  // a new page: a hold an earlier test set must not hold this page's writes.
  WBDeskSink.setHold(false);
  return withoutBroadcast(() =>
    loadModule(createConsole, {
      window: extras,
      document: { readyState: "loading", ...docExtras },
      deps: opts,
    }),
  );
}


// A page over a fake daemon: the GET serves `served`, and the sink records
// every body it is handed and answers `ok` with no desk.
export async function deskPage(served = {}, extras = {}, docExtras = {}) {
  const realFetch = globalThis.fetch;
  const gets = [];
  globalThis.fetch = (url) => { // own: the fake changes inside the test
    gets.push(String(url));
    return Promise.resolve({ ok: true, status: 200, json: () => Promise.resolve({ windows: [], fences: [], notes: [], ...served }) });
  };
  const sent = [];
  const answers = extras.answers || [];
  const wb = load({ ...extras.window }, { ...idLookups(() => null), ...docExtras }, {
    deskSink: {
      put(body) {
        sent.push(JSON.parse(body));
        return Promise.resolve(answers.shift() || { kind: "ok", reply: null });
      },
      putSync(body) {
        sent.push({ closing: true, ...JSON.parse(body) });
      },
    },
  });
  await wb.whenDeskLoaded();
  return { wb, sent, gets, restore: () => (globalThis.fetch = realFetch) }; // own: the fake changes inside the test
}
export const settle = (ms = 400) => new Promise((r) => { setTimeout(r, ms); });
export const changesOf = (sent) => sent.flatMap((b) => b.changes);

// A window the module writes without a gesture: a fake with the fields the
// writes read.
export function deskWin(id, extra = {}) {
  return {
    _deskId: id,
    _deskRepo: "o/r",
    _deskAgent: "console",
    _deskKind: "console",
    _deskDaemonId: null,
    _deskEnvironment: null,
    _deskCheckout: null,
    _deskLocked: false,
    _deskConsoleName: "r #1",
    _deskUnrecorded: false,
    isConnected: true,
    classList: fakeClassList(),
    offsetLeft: 30,
    offsetTop: 40,
    offsetWidth: 640,
    offsetHeight: 420,
    style: {},
    ...extra,
  };
}
export const SAVED = { id: "w-1", repo: "o/r", agent: "console", kind: "console", rect: { left: 100, top: 100, width: 600, height: 400 }, max: false, sessionId: 7, checkout: "wt-a", consoleName: "r #1" };
