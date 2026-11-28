// The changes a page of assets/ui/wb-console.ts sends to the desk.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { WBColumns } from "../assets/ui/wb-columns.ts";
import { idLookups } from "./fake-document.mjs";
import { fakeClassList } from "./harness.mjs";
import { SAVED, UI, changesOf, deskPage, deskWin, settle } from "./wb-console-fixture.mjs";

// --- columns: only the leftmost is the maximized console the desk records ----
// `wb-console.ts` never loads `wb-columns.ts` (the popup boots without it), so
// the harness runs the REAL fold beside it, as `app.ts` does in the browser.
function loadColumns() {
  return WBColumns;
}

// A page sends only the fields each act changed (ADR-0050 amendment 2026-10-04).

// The selected checkout per project (ADR-0063 §4) is the fourth record type.
test("a checkout pick and a clear are two changes, and the clear names the tree it clears", async () => {
  const page = await deskPage({ checkouts: { "o/r": "wt" } });
  try {
    const { wb, sent } = page;
    assert.equal(wb.checkoutOf("o/r"), "wt");
    assert.deepEqual(wb.checkouts(), { "o/r": "wt" });
    // A copy, never the view itself.
    wb.checkouts()["o/r"] = "tampered";
    assert.equal(wb.checkoutOf("o/r"), "wt");

    wb.setCheckout("o/r", null);
    assert.equal(wb.checkoutOf("o/r"), null);
    wb.setCheckout("o/s", null); // nothing selected there: nothing to say
    wb.setCheckout("o/r", "wt-b");
    assert.equal(wb.checkoutOf("o/r"), "wt-b");
    await settle();
    assert.deepEqual(changesOf(sent), [
      { op: "checkout-clear", repo: "o/r", ifName: "wt" },
      { op: "checkout", repo: "o/r", name: "wt-b" },
    ]);
  } finally {
    page.restore();
  }
});

test("a desk from an older daemon has no checkouts and reads as none", async () => {
  const page = await deskPage({ checkouts: undefined });
  try {
    assert.equal(page.wb.checkoutOf("o/r"), null);
    assert.deepEqual(page.wb.checkouts(), {});
  } finally {
    page.restore();
  }
});

// The measured loss of 2026-10-04 came from a page that wrote what it had not
// changed. A flush reads nothing first and carries no record it was not told.
test("a flush sends only this page's changes: no read before it, no whole desk", async () => {
  const theirs = { id: "theirs", repo: "o/r", agent: "console", kind: "console", rect: { left: 1, top: 1, width: 300, height: 200 }, sessionId: 4, consoleName: "r #1" };
  const page = await deskPage({ rev: 3, generation: 7, windows: [theirs] });
  try {
    page.wb.setCheckout("o/r", "wt");
    await settle();
    assert.equal(page.gets.length, 1, `only the load read the desk: ${page.gets}`);
    assert.equal(page.sent.length, 1);
    assert.deepEqual(Object.keys(page.sent[0]).toSorted(), ["changes", "generation", "seq"]);
    assert.equal(page.sent[0].generation, 7);
    assert.ok(!JSON.stringify(page.sent).includes("theirs"), JSON.stringify(page.sent));
  } finally {
    page.restore();
  }
});

const RECT = { left: 80, top: 120, width: 240, height: 180 };
const card = (id, path, extra = {}) => ({ id, repo: "o/r", path, rect: RECT, ...extra });

test("saveNotes sends a create, the fields that changed, and a remove", async () => {
  const page = await deskPage({ notes: [card("n1", "a.note"), card("n2", "b.note")] });
  try {
    const { wb, sent } = page;
    // A move stamps nothing: a `ts` handed back is not a field.
    wb.saveNotes(wb.notes().map((n) => (n.id === "n1" ? { ...n, rect: { ...RECT, left: 500 }, ts: 99 } : n)));
    wb.saveNotes(wb.notes().filter((n) => n.id !== "n2"));
    wb.saveNotes(wb.notes().concat([card("n3", "")]));
    wb.saveNotes(wb.notes().map((n) => (n.id === "n3" ? { ...n, path: "c.note" } : n)));
    wb.saveNotes(wb.notes());
    await settle();
    assert.deepEqual(changesOf(sent), [
      { op: "set", type: "note", id: "n1", fields: { rect: { ...RECT, left: 500 } } },
      { op: "remove", type: "note", id: "n2" },
      { op: "create", type: "note", record: card("n3", "") },
      { op: "set", type: "note", id: "n3", fields: { file: { repo: "o/r", path: "c.note", checkout: null } } },
    ]);
    // A copy: mutating what was handed out must not reach the view.
    const kept = wb.notes();
    kept.pop();
    assert.equal(wb.notes().length, 2);
  } finally {
    page.restore();
  }
});

test("a fence rename sends its name, and a fence remove sends the remove", async () => {
  const fence = { id: "f1", name: "Fence 1", rect: { left: 0, top: 0, width: 400, height: 300 } };
  const page = await deskPage({ fences: [fence] });
  try {
    page.wb.renameFence("f1", "backend");
    page.wb.removeFence("f1");
    await settle();
    assert.deepEqual(changesOf(page.sent), [
      { op: "set", type: "fence", id: "f1", fields: { name: "backend" } },
      { op: "remove", type: "fence", id: "f1" },
    ]);
  } finally {
    page.restore();
  }
});


const match = (w, sel) =>
  sel.split(",").some((one) =>
    one.trim().split(".").filter(Boolean).every((c) => w.classList.contains(c)),
  );

// The DOM that `applyColumns` paints: a stage whose windows match the few
// selectors it queries, and a workspace with no scroll. The stage measures 0
// wide, so the fence refresh after a write returns early.
function columnDom(wins) {
  const all = (sel) => wins.filter((w) => match(w, sel));
  const one = (sel) => all(sel)[0] ?? null;
  const stageEl = { offsetWidth: 0, offsetHeight: 0, style: {}, querySelectorAll: all, querySelector: one };
  const workspaceEl = {
    scrollLeft: 0,
    scrollTop: 0,
    classList: fakeClassList(),
    querySelectorAll: all,
    querySelector: one,
  };
  return { stage: stageEl, workspace: workspaceEl };
}
function columnWin(id, maximized = false) {
  const classes = new Set(maximized ? ["session-window", "maximized"] : ["session-window"]);
  return deskWin(id, {
    classList: fakeClassList(...classes),
    style: { setProperty() {}, removeProperty() {} },
  });
}
const colRecord = (id, max) => ({ ...SAVED, id, max, sessionId: null, checkout: null });
const byId = (changes) => [...changes].toSorted((x, y) => x.id.localeCompare(y.id));

// ADR-0051 §5: the console that becomes first in the columns is written to
// the desk as an ordinary maximize, and the one that stops being first is
// written as not maximized. `WBColumns.fromStored` keeps the stored grid only
// when the desk records its first console as maximized.
test("applyColumns writes the moved maximize to the desk when asked to persist", async () => {
  let dom = null;
  const page = await deskPage(
    { windows: [colRecord("w-a", true), colRecord("w-b", false)] },
    {},
    { ...idLookups((id) => dom?.[id] ?? null), dispatchEvent() {} },
  );
  try {
    const C = loadColumns();
    const a = columnWin("w-a", true);
    const b = columnWin("w-b");
    dom = columnDom([a, b]);
    const r = C.restore([["w-a"], ["w-b"]], "w-a");
    page.wb.applyColumns(C.painted(r.columns, 2), { cap: 2, unmax: r.unmax, persist: true });
    await settle();
    assert.deepEqual(byId(changesOf(page.sent)), [
      { op: "set", type: "window", id: "w-a", fields: { max: false } },
      { op: "set", type: "window", id: "w-b", fields: { max: true } },
    ]);
  } finally {
    page.restore();
  }
});

// The old maximized console can stay in the grid, not first: a fence opened as
// columns whose leftmost member is another console. No `unmax` names it.
test("applyColumns writes the unmaximize of a console that stays a column, not first", async () => {
  let dom = null;
  const page = await deskPage(
    { windows: [colRecord("w-a", true), colRecord("w-b", false)] },
    {},
    { ...idLookups((id) => dom?.[id] ?? null), dispatchEvent() {} },
  );
  try {
    const C = loadColumns();
    const a = columnWin("w-a", true);
    const b = columnWin("w-b");
    dom = columnDom([a, b]);
    page.wb.applyColumns(C.painted([["w-b"], ["w-a"]], 2), { cap: 2, unmax: null, persist: true });
    await settle();
    assert.deepEqual(byId(changesOf(page.sent)), [
      { op: "set", type: "window", id: "w-a", fields: { max: false } },
      { op: "set", type: "window", id: "w-b", fields: { max: true } },
    ]);
  } finally {
    page.restore();
  }
});

// The torn-off fence window's grid is never stored (ADR-0051 §8, amended
// 2026-10-05), so its one call never asks to write the maximize.
// pin: the torn-off fence window never stores its grid —
// its page script runs only in its own browser window, never under node.
test("the torn-off fence window paints its columns without persist", () => {
  // The page and its page script module.
  const html =
    readFileSync(join(UI, "detached-fence.html"), "utf8") + readFileSync(join(UI, "wb-detached-fence.ts"), "utf8");
  const calls = html.match(/WBConsole\.applyColumns\([^;]*;/gu) || [];
  assert.equal(calls.length, 1, calls.join("\n"));
  assert.ok(!/\bpersist\b/u.test(html), "the torn-off fence window names persist");
});

// The torn-off fence window paints its own grid and never stores it
// (ADR-0051 §8, amended 2026-10-05), so its maximize is not written.
test("applyColumns writes nothing to the desk without persist", async () => {
  let dom = null;
  const page = await deskPage(
    { windows: [colRecord("w-a", true), colRecord("w-b", false)] },
    {},
    { ...idLookups((id) => dom?.[id] ?? null), dispatchEvent() {} },
  );
  try {
    const C = loadColumns();
    const a = columnWin("w-a", true);
    const b = columnWin("w-b");
    dom = columnDom([a, b]);
    const r = C.restore([["w-a"], ["w-b"]], "w-a");
    page.wb.applyColumns(C.painted(r.columns, 2), { cap: 2, unmax: r.unmax });
    await settle();
    assert.ok(b.classList.contains("maximized"), "the page still shows the move");
    assert.deepEqual(page.sent, []);
  } finally {
    page.restore();
  }
});

// Seven of the eleven old window writes were not about the rect, and each
// wrote the rect anyway. A reconnect writes the session, only when it moved.
test("a reconnect writes the session only when it differs, and never the rect", async () => {
  const page = await deskPage({ windows: [SAVED] });
  try {
    const win = deskWin("w-1", { _term: { sessionId: 7 }, _deskCheckout: "wt-a" });
    page.wb.recordSession(win);
    await settle();
    assert.deepEqual(page.sent, [], "the same session writes nothing");
    win._term.sessionId = 8;
    page.wb.recordSession(win);
    await settle();
    assert.deepEqual(changesOf(page.sent), [
      { op: "set", type: "window", id: "w-1", fields: { session: { sessionId: 8, daemonId: null, environment: null } } },
    ]);
  } finally {
    page.restore();
  }
});

test("a birth writes only what it changed; an adopted console waits for the operator's first act", async () => {
  const page = await deskPage({ windows: [SAVED] });
  try {
    const { wb, sent } = page;
    // A relaunch in the primary tree: the record said `wt-a`.
    wb.recordBirth(deskWin("w-1"), SAVED);
    // An adopted console: its record is another page's to write.
    const adopted = deskWin("w-adopted");
    wb.recordBirth(adopted, { unrecorded: true });
    // A new console: its record is created.
    wb.recordBirth(deskWin("w-new"));
    await settle();
    assert.deepEqual(changesOf(sent), [
      { op: "set", type: "window", id: "w-1", fields: { checkout: null } },
      {
        op: "create",
        type: "window",
        record: {
          id: "w-new", repo: "o/r", agent: "console", kind: "console",
          rect: { left: 30, top: 40, width: 640, height: 420 }, max: false, sessionId: null,
          daemonId: null, environment: null, checkout: null, locked: false, consoleName: "r #1",
        },
      },
    ]);
    assert.equal(adopted._deskUnrecorded, true);
    // The operator's first act on it: the record, then the act, in one batch.
    sent.length = 0;
    wb.setWin(adopted, { locked: true });
    await settle();
    assert.equal(sent.length, 1, "one batch");
    assert.deepEqual(
      sent[0].changes.map((c) => [c.op, c.type, c.record?.id ?? c.id]),
      [["create", "window", "w-adopted"], ["set", "window", "w-adopted"]],
    );
    assert.equal(adopted._deskUnrecorded, false);
  } finally {
    page.restore();
  }
});

// A failure the daemon may still accept keeps the batch, resent with its
// `seq`; a 400 drops it; a restore reloads the page.
test("a flush keeps a batch the network lost, drops one the daemon calls malformed, and reloads on a restore", async () => {
  let reloads = 0;
  const page = await deskPage(
    {},
    {
      answers: [{ kind: "network" }, { kind: "ok", reply: null }, { kind: "refused", status: 400, reply: { error: "x" } }, { kind: "refused", status: 409, reply: { state: "restored" } }],
      window: { location: { reload: () => reloads++ } },
    },
  );
  try {
    const { wb, sent } = page;
    wb.setCheckout("o/r", "a");
    await settle(1600); // the first retry waits one second
    assert.equal(sent.length, 2);
    assert.deepEqual(sent[1], sent[0], "the resend is the same batch, with the same seq");
    wb.setCheckout("o/r", "b");
    await settle();
    assert.equal(sent[2].seq, sent[0].seq + 1);
    wb.setCheckout("o/r", "c");
    await settle();
    assert.deepEqual(changesOf([sent[3]]), [{ op: "checkout", repo: "o/r", name: "c" }], "the dropped change is gone");
    assert.equal(reloads, 1, "a restore reloads the page");
    wb.setCheckout("o/r", "d");
    await settle();
    assert.equal(sent.length, 4, "a page that reloads sends nothing more");
  } finally {
    page.restore();
  }
});
