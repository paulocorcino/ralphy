// Unit tests for the progress events that assets/ui/wb-hosts.ts folds into the
// Hosts dialog's state. They are here, not in wb-hosts.test.mjs, so a change of
// the dialog's markup and a change of this fold never edit one file.
import { test } from "node:test";
import assert from "node:assert/strict";
import * as WBHosts from "../assets/ui/wb-hosts.ts";

const fold = (event) => WBHosts.next(WBHosts.initial(), { type: "event", event });

test("a check event with no label shows its id as the label", () => {
  const { checks } = fold({ event: "check", id: "autostart", status: "pass" });
  assert.deepEqual(checks, [{ id: "autostart", label: "autostart", status: "pass", text: "", command: null }]);
  const named = fold({ event: "check", id: "autostart", label: "Start at boot", status: "pass" });
  assert.equal(named.checks[0].label, "Start at boot");
});
