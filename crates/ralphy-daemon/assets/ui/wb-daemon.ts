/* ---------------------------------------------------------------------------
   ralphy workbench shell — the daemon call door (the run/triage/push verbs)

   The browser's `workbench:action` seam is verb-agnostic; this adapter maps the
   actions that reach the daemon (`ACTION_TO_VERB`) to a `Command {id, verb,
   payload}`. A verb that streams (a Spawn call) runs on a `spawn` channel of
   the page's one socket; a command with one reply (`observe`, `write`, `readImage`)
   goes over `POST /api/command`. The client
   never composes a command line — it sends closed-enum params and the daemon's
   verb registry (dispatch.rs) builds the argv. Raw `status:"output"` chunks feed
   the Runs panel live (ADR-0032 §5, ADR-0036).
--------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import { apiFetch, commandReply, socketFrame } from "./wb-api.ts";
import { WBProject } from "./wb-project.ts";
import { WBRun } from "./wb-runs.ts";
import { sendWindow } from "./wb-events.ts";
import {
  resumeDecision,
  pageIsStale,
  CONNECT_TIMEOUT_MS,
  HEARTBEAT_LOST_MS,
  RESUME_DEBOUNCE_MS,
} from "./wb-resume.ts";
import type { ResumeTrigger } from "./wb-resume.ts";
import { pageChannels } from "./wb-channels.ts";
import type { Channel, ChannelSocket, CloseReason, PageChannels } from "./wb-channels.ts";
import { createSubscriptions } from "./wb-daemon-subscriptions.ts";
import type { SocketUser } from "./wb-daemon-subscriptions.ts";
import type { ConsoleChannel, ConsoleHandlers } from "./wb-console-terminal-types.d.ts";
import type { Messages } from "./wb-messages.ts";
import type { PresenceReport } from "./wb-presence.ts";
import type { SessionPayload } from "./wb-session-route.ts";

/** What the presence socket's owner hears besides the heartbeat. */
type PresenceHandlers = {
  onPush?: (verb: string, payload: PushPayload) => void;
  onOpen?: (reopened: boolean) => void;
};

/** What the entry module hands the daemon door. */
export type DaemonDeps = {
  /** The page's door for operator messages: a verb that could not start is flashed. */
  messages: Messages;
  /** The wait between two resumes of the page socket; `RESUME_DEBOUNCE_MS`
   *  when absent. Only a test sets it. */
  resumeDebounceMs?: number;
};

// The channel layer's view of a WebSocket. `WebSocket.send` takes only bytes
// on an `ArrayBuffer`; every frame the layer makes is on one, and any other
// frame is copied onto one.
function channelSocket(sock: WebSocket): ChannelSocket {
  return {
    get readyState() {
      return sock.readyState;
    },
    send: (data: Uint8Array) => sock.send(onArrayBuffer(data) ? data : data.slice()),
  };
}

function onArrayBuffer(bytes: Uint8Array): bytes is Uint8Array<ArrayBuffer> {
  return bytes.buffer instanceof ArrayBuffer;
}

export function createDaemon(
  window: Window,
  document: Document,
  location: Location,
  deps: DaemonDeps,
) {
  const { messages } = deps;
  const resumeDebounceMs = deps.resumeDebounceMs ?? RESUME_DEBOUNCE_MS;
  // The tagged-frame codec, mirrored from src/protocol.rs (see wb-console.ts).
  const TAG_TERMINAL = 0x01;
  const TAG_COMMAND = 0x02;
  const TAG_PRESENCE = 0x03;

  // The WebSocket origin, scheme-matched to the page: `wss://` when the workbench
  // is served over TLS (a dev-tunnel/reverse-proxy reaching a loopback daemon),
  // else `ws://` for a plain-http localhost bind. Hardcoding `ws://` would trip
  // the browser's mixed-content block on an https origin and kill every socket.
  const WS_ORIGIN = (location.protocol === "https:" ? "wss://" : "ws://") + location.host;

  // Which `workbench:action`s reach the daemon, and as which verb. The generic
  // `command` action carries its verb in the event detail (triage/push).
  const ACTION_TO_VERB: Partial<Record<WorkbenchActionName, string>> = { "run-start": "run" };

  let nextId = 1;

  // How long a command waits for its one reply: `dispatch::REPLY_DEADLINE`
  // (60 s) of the daemon, plus the 5 s margin of `relay_to_peer` for a peer.
  const COMMAND_DEADLINE_MS = 70000;

  // The wait before a lost page socket is opened again.
  const RECONNECT_MS = 3000;

  // Fail a handshake that has not opened by the deadline. `close()` on a
  // CONNECTING socket fires `close`, so the caller's ordinary retry takes over.
  function armHandshakeDeadline(ws: WebSocket) {
    setTimeout(() => {
      if (ws.readyState !== 0) return;
      try {
        ws.close();
      } catch {}
    }, CONNECT_TIMEOUT_MS);
  }

  // Retire a socket so its pending events cannot reach us. `onmessage` matters as
  // much as `onclose`: a frame still queued on the outgoing socket would land
  // after the replacement is wired and be read as the NEW connection's news.
  function detachSocket(ws: WebSocket | null) {
    if (!ws) return;
    ws.onclose = null;
    ws.onmessage = null;
    ws.onopen = null;
    ws.onerror = null;
    try {
      if (ws.readyState <= 1) ws.close();
    } catch {}
  }

  function encodeCommand({
    id,
    verb,
    payload,
  }: {
    id: number;
    verb: string;
    payload: CommandPayload;
  }) {
    const body = new TextEncoder().encode(JSON.stringify({ id, verb, payload }));
    const out = new Uint8Array(1 + body.length);
    out[0] = TAG_COMMAND;
    out.set(body, 1);
    return out;
  }

  // The optional `checkout` argument of a repo-scoped verb (#406, ADR-0063
  // §2): a worktree NAME the daemon resolves under the repo's own root. Added
  // ONLY for a real name — with no selection the payload is byte-identical to
  // the pre-#406 one, so an older daemon never sees a key it does not know.
  function withCheckout(
    payload: CommandPayload,
    checkout: string | null | undefined,
  ): CommandPayload {
    const out = { ...payload };
    if (checkout) out.checkout = String(checkout);
    return out;
  }

  // Listeners for the ONE reply that means a selected worktree is gone:
  // `observe` runs `WBProject.checkoutAfter` on every reply to a payload that
  // carried `checkout`, and an `unknown checkout` fans out here as
  // `(repo, name)`. This is the single path every Observe AND Write verb takes,
  // which is what "reset from any verb" asks for. A listener that throws must
  // never reject the read it rode on.
  const unknownCheckout: ((repo: string | null | undefined, name: string) => void)[] = [];
  function onUnknownCheckout(fn: (repo: string | null | undefined, name: string) => void) {
    unknownCheckout.push(fn);
  }
  function noteUnknownCheckout(payload: CommandPayload, reply: DaemonReply) {
    if (!payload || !payload.checkout) return;
    if (WBProject?.checkoutAfter?.(payload.checkout, reply) !== null) return;
    for (const fn of unknownCheckout) {
      try {
        fn(payload.repo, payload.checkout);
      } catch {}
    }
  }

  // Fire a command with ONE reply (`tree.list`/`file.read`, the Write byte-ops)
  // and resolve with that reply. It goes over `POST /api/command`, which shares
  // the HTTP/2 connection the page already holds, so it never waits in the
  // browser's queue of new sockets. A stall reports the verb, not the route.
  function observe<V extends string>(verb: V, payload: CommandPayload) {
    const id = nextId++;
    return apiFetch("POST /api/command", {
      body: JSON.stringify({ id, verb, payload }),
      headers: { "content-type": "application/json" },
      deadlineMs: COMMAND_DEADLINE_MS,
      what: verb,
    })
      .then((res) => {
        if (!res.ok) throw new Error(`the command answered HTTP ${res.status}`);
        return commandReply<V>(res);
      })
      .then((reply) => {
        // AFTER the resolve, on a later task: a listener that remounts the
        // tree must not run before the read that failed has settled its whole
        // `.then`/`.catch` chain, or the stale mount's catch paints its error
        // onto the fresh one (a microtask would still race that chain).
        setTimeout(() => noteUnknownCheckout(payload, reply), 0);
        return reply;
      });
  }

  // Read an image (`file.image`, ADR-0049) and resolve with a `data:` URL ready
  // to hang off an `<img src>`, or `null` when the daemon refused. The MEDIA TYPE
  // comes from the daemon, which verified it against the file's magic bytes — it
  // is never guessed from the extension here, and the bytes never get a URL of
  // their own on this origin (§2). A refusal reason is reported through
  // `onRefused` rather than thrown, because every caller wants to keep going.
  // `checkout` names the worktree the bytes come from (#406), or nothing.
  function readImage(
    repo: string,
    path: string,
    onRefused: ((reason: string) => void) | undefined,
    checkout: string | null | undefined,
  ) {
    return observe("file.image", withCheckout({ repo, path }, checkout)).then((reply) => {
      if (WBFail.isError(reply) || !reply.base64 || !reply.mediaType) {
        onRefused?.(WBFail.message(reply, "refused"));
        return null;
      }
      return `data:${reply.mediaType};base64,${reply.base64}`;
    });
  }

  // Fire a Write byte-op (`file.write`/`file.create`/`file.rename`/`file.copy`/
  // `file.delete`, #197/#362) and resolve with the single reply payload. Same one-socket-one-reply
  // shape as `observe` — the daemon answers ONE frame on the id and returns (no
  // spawn/stream); a confinement refusal comes back as `{status:"error",reason}`.
  function write<V extends string>(verb: V, payload: CommandPayload) {
    return observe(verb, payload);
  }

  // The page's presence reports (wb-presence.ts) ride the page socket:
  // `reportPresence` sends one when it is open, and each open runs the
  // `onPresenceOpen` hooks, so a new socket hears the page at once.
  const presenceOpens: (() => void)[] = [];

  // The page's ONE socket on `/ws` (ADR-0082): presence and the daemon's
  // pushes are channel 0, and each subscription has a channel of its own
  // (wb-channels.ts). The first user opens it, and it closes when the last
  // one leaves. Reconnects on `close` ONLY — the spec guarantees `error` is
  // always followed by `close`, so scheduling on both would double the backoff
  // into a storm. One fixed 3s timer per drop, no exponential backoff.
  // A new socket starts with no channel on the daemon, so every open runs
  // each user's `onOpen` again, with `reopened` true after its first open: a
  // caller's catch-up read rides the reconnect with no second code path.
  function persistentSocket() {
    const users = new Map<SocketUser, { opened: boolean }>();
    let ws: WebSocket | null = null;
    // The channel layer of the open socket; `null` while it is not open.
    let channels: PageChannels | null = null;
    let timer: number | null = null;
    let lastResumeAt = 0;
    let connectingSince = 0;
    let handshakeMs: number | null = null;
    // The page's read of its session, given before the first socket. The
    // daemon mints the device cookie only on that read, and reads the device
    // once, at the upgrade (#834): the first socket waits for the read, or
    // for the handshake deadline. `null` once the wait began.
    let sessionRead: Promise<unknown> | null = null;
    let waitingForSession = false;
    const greet = (user: SocketUser, state: { opened: boolean }, layer: PageChannels) => {
      const reopened = state.opened;
      state.opened = true;
      user.onOpen(layer, reopened);
    };
    const bare = (bytes: Uint8Array) => {
      for (const user of [...users.keys()]) user.onBare?.(bytes);
    };
    // The channels of a socket that ended are over; the next open opens them again.
    const lose = () => {
      const layer = channels;
      channels = null;
      layer?.socketLost();
    };
    const waitForSession = (read: Promise<unknown>) => {
      sessionRead = null;
      waitingForSession = true;
      const go = () => {
        if (!waitingForSession) return;
        waitingForSession = false;
        if (users.size && !ws) connect();
      };
      void read.then(go, go);
      setTimeout(go, CONNECT_TIMEOUT_MS);
    };
    const connect = () => {
      if (sessionRead) {
        waitForSession(sessionRead);
        return;
      }
      const sock = new WebSocket(WS_ORIGIN + "/ws");
      ws = sock;
      connectingSince = Date.now();
      armHandshakeDeadline(sock);
      sock.binaryType = "arraybuffer";
      sock.onopen = () => {
        handshakeMs = Date.now() - connectingSince;
        const layer = pageChannels(channelSocket(sock), bare);
        channels = layer;
        for (const [user, state] of [...users]) greet(user, state, layer);
        for (const fn of presenceOpens) fn();
      };
      sock.onmessage = (ev: MessageEvent<ArrayBuffer>) =>
        channels?.receive(new Uint8Array(ev.data));
      sock.onclose = () => {
        lose();
        // The last user left as its channel ended: no user, no socket. A
        // timer here would open a second socket beside the one the next
        // user opens.
        if (ws !== sock) return;
        timer = setTimeout(connect, RECONNECT_MS);
      };
    };
    const leave = (user: SocketUser) => {
      if (!users.delete(user) || users.size) return;
      clearTimeout(timer);
      timer = null;
      lose();
      detachSocket(ws);
      ws = null;
    };
    return {
      // Add a user; it returns the user's leave.
      join: (user: SocketUser) => {
        const state = { opened: false };
        users.set(user, state);
        if (ws) {
          if (channels) greet(user, state, channels);
        } else if (!waitingForSession) {
          connect();
        }
        return () => leave(user);
      },
      // Hold the first socket until `read`, the page's read of its session,
      // settles. A page that already opened a socket ignores it.
      afterSessionRead: (read: Promise<unknown>) => {
        if (connectingSince === 0 && !waitingForSession) sessionRead = read;
      },
      // Send a bare frame on channel 0 if the socket is open; `false` means
      // the frame was not sent, and the caller's open hook re-sends its state.
      sendIfOpen: (frame: Uint8Array<ArrayBuffer>) => {
        if (!channels || !ws) return false;
        ws.send(frame);
        return true;
      },
      resume: (stale: boolean) => {
        if (!ws) return false;
        const now = Date.now();
        if (now - lastResumeAt < resumeDebounceMs) return false;
        const connectingMs = now - connectingSince;
        if (resumeDecision({ readyState: ws.readyState, stale, connectingMs }) === "none") {
          return false;
        }
        lastResumeAt = now;
        clearTimeout(timer);
        timer = null;
        // The daemon reads the frames sent before the close (#834).
        for (const user of [...users.keys()]) user.onResume?.();
        const old = ws;
        detachSocket(old);
        lose();
        // A console that reopens as its channel ends joins inside `lose`. When
        // it was the only user, its leave cleared `ws` and its join opened the
        // next socket; a second one here would open the console twice.
        if (ws === old) connect();
        return true;
      },
      // The milliseconds from the construction of the last socket that opened
      // to its `open`, or `null` before the first open.
      handshakeMs: () => handshakeMs,
    };
  }
  const page = persistentSocket();

  // Decode a `[0x02][JSON]` command frame, or `null` for anything else.
  function commandFrame(bytes: Uint8Array): PushFrame | null {
    if (bytes[0] !== TAG_COMMAND) return null;
    try {
      return socketFrame<PushFrame>(bytes);
    } catch {
      return null;
    }
  }

  const { subscribeTree, subscribeRuns, subscribeChanges } = createSubscriptions({
    page,
    encodeCommand,
    withCheckout,
    commandFrame,
    reopenMs: RECONNECT_MS,
  });

  // Fire the verb on a `spawn` channel of the page socket and stream each
  // reply's `payload` (which carries `status`) to `onStatus`. The channel ends
  // on the terminal `exited`/`error`, or when the daemon or the socket ends
  // it. A socket that ends mid-run ends the command: the next socket does not
  // run the verb again. A page socket that does not open within the handshake
  // deadline runs nothing, as a command socket whose handshake failed did.
  function spawn(verb: string, payload: CommandPayload, onStatus: (st: SpawnStatus) => void) {
    const id = nextId++;
    let over = false;
    let left = false;
    let leave: (() => void) | null = null;
    let deadline: ReturnType<typeof setTimeout> | undefined;
    const finish = () => {
      over = true;
      left = true;
      clearTimeout(deadline);
      leave?.();
    };
    deadline = setTimeout(finish, CONNECT_TIMEOUT_MS);
    leave = page.join({
      onOpen: (channels: PageChannels) => {
        if (over) return;
        clearTimeout(deadline);
        const ch = channels.open("spawn", { command: { id, verb, payload } });
        ch.onFrame = (inner: Uint8Array) => {
          if (over || inner[0] !== TAG_COMMAND) return;
          let status: SpawnStatus;
          try {
            status = socketFrame<{ payload: SpawnStatus }>(inner).payload;
          } catch {
            return;
          }
          onStatus(status);
          if (status.status === "exited" || status.status === "error") finish();
        };
        ch.onClosed = finish;
      },
    });
    if (over && !left) finish();
    return id;
  }

  // A console's channel on the page socket (ADR-0082 D3): one channel per
  // call, with `payload` as its open. It opens when the page socket does, at
  // once when it is open. The console decides when to open another, and
  // with which byte count: a lost socket ends this channel with
  // `socket-lost`, and the next socket does not open it again.
  function openConsole(payload: SessionPayload, handlers: ConsoleHandlers) {
    let channel: Channel | null = null;
    let over = false;
    let leave: (() => void) | null = null;
    // The console leaves the page socket when its channel ends, so the next
    // socket does not open it again.
    leave = page.join({
      onOpen: (channels: PageChannels) => {
        const ch = channels.open("console", payload);
        channel = ch;
        ch.onFrame = handlers.onFrame;
        ch.onClosed = (reason: CloseReason) => {
          over = true;
          leave?.();
          handlers.onClosed(reason);
        };
      },
      onResume: () => handlers.onResume?.(),
    });
    const opened: ConsoleChannel = {
      get readyState() {
        if (over) return 3;
        if (!channel) return 0;
        return channel.readyState === "open" ? 1 : 3;
      },
      send: (inner: Uint8Array) => channel?.send(inner) ?? false,
      close: () => {
        if (over) return;
        over = true;
        channel?.close();
        leave?.();
      },
    };
    return opened;
  }

  // Presence on channel 0 of the page socket (#204): the daemon pushes a
  // `[0x03][JSON]` heartbeat every ~2s carrying `{name, avatar, uptime_secs}`.
  // Invoke `onPresence(payload)` per tick; the reconnect re-lights the topbar
  // after a daemon restart without a page reload. The heartbeat is the
  // page's stale signal (`noteHeartbeat`).
  // Channel 0 carries the daemon's pushes for the shown facts it owns
  // (`sessions.dirty`, `desk.dirty`, `repos.dirty`, `peers.dirty`,
  // `attention.dirty`; ADR-0070 D2, ADR-0085 D10): each `[0x02]` frame goes to
  // `onPush(verb, payload)`, and every open to `onOpen(reopened)`, so a reopen
  // reads again what a lost push would have said.
  function reportPresence(report: PresenceReport) {
    return page.sendIfOpen(encodeCommand({ id: 0, verb: "presence.report", payload: report }));
  }
  function onPresenceOpen(fn: () => void) {
    presenceOpens.push(fn);
  }

  function subscribePresence(
    onPresence: (presence: Presence) => void,
    { onPush, onOpen }: PresenceHandlers = {},
  ) {
    hasHeartbeat = true;
    const leave = page.join({
      onOpen: (_channels: PageChannels, reopened: boolean) => onOpen?.(reopened),
      onBare: (bytes: Uint8Array) => {
        if (bytes[0] === TAG_COMMAND) {
          const f = commandFrame(bytes);
          if (f?.verb) onPush?.(f.verb, f.payload || {});
          return;
        }
        if (bytes[0] !== TAG_PRESENCE) return;
        noteHeartbeat();
        try {
          onPresence(socketFrame<Presence>(bytes));
        } catch {}
      },
    });
    return { close: leave };
  }

  // ---- the resume of the page (ADR-0082, amendment of 2026-11-01) ----------
  // The one owner of the page's resume. Each trigger is heard here once,
  // "stale" is judged once (`pageIsStale`), the page socket resumes once,
  // and then each `onPageResume` hook runs the console's own resume. No
  // other module listens for a trigger to resume the page.
  const resumeHooks: ((stale: boolean, trigger: ResumeTrigger) => void)[] = [];
  function onPageResume(fn: (stale: boolean, trigger: ResumeTrigger) => void) {
    resumeHooks.push(fn);
  }
  // `hasHeartbeat` is true once the page subscribed to presence: the shell
  // does, the detached fence page does not. `lastHeartbeat` is 0 before the
  // first heartbeat.
  let hasHeartbeat = false;
  let lastHeartbeat = 0;
  let heartbeatWatch: ReturnType<typeof setTimeout> | undefined;
  let hiddenAt = 0;
  function heartbeatAgeMs() {
    if (!hasHeartbeat) return null;
    return lastHeartbeat ? Date.now() - lastHeartbeat : Infinity;
  }
  // Whether the page's heartbeat is lost; the account menu shows it.
  function heartbeatStale() {
    return (heartbeatAgeMs() ?? Infinity) > HEARTBEAT_LOST_MS;
  }
  function resumeOn(trigger: ResumeTrigger, hiddenMs: number) {
    const stale = pageIsStale(trigger, heartbeatAgeMs(), hiddenMs);
    page.resume(stale);
    for (const fn of resumeHooks) fn(stale, trigger);
  }
  // Each heartbeat starts the watch again; the watch ends only when none
  // came for `HEARTBEAT_LOST_MS`. So one loss is one resume, and a daemon
  // that stays down is not dialled again: the page socket's own retry does
  // that. A page that never had a heartbeat lost nothing.
  function noteHeartbeat() {
    lastHeartbeat = Date.now();
    clearTimeout(heartbeatWatch);
    heartbeatWatch = setTimeout(heartbeatLost, HEARTBEAT_LOST_MS);
  }
  // A desktop that slept with the tab in front may fire neither
  // `visibilitychange` nor `online`, and its socket reads OPEN with no byte
  // to come (#751). A hidden page resumes when it is shown instead.
  function heartbeatLost() {
    if (document.visibilityState === "visible") resumeOn("heartbeat", 0);
  }
  function onVisibility() {
    if (document.visibilityState !== "visible") {
      hiddenAt = Date.now();
      return;
    }
    const hiddenMs = hiddenAt ? Date.now() - hiddenAt : 0;
    hiddenAt = 0;
    resumeOn("visible", hiddenMs);
  }
  document.addEventListener("visibilitychange", onVisibility);
  // A tablet resumes on a different link; its socket died without a close.
  window.addEventListener("online", () => resumeOn("online", 0));

  // Turn a daemon-bound `workbench:action` into a Spawn call. `project`→`repo`
  // (the handler reads `payload.repo`); run params ride the payload as closed-enum
  // values the daemon validates.
  document.addEventListener("workbench:action", (e) => {
    // A `workbench:action` always carries its `action`; a bare event names none.
    const d = e.detail;
    if (!d) return;
    const verb = ACTION_TO_VERB[d.action] || (d.action === "command" ? d.verb : null);
    if (!verb) return;
    const payload =
      d.action === "run-start"
        ? { repo: d.project, agent: d.agent, planAgent: d.planAgent, branchMode: d.branchMode }
        : { repo: "project" in d ? d.project : undefined };
    // The CLI refuses by EXITING NON-ZERO after streaming its complaint to
    // stdout — `WBFail.isError` never fires for that shape, which is why a
    // refusal used to live only in the raw feed (#331). Both terminal paths
    // (non-zero `exited`, and an `error` frame) raise the sticky panel line;
    // no other frame does, so a socket that dies mid-stream leaves the last
    // state rather than a false success.
    // `pending` is load-bearing: the chunks are raw bytes, NOT lines — a
    // refusal measured live arrived split mid-sentence, so a per-chunk "last
    // line" reports a fragment. Lines are finalized on the newline that ends
    // them, and the trailing partial (a CLI that does not end with one) on the
    // terminal frame.
    // A lone `\r` is a line break here too: a CLI rendering a progress bar
    // emits nothing else, and `pending` must not grow for the run's lifetime —
    // an unbounded buffer feeding an unbounded banner is the very deformation
    // this issue removes, so it is capped like the raw feed is.
    const PENDING_CAP = 4096;
    let lastLine = "";
    let pending = "";
    const feedLines = (text: string) => {
      pending += text;
      const parts = pending.split(/\r\n|\n|\r/);
      pending = parts.pop() as string;
      if (pending.length > PENDING_CAP) pending = pending.slice(-PENDING_CAP);
      for (const line of parts) if (line.trim() !== "") lastLine = line.trim();
    };
    const finalLine = () => {
      if (pending.trim() !== "") lastLine = pending.trim();
      pending = "";
      return lastLine;
    };
    spawn(verb, payload, (s: SpawnStatus) => {
      if (s.status === "output") {
        const chunk = s.chunk || "";
        window.WBRuns?.output?.(chunk);
        feedLines(chunk);
      } else if (WBFail.isError(s)) {
        const msg = WBFail.failed(s, "Could not start: the daemon gave no reason.");
        messages.flash(msg);
        sendWindow(window, "workbench:run-verb-failed", { message: msg });
      } else if (s.status === "exited") {
        sendWindow(window, "workbench:run-verb-failed", {
          message: WBRun.exitNote(verb, s.code, finalLine()),
        });
      }
    });
  });

  return {
    spawn,
    observe,
    readImage,
    write,
    withCheckout,
    onUnknownCheckout,
    subscribeTree,
    subscribeRuns,
    subscribeChanges,
    subscribePresence,
    openConsole,
    onPageResume,
    heartbeatStale,
    afterSessionRead: page.afterSessionRead,
    reportPresence,
    onPresenceOpen,
    pageHandshakeMs: page.handshakeMs,
    resumeDecision,
    detachSocket,
    RESUME_DEBOUNCE_MS,
    CONNECT_TIMEOUT_MS,
    encodeCommand,
    ACTION_TO_VERB,
    TAG_TERMINAL,
    TAG_COMMAND,
    TAG_PRESENCE,
  };
}

// The type of the `window.WBDaemon` instance that `app.ts` and the dialogs read
// (ADR-0075 D9).
export type WBDaemonApi = ReturnType<typeof createDaemon>;
