// Unit tests for assets/ui/wb-read-retry.ts — imports the real module with no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { ATTEMPTS, RETRY_DELAY_MS, Refused, patiently } from "../assets/ui/wb-read-retry.ts";

// A read that fails `fails` times with `err`, then answers "ok".
function flaky(fails, err) {
  const calls = { n: 0 };
  const read = async () => {
    calls.n += 1;
    if (calls.n <= fails) throw err;
    return "ok";
  };
  return { read, calls };
}

test("a read lost without a reply is asked again and the caller gets the answer", async () => {
  const { read, calls } = flaky(2, new Error("connection closed before a reply"));
  const waits = [];
  assert.equal(await patiently(read, async (ms) => void waits.push(ms)), "ok");
  assert.equal(calls.n, 3);
  assert.deepEqual(waits, [RETRY_DELAY_MS, RETRY_DELAY_MS]);
});

test("a reply that said no is not asked again", async () => {
  const { read, calls } = flaky(5, new Refused("not found"));
  await assert.rejects(patiently(read, async () => {}), /not found/);
  assert.equal(calls.n, 1);
});

test("a read that never lands gives up with its last error after ATTEMPTS tries", async () => {
  const { read, calls } = flaky(99, new Error("gone"));
  await assert.rejects(patiently(read, async () => {}), /gone/);
  assert.equal(calls.n, ATTEMPTS);
});
