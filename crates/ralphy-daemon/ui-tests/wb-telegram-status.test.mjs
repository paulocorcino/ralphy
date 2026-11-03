// Unit tests for assets/ui/wb-telegram-status.ts — Settings → Telegram.
// Runs the real source with no DOM, over the daemon's own reply
// (`fixtures/api-telegram.json`) and joined replies built from its shape.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { rows } from "../assets/ui/wb-telegram-status.ts";
import { bindingNames, loadComponent, partMarkup } from "./harness.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));

function fixture(name) {
  return JSON.parse(readFileSync(join(HERE, "fixtures", name + ".json"), "utf8"));
}

test("a daemon with no bot says why, and is not a fault", () => {
  assert.deepEqual(rows(fixture("api-telegram")), [
    { key: "this", name: "This daemon", text: "Sends nothing: no bot is set up", fault: false, noBot: true },
  ]);
});

test("each peer is a line: who sends for it, a bot that cannot send, a peer that did not answer", () => {
  const reply = {
    state: "sends",
    peers: [
      { daemon_id: "w", name: "wsl-ubuntu", environment: "WSL: Ubuntu", state: "through-main", main: "01M", main_name: "ralphy" },
      { daemon_id: "m", name: "", environment: "macOS 12", state: "off", why: "no-chat" },
      { daemon_id: "o", name: "old-peer", environment: "Ubuntu", state: "off" },
    ],
    unanswered: [{ daemon_id: "v", name: "vps-ralphy1", environment: "Ubuntu 20.04" }],
  };
  assert.deepEqual(
    rows(reply).map((r) => [r.name, r.text, r.fault]),
    [
      ["This daemon", "Sends the messages", false],
      ["wsl-ubuntu", "The main daemon ralphy sends its messages", false],
      ["macOS 12", "Sends nothing: the bot has no chat yet", true],
      ["old-peer", "Sends nothing", false],
      ["vps-ralphy1", "Did not answer", false],
    ],
  );
});

test("the section shows the setup command only when a daemon has no bot", async () => {
  const { scope } = loadComponent("wbTelegramStatus");
  const realFetch = globalThis.fetch;
  const replies = [fixture("api-telegram"), { state: "sends" }];
  globalThis.fetch = async () => ({ ok: true, status: 200, json: async () => replies.shift() });
  try {
    await scope.load();
    assert.equal(scope.loaded, true);
    assert.equal(scope.needsSetup(), true);
    await scope.load();
    assert.equal(scope.needsSetup(), false);
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.match(partMarkup("wb-settings-dialog.part.html"), /x-show="needsSetup\(\)"[^<]*<code>ralphy telegram setup<\/code>/);
});

// A failed read is never "sends nothing" (ADR-0070 D3), and no input asks
// for a token.
test("the Telegram section says why it could not read the state, and has no field", async () => {
  const { scope } = loadComponent("wbTelegramStatus");
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => ({ ok: false, status: 503, json: async () => ({}) });
  try {
    await scope.load();
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.equal(scope.loaded, false);
  assert.deepEqual(scope.rows, []);
  assert.equal(scope.error, "Could not read the Telegram state: the daemon answered 503.");
  const part = partMarkup("wb-settings-dialog.part.html");
  const section = part.slice(part.indexOf('x-data="wbTelegramStatus"'), part.indexOf("<template x-if=\"sec.id === 'devices'"));
  assert.doesNotMatch(section, /<input/);
});

test("every name the Telegram section's markup reads is its own", () => {
  const { scope } = loadComponent("wbTelegramStatus");
  const part = partMarkup("wb-settings-dialog.part.html");
  const section = part.slice(part.indexOf('x-data="wbTelegramStatus"'), part.indexOf("<template x-if=\"sec.id === 'devices'"));
  let read = 0;
  for (const [, attr, value] of section.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)) {
    if (attr === "x-data" || attr === "x-if" || attr === ":key") continue;
    const expr = attr === "x-for" ? value.split(" in ").slice(1).join(" in ") : value;
    for (const name of bindingNames(expr)) {
      if (name.startsWith("$") || name === "row") continue;
      assert.ok(name in scope, `${attr}="${value}" reads ${name}`);
      read++;
    }
  }
  assert.ok(read >= 5, `the markup was read: ${read} names`);
});
