/* ---------------------------------------------------------------------------
   Settings → Telegram: whether each daemon of the local fleet sends the
   Telegram messages, and why one does not (ADR-0085 D14, amendment of
   2026-11-03).

   The daemon owns the state. This section reads `GET /api/telegram` each time
   it opens; the page keeps no copy, and never sees a token. The bot is set up
   in a terminal on each machine, so the section shows the command, not a
   field.

   `rows` and `stateText` are pure functions of a reply. `telegramStatus` is
   the Alpine component `wbTelegramStatus`; it reads nothing from `shell()`.

   `main.ts` registers it as `wbTelegramStatus` (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { apiFetch } from "./wb-api.ts";

/** Why a daemon sends no message (`telegram_state::OffReason`). */
export type OffReason = "not-set-up" | "no-chat" | "unreadable" | "no-store";

/** One daemon's state (`telegram_state::TelegramState`). An older daemon
 * sends `off` with no reason. */
export type TelegramState =
  | { state: "off"; why?: OffReason }
  | { state: "sends" }
  | { state: "through-main"; main: string };

/** Who a peer is (`telegram_state::PeerName`). */
export type PeerName = { daemon_id: string; name: string; environment: string };

/** One peer's state in the joined reply. */
export type PeerTelegram = PeerName & TelegramState & { main_name?: string };

/** The reply of `GET /api/telegram` (`telegram_state::FleetTelegram`). */
export type FleetTelegram = TelegramState & {
  main_name?: string;
  peers?: PeerTelegram[];
  unanswered?: PeerName[];
};

/** One line of the section: a daemon, what it does, and whether the
 * operator has something to fix there. */
export type Row = { key: string; name: string; text: string; fault: boolean; noBot: boolean };

/** What an `off` daemon's reason says. */
function whyOff(why: OffReason) {
  switch (why) {
    case "not-set-up":
      return "no bot is set up";
    case "no-chat":
      return "the bot has no chat yet";
    case "unreadable":
      return "its Telegram settings file cannot be read";
    case "no-store":
      return "its settings folder cannot be found";
    default: {
      const unknown: never = why;
      return unknown;
    }
  }
}

/** What a daemon does with the messages, in one short sentence. */
export function stateText(state: TelegramState, mainName?: string) {
  switch (state.state) {
    case "off":
      return state.why ? `Sends nothing: ${whyOff(state.why)}` : "Sends nothing";
    case "sends":
      return "Sends the messages";
    case "through-main":
      return `The main daemon ${mainName || state.main} sends its messages`;
    default: {
      const unknown: never = state;
      return unknown;
    }
  }
}

/** A bot is there but cannot send, or the settings cannot be read. */
function isFault(state: TelegramState) {
  return state.state === "off" && !!state.why && state.why !== "not-set-up";
}

function row(key: string, name: string, state: TelegramState, mainName?: string): Row {
  return {
    key,
    name,
    text: stateText(state, mainName),
    fault: isFault(state),
    noBot: state.state === "off",
  };
}

/** The lines of the section: this daemon first, then each peer, then each
 * peer that did not answer. */
export function rows(reply: FleetTelegram): Row[] {
  const peerName = (p: PeerName) => p.name || p.environment;
  return [
    row("this", "This daemon", reply, reply.main_name),
    ...(reply.peers || []).map((p) => row(p.daemon_id, peerName(p), p, p.main_name)),
    ...(reply.unanswered || []).map((p) => ({
      key: p.daemon_id,
      name: peerName(p),
      text: "Did not answer",
      fault: false,
      noBot: false,
    })),
  ];
}

// `loaded` is set only by a state the daemon served, so a failed read never
// shows as "sends nothing" (ADR-0070 D3).
export function telegramStatus() {
  return component([], {
    loaded: false,
    error: "",
    rows: [] as Row[],
    init() {
      void this.load();
    },
    // Whether to show the command that sets a bot up.
    needsSetup() {
      return this.rows.some((r) => r.noBot);
    },
    async load() {
      this.error = "";
      try {
        const res = await apiFetch("GET /api/telegram");
        if (!res.ok) throw new Error(`the daemon answered ${res.status}`);
        this.rows = rows(await res.json());
        this.loaded = true;
      } catch (e) {
        this.loaded = false;
        this.rows = [];
        this.error = `Could not read the Telegram state: ${(e as Error).message}.`;
      }
    },
  });
}
