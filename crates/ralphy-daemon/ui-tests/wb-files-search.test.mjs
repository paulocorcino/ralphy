// Unit tests for the FILES search of assets/ui/wb-files.ts.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent } from "./harness.mjs";

// ---- the FILES search (ADR-0036 amendment 2026-09-15) --------------------------
// The component's half, driven with `WBDaemon.observe` stubbed and no tree
// mounted — the folds that decide what is sent and which reply is believed run
// without Wunderbaum.
const NO_HITS = { status: "ok", hits: [], truncated: false };
async function withSearchShell(run, reply = NO_HITS) {
  const { scope: state, window } = loadComponent("wbFiles", { magics: { $refs: {}, $nextTick: (f) => f() } });
  const calls = [];
  let answer = reply;
  window.WBDaemon = {
    observe: (verb, payload) => {
      calls.push({ verb, payload });
      return Promise.resolve(typeof answer === "function" ? answer() : answer);
    },
    // The real door's rule (wb-daemon.test.mjs pins it): the key only for a
    // real name, so a no-selection payload is the pre-#406 one.
    withCheckout: (payload, checkout) =>
      checkout ? { ...payload, checkout: String(checkout) } : { ...payload },
  };
  state.$store.projects.setOpen("owner/repo");
  return await run(state, calls, (a) => (answer = a));
}

test("fileSearchNow sends the mode's verb with the trimmed query", async () => {
  await withSearchShell(async (s, calls) => {
    s.fileSearch.open = true;
    s.fileSearch.query = "  plan ";
    await s.fileSearchNow();
    assert.deepEqual(calls.at(-1), { verb: "tree.find", payload: { repo: "owner/repo", query: "plan" } });
    await s.setFileSearchMode("content");
    assert.deepEqual(calls.at(-1), { verb: "tree.grep", payload: { repo: "owner/repo", query: "plan" } });
    // NEGATIVE CONTROL: under the floor nothing is sent and the note is clear.
    s.fileSearch.query = "p";
    await s.fileSearchNow();
    assert.equal(calls.length, 2);
    assert.equal(s.fileSearch.note, "");
  });
});

test("a reply that is not the newest is dropped, never painted", async () => {
  await withSearchShell(async (s, calls, answer) => {
    s.fileSearch.open = true;
    let release;
    answer(() => new Promise((r) => { release = r; }));
    s.fileSearch.query = "old";
    const slow = s.fileSearchNow();
    // A newer search lands first.
    answer({ status: "ok", hits: [{ path: "new.md" }], truncated: false });
    s.fileSearch.query = "new";
    await s.fileSearchNow();
    assert.deepEqual(s.fileSearch.hits, [{ path: "new.md" }]);
    // …then the slow one resolves with its stale hits: ignored.
    release({ status: "ok", hits: [{ path: "old.md" }], truncated: false });
    await slow;
    assert.deepEqual(s.fileSearch.hits, [{ path: "new.md" }]);
    assert.equal(calls.length, 2);
  });
});

test("the gutter says what the tree cannot: the cap, a miss, a refusal", async () => {
  await withSearchShell(async (s, calls, answer) => {
    s.fileSearch.open = true;
    s.fileSearch.query = "task";
    // [case, the daemon's reply, the note under the search box]
    const rows = [
      [
        "the cap",
        { status: "ok", hits: [{ path: "a" }], truncated: true },
        "First 200 matches. Narrow the search to see more.",
      ],
      ["a miss", { status: "ok", hits: [], truncated: false }, "No matches"],
      [
        "a refusal",
        { status: "error", reason: "unknown verb" },
        "Could not search: the daemon does not know that command.",
      ],
    ];
    for (const [name, reply, note] of rows) {
      answer(reply);
      await s.fileSearchNow();
      assert.equal(s.fileSearch.note, note, name);
    }
  });
});

test("only a live CONTENT search lends its term to a tab opened from the tree", async () => {
  await withSearchShell(async (s, calls, answer) => {
    s.fileSearch.open = true;
    s.fileSearch.query = "needle";
    answer({ status: "ok", hits: [{ path: "a.md", count: 2 }], truncated: false });
    await s.setFileSearchMode("content");
    assert.equal(s.fileSearchFindTerm(), "needle");
    // NEGATIVE CONTROLS: a name search, or a closed field, says nothing about
    // what is inside a file.
    await s.setFileSearchMode("name");
    assert.equal(s.fileSearchFindTerm(), null);
    await s.setFileSearchMode("content");
    s.fileSearch.open = false;
    assert.equal(s.fileSearchFindTerm(), null);
  });
});

test("closing the search forgets the query and the hits", async () => {
  await withSearchShell(async (s, calls, answer) => {
    s.fileSearch.open = true;
    s.fileSearch.query = "task";
    answer({ status: "ok", hits: [{ path: "a" }], truncated: false });
    await s.fileSearchNow();
    await s.closeFileSearch();
    assert.equal(s.fileSearch.open, false);
    assert.equal(s.fileSearch.query, "");
    assert.deepEqual(s.fileSearch.hits, []);
    assert.equal(s.fileSearch.note, "");
    assert.equal(s.fileSearch.expandedBefore, null);
  });
});
