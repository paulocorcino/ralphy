// Unit tests for assets/ui/wb-detached.ts, the page script of the torn-off
// file window, on stub `window` and `document` objects.
import { test } from "node:test";
import assert from "node:assert/strict";
import { isDetachOpen, wireDetached } from "../assets/ui/wb-detached.ts";

const ORIGIN = "https://daemon.test";

// One log of every listener added and every message posted, in order.
function page({ opener = true } = {}) {
  const log = [];
  const viewers = { innerHTML: "" };
  const window = {
    location: { origin: ORIGIN },
    opener: opener ? { postMessage: (data, target) => log.push(["post", data, target]) } : null,
    addEventListener: (type) => log.push(["window", type]),
    close() {},
  };
  const document = {
    title: "",
    addEventListener: (type) => log.push(["document", type]),
    getElementById: (id) => (id === "viewers" ? viewers : null),
    querySelector: () => null,
  };
  return { window, document, log, viewers };
}

test("the file window adds its listeners, then asks its own origin for the file", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const p = page();
  wireDetached(p.window, p.document);
  const ready = p.log.findIndex(([kind, data]) => kind === "post" && data.type === "wb-detach-ready");
  assert.ok(ready >= 0, JSON.stringify(p.log));
  assert.equal(p.log[ready][2], ORIGIN, "the handshake goes to the page's own origin");
  for (const [kind, type] of [
    ["document", "workbench:open-request"],
    ["document", "workbench:reattach-request"],
    ["window", "beforeunload"],
    ["window", "pagehide"],
    ["window", "message"],
  ]) {
    const at = p.log.findIndex(([k, ty]) => k === kind && ty === type);
    assert.ok(at >= 0 && at < ready, `${kind} ${type} is added before "ready": ${JSON.stringify(p.log)}`);
  }
  // The intent bus forwards to the same origin.
  p.window.WB.emit("save", { path: "a.rs" });
  assert.deepEqual(p.log.at(-1), ["post", { type: "wb-emit", action: "save", detail: { path: "a.rs" } }, ORIGIN]);
});

test("the file window with no opener says so and asks nobody", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const p = page({ opener: false });
  wireDetached(p.window, p.document);
  assert.match(p.viewers.innerHTML, /Nothing to show\. Open a file in the workbench, then detach it\./);
  assert.ok(!p.log.some(([kind, type]) => kind === "window" && type === "message"), JSON.stringify(p.log));
});

// The shell can be an older build than this page: its answer is read only in
// the shape this page knows.
test("the shell's answer opens a file only when it carries one", () => {
  const desc = { project: "o/r", label: "a.md", path: "a.md", ftype: "markdown", content: "x", checkout: null, bom: false };
  assert.equal(isDetachOpen({ type: "wb-detach-open", desc }), true);
  for (const bad of [
    { type: "wb-detach-open" },
    { type: "wb-detach-open", desc: { ...desc, path: null } },
    { type: "wb-fence-open", desc },
    null,
  ]) {
    assert.equal(isDetachOpen(bad), false, JSON.stringify(bad));
  }
});
