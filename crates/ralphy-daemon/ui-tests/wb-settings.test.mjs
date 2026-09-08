// The settings surface: the schema in `wb-settings.js` and the one fold in
// `app.js` that persists it (`saveSetting`).
//
// This file exists because `claude.console_name` is the workbench's FIRST
// project-scoped toggle. Every other toggle in the schema is `scope: "client"`
// and returns before the daemon call; every other project-scoped boolean
// (`verify.require_verify_gate`) is a `tristate` that sends the STRING "true".
// So the path "checkbox → real boolean → String(value) → config.set" is new,
// and its interesting case is the one that turns a setting OFF: `false` is
// falsy, and one `==` away from being read as "no value" and routed to
// `config.unset`. Un-set and set-to-false happen to converge for this key, but
// they are not the same verb, and nothing else covers the difference.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadShell } from "./harness.mjs";

// `saveSetting` ends by announcing on the `WB` bus, which app.js reads as a BARE
// global — in a browser `window.WB` IS a global, in this harness `window` is a
// parameter object and the reference does not resolve. Promote the REAL
// namespace the siblings built (never a fake one) and restore after, the way
// the harness itself borrows and restores `BroadcastChannel`.
// `async`, and the body is AWAITED inside the try: a synchronous `finally`
// around a returned promise restores the global before the fold has run, which
// is exactly the "WB is not defined" this helper exists to prevent.
async function withShell(run) {
  const { state, window } = loadShell();
  const calls = [];
  window.WBDaemon = {
    observe: async (verb, payload) => {
      calls.push({ verb, payload });
      return { status: "ok" };
    },
  };
  // A repo must be open or the fold skips the daemon entirely.
  state.openSlug = "owner/repo";
  const real = globalThis.WB;
  globalThis.WB = window.WB;
  try {
    return await run(state, calls);
  } finally {
    if (real === undefined) delete globalThis.WB;
    else globalThis.WB = real;
  }
}

test("turning a toggle off SETS false — it does not unset the key", async () => {
  await withShell(async (s, calls) => {
    await s.saveSetting("claude.console_name", false);
    assert.deepEqual(calls.at(-1), {
      verb: "config.set",
      payload: { repo: "owner/repo", key: "claude.console_name", value: "false" },
    });
    // The optimistic local edit is what the checkbox re-renders off, and
    // `:checked="settings[key] === true"` needs a real boolean to read false.
    assert.equal(s.settings["claude.console_name"], false);

    await s.saveSetting("claude.console_name", true);
    assert.deepEqual(calls.at(-1), {
      verb: "config.set",
      payload: { repo: "owner/repo", key: "claude.console_name", value: "true" },
    });
    assert.equal(s.settings["claude.console_name"], true);
  });
});

// The control that gives the test above its meaning: emptying a TEXT field is
// what `config.unset` is for. Were the falsy check widened to catch `false`,
// this case would keep passing and the boolean one would silently start
// clearing keys instead of writing them.
test("an emptied value still unsets, and that is a different verb", async () => {
  await withShell(async (s, calls) => {
    for (const empty of ["", "unset", null]) {
      await s.saveSetting("claude.plan_model", empty);
      assert.equal(calls.at(-1).verb, "config.unset", `${JSON.stringify(empty)} must unset`);
    }
  });
});

test("a client-scoped key never reaches the daemon", async () => {
  await withShell(async (s, calls) => {
    await s.saveSetting("consoles.relaunch_on_load", true);
    assert.equal(calls.length, 0, "a per-browser choice must not land in a repo's settings.json");
  });
});

// `openSettings()` merges `config.get` over the schema defaults with
// `k in this.settings` — a key the schema never seeded is dropped on the floor,
// and the control then renders its default forever while the repo says
// otherwise. Seeding is what makes the read-back work, so it is asserted for
// every key rather than for the new one.
test("every schema key is seeded, so config.get can merge over it", () => {
  const { state, window } = loadShell();
  for (const section of window.WB_SETTINGS) {
    for (const item of section.items) {
      assert.ok(
        item.key in state.settings,
        `${item.key} is rendered but never seeded — config.get would not reach it`,
      );
    }
  }
  assert.equal(state.settings["claude.console_name"], false, "the name is off until asked for");
});
