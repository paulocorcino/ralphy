// Unit tests for assets/ui/wb-desk-folds.ts: the fence walk, the detach registry,
// the columns, the notes and the popup rules. Pure functions, with no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBColumns } from "../assets/ui/wb-columns.ts";
import {
  DETACH_MAX,
  addNote,
  atCap,
  columnClasses,
  detachFold,
  fenceCycle,
  nextFenceName,
  noteNameOk,
  placeholderSession,
  popupMatches,
  reconcileDesk,
  removeNote,
} from "../assets/ui/wb-desk-folds.ts";

// ---- walking the fences from the keyboard -----------------------------------
// Alt+Shift+←/→ steps through the fences in the plane's own READING ORDER — top
// band first, left to right inside it — not in the order the desk array happens
// to carry them. Creation order on a plane means the walk teleports across the
// stage; reading order makes the shortcut a sweep.
//
// A 120 px band is what keeps a row a row: two fences placed side by side are
// never pixel-aligned on `top`, and a raw `top` sort would zig-zag between them.
const at = (id, left, top) => ({ id, rect: { left, top, width: 400, height: 300 } });

// Deliberately shuffled against reading order: `c` is first in the array and
// last on the plane, so every row below is red under a fold that walks the
// array.
const GRID = [
  at("c", 900, 700), //  bottom row, right
  at("b", 900, 40), //   top row, right
  at("d", 60, 700), //   bottom row, left
  at("a", 60, 40), //    top row, left
];

const CYCLE = [
  { name: "no fences at all: nothing to walk", fences: [], from: null, step: 1, want: null },
  {
    name: "forward with nothing in hand enters at the top-left fence",
    from: null,
    step: 1,
    want: "a",
  },
  {
    // The other end, so "enters at an end" cannot pass by answering `order[0]`
    // for both directions.
    name: "backward with nothing in hand enters at the bottom-right fence",
    from: null,
    step: -1,
    want: "c",
  },
  { name: "forward walks left to right inside the top band", from: "a", step: 1, want: "b" },
  {
    // The band's own boundary: the walk leaves the top row only after both of
    // its fences, which is what a raw `top` sort would get wrong.
    name: "forward crosses to the next band after the last fence of this one",
    from: "b",
    step: 1,
    want: "d",
  },
  { name: "forward walks left to right inside the bottom band too", from: "d", step: 1, want: "c" },
  { name: "forward wraps from the last fence to the first", from: "c", step: 1, want: "a" },
  { name: "backward wraps from the first fence to the last", from: "a", step: -1, want: "c" },
  { name: "backward retraces the same order", from: "d", step: -1, want: "b" },
  {
    // A focus can outlive the fence that carried it (another client removed it
    // between the jump and the key): entering from an unknown id is the
    // no-fence-in-hand case, not a crash and not a stall.
    name: "an id no fence carries re-enters at the end the step names",
    from: "gone",
    step: -1,
    want: "c",
  },
  {
    name: "one fence: the walk is a no-op that still answers that fence",
    fences: [at("solo", 40, 40)],
    from: "solo",
    step: 1,
    want: "solo",
  },
];

for (const row of CYCLE) {
  test(`fenceCycle: ${row.name}`, () => {
    const got = fenceCycle(row.fences || GRID, row.from, row.step);
    assert.equal(got, row.want);
  });
}

test("fenceCycle breaks a tie on id, so every client walks the same order", () => {
  const same = [at("z", 100, 100), at("a", 100, 100)];
  assert.equal(fenceCycle(same, null, 1), "a");
  assert.equal(fenceCycle(same, "a", 1), "z");
});

test("fenceCycle reads a fence with no rect as the top-left corner and does not throw", () => {
  const fences = [at("far", 0, 500), { id: "b" }, { id: "a", rect: null }];
  assert.equal(fenceCycle(fences, null, 1), "a");
  assert.equal(fenceCycle(fences, "a", 1), "b");
  assert.equal(fenceCycle(fences, "b", 1), "far");
});

// ---- detachFold: the detach registry's transitions ---------------------------
// The fold is the whole decision surface for detaching a fence into its own
// window — every case below is a RELATION (what the registry becomes, which
// effect comes back), never a count of popups, because the caller is what turns
// an effect into a `window.open`.

test("detachFold: the cap is four — the fifth detach is refused, not opened", () => {
  assert.equal(DETACH_MAX, 4);
  let reg = [];
  const opened = [];
  for (const id of ["f-a", "f-b", "f-c", "f-d"]) {
    const out = detachFold(reg, { type: "detach", fenceId: id });
    opened.push(...out.effects.filter((e) => e.type === "open").map((e) => e.fenceId));
    reg = out.registry;
  }
  assert.deepStrictEqual(opened, ["f-a", "f-b", "f-c", "f-d"]);
  assert.equal(reg.length, 4);

  // THE NEGATIVE CONTROL. With the cap check deleted this line goes green with
  // an `open` and a 5-long registry, so both halves are asserted: the effect is
  // the literal `refuse`, AND the registry came back untouched.
  const fifth = detachFold(reg, { type: "detach", fenceId: "f-e" });
  assert.deepStrictEqual(fifth.effects, [{ type: "refuse", fenceId: "f-e", reason: "cap" }]);
  assert.equal(fifth.registry.length, 4);
  assert.deepStrictEqual(fifth.registry, reg);
});

test("detachFold: one popup per fence — detaching a detached fence focuses it", () => {
  const first = detachFold([], { type: "detach", fenceId: "f-a" });
  assert.deepStrictEqual(first.effects, [{ type: "open", fenceId: "f-a" }]);
  assert.equal(first.registry.length, 1);

  const again = detachFold(first.registry, { type: "detach", fenceId: "f-a" });
  assert.equal(again.registry.length, 1, "no second entry for the same fence");
  assert.equal(again.effects.length, 1);
  assert.equal(again.effects[0].type, "focus");
  assert.equal(again.effects[0].fenceId, "f-a");
});

test("detachFold: re-attach empties the registry, and a SECOND one is a no-op", () => {
  const held = detachFold([], { type: "detach", fenceId: "f-a" }).registry;

  const home = detachFold(held, { type: "reattach", fenceId: "f-a" });
  assert.deepStrictEqual(home.effects, [{ type: "close", fenceId: "f-a" }]);
  assert.deepStrictEqual(home.registry, []);

  // Both the popup's `beforeunload` AND the opener's `closed` poll report the
  // re-attach: the doubled signal must not spawn the consoles twice.
  const twice = detachFold(home.registry, { type: "reattach", fenceId: "f-a" });
  assert.deepStrictEqual(twice.effects, []);
  assert.deepStrictEqual(twice.registry, []);
});

test("detachFold: re-attaching a fence that was never detached changes nothing", () => {
  const out = detachFold(["f-a"], { type: "reattach", fenceId: "f-zzz" });
  assert.deepStrictEqual(out.effects, []);
  assert.deepStrictEqual(out.registry, ["f-a"]);
});

test("detachFold: focus reaches a detached fence only", () => {
  const member = detachFold(["f-a"], { type: "focus", fenceId: "f-a" });
  assert.deepStrictEqual(member.effects, [{ type: "focus", fenceId: "f-a" }]);
  assert.deepStrictEqual(member.registry, ["f-a"]);

  const stranger = detachFold(["f-a"], { type: "focus", fenceId: "f-b" });
  assert.deepStrictEqual(stranger.effects, [], "nothing to focus for an attached fence");
  assert.deepStrictEqual(stranger.registry, ["f-a"]);
});

test("detachFold: an unknown event is inert, so a stray message cannot detach", () => {
  const out = detachFold(["f-a"], { type: "heartbeat", fenceId: "f-a" });
  assert.deepStrictEqual(out.effects, []);
  assert.deepStrictEqual(out.registry, ["f-a"]);
});

// --- the fence cap and the default name -------------------------------------
// The name used to be `Fence ${fences.length + 1}`. MEASURED against the running
// shell: `length` freezes at the cap, so the 13th fence, the 14th and every one
// after were all born "Fence 13" — three of them coexisting in the list that IS
// the plane's map. Numbering from the names already present cannot collide.
test("nextFenceName counts from the names on the plane, not from how many there are", () => {
  assert.equal(nextFenceName([]), "Fence 1");
  assert.equal(nextFenceName(), "Fence 1");
  assert.equal(
    nextFenceName([{ name: "Fence 1" }, { name: "Fence 2" }, { name: "Fence 3" }]),
    "Fence 4",
  );
  // THE COLLISION: twelve fences whose numbers run past twelve. Counting would
  // answer "Fence 13" for the second time.
  const twelve = Array.from({ length: 12 }, (_, i) => ({ name: `Fence ${i + 2}` }));
  assert.equal(nextFenceName(twelve), "Fence 14");
  // A gap is not filled: the highest number wins, so a name is never reused by a
  // fence the operator did not delete.
  assert.equal(nextFenceName([{ name: "Fence 1" }, { name: "Fence 9" }]), "Fence 10");
  // A renamed fence is not a number. "backend" must not push the next default
  // anywhere, and "Fence 99" is a number the operator chose — so it counts.
  assert.equal(nextFenceName([{ name: "backend" }, { name: "planning" }]), "Fence 1");
  assert.equal(nextFenceName([{ name: "Fence 99" }]), "Fence 100");
  // Near-misses are not numbers either.
  assert.equal(
    nextFenceName([{ name: "Fence" }, { name: "Fence 2b" }, { name: "fence 5" }, { name: null }]),
    "Fence 1",
  );
});

// `applyColumns` maximizes only where `columnClasses(...).maximized` is true,
// and with `persist` that maximize is written to the desk.
// NEGATIVE CONTROL: answering `maximized: true` for every entry fails the "b"
// assertion below — that is two consoles recorded as maximized.
test("columnClasses never marks a column right of the leftmost maximized", () => {
  const C = WBColumns;
  const p = C.painted([["a"], ["b"], ["c"]], 3);
  assert.deepEqual(columnClasses(p, "a"), { column: true, maximized: true });
  assert.deepEqual(columnClasses(p, "b"), { column: true, maximized: false });
  assert.deepEqual(columnClasses(p, "c"), { column: true, maximized: false });
  assert.deepEqual(columnClasses(p, "x"), { column: false, maximized: null });
});

test("restoring the leftmost column promotes the next to the maximize the desk records", () => {
  const C = WBColumns;
  const r = C.restore([["a"], ["b"], ["c"]], "a");
  assert.equal(r.unmax, "a");
  assert.equal(columnClasses(C.painted(r.columns, 3), "b").maximized, true);
  assert.equal(columnClasses(C.painted(r.columns, 3), "c").maximized, false);
  assert.equal(columnClasses(C.painted(r.columns, 3), "a").maximized, null);
  // The last column left is an ordinary maximized console again.
  const last = C.restore([["a"], ["b"]], "a");
  assert.deepEqual(columnClasses(C.painted(last.columns, 2), "b"), {
    column: false,
    maximized: true,
  });
});

// Rows (ADR-0051 §5): only the top row of the leftmost column is the
// maximized console, and one column of two rows is already columns.
// NEGATIVE CONTROL: dropping the row check from `maximized` marks "b" too;
// counting columns instead of painted consoles leaves "a" and "b" plain.
test("columnClasses marks only the top row of the leftmost column maximized", () => {
  const C = WBColumns;
  const p = C.painted([["a", "b"], ["c"]], Infinity);
  assert.deepEqual(columnClasses(p, "a"), { column: true, maximized: true });
  assert.deepEqual(columnClasses(p, "b"), { column: true, maximized: false });
  assert.deepEqual(columnClasses(p, "c"), { column: true, maximized: false });
  const one = C.painted([["a", "b"]], Infinity);
  assert.deepEqual(columnClasses(one, "a"), { column: true, maximized: true });
  assert.deepEqual(columnClasses(one, "b"), { column: true, maximized: false });
});

// A live session no record claims is that console's own placeholder come back
// to life (its `sessionId` lost to a lost flush, or reissued by a restarted
// daemon), and it attaches THERE — not into a fresh cascaded record next to
// it. A shell record would otherwise `relaunch` a SECOND PTY beside the one
// still running. Only with no waiting record on the same repo, vendor, kind
// and worktree is it adopted.
test("reconcileDesk attaches an unclaimed session to its waiting record before adopting", () => {
  const agent = { repo: "owner/repo", agent: "claude", kind: "agent" };
  const out = reconcileDesk({
    layout: [
      { id: "wt", ...agent, checkout: "wt-a", sessionId: null },
      { id: "primary", ...agent, sessionId: null },
      { id: "shell", repo: "owner/repo", agent: "console", kind: "console", sessionId: 1 },
    ],
    sessions: [
      { id: 5, ...agent, checkout: null },
      { id: 6, repo: "owner/repo", agent: "console", kind: "console" },
      { id: 7, repo: "owner/other", agent: "codex", kind: "agent" },
    ],
  });
  assert.deepEqual(
    out.map(({ record, session, action }) => [record?.id ?? null, session?.id ?? null, action]),
    [
      ["wt", null, "placeholder"],
      ["primary", 5, "attach"],
      ["shell", 6, "attach"],
      [null, 7, "adopt"],
    ],
  );
});

// Two sessions no record claims: the second one's search for a waiting record
// passes the first one's `adopt` entry, which has no record.
test("reconcileDesk adopts every unclaimed session, not only the first", () => {
  const out = reconcileDesk({
    layout: [],
    sessions: [
      { id: 1, repo: "owner/repo", agent: "claude", kind: "agent" },
      { id: 2, repo: "owner/repo", agent: "codex", kind: "agent" },
    ],
  });
  assert.deepEqual(
    out.map(({ session, action }) => [session.id, action]),
    [
      [1, "adopt"],
      [2, "adopt"],
    ],
  );
});

// A placeholder on a page loaded before another device started its console:
// the fresh desk and session list decide whether Relaunch attaches instead of
// launching a second vendor CLI.
test("placeholderSession finds the session this record owns by now, and no other", () => {
  const agent = { repo: "owner/repo", agent: "claude", kind: "agent" };
  const rec = (id, sessionId) => ({ id, ...agent, sessionId });
  const live = (id) => ({ id, ...agent, checkout: null });
  const rows = [
    // [what, layout, sessions, held, want]
    ["another device relaunched the record", [rec("ph", 8)], [live(8)], [], 8],
    ["a stale id, one unclaimed session on the same tuple", [rec("ph", 3)], [live(8)], [], 8],
    [
      "the only match is already shown on this page",
      [rec("ph", 3)],
      [live(8)],
      [{ id: 8, repo: "owner/repo" }],
      null,
    ],
    [
      "the other console claims its own session by id",
      [rec("other", 8), rec("ph", 3)],
      [live(8)],
      [],
      null,
    ],
    ["no live session", [rec("ph", 3)], [], [], null],
    [
      "held ids on another repo do not hide this one",
      [rec("ph", 3)],
      [live(8)],
      [{ id: 8, repo: "owner/other" }],
      8,
    ],
  ];
  for (const [what, layout, sessions, held, want] of rows) {
    const got = placeholderSession({ layout, sessions, recordId: "ph", held });
    assert.equal(got?.id ?? null, want, what);
  }
});

// The shell records a name the popup reports only for a note that popup holds
// and that has no name yet (#475): the popup cannot write the desk itself.
test("a popup's note-name report is checked before the shell records it", () => {
  const entry = { members: [{ id: "w-1" }, { id: "n-1", kind: "note" }] };
  const record = { id: "n-1" };
  const msg = { noteId: "n-1", path: ".ralphy/notes/a.note" };
  assert.equal(noteNameOk(entry, record, msg), true);
  // Not a note this popup holds: a console id, an unknown id, no entry.
  assert.equal(noteNameOk(entry, { id: "w-1" }, { ...msg, noteId: "w-1" }), false);
  assert.equal(noteNameOk(entry, { id: "x" }, { ...msg, noteId: "x" }), false);
  assert.equal(noteNameOk(undefined, record, msg), false);
  // A name is given once.
  assert.equal(noteNameOk(entry, { ...record, path: "b.note" }, msg), false);
  assert.equal(noteNameOk(entry, null, msg), false);
  // The path must name a note, inside the repo.
  assert.equal(noteNameOk(entry, record, { ...msg, path: "a.md" }), false);
  assert.equal(noteNameOk(entry, record, { ...msg, path: "../a.note" }), false);
  assert.equal(noteNameOk(entry, record, { ...msg, path: 42 }), false);
  assert.equal(noteNameOk(entry, record, { ...msg, path: "/etc/a.note" }), false);
  assert.equal(noteNameOk(entry, record, { ...msg, path: "\\\\host\\a.note" }), false);
  assert.equal(noteNameOk(entry, record, { ...msg, path: "C:\\a.note" }), false);
  assert.equal(noteNameOk(entry, record, { ...msg, path: "c:a.note" }), false);
});

// A popup that is closing still talks on the channel, and a newer popup of
// the same fence may already be open (#476). Only the popup the entry holds
// is heard.
test("a lifecycle message is heard only from the popup the entry holds", () => {
  assert.equal(popupMatches(undefined, { pid: "a" }), false);
  // Restored after a reload: no `pid` yet, so any popup of the fence is heard.
  assert.equal(popupMatches({ pid: null }, { pid: "a" }), true);
  assert.equal(popupMatches({}, {}), true);
  assert.equal(popupMatches({ pid: "b" }, { pid: "b" }), true);
  assert.equal(popupMatches({ pid: "b" }, { pid: "a" }), false);
  assert.equal(popupMatches({ pid: "b" }, {}), false);
});

// --- one session per window record (ADR-0050 amendment 2026-10-04) -----------

// The session list says which record each session serves. That beats the
// `sessionId` tuple: two shells on one repo whose ids are stale would
// otherwise be handed out in layout order, each to the other's window.
test("reconcileDesk attaches each session to the record it names", () => {
  const shell = { repo: "owner/repo", agent: "console", kind: "console" };
  const out = reconcileDesk({
    layout: [
      { id: "w-a", ...shell, sessionId: 9 },
      { id: "w-b", ...shell, sessionId: null },
    ],
    sessions: [
      { id: 5, ...shell, record: "w-b" },
      { id: 6, ...shell, record: "w-a" },
    ],
  });
  assert.deepEqual(
    out.map(({ record, session, action }) => [record?.id ?? null, session?.id ?? null, action]),
    [
      ["w-a", 6, "attach"],
      ["w-b", 5, "attach"],
    ],
  );
});

// A session whose record this page has not read yet comes back under THAT id,
// so the page that launched it and the page that adopted it write one record.
test("reconcileDesk adopts a session under the record it names", () => {
  const shell = { repo: "owner/repo", agent: "console", kind: "console" };
  const out = reconcileDesk({
    layout: [],
    sessions: [
      { id: 5, ...shell, record: "w-new" },
      { id: 6, ...shell, record: "w-new" },
      { id: 7, ...shell },
    ],
  });
  assert.deepEqual(
    out.map(({ session, action, id }) => [session.id, action, id]),
    [
      [5, "adopt", "w-new"],
      // A second claim on the same id would be two windows under one record.
      [6, "adopt", null],
      [7, "adopt", null],
    ],
  );
});

// One cap rule for the desk and the cards: a list at its cap refuses, it does
// not evict, and a list under it grows at the end.
test("addNote appends below the cap and refuses at it, leaving the list alone", () => {
  const a = { id: "a", rect: {} };
  const b = { id: "b", rect: {} };
  const list = [a];
  assert.deepEqual(addNote(list, b, 2), [a, b]);
  assert.deepEqual(list, [a], "the input list is not changed");
  assert.equal(addNote([a, b], { id: "c", rect: {} }, 2), null);
  assert.equal(atCap([a], 1), true);
  assert.equal(atCap([], 1), false);
});

test("removeNote drops the card with the id and keeps the order of the rest", () => {
  const list = [{ id: "a" }, { id: "b" }, { id: "c" }];
  assert.deepEqual(removeNote(list, "b").map((n) => n.id), ["a", "c"]);
  assert.deepEqual(removeNote(list, "zz").map((n) => n.id), ["a", "b", "c"]);
  assert.equal(list.length, 3, "the input list is not changed");
});
