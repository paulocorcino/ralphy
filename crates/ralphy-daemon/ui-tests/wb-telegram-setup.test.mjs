// Unit tests for assets/ui/wb-telegram-setup.ts — Settings → Telegram, the
// setup of the bot of the daemon the page talks to. Runs the real source
// with no DOM, over the daemon's own replies (`fixtures/api-telegram-*.json`).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { botText, stateText } from "../assets/ui/wb-telegram-setup.ts";
import { bindingNames, loadComponent, partMarkup } from "./harness.mjs";

const HERE = import.meta.dirname;

function fixture(name) {
  return JSON.parse(readFileSync(join(HERE, "fixtures", name + ".json"), "utf8"));
}

const section = () => {
  const part = partMarkup("wb-settings-dialog.part.html");
  return part.slice(part.indexOf('x-data="wbTelegramSetup"'), part.indexOf("<template x-if=\"sec.id === 'devices'"));
};

// The setup with a daemon that answers each request with the next reply, and
// keeps what it was sent.
async function withDaemon(replies, run) {
  const sent = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = (url, init = {}) => {
    // The harness's own shell reads the desk; only the Telegram routes count.
    if (!String(url).startsWith("/api/telegram")) return Promise.resolve({ ok: false, status: 404, json: () => Promise.resolve({}) });
    sent.push({ url: String(url), method: init.method, body: init.body });
    const [status, body] = replies.shift();
    return Promise.resolve({ ok: status < 300, status, json: () => Promise.resolve(body) });
  };
  try {
    const { scope } = loadComponent("wbTelegramSetup", {
      magics: { $store: { modals: { askConfirm: () => Promise.resolve(true) } } },
    });
    await run(scope, sent);
  } finally {
    globalThis.fetch = realFetch;
  }
}

test("a daemon with no bot asks for the token, and shows no chat or test", async () => {
  const reply = fixture("api-telegram-bot");
  assert.equal(botText(reply), "No bot");
  assert.equal(stateText(reply), "No bot is set up. Paste the token that BotFather gave you, then save it.");
  await withDaemon([[200, reply]], async (s) => {
    await s.load();
    assert.equal(s.loaded, true);
    assert.equal(s.hasBot(), false);
  });
});

test("the token is sent once, the field is emptied, and the page shows the bot's name", async () => {
  const saved = { ...fixture("api-telegram-bot"), why: "no-chat", bot: "ralphy_bot" };
  const sends = { state: "sends", bot: "ralphy_bot", chat: 4242, answered: true };
  await withDaemon([[200, fixture("api-telegram-bot")], [200, saved], [200, sends], [200, sends]], async (s, sent) => {
    await s.load();
    s.token = "  123:abc  ";
    await s.saveToken();
    assert.deepEqual(sent[1], {
      url: "/api/telegram/bot",
      method: "POST",
      body: JSON.stringify({ token: "123:abc" }),
    });
    assert.equal(s.token, "", "the field does not keep the token");
    assert.equal(s.botLine(), "@ralphy_bot");
    assert.equal(s.hasBot(), true);
    assert.equal(s.canTest(), false, "no chat yet: no test");
    assert.equal(s.stateLine(), "The bot has no chat yet. Send /start to the bot, then find your chat.");

    await s.findChat();
    assert.equal(sent[2].url, "/api/telegram/chat");
    assert.equal(s.chatLine(), "4242");
    assert.equal(s.canTest(), true);

    await s.sendTest();
    assert.equal(sent[3].url, "/api/telegram/test");
    assert.equal(s.note, "The test message was sent. Look in your Telegram chat.");
  });
});

test("a refusal says the daemon's reason, and keeps the state", async () => {
  const noChat = { ...fixture("api-telegram-bot"), why: "no-chat", bot: "ralphy_bot" };
  await withDaemon(
    [[200, noChat], [409, fixture("api-telegram-chat--no-bot")], [409, fixture("api-telegram-test--no-chat")]],
    async (s) => {
      await s.load();
      await s.findChat();
      assert.equal(s.note, "Could not find the chat: no bot is set up.");
      assert.equal(s.botLine(), "@ralphy_bot");
      await s.sendTest();
      assert.equal(s.note, "Could not send the test message: the bot has no chat yet.");
    },
  );
});

test("the removal asks first, then shows the daemon's state", async () => {
  const sends = { state: "sends", bot: "ralphy_bot", chat: 4242, answered: true };
  await withDaemon([[200, sends], [200, fixture("api-telegram-bot")]], async (s, sent) => {
    await s.load();
    await s.removeBot();
    assert.equal(sent[1].method, "DELETE");
    assert.equal(s.hasBot(), false);
    assert.equal(s.note, "The bot is removed.");
  });
});

// A failed read is never "no bot" (ADR-0070 D3).
test("the Telegram section says why it could not read the bot", async () => {
  await withDaemon([[503, {}]], async (s) => {
    await s.load();
    assert.equal(s.loaded, false);
    assert.equal(s.hasBot(), false);
    assert.equal(s.error, "Could not read the Telegram bot: the daemon answered 503.");
  });
  assert.match(section(), /type="password"[^>]*x-model="token"/u);
});

test("every name the Telegram section's markup reads is its own", () => {
  const { scope } = loadComponent("wbTelegramSetup");
  let read = 0;
  for (const [, attr, value] of section().matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/gu)) {
    if (attr === "x-data" || attr === "x-if" || attr === ":key") continue;
    for (const name of bindingNames(value)) {
      if (name.startsWith("$")) continue;
      assert.ok(name in scope, `${attr}="${value}" reads ${name}`);
      read++;
    }
  }
  assert.ok(read >= 12, `the markup was read: ${read} names`);
});
