// Unit tests for assets/ui/wb-settings-dialog.js, the Settings dialog's Alpine
// component. It is built with `loadComponent`, so every test here also fails
// when the component reads a shell() name it does not list in `uses`, or
// assigns a shell() field (ADR-0073 D4). The schema itself (`wb-settings.js`)
// is tested in wb-settings.test.mjs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { bindingNames, componentMarkup, loadComponent, loadShell, UI, withoutComments } from "./harness.mjs";

const HTML = readFileSync(join(UI, "index.html"), "utf8");

// Evaluate a markup expression the way Alpine does: names resolve on the
// scope. `with` needs sloppy mode, which `new Function` gives.
const evalIn = (scope, expr) => new Function("scope", `with (scope) { ${expr}; }`)(scope);
const valueIn = (scope, expr) => new Function("scope", `with (scope) { return ${expr}; }`)(scope);

// The element that holds the dialog's component, up to its `>`.
const wrapperTag = () => {
  const html = withoutComments(HTML);
  const at = html.indexOf('x-data="wbSettingsDialog"');
  return html.slice(html.lastIndexOf("<div", at), html.indexOf(">", at) + 1);
};
const handlerOf = (event) => wrapperTag().match(new RegExp(`@${event}\\.window="([^"]*)"`))[1];

// `saveSetting` ends by announcing on the `WB` bus, which app.js reads as a BARE
// global — in a browser `window.WB` IS a global, in this harness `window` is a
// parameter object and the reference does not resolve. Promote the REAL
// namespace the siblings built (never a fake one) and restore after, the way
// the harness itself borrows and restores `BroadcastChannel`.
// `async`, and the body is AWAITED inside the try: a synchronous `finally`
// around a returned promise restores the global before the fold has run, which
// is exactly the "WB is not defined" this helper exists to prevent.
async function withDialog(run) {
  const { scope: state, shell, window } = loadComponent("wbSettingsDialog");
  const calls = [];
  window.WBDaemon = {
    observe: async (verb, payload) => {
      calls.push({ verb, payload });
      return { status: "ok" };
    },
  };
  // A repo must be open or the fold skips the daemon entirely.
  shell.openSlug = "owner/repo";
  const real = globalThis.WB;
  globalThis.WB = window.WB;
  try {
    return await run(state, calls);
  } finally {
    if (real === undefined) delete globalThis.WB;
    else globalThis.WB = real;
  }
}

test("turning a toggle off SETS false — it does not unset the key", async () => {
  await withDialog(async (s, calls) => {
    await s.saveSetting("claude.console_name", false);
    assert.deepEqual(calls.at(-1), {
      verb: "config.set",
      payload: { repo: "owner/repo", key: "claude.console_name", value: "false" },
    });
    // The optimistic local edit is what the checkbox re-renders off, and
    // `:checked="settings[key] === true"` needs a real boolean to read false.
    assert.equal(s.settings["claude.console_name"], false);

    await s.saveSetting("claude.console_name", true);
    assert.deepEqual(calls.at(-1), {
      verb: "config.set",
      payload: { repo: "owner/repo", key: "claude.console_name", value: "true" },
    });
    assert.equal(s.settings["claude.console_name"], true);
  });
});

// The control that gives the test above its meaning: emptying a TEXT field is
// what `config.unset` is for. Were the falsy check widened to catch `false`,
// this case would keep passing and the boolean one would silently start
// clearing keys instead of writing them.
test("an emptied value still unsets, and that is a different verb", async () => {
  await withDialog(async (s, calls) => {
    for (const empty of ["", "unset", null]) {
      await s.saveSetting("claude.plan_model", empty);
      assert.equal(calls.at(-1).verb, "config.unset", `${JSON.stringify(empty)} must unset`);
    }
  });
});

test("a client-scoped key never reaches the daemon", async () => {
  await withDialog(async (s, calls) => {
    await s.saveSetting("consoles.relaunch_on_load", true);
    assert.equal(calls.length, 0, "a per-browser choice must not land in a repo's settings.json");
  });
});

// The console text size is the same `font` field the key bar's A−/A+ write, so
// the setting and the buttons never disagree. `wb-view.js` reads a BARE
// `localStorage`; a Map-backed one stands in for the browser's for this test.
test("the console text size is saved to the view store, held to the key bar's range", async () => {
  const real = globalThis.localStorage;
  const map = new Map();
  globalThis.localStorage = {
    getItem: (k) => (map.has(k) ? map.get(k) : null),
    setItem: (k, v) => map.set(k, String(v)),
  };
  try {
    await withDialog(async (s, calls) => {
      const font = () => JSON.parse(map.get("wb.view.v1") || "{}").font;
      await s.saveSetting("consoles.font_size", "12");
      assert.equal(font(), 12);
      assert.equal(s.settings["consoles.font_size"], 12);
      await s.saveSetting("consoles.font_size", "40");
      assert.equal(font(), 28, "above the range: the largest size");
      assert.equal(s.settings["consoles.font_size"], 28, "the field shows the size used");
      await s.saveSetting("consoles.font_size", "");
      assert.equal(font(), 15, "an emptied field is the default size");
      assert.equal(calls.length, 0, "a per-browser choice must not reach the daemon");
    });
  } finally {
    if (real === undefined) delete globalThis.localStorage;
    else globalThis.localStorage = real;
  }
});

// `openSettings()` merges `config.get` over the schema defaults with
// `k in this.settings` — a key the schema never seeded is dropped on the floor,
// and the control then renders its default forever while the repo says
// otherwise. Seeding is what makes the read-back work, so it is asserted for
// every key rather than for the new one.
test("every schema key is seeded, so config.get can merge over it", () => {
  const { scope: state, window } = loadComponent("wbSettingsDialog");
  for (const section of window.WB_SETTINGS) {
    for (const item of section.items) {
      assert.ok(
        item.key in state.settings,
        `${item.key} is rendered but never seeded — config.get would not reach it`,
      );
    }
  }
  assert.equal(state.settings["claude.console_name"], false, "the name is off until asked for");
});

test("a failed settings read says so, and a project setting is not written", async () => {
  const { scope: state, shell, window } = loadComponent("wbSettingsDialog");
  const verbs = [];
  window.WBDaemon.observe = async (verb) => {
    verbs.push(verb);
    return verb === "config.get" ? { status: "error", message: "settings.json is not JSON" } : { status: "ok" };
  };
  window.WBView.read = () => ({});
  shell.openSlug = "o/r";
  shell._flashAction = () => {};
  // `openSettings` names the bare `WBDaemon` global, as the page does.
  const realDaemon = globalThis.WBDaemon;
  globalThis.WBDaemon = window.WBDaemon;
  try {
    state.openSettings();
    for (let i = 0; i < 5; i++) await new Promise((r) => setImmediate(r));
  } finally {
    globalThis.WBDaemon = realDaemon;
  }
  assert.equal(state.settingsError, "Could not read the settings: settings.json is not JSON.");
  await state.saveSetting("queue.label", "ready");
  assert.deepEqual(verbs, ["config.get"], "no config.set over values never read");
});

test("the dialog lists at most 12 shell() names, and shell() has each one", () => {
  const { data, shell } = loadComponent("wbSettingsDialog");
  assert.ok(data.uses.length <= 12, `${data.uses.length} names: ${data.uses.join(", ")}`);
  for (const name of data.uses) assert.ok(name in shell, name);
  // The dialog's members left shell(): one owner for each name.
  for (const name of Object.keys(data)) {
    if (name !== "uses") assert.ok(!(name in shell), `shell() still has ${name}`);
  }
});

test("every name the dialog's markup reads is its own or in its uses list", () => {
  const { scope } = loadComponent("wbSettingsDialog");
  // The Devices section is its own component, nested one level deeper: it
  // reads its own names, not this dialog's.
  const markup = componentMarkup(HTML, "wbSettingsDialog").replace(
    /<template x-if="sec\.id === 'devices'[\s\S]*?<\/template>\s*<\/section>/,
    "</section>",
  );
  const attrs = [...markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
  // `x-for="sec in …"` and `x-for="(e, i) in …"` name the loop's own variables.
  const loop = new Set(attrs.filter((m) => m[1] === "x-for").flatMap((m) => bindingNames(m[2].split(" in ")[0])));
  // The parameters of an arrow function in a binding are its own too.
  for (const m of attrs) for (const a of m[2].matchAll(/(?:\(([\w\s,]*)\)|(\w+))\s*=>/g)) {
    for (const name of (a[1] ?? a[2]).split(",")) loop.add(name.trim());
  }
  let read = 0;
  for (const [attr, value] of attrs.map((m) => [m[1], m[2]])) {
    if (attr === "x-data") continue;
    for (const name of bindingNames(attr === "x-for" ? value.split(" in ").slice(1).join(" in ") : value)) {
      if (name.startsWith("$") || loop.has(name)) continue;
      assert.doesNotThrow(() => scope[name], `${attr}="${value}" reads ${name}`);
      read++;
    }
  }
  assert.ok(read > 40, `the markup was read: ${read} names`);
});

test("the rail button asks the dialog to open with workbench:settings-open, and shows the modal stack", () => {
  const html = withoutComments(HTML);
  const at = html.indexOf('title="Settings"');
  const button = html.slice(html.lastIndexOf("<button", at), at);
  const click = button.match(/@click="([^"]*)"/)[1];
  const active = button.match(/:class="([^"]*)"/)[1];
  assert.match(wrapperTag(), /@workbench:settings-open\.window="openSettings\(\)"/);

  // The opener runs in shell() scope: it closes the account menu and sends the event.
  const loaded = loadShell();
  const { scope: state } = loadComponent("wbSettingsDialog", { from: loaded });
  const sent = [];
  loaded.state.avatarMenu = true;
  loaded.state.$dispatch = (name) => sent.push(name);
  evalIn(loaded.state, click);
  assert.equal(loaded.state.avatarMenu, false, "the menu closes");
  assert.deepEqual(sent, ["workbench:settings-open"]);

  // The dialog's element hears it and opens.
  loaded.window.WBView.read = () => ({});
  evalIn(state, handlerOf("workbench:settings-open"));
  assert.equal(state.settingsOpen, true);

  // The button reads the modal stack, never the dialog's flag.
  const scrimEl = { querySelector: () => null };
  assert.equal(valueIn(loaded.state, active).active, false, "no modal: not active");
  loaded.state.modalOpened("settingsOpen", scrimEl);
  assert.equal(valueIn(loaded.state, active).active, true, "Settings on the modal stack: active");
  loaded.state.modalClosed("settingsOpen");
  assert.equal(valueIn(loaded.state, active).active, false);

  // Nothing outside the component names its state.
  const start = HTML.indexOf('x-data="wbSettingsDialog"');
  const outside = withoutComments(HTML.slice(0, start) + HTML.slice(HTML.indexOf("<!-- ===", start)));
  for (const name of ["openSettings", "closeSettings", "settingsSection", "settingsError", "deskHistory", "saveSetting"]) {
    assert.doesNotMatch(outside, new RegExp(`\\b${name}\\b`), name);
  }
  assert.equal(outside.match(/\bsettingsOpen\b/g)?.length, 1, "only the rail button names the open flag, as a modal-stack path");
});

test("log off closes the dialog with the workbench:log-off event", async () => {
  const sent = [];
  const loaded = loadShell({ window: { dispatchEvent: (e) => sent.push(e) } });
  const { scope: state } = loadComponent("wbSettingsDialog", { from: loaded });
  state.settingsOpen = true;
  // `logOff` emits on the bare `WB` global, which only the page defines.
  const real = globalThis.WB;
  globalThis.WB = loaded.window.WB;
  try {
    await loaded.state.logOff();
  } finally {
    if (real === undefined) delete globalThis.WB;
    else globalThis.WB = real;
  }
  const ev = sent.find((e) => e.type === "workbench:log-off");
  assert.ok(ev, `log off sends workbench:log-off on the window; sent ${sent.map((e) => e.type)}`);

  evalIn(state, handlerOf("workbench:log-off"));
  assert.equal(state.settingsOpen, false);
});

test("the open dialog reads its settings again on workbench:panels-reread, and a closed one does not", () => {
  const sent = [];
  const loaded = loadShell({ window: { dispatchEvent: (e) => sent.push(e) } });
  const { scope: state } = loadComponent("wbSettingsDialog", { from: loaded });
  loaded.state.tabs = loaded.state.tabs.filter((t) => t.id !== "spend");
  loaded.state.rereadOpenPanels();
  const ev = sent.find((e) => e.type === "workbench:panels-reread");
  assert.ok(ev, `rereadOpenPanels sends workbench:panels-reread on the window; sent ${sent.map((e) => e.type)}`);

  const reads = [];
  state.readSettings = () => reads.push("settings");
  state.settingsOpen = false;
  evalIn(state, handlerOf("workbench:panels-reread"));
  assert.deepEqual(reads, [], "a closed dialog reads nothing");
  state.settingsOpen = true;
  evalIn(state, handlerOf("workbench:panels-reread"));
  assert.deepEqual(reads, ["settings"]);
});
