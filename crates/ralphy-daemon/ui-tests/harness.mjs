// Loading `app.ts` — the one asset the per-module `load()` copies cannot reach.
//
// Every other test file in this directory carries its own small `load()`: read
// one source, run it against two or three stubs, take the global it defines.
// That shape works because those modules are leaves. `app.ts` is not — it is the
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
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { shell, wire } from "../assets/ui/app.ts";
import { hostsDialog } from "../assets/ui/wb-hosts-dialog.ts";
import { settingsDialog } from "../assets/ui/wb-settings-dialog.ts";
import { devices } from "../assets/ui/wb-devices.ts";
import { securityDialog } from "../assets/ui/wb-security-dialog.ts";
import { releaseDialogs } from "../assets/ui/wb-release-dialogs.ts";
import { addProjectDialog } from "../assets/ui/wb-add-project-dialog.ts";
import { wbColumns, wbConsoleMenus } from "../assets/ui/wb-consoles-tab.ts";
import { wbFiles } from "../assets/ui/wb-files.ts";
import { wbMoveDialog } from "../assets/ui/wb-move-dialog.ts";
import { projectsStore } from "../assets/ui/wb-projects-store.ts";
import { WBSpend } from "../assets/ui/wb-spend.ts";
import { WBKanban } from "../assets/ui/wb-kanban.ts";
import { createDaemon } from "../assets/ui/wb-daemon.ts";
import { createViewer } from "../assets/ui/wb-file-viewer.ts";
import { createNotes } from "../assets/ui/wb-notes.ts";
import { createConsole } from "../assets/ui/wb-console.ts";
import { createMessages } from "../assets/ui/wb-messages.ts";
import { WBView } from "../assets/ui/wb-client-view.ts";
import { WBDeskSink } from "../assets/ui/wb-desk-sink.ts";
import { WBConsoleName } from "../assets/ui/wb-console-name.ts";

// The `window.WB*` names a module sets in the browser because a reader outside
// the modules reads them there (ADR-0075 D9): `WBKanban` and `WBSpend` for
// browser checks, `WBConsoleName` for the `index.html` markup. `app.ts` reads
// all three on `window`. A module ran once, at import, so the loader sets these
// on each page's window itself.
const MODULE_NAMESPACES = {
  WBSpend,
  WBKanban,
  WBConsoleName,
};

// The Alpine components, by the name the markup gives `x-data`: `loadComponent`
// builds each from its module's export (ADR-0073 D3, ADR-0075 D7).
const MODULE_COMPONENTS = {
  wbHostsDialog: hostsDialog,
  wbSettingsDialog: settingsDialog,
  wbDevices: devices,
  wbSecurityDialog: securityDialog,
  wbReleaseDialogs: releaseDialogs,
  wbAddProjectDialog: addProjectDialog,
  wbConsoleMenus,
  wbColumns,
  wbFiles,
  wbMoveDialog,
};

export const UI = join(dirname(fileURLToPath(import.meta.url)), "../assets/ui");

// A document's text with its `<!-- … -->` comments cut out, so a pin on markup
// does not match the tags that comment prose quotes. Our own asset, not a
// sanitizer: an unclosed comment drops the rest of the text.
export function withoutComments(text) {
  return text
    .split("<!--")
    .map((part, i) => {
      if (i === 0) return part;
      const end = part.indexOf("-->");
      return end < 0 ? "" : part.slice(end + 3);
    })
    .join("");
}

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
    // A SINK, like the document's: `shell()` sends the `workbench:*` events
    // that a component hears with `.window` on the window.
    dispatchEvent() {},
    innerWidth: 1440,
    innerHeight: 900,
    devicePixelRatio: 1,
    matchMedia: () => ({
      matches: false,
      addEventListener() {},
      removeEventListener() {},
      addListener() {},
    }),
    location: { protocol: "http:", host: "127.0.0.1:7431", hostname: "127.0.0.1", pathname: "/", search: "" },
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

// Create this page's instances the way `main.ts` does, run app.ts's `wire`
// on this page, and return a FRESH `shell()` state object.
//
// Fresh per call, never shared: `shell()` returns a mutable object with ~390
// keys, and a test that mutated a shared one would leak into whichever test ran
// next — the failure that reads as "passes alone, fails in the suite".
//
// `BroadcastChannel` is hidden while the console is created, exactly as
// wb-console.test.mjs does it: Node 22 ships a real one, `createConsole`
// subscribes, and an open channel per load holds the event loop open so
// `node --test` never exits.
//
// `opts.document` overrides stub members for the one test that must HEAR a
// listener the module registers at load (the write seam) — the stub's own
// `addEventListener` is a sink. `opts.window` does the same for the window
// (the detached popups' `message` listener and their `closed` poll).
//
// The Alpine store `projects` is a real `projectsStore()`, one per page, or
// `opts.projects`. `shell()` reads it as `this.$store.projects`, and the
// window bridge as `Alpine.store("projects")`, so both are set here.
export function loadShell(opts = {}) {
  const window = Object.assign(stubWindow(), opts.window || {});
  const projects = opts.projects || projectsStore();
  window.Alpine = { store: (name) => (name === "projects" ? projects : undefined) };
  const document = Object.assign(stubDocument(), opts.document || {});
  window.window = window;
  window.document = document;
  Object.assign(window, MODULE_NAMESPACES);
  // A copy per page of `WBView`, whose members tests replace, so a replaced
  // member stays on its own page. The sink's hold lives once per document
  // (the module), and each call is a new page, so it starts clear.
  window.WBView = { ...WBView };
  WBDeskSink.setHold(false);
  // One message door, one console, one daemon door, one file pane and one set
  // of note cards per page, in the order the entry module makes them. `boot`
  // is not called: the stub document has no stage. The stub document has no
  // `[x-data]` root, so `getShell` finds no shell: the door falls back to this
  // page's `shell()` state, the shell the browser would find.
  let state = null;
  const messages = createMessages(window, document, { shell: () => window.getShell?.() ?? state });
  const realBC = globalThis.BroadcastChannel;
  delete globalThis.BroadcastChannel;
  try {
    window.WBConsole = createConsole(window, document, window.location, { messages });
  } finally {
    globalThis.BroadcastChannel = realBC;
  }
  window.WBDaemon = createDaemon(window, document, window.location, { messages });
  window.WBViewer = createViewer(window, document, { messages });
  window.WBNotes = createNotes(window, document, { console: window.WBConsole, messages });
  // `shell()` reads the page's globals on the real `window` and `document`:
  // the last page loaded owns them, as in `loadComponent`.
  globalThis.window = window;
  globalThis.document = document;
  wire(window, document, { messages });
  state = shell();
  state.$store = { projects };
  return { state, window, document };
}

// Build the Alpine component registered as `alpineName` inside `shell()`, the
// way the page nests it, and return the scope its code and markup see.
//
// In the browser a nested `x-data` sees every member of `shell()`. ADR-0073 D4
// allows only the names in the component's `uses` list, and never a write to a
// `shell()` field or to a property inside one. `scope` is that rule: reading
// any other name throws, and so does assigning anything that is not the
// component's own. A `uses` value that is an object comes back read-only: a set
// or a delete on it, at any depth, throws a TypeError that names the component
// and the path (`security.passwordSet`). A `shell()` method in `uses` runs with
// `this` set to the merged scope, as Alpine runs it, so `scrim()` still reaches
// the modal stack and the dialog's open flag, and the method may write
// `shell()` state: that write is the one D4 allows. A getter of the component
// runs with the scope as `this`, as Alpine 3.14 runs it (`mergeProxies` passes
// the scope as the receiver of `Reflect.get`), so a getter that reads a
// `shell()` name obeys `uses` too.
//
// The factory is the module's export in `MODULE_COMPONENTS`. A module reads
// the page's globals on the real `window` and `document`, so both are set to
// this page's stubs: the last component built owns them, and node --test
// runs each file in its own process. `opts.magics` gives the `$` magics
// (`$nextTick`) that Alpine would add; `$store` is the page's stores.
// `opts.from` is a `loadShell()` result to nest the component in, so several
// components share one page; without it the other `opts` go to a new
// `loadShell`.
export function loadComponent(alpineName, opts = {}) {
  const { state: shell, window, document } = opts.from || loadShell(opts);
  const factory = MODULE_COMPONENTS[alpineName];
  if (typeof factory !== "function") {
    throw new Error(`no Alpine component ${alpineName} in MODULE_COMPONENTS`);
  }
  globalThis.window = window;
  globalThis.document = document;
  const data = factory();
  const uses = new Set(data.uses);
  for (const name of uses) {
    if (!(name in shell)) throw new Error(`${alpineName} lists ${name} in uses, and shell() has no such member`);
  }
  const magics = Object.assign({ $store: shell.$store }, opts.magics);
  const own = (k) => Object.prototype.hasOwnProperty.call(data, k);

  // Alpine's merged scope: the component first, then `shell()`.
  const full = new Proxy(
    {},
    {
      get: (_, k, receiver) => (own(k) ? Reflect.get(data, k, receiver) : k in magics ? magics[k] : shell[k]),
      set: (_, k, v) => {
        if (own(k)) data[k] = v;
        else shell[k] = v;
        return true;
      },
      has: (_, k) => own(k) || k in magics || k in shell,
    },
  );

  const scope = new Proxy(
    {},
    {
      get(_, k, receiver) {
        if (typeof k === "symbol") return undefined;
        if (own(k)) return Reflect.get(data, k, receiver);
        if (k in magics) return magics[k];
        if (uses.has(k)) {
          const v = shell[k];
          return typeof v === "function" ? (...args) => shell[k].apply(full, args) : readOnly(alpineName, v, k);
        }
        throw new ReferenceError(`${alpineName} reads ${k}, which is neither its own nor in its uses list`);
      },
      set(_, k, v) {
        if (own(k)) data[k] = v;
        else if (k.startsWith("$")) magics[k] = v;
        else throw new TypeError(`${alpineName} assigns ${k}, which is not its own: it calls a shell() method instead`);
        return true;
      },
      // `with (scope)` in a markup test: names the scope does not hold fall
      // through to the globals, and an unknown one is a ReferenceError there.
      has: (_, k) => own(k) || k in magics || uses.has(k),
    },
  );
  return { scope, data, shell, window, document };
}

// `value` seen through a Proxy that reads as the value and refuses every write,
// at any depth: ADR-0073 D4 (amendment of 2026-10-05). `path` names the value
// in the error. A property the object itself can never change (frozen) is
// given as it is, because a Proxy must return exactly that value.
function readOnly(alpineName, value, path) {
  if (value === null || typeof value !== "object") return value;
  const refuse = (k) => {
    throw new TypeError(`${alpineName} writes ${path}.${String(k)}, inside a shell() value: it calls a shell() method instead`);
  };
  return new Proxy(value, {
    get(target, k) {
      const v = Reflect.get(target, k);
      const d = Reflect.getOwnPropertyDescriptor(target, k);
      if (d && !d.configurable && !d.writable) return v;
      return typeof k === "symbol" ? v : readOnly(alpineName, v, `${path}.${k}`);
    },
    set: (_, k) => refuse(k),
    deleteProperty: (_, k) => refuse(k),
    defineProperty: (_, k) => refuse(k),
  });
}

// The text from the element that binds `x-data="<alpineName>"` to the close of
// the document part that holds it, with comments cut out. Ends at the next
// banner comment (`<!-- ===`), which in index.html starts each feature.
export function componentMarkup(html, alpineName) {
  const start = html.indexOf(`x-data="${alpineName}"`);
  if (start < 0) throw new Error(`index.html binds no x-data="${alpineName}"`);
  const end = html.indexOf("<!-- ===", start);
  return withoutComments(html.slice(start, end < 0 ? undefined : end));
}

// The names a markup binding reads from its scope: every identifier that is not
// a property (`a.b`), an object key (`{ on: … }`), a string, or a keyword.
// Loop variables of `x-for` and `$` magics are left to the caller.
export function bindingNames(expr) {
  const code = expr
    // `&amp;` last, so `&amp;lt;` stays the text `&lt;`.
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&")
    .replace(/'(?:[^'\\]|\\.)*'|"(?:[^"\\]|\\.)*"/g, "''");
  const names = new Set();
  for (const m of code.matchAll(/[A-Za-z_$][\w$]*/g)) {
    const before = code.slice(0, m.index);
    const after = code.slice(m.index + m[0].length);
    if (/\.\s*$/.test(before)) continue;
    if (/[{,]\s*$/.test(before) && /^\s*:/.test(after)) continue;
    if (["true", "false", "null", "undefined", "in", "of", "typeof"].includes(m[0])) continue;
    names.add(m[0]);
  }
  return [...names];
}
