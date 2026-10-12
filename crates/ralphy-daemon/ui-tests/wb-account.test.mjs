// Unit tests for assets/ui/wb-account.ts, the Alpine component of the account
// menu at the head of the rail. The component is built with `loadComponent`,
// so every test here also fails when it reads a shell() name it does not list
// in `uses`, or assigns a shell() field (ADR-0073 D4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { bindingNames, loadComponent, loadShell, UI, withoutComments } from "./harness.mjs";

const HTML = readFileSync(join(UI, "index.html"), "utf8");

// The menu's element, from its `<div` to the `</div>` that closes it, with
// comments cut out. It sits inside the rail, so no banner comment ends it.
function accountElement() {
  const html = withoutComments(HTML);
  const start = html.lastIndexOf("<div", html.indexOf('x-data="wbAccount"'));
  let depth = 0;
  for (const m of html.slice(start).matchAll(/<div\b|<\/div>/g)) {
    depth += m[0] === "</div>" ? -1 : 1;
    if (depth === 0) return { html, start, end: start + m.index + m[0].length };
  }
  throw new Error("the account menu's element is not closed");
}

// One page with the shell and the menu, and a window that delivers the events
// it is sent, as the browser does. `init()` adds the listeners, as Alpine does
// when it builds the component.
function page() {
  const listeners = {};
  const events = [];
  const loaded = loadShell({
    window: {
      addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
      dispatchEvent: (e) => {
        events.push(e);
        for (const fn of listeners[e.type] || []) fn(e);
        return true;
      },
    },
  });
  const account = loadComponent("wbAccount", { from: loaded });
  account.scope.init();
  return { loaded, shell: loaded.state, menu: account.scope, events };
}

// ---- the component and its markup ---------------------------------------------

test("wbAccount lists only shell() names in uses, and its members left shell()", () => {
  const { data, shell } = loadComponent("wbAccount");
  for (const n of data.uses) assert.ok(n in shell, n);
  for (const n of Object.keys(data)) {
    if (n !== "uses" && n !== "init") assert.ok(!(n in shell), `shell() still has ${n}`);
  }
});

test("every name the markup of wbAccount reads is its own or in its uses list", () => {
  const { scope: state } = loadComponent("wbAccount");
  const { html, start, end } = accountElement();
  const attrs = [...html.slice(start, end).matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
  let read = 0;
  for (const [, attr, value] of attrs) {
    if (attr === "x-data") continue;
    for (const n of bindingNames(value)) {
      if (n.startsWith("$")) continue;
      assert.doesNotThrow(() => state[n], `${attr}="${value}" reads ${n}`);
      read++;
    }
  }
  assert.ok(read >= 6, `the markup was read: ${read} names`);
});

test("nothing outside the account menu names its state", () => {
  const { html, start, end } = accountElement();
  const outside = html.slice(0, start) + html.slice(end);
  for (const name of ["avatarMenu", "toggleAvatarMenu", "closeAvatarMenu"]) {
    assert.doesNotMatch(outside, new RegExp(`\\b${name}\\b`), name);
  }
});

// ---- the workbench events ------------------------------------------------------

// ONE dropdown at a time: every menu trigger sends `workbench:menus-close`,
// and the account menu closes when it hears one.
test("the account menu closes on workbench:menus-close, and opening it sends one", () => {
  const { loaded, menu, events } = page();
  menu.toggleAvatarMenu();
  assert.equal(menu.avatarMenu, true);
  assert.deepEqual(
    events.map((e) => e.type).filter((t) => t === "workbench:menus-close"),
    ["workbench:menus-close"],
  );
  loaded.window.dispatchEvent(new CustomEvent("workbench:menus-close"));
  assert.equal(menu.avatarMenu, false);
});

test("log off closes the account menu it was chosen from", async () => {
  const { shell, menu } = page();
  menu.avatarMenu = true;
  await shell.$store.connection.logOff();
  assert.equal(menu.avatarMenu, false);
});
