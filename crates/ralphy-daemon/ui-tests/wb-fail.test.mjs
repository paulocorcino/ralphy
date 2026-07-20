// Unit tests for assets/ui/wb-fail.js — runs the real source with no DOM.
// This file lives OUTSIDE assets/ui on purpose: lib.rs embeds all of
// assets/ui into the daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-fail.js"),
  "utf8",
);

function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBFail;
}

test("message() returns the verbatim message", () => {
  assert.equal(
    load().message({ status: "error", message: "gh not authed" }),
    "gh not authed",
  );
});

test("message() returns the verbatim reason", () => {
  assert.equal(load().message({ status: "error", reason: "binary" }), "binary");
});

test("message() falls back when neither message nor reason is present", () => {
  assert.equal(load().message({}, "fallback"), "fallback");
});

test("isError() true for an error reply", () => {
  assert.equal(load().isError({ status: "error" }), true);
});

test("isError() false for an ok reply", () => {
  assert.equal(load().isError({ status: "ok" }), false);
});

test("isError() false for a spawn exited frame", () => {
  assert.equal(load().isError({ status: "exited" }), false);
});

test("isError() false for a spawn output frame", () => {
  assert.equal(load().isError({ status: "output" }), false);
});

test("isError() true for a null reply", () => {
  assert.equal(load().isError(null), true);
});
