// Unit tests for assets/ui/wb-kanban.ts — the graph order of the board's two
// Ready columns. The module is pure and touches nothing at load.
//
// The order is a port of `sort_queue_in_graph` (ralphy-core/src/blocked.rs).
// Both sides read one shared fixture: the Rust parity test
// `board_order_matches_shared_fixture` and the test below.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { WBKanban } from "../assets/ui/wb-kanban.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const FIXTURE = JSON.parse(
  readFileSync(join(HERE, "..", "..", "ralphy-core", "tests", "fixtures", "graph-parity.json"), "utf8"),
);

// An open issue with the edges it declares, in the shape `orderGraph` reads.
function issue(number, blockedBy = [], extra = {}) {
  return { number, state: "open", blockedBy, ...extra };
}

// The fixture is one case: `issues` is the open graph, `expectedOrder` the
// queue in core order. The queue is the issues `expectedOrder` names, as the
// Rust test derives it.
const CASES = [FIXTURE];

test(`orderGraph gives the order of the shared graph-parity fixture (${CASES.length} case)`, () => {
  for (const [index, c] of CASES.entries()) {
    const all = c.issues.map((i) => issue(i.number, i.blockedBy || []));
    const queue = all.filter((i) => c.expectedOrder.includes(i.number));
    const order = WBKanban.orderGraph(queue, all).map((i) => i.number);
    assert.deepEqual(order, c.expectedOrder, `case ${index + 1} of ${CASES.length}`);
  }
});

// A closed bundle that is not in the queue is stood in for by the queue
// members that name it as `## Parent`; one of them never waits for itself.
test("orderGraph: a closed bundle's children stand in for it as a blocker", () => {
  const all = [issue(1, [], { state: "closed" }), issue(5, [], { parent: 1 }), issue(6, [1], { parent: 1 }), issue(4, [1])];
  const queue = all.slice(1);
  // 4 waits for both children. 6 is a child itself and waits only for 5.
  assert.deepEqual(WBKanban.orderGraph(queue, all).map((i) => i.number), [5, 6, 4]);
});

// A cycle never places a member: the rest is appended in ascending order.
test("orderGraph: the members of a cycle come last, in ascending order", () => {
  const all = [issue(9, [7]), issue(7, [8]), issue(8, [7]), issue(5)];
  assert.deepEqual(WBKanban.orderGraph(all, all).map((i) => i.number), [5, 7, 8, 9]);
});
