// The arrow keys in a menu and in a tab list (wb-arrow-keys.ts), driven with
// fake elements: no browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { ARROW_LISTS, arrowStep, arrowTarget, menuEscape } from "../assets/ui/wb-arrow-keys.ts";

const [MENU, TABS] = ARROW_LISTS;

// A row: `matches` answers the selectors the module asks; `focus` records.
function row(name, { disabled = false, hidden = false, field = false } = {}) {
  return {
    name,
    focused: false,
    matches: (sel) => (sel === ":disabled" ? disabled : field && sel.startsWith("input")),
    getClientRects: () => (hidden ? [] : [{}]),
    focus() {
      this.focused = true;
    },
  };
}

// A container that holds `rows`; each row's `closest` finds it for `kind`.
function list(kind, rows) {
  const box = { querySelectorAll: () => rows };
  for (const r of rows) r.closest = (sel) => (sel === kind.container ? box : null);
  return rows;
}

const key = (k, mods = {}) => ({ key: k, altKey: false, ctrlKey: false, metaKey: false, ...mods });

test("Down and Up walk the enabled rows of a menu and wrap at the ends", () => {
  const [a, off, b, c] = list(MENU, [row("a"), row("off", { disabled: true }), row("b"), row("c")]);
  assert.equal(arrowStep(key("ArrowDown"), a), true);
  assert.equal(b.focused, true, "the disabled row is skipped");
  assert.equal(off.focused, false);
  assert.equal(arrowStep(key("ArrowDown"), c), true);
  assert.equal(a.focused, true, "Down on the last row goes to the first");
  a.focused = false;
  assert.equal(arrowStep(key("ArrowUp"), a), true);
  assert.equal(c.focused, true, "Up on the first row goes to the last");
});

test("Left and Right walk a tab list; Home and End go to its ends", () => {
  const [t1, t2, t3] = list(TABS, [row("t1"), row("t2"), row("t3")]);
  assert.equal(arrowStep(key("ArrowRight"), t1), true);
  assert.equal(t2.focused, true);
  assert.equal(arrowStep(key("End"), t1), true);
  assert.equal(t3.focused, true);
  assert.equal(arrowStep(key("ArrowDown"), t1), false, "Down is not a tab list key");
});

test("a field in a menu, a modifier or a hidden row keeps the key", () => {
  const [field, a] = list(MENU, [row("field", { field: true }), row("a")]);
  assert.equal(arrowStep(key("ArrowDown"), field), false, "the field keeps its caret keys");
  assert.equal(
    arrowStep(key("ArrowDown", { altKey: true }), a),
    false,
    "Alt+Shift chords are the page's",
  );
  assert.equal(arrowTarget(0, -1, "ArrowDown", MENU), -1, "an empty list moves nothing");
  assert.equal(arrowStep(key("ArrowDown"), null), false);
});

// Escape in a menu (wb-arrow-keys.ts `menuEscape`), on a fake page.
function menuPage({ shown = true, active = null } = {}) {
  const page = { closed: 0 };
  const menu = { getClientRects: () => (shown ? [{}] : []), contains: (el) => el?.inMenu === true };
  page.menus = () => [menu];
  page.active = () => active;
  page.closeAll = () => void page.closed++;
  return page;
}
const focusable = () => ({ focused: false, focus() { this.focused = true; } });
const inMenu = (field = false) => ({
  inMenu: true,
  matches: (sel) => field && sel.startsWith("input"),
  closest: () => ({}),
});

test("Escape closes an open menu and gives the focus back to the element it came from", () => {
  const opener = focusable();
  const page = menuPage({ active: inMenu() });
  assert.equal(menuEscape({ key: "Escape" }, page, opener), true);
  assert.equal(page.closed, 1);
  assert.equal(opener.focused, true);
});

test("Escape on the trigger closes the menu and leaves the focus where it is", () => {
  const opener = focusable();
  const page = menuPage({ active: { matches: () => false, closest: () => null } });
  assert.equal(menuEscape({ key: "Escape" }, page, opener), true);
  assert.equal(page.closed, 1);
  assert.equal(opener.focused, false);
});

test("Escape leaves the key when no menu is open, and to a field inside a menu", () => {
  const none = menuPage({ shown: false, active: inMenu() });
  assert.equal(menuEscape({ key: "Escape" }, none, null), false);
  const field = menuPage({ active: inMenu(true) });
  assert.equal(menuEscape({ key: "Escape" }, field, null), false);
  assert.equal(menuEscape({ key: "Enter" }, menuPage({ active: inMenu() }), null), false);
  assert.equal(none.closed + field.closed, 0);
});
