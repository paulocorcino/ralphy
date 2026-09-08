// Loading `app.js` — the one asset the per-module `load()` copies cannot reach.
//
// Every other test file in this directory carries its own small `load()`: read
// one source, run it against two or three stubs, take the global it defines.
// That shape works because those modules are leaves. `app.js` is not — it is the
// Alpine component the whole document hangs off, it reads its siblings, and
// it wants enough of a DOM to answer `matchMedia` and `querySelector`. The
// loader for it is thirty lines, and thirty lines copied into an eleventh file
// is how the eleventh file drifts from the ten. So it lives here.
//
// This deliberately does NOT replace the ten existing `load()` copies. They are
// each three lines, each states exactly which globals its module touches at load
// — which is documentation the module's own test should carry — and rewriting
// them to route through a shared stub factory would trade that for a shared
// object nobody reads. See ADR-0022's "smallest change that fits the seam".
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

export const UI = join(dirname(fileURLToPath(import.meta.url)), "../assets/ui");

const read = (name) => readFileSync(join(UI, name), "utf8");

// The siblings index.html loads BEFORE app.js, in ITS order — DERIVED from the
// document, never hand-listed. Order is not decoration: each one assigns its
// namespace onto `window`, `wb-console.js` destructures `WBGeometry` at module
// scope, and `shell()` reads several of them while it is still building its
// state object.
//
// A hand-kept copy of this list drifts silently and nothing notices: its own
// comment said "nineteen" while the array held twenty, and a module added to the
// document but not to the array would leave `app.test.mjs` characterizing a boot
// order the browser does not have. Reading the document removes the copy.
export const SIBLINGS = readFileSync(join(UI, "index.html"), "utf8")
  .split("\n")
  .map((line) => line.match(/<script src="([^"]+)"><\/script>/))
  .filter(Boolean)
  .map((m) => m[1])
  // Vendor bundles are third-party and none of them defines a `WB*` namespace;
  // the seed loader is a template literal resolved at runtime and points outside
  // the embedded tree. `app.js` itself is loaded last, by `loadShell`.
  .filter((src) => !src.startsWith("vendor/") && !src.includes("${") && src !== "app.js");

// A DOM that answers, and answers NOTHING. Every query misses, every element is
// absent: that is the honest state for a document whose body was never parsed,
// and it keeps the fold under test to the part that computes rather than the
// part that paints. A fold that cannot survive an empty document is a fold that
// is doing DOM work, which is the signal this harness exists to give.
function stubDocument() {
  return {
    readyState: "loading",
    addEventListener() {},
    removeEventListener() {},
    // A SINK, like the two listeners above, not an answer: `WB.emit` dispatches
    // a CustomEvent on the document, so a fold that announces something reaches
    // this. Absent, it throws — which reads as "the fold is broken" rather than
    // "this document paints nothing", the one thing this stub exists to say.
    dispatchEvent() {},
    querySelector: () => null,
    querySelectorAll: () => [],
    getElementById: () => null,
    createElement: () => ({
      style: {},
      classList: { add() {}, remove() {}, toggle() {}, contains: () => false },
      appendChild() {},
      setAttribute() {},
      addEventListener() {},
    }),
    documentElement: { style: { setProperty() {} }, classList: { add() {}, remove() {} } },
    body: { classList: { add() {}, remove() {}, contains: () => false }, appendChild() {} },
  };
}

function stubWindow() {
  return {
    addEventListener() {},
    removeEventListener() {},
    innerWidth: 1440,
    innerHeight: 900,
    devicePixelRatio: 1,
    matchMedia: () => ({
      matches: false,
      addEventListener() {},
      removeEventListener() {},
      addListener() {},
    }),
    location: { protocol: "http:", host: "127.0.0.1:7431", pathname: "/", search: "" },
    setTimeout: () => 0,
    clearTimeout() {},
    setInterval: () => 0,
    clearInterval() {},
    requestAnimationFrame: () => 0,
    fetch: async () => {
      throw new Error(
        "the harness serves no network: a fold that fetches is not a read-only fold",
      );
    },
  };
}

// Evaluate app.js and its siblings the way the browser does, and return a FRESH
// `shell()` state object.
//
// Fresh per call, never shared: `shell()` returns a mutable object with ~390
// keys, and a test that mutated a shared one would leak into whichever test ran
// next — the failure that reads as "passes alone, fails in the suite".
//
// `BroadcastChannel` is hidden for the load exactly as wb-console.test.mjs does
// it: Node 22 ships a real one, `wb-console.js` subscribes at module scope, and
// an open channel per load holds the event loop open so `node --test` never
// exits.
export function loadShell() {
  const window = stubWindow();
  const document = stubDocument();
  window.window = window;
  window.document = document;

  const realBC = globalThis.BroadcastChannel;
  delete globalThis.BroadcastChannel;
  try {
    for (const name of SIBLINGS) {
      new Function("window", "document", "location", read(name))(
        window,
        document,
        window.location,
      );
    }
    new Function("window", "document", read("app.js"))(window, document);
  } finally {
    globalThis.BroadcastChannel = realBC;
  }

  if (typeof window.shell !== "function") {
    throw new Error("app.js must define window.shell — the whole document hangs off it");
  }
  const state = window.shell();
  return { state, window, document };
}
