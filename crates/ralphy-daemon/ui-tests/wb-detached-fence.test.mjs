// Unit tests for assets/ui/wb-detached-fence.ts, the page script of the
// torn-off fence window, on stub `window` and `document` objects.
import { test } from "node:test";
import assert from "node:assert/strict";
import { wireDetachedFence } from "../assets/ui/wb-detached-fence.ts";

const ORIGIN = "https://daemon.test";

// One log of every listener added and every message posted, in order.
function page({ opener = true } = {}) {
  const log = [];
  const stage = { innerHTML: "" };
  const window = {
    location: { origin: ORIGIN },
    opener: opener ? { postMessage: (data, target) => log.push(["post", data, target]) } : null,
    addEventListener: (type) => log.push(["window", type]),
  };
  const document = {
    title: "",
    addEventListener: (type) => log.push(["document", type]),
    getElementById: (id) => (id === "stage" ? stage : null),
  };
  return { window, document, log, stage };
}

// The opener answers "ready" in a later task, so every listener the wire
// function adds is in place before the answer can arrive. The one that
// takes the answer is added before "ready" is posted.
test("the fence window asks its own origin for the fence, with its listeners in place", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const p = page();
  wireDetachedFence(p.window, p.document);
  const ready = p.log.findIndex(([kind, data]) => kind === "post" && data.type === "wb-fence-ready");
  assert.ok(ready >= 0, JSON.stringify(p.log));
  assert.equal(p.log[ready][2], ORIGIN, "the handshake goes to the page's own origin");
  const message = p.log.findIndex(([kind, type]) => kind === "window" && type === "message");
  assert.ok(message >= 0 && message < ready, `the answer's listener is added first: ${JSON.stringify(p.log)}`);
  for (const [kind, type] of [
    ["window", "beforeunload"],
    ["window", "resize"],
    ["document", "workbench:column-restore"],
    ["document", "workbench:columns-stale"],
    ["document", "workbench:consoles-changed"],
  ]) {
    assert.ok(
      p.log.some(([k, ty]) => k === kind && ty === type),
      `${kind} ${type} is added: ${JSON.stringify(p.log)}`,
    );
  }
});

test("the fence window with no opener says so and asks nobody", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const p = page({ opener: false });
  wireDetachedFence(p.window, p.document);
  assert.match(p.stage.innerHTML, /Nothing to show\. Detach a fence from the workbench first\./);
  assert.deepEqual(p.log, []);
});
