/* ---------------------------------------------------------------------------
   Settings → Telegram: the setup of the bot of the daemon this page talks to,
   in the steps of `ralphy telegram setup` (ADR-0085 D14, amendment of
   2026-11-07). The main daemon sends the messages of its peers, so the bot
   is set up there only.

   The daemon owns the bot. The page sends the token once and never reads it
   back: each reply holds the bot's name, the chat and the state. The section
   reads `GET /api/telegram/bot` each time it opens.

   `stateText` and `botText` are pure functions of a reply. `telegramSetup` is
   the Alpine component `wbTelegramSetup`; it reads nothing from `shell()`.

   `main.ts` registers it as `wbTelegramSetup` (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { apiFetch } from "./wb-api.ts";
import type { ApiFailed, ApiResponse } from "./wb-api.ts";

/** Why a daemon sends no message (`telegram_state::OffReason`). */
export type OffReason = "not-set-up" | "no-chat" | "unreadable" | "no-store";

/** The daemon's state (`telegram_state::TelegramState`). */
export type TelegramState =
  | { state: "off"; why?: OffReason }
  | { state: "sends" }
  | { state: "through-main"; main: string };

/** The reply of the setup routes (`api_telegram::bot::BotReply`): never the
 * token. `answered` is false when Telegram did not answer for the token. */
export type BotReply = TelegramState & {
  bot: string | null;
  chat: number | null;
  answered: boolean;
};

/** What the state means for the operator, in one sentence. */
export function stateText(reply: TelegramState) {
  switch (reply.state) {
    case "off":
      return offText(reply.why);
    case "sends":
      return "This daemon sends the messages, and the messages of its peers.";
    case "through-main":
      return "Another daemon sends the messages of this daemon.";
    default: {
      const unknown: never = reply;
      return unknown;
    }
  }
}

function offText(why: OffReason | undefined) {
  switch (why) {
    case undefined:
    case "not-set-up":
      return "No bot is set up. Paste the token that BotFather gave you, then save it.";
    case "no-chat":
      return "The bot has no chat yet. Send /start to the bot, then find your chat.";
    case "unreadable":
      return "The Telegram settings file of this daemon cannot be read.";
    case "no-store":
      return "The settings folder of this daemon cannot be found.";
    default: {
      const unknown: never = why;
      return unknown;
    }
  }
}

/** The bot line: its name, or why there is none. */
export function botText(reply: BotReply) {
  if (reply.bot) return `@${reply.bot}`;
  if (!reply.answered) return "Telegram did not answer for this token";
  return "No bot";
}

/** The routes of the setup: each answers a [BotReply], or `{"error"}`. */
type BotRoute =
  | "POST /api/telegram/bot"
  | "POST /api/telegram/chat"
  | "POST /api/telegram/test"
  | "DELETE /api/telegram/bot";

/** The text of a refusal: the daemon's `error`, or its status. */
async function refusal(res: ApiFailed<{ error: string }>) {
  const body = await res.json().catch(() => null);
  return body?.error || `the daemon answered ${res.status}`;
}

/** A setup action: its route and what the page says when it is done. */
type Action = {
  route: Exclude<BotRoute, "POST /api/telegram/bot">;
  done: string;
  failed: string;
};

const FIND_CHAT: Action = {
  route: "POST /api/telegram/chat",
  done: "The chat is saved.",
  failed: "Could not find the chat",
};
const SEND_TEST: Action = {
  route: "POST /api/telegram/test",
  done: "The test message was sent. Look in your Telegram chat.",
  failed: "Could not send the test message",
};
const REMOVE: Action = {
  route: "DELETE /api/telegram/bot",
  done: "The bot is removed.",
  failed: "Could not remove the bot",
};

// `loaded` is set only by a reply the daemon served, so a failed read never
// shows as "no bot" (ADR-0070 D3).
export function telegramSetup() {
  return component([], {
    loaded: false,
    busy: false,
    error: "",
    note: "",
    token: "",
    reply: null as BotReply | null,
    init() {
      void this.load();
    },
    // A token is kept: the chat, the test and the removal apply.
    hasBot() {
      const r = this.reply;
      if (!this.loaded || !r) return false;
      return r.state !== "off" || r.why === "no-chat" || r.why === "unreadable";
    },
    canTest() {
      return this.reply?.state === "sends";
    },
    fault() {
      return !!this.reply && (!this.reply.answered || this.reply.state === "off");
    },
    botLine() {
      return this.reply ? botText(this.reply) : "";
    },
    stateLine() {
      return this.reply ? stateText(this.reply) : "";
    },
    chatLine() {
      return this.reply?.chat ? String(this.reply.chat) : "No chat yet";
    },
    async load() {
      this.error = "";
      try {
        const res = await apiFetch("GET /api/telegram/bot");
        if (!res.ok) throw new Error(await refusal(res));
        this.reply = await res.json();
        this.loaded = true;
      } catch (e) {
        this.loaded = false;
        this.reply = null;
        this.error = `Could not read the Telegram bot: ${(e as Error).message}.`;
      }
    },
    // Send the token once; the field is emptied whatever the answer.
    async saveToken() {
      const token = this.token.trim();
      this.token = "";
      if (!token) return;
      await this.run(
        () =>
          apiFetch("POST /api/telegram/bot", {
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ token }),
          }),
        { done: "The token is saved.", failed: "Could not save the token" },
      );
    },
    findChat() {
      return this.act(FIND_CHAT);
    },
    sendTest() {
      return this.act(SEND_TEST);
    },
    async removeBot() {
      const ok = await this.$store.modals.askConfirm({
        title: "Remove the bot?",
        message: "This daemon and its peers stop sending Telegram messages.",
        confirmLabel: "Remove",
      });
      if (ok) await this.act(REMOVE);
    },
    act(action: Action) {
      return this.run(() => apiFetch(action.route), action);
    },
    // One request at a time; the reply replaces the shown state.
    async run(send: () => Promise<ApiResponse<BotRoute>>, says: { done: string; failed: string }) {
      this.busy = true;
      this.note = "";
      try {
        const res = await send();
        if (!res.ok) {
          this.note = `${says.failed}: ${await refusal(res)}.`;
          return;
        }
        this.reply = await res.json();
        this.loaded = true;
        this.note = says.done;
      } catch {
        this.note = `${says.failed}: the daemon did not answer.`;
      } finally {
        this.busy = false;
      }
    },
  });
}
