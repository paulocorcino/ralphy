/* ---------------------------------------------------------------------------
   The strips of a live console window: the peer-down strip and the parked
   strip. `createSpawn` (`wb-console-spawn.ts`) shows them from the terminal's
   events; each is built once in the window and then reworded.
--------------------------------------------------------------------------- */
import * as WBConsoleSession from "./wb-console-session.ts";
import { need } from "./wb-dom-need.ts";
import type { Group } from "./wb-fleet.ts";
import type { ConsoleWin } from "./wb-types.d.ts";

const { peerOfflineView, peerActionLabel } = WBConsoleSession;

// What the peer-down strip reads from the console. `peerDaemon` is the peer
// of the window's project.
export type PeerStripDeps = {
  document: Document;
  peerDaemon: string;
  wakePeerNow: () => ((daemon: string) => Promise<boolean>) | null;
  readFleetNow: () => (() => void) | null;
};

// A held window's strip: the fleet's sentence for the peer, its action, and
// the daemon's diagnosis under Details. Built once, then reworded on each
// fleet read.
export function showPeerDown(deps: PeerStripDeps, win: ConsoleWin, group: Group | null) {
  const { document, peerDaemon, wakePeerNow, readFleetNow } = deps;
  let strip: HTMLElement | null = win.querySelector<HTMLElement>(".session-peer-down");
  if (!strip) {
    strip = document.createElement("div");
    strip.className = "session-peer-down";
    const text = document.createElement("span");
    text.className = "session-peer-down-text";
    const detail = document.createElement("details");
    detail.className = "session-detail";
    const summary = document.createElement("summary");
    summary.textContent = "Details";
    detail.append(summary, document.createElement("p"));
    const btn = document.createElement("button");
    btn.className = "session-reconnect";
    btn.addEventListener("click", (e: MouseEvent) => {
      e.stopPropagation();
      if (btn.dataset["act"] === "wake") void wakePeerNow()?.(peerDaemon);
      else readFleetNow()?.();
    });
    strip.append(text, detail, btn);
    win.insertBefore(strip, win.querySelector(".session-body"));
  }
  const view = peerOfflineView(group, null, win._deskEnvironment);
  need(strip, ".session-peer-down-text").textContent = view.text;
  const detail = need(strip, ".session-detail");
  need(detail, "p").textContent = view.detail || "";
  detail.hidden = !view.detail;
  const btn = need(strip, ".session-reconnect");
  btn.dataset["act"] = view.action ?? "";
  const actionLabel = peerActionLabel(view.action);
  btn.hidden = !actionLabel;
  if (actionLabel) btn.textContent = actionLabel;
}

// The link-lost strip: built once while the socket is down.
export function showLinkLost(document: Document, win: ConsoleWin) {
  if (win.querySelector(".session-link-lost")) return;
  const strip = document.createElement("div");
  strip.className = "session-link-lost";
  strip.textContent = "Connection lost. Reconnecting…";
  win.insertBefore(strip, win.querySelector(".session-body"));
}

// The parked strip: this window watches a session another window drives.
// Built once; returns the strip.
export function showParked(document: Document, win: ConsoleWin): HTMLElement {
  const shown = win.querySelector<HTMLElement>(".session-parked");
  if (shown) return shown;
  const strip = document.createElement("div");
  strip.className = "session-parked";
  const text = document.createElement("span");
  // The titlebar already names the agent and the repo; repeating them here
  // wraps the strip on a phone.
  text.textContent = "Read-only. Another window has control.";
  const hint = document.createElement("span");
  hint.className = "session-parked-hint";
  const btn = document.createElement("button");
  btn.className = "session-reconnect";
  btn.dataset["act"] = "take-over";
  btn.textContent = "Take over";
  btn.addEventListener("click", (e: MouseEvent) => {
    e.stopPropagation();
    win._term?.takeOver();
  });
  strip.append(text, hint, btn);
  // The body is read at insert time: a dormant window gets a new body node
  // (`sleepWindow`), and the old one is no longer a child of `win`.
  win.insertBefore(strip, win.querySelector(".session-body"));
  return strip;
}
