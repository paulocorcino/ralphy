// Unit tests for assets/ui/wb-devices.js — Settings → Devices (ADR-0074 D9).
// Runs the real source with no DOM, over the daemon's own replies
// (`fixtures/api-audit-devices.json`, `fixtures/api-audit-events.json`).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const SRC = readFileSync(join(HERE, "../assets/ui/wb-devices.js"), "utf8");

function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBDevices;
}

function fixture(name) {
  return JSON.parse(readFileSync(join(HERE, "fixtures", name + ".json"), "utf8"));
}

test("a device the daemon lists is one row: its system, browser and form, then where and when", () => {
  const D = load();
  const rows = D.rows(fixture("api-audit-devices"), (at) => `t(${at})`);
  assert.deepEqual(rows, [
    {
      device: "<device>",
      this: true,
      name: "Android 12 · Chrome 148 · phone",
      detail: "moto g(30) · Adreno (TM) 610 · 203.0.113.10 · last seen t(<last_seen>) · 3 events",
    },
  ]);
});

test("one event is one event", () => {
  const [row] = load().rows({ devices: [{ device: "d", events: 1, last_seen: "x" }] }, (at) => at);
  assert.equal(row.detail, "last seen x · 1 event");
});

test("a device that sent no facts still has a name", () => {
  assert.equal(load().deviceName({}), "A device that sent no facts yet");
});

test("each line of a device is one plain sentence, newest first", () => {
  const D = load();
  const rows = D.eventRows(fixture("api-audit-events"), (at) => at);
  assert.deepEqual(
    rows.map((r) => [r.text, r.ip]),
    [
      ["Command branch.switch in nope", "203.0.113.10"],
      ["Closed a console (refused: 404)", "203.0.113.10"],
      ["Reported its device facts", "203.0.113.10"],
    ],
  );
});

test("a command with no project names only its verb", () => {
  assert.equal(load().eventLine({ event: "command", verb: "host.trust" }), "Command host.trust");
});

test("a console launch names the agent and the project", () => {
  const { eventLine } = load();
  assert.equal(
    eventLine({ event: "console_launch", agent: "console", repo: "owner/repo" }),
    "Opened a console in owner/repo",
  );
  assert.equal(
    eventLine({ event: "console_launch", agent: "claude", repo: "owner/repo" }),
    "Opened claude in owner/repo",
  );
  assert.equal(eventLine({ event: "console_launch", agent: "console" }), "Opened a console");
});

test("a line names the project by the name the daemon gives", () => {
  const { eventLine } = load();
  assert.equal(
    eventLine({ event: "console_launch", agent: "console", repo: "path-0123", repo_name: "widget" }),
    "Opened a console in widget",
  );
  assert.equal(
    eventLine({ event: "command", verb: "sync.push", repo: "path-0123", repo_name: "widget" }),
    "Command sync.push in widget",
  );
});

test("a take-over is one plain sentence", () => {
  const { eventLine } = load();
  assert.equal(eventLine({ event: "console_takeover", session: 7 }), "Took over a console");
  assert.equal(
    eventLine({ event: "console_takeover", repo: "owner/repo" }),
    "Took over a console in owner/repo",
  );
});

test("a desk save is named in plain words", () => {
  const line = load().eventLine({ event: "action", method: "PUT", path: "/api/desk", status: 200 });
  assert.equal(line, "Saved the desk");
});

test("a request with no plain name shows its method, path and status", () => {
  const line = load().eventLine({ event: "action", method: "POST", path: "/api/fleet/nudge", status: 200 });
  assert.equal(line, "POST /api/fleet/nudge (200)");
});

test("repeated lines in a row are one row with a count", () => {
  const out = (at, ip = "a") => ({ event: "logout", at, ip });
  const rows = load().eventRows(
    { events: [out("4"), out("3"), out("2", "b"), { event: "login_ok", at: "1", ip: "a" }, out("0")] },
    (at) => at,
  );
  assert.deepEqual(rows, [
    { when: "4", text: "Signed out · 2 times", ip: "a" },
    { when: "2", text: "Signed out", ip: "b" },
    { when: "1", text: "Signed in", ip: "a" },
    { when: "0", text: "Signed out", ip: "a" },
  ]);
});

test("a changed profile names what changed in plain words", () => {
  const line = load().eventLine({ event: "device_profile_changed", changed: ["gpu", "time_zone"] });
  assert.equal(line, "Its device facts changed: graphics card, time zone");
});
