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

const STAGE = "Could not stage: the daemon gave no reason.";

test("failed() turns CLI output into Could not <act>: <cause>.", () => {
  const f = load().failed;
  assert.equal(
    f({ status: "error", message: "Error: cannot commit: nothing is staged — stage a file first" },
      "Could not commit: the daemon gave no reason."),
    "Could not commit: nothing is staged. Stage a file first.",
  );
  assert.equal(
    f({ status: "error", message: "Error: refusing to changes stage: a run holds this repo's lock (pid 9, since 10:00) — wait for it to finish or stop it" }, STAGE),
    "Could not stage: a run holds this repo's lock (pid 9, since 10:00). Wait for it to finish or stop it.",
  );
  assert.equal(
    f({ status: "error", message: "Error: branch 'x' already exists" }, "Could not create the branch: the daemon gave no reason."),
    "Could not create the branch: branch 'x' already exists.",
  );
});

test("failed() folds the Caused by lines and drops a backtrace and earlier output", () => {
  const out = [
    "progress line",
    "Error: invalid base 'nope'",
    "",
    "Caused by:",
    "    invalid ref 'nope'",
    "",
    "Stack backtrace:",
    "   0: anyhow::error",
  ].join("\n");
  assert.equal(
    load().failed({ status: "error", message: out }, "Could not create the worktree: the daemon gave no reason."),
    "Could not create the worktree: invalid base 'nope': invalid ref 'nope'.",
  );
});

test("failed() maps a code to its cause and keeps a sentence as it is", () => {
  const f = load().failed;
  assert.equal(f({ status: "error", reason: "exists" }, "Could not rename: the daemon gave no reason."),
    "Could not rename: a file with that name already exists.");
  assert.equal(f({ status: "error", message: "The peer did not answer." }, STAGE), "The peer did not answer.");
  assert.equal(f({ status: "error" }, STAGE), STAGE);
  assert.equal(f(null, STAGE), STAGE);
  assert.equal(f({ status: "error", message: "refused" }, "Could not stop the run."),
    "Could not stop the run: the daemon refused it.");
});

test("cause() gives the cause alone as a sentence", () => {
  const c = load().cause;
  assert.equal(
    c({ status: "error", message: "Error: worktree 'wt' has uncommitted changes: commit or discard them first" }, "x"),
    "Worktree 'wt' has uncommitted changes: commit or discard them first.",
  );
  assert.equal(c({ status: "error", reason: "not found" }, "x"), "It does not exist.");
  assert.equal(c({}, "The daemon gave no reason."), "The daemon gave no reason.");
});

test("message() stays raw, so a compare site still sees the code", () => {
  const w = load();
  assert.equal(w.message({ status: "error", message: "unknown repo" }, ""), "unknown repo");
  assert.equal(w.sentence("warning: settings.json not read"), "Warning: settings.json not read.");
});

// The lint does not read a keyed map (ui-copy MISSES), so every CAUSE value
// is checked here against the same rules file (ADR-0065 §6, §10).
const RULES = JSON.parse(
  readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../../../docs/ui-copy-rules.json"), "utf8"),
);

function hasTerm(text, term, wholeWord, caseSensitive) {
  const hay = caseSensitive ? text : text.toLowerCase();
  const needle = caseSensitive ? term : term.toLowerCase();
  for (let at = hay.indexOf(needle); at >= 0; at = hay.indexOf(needle, at + 1)) {
    const before = hay[at - 1] || " ";
    const after = hay[at + needle.length] || " ";
    if (!wholeWord || (!/[\p{L}\p{N}]/u.test(before) && !/[\p{L}\p{N}]/u.test(after))) return true;
  }
  return false;
}

test("every CAUSE value fits after `Could not <act>: ` and follows the rules file", () => {
  const causes = Object.entries(load().CAUSE);
  assert.ok(causes.length > 0);
  for (const [code, cause] of causes) {
    assert.match(cause, /^[a-z]/, `${code}: a cause starts lowercase`);
    assert.doesNotMatch(cause, /[.!?:;…]$/, `${code}: the sentence adds the final period`);
    const plain = cause.replace(/’/g, "'");
    for (const c of RULES.contractions.banned) {
      assert.ok(!plain.toLowerCase().includes(c), `${code}: contraction ${c}`);
    }
    for (const t of [...RULES.banned, ...RULES.plain]) {
      assert.ok(!hasTerm(plain, t.term, !!t.whole_word, !!t.case_sensitive), `${code}: ${t.term}`);
    }
    const words = cause.split(/\s+/).length + "Could not act:".split(" ").length;
    assert.ok(words <= RULES.sentence_max_words, `${code}: ${words} words`);
  }
});
