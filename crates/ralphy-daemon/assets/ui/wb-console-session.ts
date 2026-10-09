/* ---------------------------------------------------------------------------
   The console's session folds — the wire codec, the reconnect, resume and
   dormancy rules, and what a box says and does about a peer, as pure
   functions of their arguments (ADR-0075 phase 5).

   Nothing here reads the DOM, the clock, a timer, `window` or a module-scope
   `let`: the same arguments give the same answer. `wb-console.ts` owns the
   sockets and the terminals, feeds them through these functions, and acts on
   the answer. `wb-daemon.ts` imports the resume rule and the two deadlines,
   so the shell and the console share one copy.

   Every function and the constants read outside the module are members of
   `window.WBConsole` under their own name.
   --------------------------------------------------------------------------- */
import { WBFleet } from "./wb-fleet.ts";
import type { Group } from "./wb-fleet.ts";
import { WBWindowState } from "./wb-window-state.ts";
import type { ConsoleWin } from "./wb-types.d.ts";

const { sessionIdOf } = WBWindowState;

// The workbench session codec, mirrored from src/protocol.rs. A terminal frame
// is [0x01][session u64 BE][raw bytes]; a resize rides a command frame [0x02]
// [JSON {id, verb:"resize", payload:{rows, cols}}]. One session per socket in
// this slice, so the session id is always 1.
export const TAG_TERMINAL = 0x01;
export const TAG_COMMAND = 0x02;
const SESSION_ID = 1;

export function encodeTerminal(str: string) {
  const data = new TextEncoder().encode(str);
  const out = new Uint8Array(1 + 8 + data.length);
  out[0] = TAG_TERMINAL;
  out[8] = SESSION_ID;
  out.set(data, 9);
  return out;
}

export function encodeCommand(verb: string, payload: { [field: string]: JsonValue | undefined }) {
  const body = new TextEncoder().encode(JSON.stringify({ id: 0, verb, payload }));
  const out = new Uint8Array(1 + body.length);
  out[0] = TAG_COMMAND;
  out.set(body, 1);
  return out;
}

export function encodeResize(rows: number, cols: number) {
  return encodeCommand("resize", { rows, cols });
}

// Why this page closes a console socket, sent as DATA just before the close:
// close metadata does not survive the trip (#334), and the peer relay
// forwards data frames unchanged. The daemon only logs it. One of
// `dormant`, `reconnect`, `window-closed`.
export type DetachReason = "dormant" | "reconnect" | "window-closed";
export function encodeDetach(reason: DetachReason) {
  return encodeCommand("detach", { reason });
}

// Failed re-opens before a socket is given up on, and how many a never-opened
// would-be writer spends before settling for watching. Module scope so
// `reconnectDecision` can be tabled without an `attachTerminal` instance.
const MAX_FAILED_REOPENS = 10;
const WATCH_AFTER = 3;

// How long a popup, which has no presence heartbeat to ask, counts as hidden
// before its sockets are taken as stale on resume (the rule is `wb-resume.ts`).
// An ordinary desktop tab switch stays below it and churns nothing.
export const RESUME_HIDDEN_MS = 60000;

// Whether a new window attaches at once or starts asleep. A window that
// reattaches to a known session starts asleep, and the observer's first
// report (it always sends one for a new target) wakes it if it is visible.
// Otherwise a restored desk replays the text of every console, then puts the
// ones off the viewport to sleep 15 s later: measured on three devices
// (2026-10-05), 42% of the console bytes went to those replays.
//   "dormant" — build the chrome only;
//   "attach"  — build the terminal and open the socket now.
// A launch has no id to wake to (`dormancyDecision` D5), and without an
// observer nothing would ever wake the window.
export function birthDecision({ id, observed }: { id: number | null | undefined; observed: boolean }) {
  return id != null && observed ? "dormant" : "attach";
}

// The dormancy rule, pure and tabled. The observer supplies `intersecting`,
// `applyDormancy` owns the grace period. Returns exactly one of
//   "sleep" — dispose this window's terminal and release its socket;
//   "wake"  — rebuild the terminal and reattach;
//   "hold"  — leave it exactly as it is.
// What the dormancy rule reads of one window.
type DormancyInput = {
  intersecting: boolean;
  covered: boolean;
  dormant: boolean;
  maximized: boolean;
  fullscreen: boolean;
  focused: boolean;
  hasTerminal: boolean;
  ended: boolean;
  sessionId: number | null | undefined;
};
export function dormancyDecision({
  intersecting,
  covered,
  dormant,
  maximized,
  fullscreen,
  focused,
  hasTerminal,
  ended,
  sessionId,
}: DormancyInput) {
  // Visible outranks everything. A window under a full bleed is inside the
  // viewport but nobody sees it: the observer reports geometry, not paint.
  if (intersecting && !covered) return dormant ? "wake" : "hold";
  if (dormant) return "hold";
  // D1: maximized/fullscreen fills the viewport; "outside" is a lie the
  // observer can tell in the frame between the class and the layout.
  if (maximized || fullscreen) return "hold";
  // D2: the focused window is being typed into — and every drag/resize begins
  // with a `pointerdown` that focuses, so this covers a window mid-drag too.
  if (focused) return "hold";
  // D3: a placeholder has no terminal to dispose.
  if (!hasTerminal) return "hold";
  // D4: an ENDED session has no daemon to replay it; sleeping would throw its
  // scrollback away for good.
  if (ended) return "hold";
  // D5: no id is nothing to reattach TO — waking would compose a LAUNCH url
  // and spawn a second vendor CLI (`reconnectDecision` R1).
  if (sessionId == null) return "hold";
  return "sleep";
}

// The last line a console prints when it gives up. A launch the daemon
// refused names the reason; the browser cannot read it anywhere else,
// because a refused launch never had a session to show.
export function endNotice(announced: string | null, message: string | null | undefined) {
  if (announced !== "refused") return "[session closed]";
  const why = typeof message === "string" ? message.trim() : "";
  return why ? `[could not start: ${why}]` : "[could not start]";
}

// The reconnect rule (#334), pure and tabled. Returns one of "reconnect" /
// "park-as-watcher" / "give-up".
//
// `announced` is the daemon's reason from a data frame BEFORE the close
// ("taken-over" / "child-exited" / "daemon-shutdown" / "refused"), else
// null. It is the only trustworthy signal of a deliberate end: the browser
// reports 1005/wasClean=false even for a served Close frame, so an
// unannounced dirty close is read as a flaky link.
// What the reconnect rule reads of one socket that closed.
type ReconnectInput = {
  everOpened: boolean;
  announced: string | null;
  idKnown: boolean;
  failedReopens: number;
};
export function reconnectDecision({
  everOpened,
  announced,
  idKnown,
  failedReopens,
}: ReconnectInput) {
  // R1: no id is nothing to reattach TO; reconnecting would spawn a SECOND
  // session.
  if (!idKnown) return "give-up";
  // R2/R3: the daemon said why. Taken over → park and watch; else gone.
  if (announced === "taken-over") return "park-as-watcher";
  if (announced != null) return "give-up";
  if (failedReopens > MAX_FAILED_REOPENS) return "give-up";
  // No rule reads the close code or `wasClean` (ADR-0051 §9). A proxy in
  // the path can close the page side cleanly, with 1000, for a socket the
  // daemon dropped, so a clean close is not a deliberate end. Every
  // deliberate end is announced (R2/R3).
  // R6: held the session before, so a drop is a flaky link.
  if (everOpened) return "reconnect";
  // R7/R8: never opened. Retry a bounded number of times (an F5 racing the
  // old bridge's teardown), then settle for watching.
  if (failedReopens < WATCH_AFTER) return "reconnect";
  return "park-as-watcher";
}

// Whether a reattach may open a socket at all (ADR-0070 D2, event 7). A
// socket to a peer the fleet calls down fails and retries, so the window
// holds instead, and the fleet read that calls the peer back releases it.
// `decision` is "connect" for a window's first socket, else
// `reconnectDecision`'s answer. Returns it unchanged, or "hold".
// No group (a local project, the popup, a fleet not read yet) changes
// nothing, and a launch (no id) is held by its placeholder (`peerHeld`).
// The answers a socket decision can have, and "hold" for a peer that is down.
export type PeerDecision = "connect" | "reconnect" | "park-as-watcher" | "give-up" | "hold";
type PeerGateInput = { decision: PeerDecision; group: Group | null | undefined; id: number | null | undefined };
export function peerGate({ decision, group, id }: PeerGateInput): PeerDecision {
  if (id == null || !group || decision === "give-up") return decision;
  return WBFleet.available(group) ? decision : "hold";
}

// The launch request a desk record relaunches with (#411). The daemon labels
// a repo-less console "~"; sent back as a slug it hits `unknown repo`, so it
// relaunches with no repo. An AGENT record asks for its vendor and worktree —
// `{ console: true }` is the shell request. The checkout rides ONLY on the
// agent request: the plain console stays on the primary (the `open` rule).
// What `relaunchRequest` reads of a desk record.
type RelaunchRecord = {
  repo?: string | null;
  kind?: string | null;
  agent?: string | null;
  checkout?: string | null;
};
export function relaunchRequest(record: RelaunchRecord) {
  const repo = record.repo === "~" ? undefined : record.repo;
  if (record.kind !== "agent") return { console: true, repo, command: consoleCommand(record.agent) };
  return { repo, agent: record.agent, checkout: record.checkout ?? null };
}

// The name a console box uses for the host of a peer project: the machine
// name of a tunnel peer, else the environment (`WSL: Ubuntu`). `fallback` is
// the environment the desk record kept, for a box drawn before the fleet list.
export function peerHost(group: Group | null | undefined, fallback: string | null | undefined) {
  return WBFleet.peerName(group) || fallback || "The other computer";
}

// What a console box says about a project whose peer cannot serve it, from
// that peer's fleet state. `group` is its fleet group (wb-fleet.ts), or null
// before the fleet list arrived; `refusal` is the daemon's sentence from a
// refused launch. `action` is one of
//   "wake"  — a nudge can answer this state: wake the peer, then relaunch;
//   "retry" — launch again;
//   "wait"  — the daemon is already opening the tunnel again;
//   null    — no click here fixes it (a token, a version, a descriptor).
// The daemon's diagnosis is the detail: it names the cause and the remedy.
// What a click on a peer box does: see `peerOfflineView`.
type PeerAction = "wake" | "retry" | "wait" | null;
export function peerOfflineView(
  group: Group | null | undefined,
  refusal: string | null | undefined,
  fallbackHost: string | null | undefined,
) {
  const host = peerHost(group, fallbackHost);
  const detail = (group && group.diagnosis) || (typeof refusal === "string" ? refusal.trim() : "");
  const view = (text: string, action: PeerAction) => ({ text, detail, action });
  const wakeOrRetry: PeerAction = WBFleet.wakeable(group) ? "wake" : "retry";
  switch (group && group.state) {
    case "asleep":
      return view(`${host} is asleep.`, wakeOrRetry);
    case "unreachable":
      return view(`Ralphy is not running on ${host}.`, wakeOrRetry);
    case "tunnel-closed":
      return view(`Reconnecting to ${host}…`, "wait");
    case "tunnel-silent":
      return view(`${host} does not answer.`, "retry");
    case "unauthorized":
    case "version-mismatch":
    case "refused":
    case "malformed":
      return view(`${host} cannot open this console.`, null);
    default:
      // Reachable or not known yet: the fleet has not seen what the launch saw.
      return view(`${host} did not start this console.`, "retry");
  }
}

// What a peer placeholder does on a fleet read. `available` and `offline`
// are both false while the peer's state is unknown. Returns one of
//   "relaunch" — the peer is back and this is a shell: open it again;
//   "offer"    — the peer is back: say so and leave the click to the
//                operator, because a resume never launches a vendor CLI;
//   "stay"     — keep the box as it is.
// Only a peer this box SAW offline counts as back. A refused launch on a
// peer the fleet still calls reachable would otherwise relaunch, be refused,
// and relaunch again.
type PeerReturnInput = {
  kind: string | null | undefined;
  canLaunch: boolean;
  available: boolean;
  wasOffline: boolean;
};
export function peerReturnDecision({ kind, canLaunch, available, wasOffline }: PeerReturnInput) {
  if (!available || !wasOffline) return "stay";
  return kind === "console" && canLaunch ? "relaunch" : "offer";
}

// What a box restored while the session list did not hear from its peer
// does once that peer is available, from `liveSessionFor`'s answer:
//   "attach"   — the console still runs there;
//   "relaunch" — the list heard from the peer and it does not run: a shell
//                opens again, as a restore would have opened it;
//   "offer"    — the same for an agent console: the click is the operator's;
//   "stay"     — still not known (`undefined`): ask again on the next read.
type HeldReturnInput = {
  kind: string | null | undefined;
  canLaunch: boolean;
  session: HostedSession | null | undefined;
};
export function heldReturnDecision({ kind, canLaunch, session }: HeldReturnInput) {
  if (session === undefined) return "stay";
  if (session) return "attach";
  return kind === "console" && canLaunch ? "relaunch" : "offer";
}

export function unheardRef(ref: string | null | undefined, unheard: ReadonlySet<string> | null | undefined) {
  const daemon = WBFleet.refDaemon(ref);
  return !!daemon && !!unheard?.has(daemon);
}

// The fleet group of `ref`'s peer when that peer cannot serve it, else null:
// a local ref, an unknown peer, and a reachable one all launch as usual.
export function peerHeld(ref: string | null | undefined, groups: ReadonlyMap<string, Group>) {
  const daemon = WBFleet.refDaemon(ref);
  const group = daemon ? groups.get(daemon) : null;
  return group && !WBFleet.available(group) ? group : null;
}

// A console-kind session's `agent` label is its startup command, or the
// literal `console` for the bare shell (the daemon labels it so on launch).
// So the label alone says how to launch that console again.
export function consoleCommand(label: string | null | undefined) {
  return label && label !== "console" ? label : undefined;
}

// The session the window holds, on a `/api/sessions` listing: the daemon's
// id AND the repo ref, because a restarted daemon hands out ids from 1
// again and a peer's id 1 is not this daemon's (the ref carries the peer).
export function sessionRowFor(win: ConsoleWin, sessions: readonly HostedSession[] | null | undefined) {
  const id = sessionIdOf(win);
  if (id == null) return null;
  const ref = win._deskRepo;
  return (
    (sessions || []).find(
      (s) => s && s.id === id && (ref === "~" ? !s.repo || s.repo === "~" : s.repo === ref),
    ) || null
  );
}
