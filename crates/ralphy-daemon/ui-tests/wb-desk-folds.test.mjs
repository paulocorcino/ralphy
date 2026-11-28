// Unit tests for assets/ui/wb-desk-folds.ts: the restore decision and the fence
// slots and readouts. Pure functions, with no console and no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBGeometry } from "../assets/ui/wb-geometry.ts";
import {
  fenceRepos,
  fenceSummaries,
  isUnknownCheckout,
  nextFenceSlot,
  reconcileDesk,
} from "../assets/ui/wb-desk-folds.ts";

const { fenceSpawnRect } = WBGeometry;

// The one reply that means a worktree is gone is the daemon's `unknown
// checkout` error. Any other reply, or none, lets the launch decide (#411).
test("isUnknownCheckout is true only for the error reply that says unknown checkout", () => {
  assert.equal(isUnknownCheckout({ status: "error", message: "unknown checkout" }), true);
  assert.equal(isUnknownCheckout({ status: "error", message: "no such path" }), false);
  assert.equal(isUnknownCheckout({ status: "ok", message: "unknown checkout" }), false);
  assert.equal(isUnknownCheckout(null), false);
  assert.equal(isUnknownCheckout(), false);
});

const foldSession = (repo, environment) => ({
  id: 1,
  repo,
  agent: "console",
  kind: "console",
  environment,
});

const deskRecord = (id, repo) => ({
  id,
  repo,
  agent: "console",
  kind: "console",
  sessionId: 1,
});

test("reconcileDesk keeps same-slug sessions distinct by composite repo ref", () => {
  const peerA = "01ARZ3NDEKTSV4RRFFQ69G5FAY/owner/shared";
  const peerB = "01ARZ3NDEKTSV4RRFFQ69G5FAZ/owner/shared";
  const out = reconcileDesk({
    layout: [deskRecord("a", peerA), deskRecord("b", peerB)],
    sessions: [foldSession(peerB, "WSL: B"), foldSession(peerA, "WSL: A")],
  });
  assert.deepEqual(
    out.map(({ record: saved, session: live, action }) => [
      saved.id,
      live.repo,
      live.environment,
      action,
    ]),
    [
      ["a", peerA, "WSL: A", "attach"],
      ["b", peerB, "WSL: B", "attach"],
    ],
  );
});

// The viewport and offset a new fence is placed against.
const FENCE_VIEW = { width: 1400, height: 900 };
const ORIGIN = { left: 0, top: 0 };

// Which slot a NEW fence takes. Indexing by `fences.length` reuses a slot after
// a removal, which is how an overlap ships before ADR-0051 §6 exists to enforce
// it away — every row below is that bug's oracle.
const SLOTS = [
  { name: "the first fence on an empty plane takes slot 0", rects: [], want: 0 },
  {
    name: "a second fence takes the next free slot",
    rects: [{ left: 40, top: 40, width: 720, height: 460 }],
    want: 1,
  },
  {
    // NEGATIVE CONTROL: `fences.length` answers 2 here and lands the new fence
    // exactly on the surviving slot-2 rect.
    name: "after the middle of three is removed, the FREED slot is reused, not the survivor's",
    rects: [
      { left: 40, top: 40, width: 720, height: 460 },
      { left: 40, top: 524, width: 720, height: 460 },
    ],
    want: 1,
  },
  {
    name: "a plane whose first four slots are full spills into the fifth",
    rects: [0, 1, 2, 3].map((i) => ({
      left: 40 + (i % 2) * 744,
      top: 40 + Math.floor(i / 2) * 484,
      width: 720,
      height: 460,
    })),
    want: 4,
  },
];

for (const row of SLOTS) {
  test(`nextFenceSlot: ${row.name}`, () => {
    assert.equal(nextFenceSlot(row.rects, ORIGIN, FENCE_VIEW), row.want);
  });
}

// The scan runs PAST the fence count. Bounding it at `taken.length` made a
// viewport covered by ONE big fence unbuildable: slots 0..1 both land inside it,
// the loop ran out, and the operator's New-fence click produced a refusal flash
// instead of a fence — with free plane sitting one row below. The row below is
// that bug's oracle: the old bound answered `1` (a slot INSIDE the blocker).
test("nextFenceSlot: a fence covering the whole viewport spills below it, not onto it", () => {
  const blocker = { left: 0, top: 0, width: 2000, height: 1200 };
  const slot = nextFenceSlot([blocker], ORIGIN, FENCE_VIEW);
  assert.ok(slot > 0, `expected a slot below the blocker, got ${slot}`);
  const born = fenceSpawnRect(ORIGIN, FENCE_VIEW, slot);
  assert.ok(born.top >= blocker.top + blocker.height, "the new fence still lands on the blocker");
});

// …and `-1` is the honest "nowhere", so the caller can refuse rather than nudge
// the fence into a gap nobody chose. A single rect covering the whole scanned
// band is the only way to reach it.
test("nextFenceSlot: a plane with no free slot in the scanned band answers -1", () => {
  const wall = { left: 0, top: 0, width: 100000, height: 100000 };
  assert.equal(nextFenceSlot([wall], ORIGIN, FENCE_VIEW), -1);
});

const overlaps = (a, b) =>
  a.left < b.left + b.width &&
  a.left + a.width > b.left &&
  a.top < b.top + b.height &&
  a.top + a.height > b.top;

test("nextFenceSlot: the slot it picks never overlaps an existing fence", () => {
  // Three fences with a HOLE at slot 1 — the shape a removal leaves behind.
  const rects = [0, 2, 3].map((i) => fenceSpawnRect(ORIGIN, FENCE_VIEW, i));
  const slot = nextFenceSlot(rects, ORIGIN, FENCE_VIEW);
  const born = fenceSpawnRect(ORIGIN, FENCE_VIEW, slot);
  for (const r of rects) {
    assert.ok(!overlaps(born, r), `slot ${slot} lands on ${JSON.stringify(r)}`);
  }
});

// The repos a fence's members belong to, for the fence's own chrome. Sorted
// because DOM order is not stable, deduped because two consoles on one repo
// read as one place.
const REPOS = [
  { name: "no members read as no repos", members: [], want: "" },
  {
    // NEGATIVE CONTROL for the dedupe: a plain join answers "alpha · alpha".
    name: "two members on one repo read as that one repo",
    members: [{ repo: "alpha" }, { repo: "alpha" }],
    want: "alpha",
  },
  {
    // NEGATIVE CONTROL for the sort: a DOM-ordered join answers "beta · alpha".
    name: "two repos read alphabetically, whatever order the members arrive in",
    members: [{ repo: "beta" }, { repo: "alpha" }],
    want: "alpha · beta",
  },
  {
    // NEGATIVE CONTROL: `"~"` is the desk's spelling of "no repo" — the storage
    // token must not leak, the same rule `list()` applies.
    name: "a member with no repo reads as home",
    members: [{ repo: "~" }],
    want: "home",
  },
  {
    name: "home sorts among the named repos, deduped like any other",
    members: [{ repo: "zeta" }, { repo: "~" }, { repo: "~" }],
    want: "home · zeta",
  },
];

for (const row of REPOS) {
  test(`fenceRepos: ${row.name}`, () => {
    assert.equal(fenceRepos(row.members), row.want);
  });
}

// ---- the fence list is the map (issue #343) ---------------------------------
// One fold feeds the fence's own chrome AND the toolbar list, so the two can
// never disagree. One entry per fence, IN ORDER, whether or not it holds
// anything.
const SUM_A = { left: 0, top: 0, width: 200, height: 200 };
const box = (x, y) => ({ left: x - 10, top: y - 10, width: 20, height: 20 });

const SUMMARIES = [
  {
    name: "two members read their count and their repos, sorted and home-renamed",
    fences: [{ id: "a", name: "alpha", rect: SUM_A }],
    windows: [
      { id: "w1", repo: "~", rect: box(50, 50) },
      { id: "w2", repo: "a", rect: box(150, 150) },
    ],
    want: [{ id: "a", name: "alpha", count: 2, onStage: 2, repos: "a · home", locked: false }],
  },
  {
    // Dedup is a REPOS rule, not a count rule: two consoles on one repo read as
    // one place but still as two consoles.
    name: "two members on one repo keep a count of two",
    fences: [{ id: "a", name: "alpha", rect: SUM_A }],
    windows: [
      { id: "w1", repo: "a", rect: box(50, 50) },
      { id: "w2", repo: "a", rect: box(150, 150) },
    ],
    want: [{ id: "a", name: "alpha", count: 2, onStage: 2, repos: "a", locked: false }],
  },
  {
    name: "a locked fence says so, and a fence without the key reads unlocked",
    fences: [
      { id: "a", name: "alpha", rect: SUM_A, locked: true },
      { id: "b", name: "beta", rect: { left: 400, top: 400, width: 100, height: 100 } },
    ],
    windows: [{ id: "w1", repo: "a", rect: box(150, 150) }],
    want: [
      { id: "a", name: "alpha", count: 1, onStage: 1, repos: "a", locked: true },
      { id: "b", name: "beta", count: 0, onStage: 0, repos: "", locked: false },
    ],
  },
  {
    // A console in its own tab keeps its fence (ADR-0037 §3d): the fence counts
    // it, and only the consoles on the stage can open as its columns.
    name: "a member in its own tab is counted, and is not on the stage",
    fences: [{ id: "a", name: "alpha", rect: SUM_A }],
    windows: [
      { id: "w1", repo: "a", rect: box(50, 50), inTab: true },
      { id: "w2", repo: "a", rect: box(150, 150) },
    ],
    want: [{ id: "a", name: "alpha", count: 2, onStage: 1, repos: "a", locked: false }],
  },
  {
    name: "an empty fence is still listed, at zero",
    fences: [{ id: "a", name: "alpha", rect: SUM_A }],
    windows: [],
    want: [{ id: "a", name: "alpha", count: 0, onStage: 0, repos: "", locked: false }],
  },
  {
    // NEGATIVE CONTROL: the centre sits exactly on `left + width`. A CLOSED
    // containment counts it as a member and reds this row.
    name: "a member whose centre is on the far edge belongs to no fence",
    fences: [{ id: "a", name: "alpha", rect: SUM_A }],
    windows: [{ id: "w1", repo: "a", rect: box(200, 100) }],
    want: [{ id: "a", name: "alpha", count: 0, onStage: 0, repos: "", locked: false }],
  },
  {
    // NEGATIVE CONTROL for "exactly one fence": hand-overlapped rects (which a
    // hand-edited desk.toml can carry) must not double-count the shared member.
    name: "two overlapping fences split a shared member into the FIRST one only",
    fences: [
      { id: "a", name: "alpha", rect: SUM_A },
      { id: "b", name: "beta", rect: { left: 100, top: 100, width: 200, height: 200 } },
    ],
    windows: [{ id: "w1", repo: "a", rect: box(150, 150) }],
    want: [
      { id: "a", name: "alpha", count: 1, onStage: 1, repos: "a", locked: false },
      { id: "b", name: "beta", count: 0, onStage: 0, repos: "", locked: false },
    ],
  },
];

for (const row of SUMMARIES) {
  test(`fenceSummaries: ${row.name}`, () => {
    const got = fenceSummaries(row.fences, row.windows);
    assert.deepEqual(got, row.want);
    // Asserted as a RELATION on every row, not only the overlap one: no window
    // may be counted twice, whatever the rects.
    assert.ok(
      got.reduce((n, s) => n + s.count, 0) <= row.windows.length,
      "a window is a member of at most ONE fence",
    );
  });
}
