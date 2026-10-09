// Unit tests for assets/ui/wb-events.ts — the senders of the `workbench:*`
// events. The target is a parameter, so a recorder stands in for the window
// and the document.
import { test } from "node:test";
import assert from "node:assert/strict";
import { sendDocument, sendWindow } from "../assets/ui/wb-events.ts";

// A target that keeps every event it is sent.
function recorder() {
  const sent = [];
  return { sent, dispatchEvent: (e) => sent.push(e) };
}

test("sendWindow sends one CustomEvent with the name and the detail on the target it is given", () => {
  const win = recorder();
  const other = recorder();
  sendWindow(win, "workbench:move-confirmed", { from: "a/b", to: "c/b" });
  assert.equal(other.sent.length, 0);
  assert.equal(win.sent.length, 1);
  assert.ok(win.sent[0] instanceof CustomEvent);
  assert.equal(win.sent[0].type, "workbench:move-confirmed");
  assert.deepEqual(win.sent[0].detail, { from: "a/b", to: "c/b" });
});

test("sendDocument sends one CustomEvent with the name and the detail on the target it is given", () => {
  const doc = recorder();
  sendDocument(doc, "workbench:split-ratio", { ratio: 0.4 });
  assert.equal(doc.sent.length, 1);
  assert.ok(doc.sent[0] instanceof CustomEvent);
  assert.equal(doc.sent[0].type, "workbench:split-ratio");
  assert.deepEqual(doc.sent[0].detail, { ratio: 0.4 });
});

test("an event with no detail carries null, as `new CustomEvent(name)` does", () => {
  const win = recorder();
  const doc = recorder();
  sendWindow(win, "workbench:menus-close");
  sendDocument(doc, "workbench:columns-stale");
  assert.deepEqual(
    [...win.sent, ...doc.sent].map((e) => [e.type, e.detail]),
    [
      ["workbench:menus-close", null],
      ["workbench:columns-stale", null],
    ],
  );
});
