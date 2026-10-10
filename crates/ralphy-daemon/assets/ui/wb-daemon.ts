/* ---------------------------------------------------------------------------
   ralphy workbench shell — the daemon call door (the run/triage/push verbs)

   The browser's `workbench:action` seam is verb-agnostic; this adapter maps the
   actions that reach the daemon (`ACTION_TO_VERB`) to a `Command {id, verb,
   payload}` and drives one `/ws/command` socket per Spawn call (the daemon's
   handler is one-command-per-connection with a streamed lifecycle). The client
   never composes a command line — it sends closed-enum params and the daemon's
   verb registry (dispatch.rs) builds the argv. Raw `status:"output"` chunks feed
   the Runs panel live (ADR-0032 §5, ADR-0036).
--------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import { WBProject } from "./wb-project.ts";
import { WBRun } from "./wb-runs.ts";
import { resumeDecision, CONNECT_TIMEOUT_MS, RESUME_DEBOUNCE_MS } from "./wb-resume.ts";

/** What a persistent socket's owner hears: each open, and each frame. */
type SocketHandlers = {
  /** `reopened` is false on the first open only. */
  onOpen?: (ws: WebSocket, reopened: boolean) => void;
  onMessage: (ev: MessageEvent) => void;
};

/** What the presence socket's owner hears besides the heartbeat. */
type PresenceHandlers = {
  onPush?: (verb: string, payload: PushPayload) => void;
  onOpen?: (reopened: boolean) => void;
};

export function createDaemon(window: Window, document: Document, location: Location) {
  // The tagged-frame codec, mirrored from src/protocol.rs (see wb-console.ts).
  const TAG_TERMINAL = 0x01;
  const TAG_COMMAND = 0x02;
  const TAG_PRESENCE = 0x03;

  // The WebSocket origin, scheme-matched to the page: `wss://` when the workbench
  // is served over TLS (a dev-tunnel/reverse-proxy reaching a loopback daemon),
  // else `ws://` for a plain-http localhost bind. Hardcoding `ws://` would trip
  // the browser's mixed-content block on an https origin and kill every socket.
  const WS_ORIGIN =
    (location.protocol === "https:" ? "wss://" : "ws://") + location.host;

  // Which `workbench:action`s reach the daemon, and as which verb. The generic
  // `command` action carries its verb in the event detail (triage/push).
  const ACTION_TO_VERB: Record<string, string> = { "run-start": "run" };

  let nextId = 1;

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

  function encodeCommand({ id, verb, payload }: { id: number; verb: string; payload: CommandPayload }) {
    const body = new TextEncoder().encode(JSON.stringify({ id, verb, payload }));
    const out = new Uint8Array(1 + body.length);
    out[0] = TAG_COMMAND;
    out.set(body, 1);
    return out;
  }

  // Open a fresh `/ws/command`, fire the verb, and stream each reply's `payload`
  // (which carries `status`) to `onStatus`; close on the terminal `exited`/`error`.
  function spawn(verb: string, payload: CommandPayload, onStatus: (st: SpawnStatus) => void) {
    const id = nextId++;
    const ws = new WebSocket(WS_ORIGIN + "/ws/command");
    ws.binaryType = "arraybuffer";
    ws.onopen = () => ws.send(encodeCommand({ id, verb, payload }));
    ws.onmessage = (ev) => {
      const a = new Uint8Array(ev.data);
      if (a[0] !== TAG_COMMAND) return;
      let frame;
      try {
        frame = JSON.parse(new TextDecoder().decode(a.subarray(1)));
      } catch {
        return;
      }
      const status = frame.payload;
      onStatus(status);
      if (status.status === "exited" || status.status === "error") ws.close();
    };
    return id;
  }

  // The optional `checkout` argument of a repo-scoped verb (#406, ADR-0063
  // §2): a worktree NAME the daemon resolves under the repo's own root. Added
  // ONLY for a real name — with no selection the payload is byte-identical to
  // the pre-#406 one, so an older daemon never sees a key it does not know.
  function withCheckout(payload: CommandPayload, checkout: string | null | undefined): CommandPayload {
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

  // Fire an Observe read (`tree.list`/`file.read`) and resolve with the single
  // reply payload — the daemon answers ONE frame on the same id and returns (no
  // spawn/stream). One socket per read, mirroring `spawn`'s per-call shape.
  function observe<V extends string>(verb: V, payload: CommandPayload) {
    return new Promise<ReplyOf<V>>((resolve, reject) => {
      const id = nextId++;
      const ws = new WebSocket(WS_ORIGIN + "/ws/command");
      ws.binaryType = "arraybuffer";
      ws.onopen = () => ws.send(encodeCommand({ id, verb, payload }));
      ws.onmessage = (ev) => {
        const a = new Uint8Array(ev.data);
        if (a[0] !== TAG_COMMAND) return;
        try {
          const reply = JSON.parse(new TextDecoder().decode(a.subarray(1))).payload;
          resolve(reply);
          // AFTER the resolve, on a later task: a listener that remounts the
          // tree must not run before the read that failed has settled its whole
          // `.then`/`.catch` chain, or the stale mount's catch paints its error
          // onto the fresh one (a microtask would still race that chain).
          setTimeout(() => noteUnknownCheckout(payload, reply), 0);
        } catch (err) {
          reject(err);
        }
        ws.close();
      };
      // A socket that closes without ever answering must REJECT, not hang: the
      // daemon drops the connection with no reply on an undecodable frame
      // (lib.rs `let Ok(Frame::Command(cmd)) … else { return; }`) and on
      // shutdown/restart mid-read. A clean close fires `close`, not `error`, so
      // without this the promise pends for the page's life and every caller
      // awaiting it is wedged. Settling twice is a no-op, so the `ws.close()`
      // after a resolve above is harmless.
      ws.onclose = () => reject(new Error("connection closed before a reply"));
      ws.onerror = (err) => reject(err);
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

  // ONE persistent socket on `path`, the reconnect loop every subscription below
  // shares. Reconnects on `close` ONLY — the spec guarantees `error` is always
  // followed by `close`, so scheduling on both would double the backoff into a
  // storm. One fixed 3s timer per drop, no exponential backoff: one socket each.
  // `onOpen(ws, reopened)` runs on every open; `reopened` is false on the FIRST
  // open only and stays true across a resume, so a caller's catch-up read rides
  // the reconnect with no second code path.
  function persistentSocket(path: string, { onOpen, onMessage }: SocketHandlers) {
    let closed = false;
    let ws: WebSocket | null = null;
    let opened = false;
    let live = false;
    let timer: number | null = null;
    let lastResumeAt = 0;
    let connectingSince = 0;
    const connect = () => {
      if (closed) return;
      const sock = new WebSocket(WS_ORIGIN + path);
      ws = sock;
      live = false;
      connectingSince = Date.now();
      armHandshakeDeadline(sock);
      sock.binaryType = "arraybuffer";
      sock.onopen = () => {
        live = true;
        const reopened = opened;
        opened = true;
        onOpen?.(sock, reopened);
      };
      sock.onmessage = onMessage;
      sock.onclose = () => {
        live = false;
        if (!closed) timer = setTimeout(connect, 3000);
      };
    };
    connect();
    return {
      // Send on the current socket if it is open; `false` means the frame was
      // not sent, and the caller's `onOpen` is what re-sends its state.
      sendIfOpen: (frame: Uint8Array) => {
        if (!live) return false;
        // `live` is set only on the open socket, so `ws` is that socket.
        ws!.send(frame);
        return true;
      },
      resume: (stale: boolean) => {
        if (closed) return false;
        const now = Date.now();
        if (now - lastResumeAt < RESUME_DEBOUNCE_MS) return false;
        const rs = ws ? ws.readyState : null;
        const connectingMs = now - connectingSince;
        if (resumeDecision({ readyState: rs, stale, connectingMs }) === "none") return false;
        lastResumeAt = now;
        clearTimeout(timer);
        timer = null;
        detachSocket(ws);
        connect();
        return true;
      },
      close: () => {
        // Set the flag BEFORE closing, so our own `close` never schedules a retry.
        closed = true;
        clearTimeout(timer);
        try {
          if (ws) ws.close();
        } catch {}
      },
    };
  }

  // Decode a `[0x02][JSON]` command frame, or `null` for anything else.
  function commandFrame(ev: MessageEvent): PushFrame | null {
    const a = new Uint8Array(ev.data);
    if (a[0] !== TAG_COMMAND) return null;
    try {
      return JSON.parse(new TextDecoder().decode(a.subarray(1)));
    } catch {
      return null;
    }
  }

  // Open ONE persistent `/ws/tree` subscription for a project (#196, ADR-0036 §4):
  // `watch`/`unwatch` a rel dir as the tree expands/collapses, and invoke
  // `onDirty(relPath)` for each `tree.dirty` push. Returns the control handle; the
  // caller closes it when the project closes (the daemon tears the watcher down on
  // the last release).
  // A subscription is bound to ONE checkout (#406): every watch carries it and
  // a `tree.dirty` for another tree of the same repo is not this tree's news —
  // the caller remounts the tree (and this subscription) when the selection
  // changes, so the filter only ever drops a frame from a stale watch.
  // The same socket holds the checkout's HEAD (`head.watch`): a branch switch or
  // a commit made anywhere pushes `head.dirty`, and `onHead()` re-reads the
  // branch. The socket's close releases that hold with the tree's.
  // The held dirs are STATE, not a queue: a new socket starts empty on the
  // daemon, so every open sends `head.watch` and each held `watch` again, and a
  // RE-open re-reads each held dir and the branch once — whatever changed while
  // the socket was down was never pushed (#484).
  // A `tree.failed` push says the daemon could not watch a dir of this tree:
  // `onFailed(reason)`. A reopen holds every dir again: `onFailed(null)`.
  function subscribeTree(
    repo: string,
    onDirty: (path: string) => void,
    checkout: string | null | undefined,
    onHead: (() => void) | undefined,
    onFailed: ((reason: string | null) => void) | undefined,
  ) {
    const held = new Set<string>();
    const frame = (verb: string, path: string) =>
      encodeCommand({ id: 0, verb, payload: withCheckout({ repo, path: path || "" }, checkout) });
    // Re-reads each held dir and the branch: a reopen does it, and so does a
    // tab that becomes visible or logs in (ADR-0070 D2 events 3, 4).
    const replay = () => {
      for (const path of held) onDirty(path);
      onHead?.();
    };
    const sub = persistentSocket("/ws/tree", {
      onOpen: (ws: WebSocket, reopened: boolean) => {
        if (onHead) ws.send(frame("head.watch", ""));
        for (const path of held) ws.send(frame("watch", path));
        if (reopened) {
          // A new socket holds every dir again, so a failed watch is cleared.
          onFailed?.(null);
          replay();
        }
      },
      onMessage: (ev: MessageEvent) => {
        const f = commandFrame(ev);
        if (!f || !["tree.dirty", "head.dirty", "tree.failed"].includes(f.verb)) return;
        const p = f.payload || {};
        if ((p.checkout || null) !== (checkout || null)) return;
        if (f.verb === "tree.failed") onFailed?.(p.reason || "");
        else if (f.verb === "head.dirty") onHead?.();
        else onDirty(p.path || "");
      },
    });
    return {
      watch: (path: string) => {
        const rel = path || "";
        if (held.has(rel)) return;
        held.add(rel);
        sub.sendIfOpen(frame("watch", rel));
      },
      // While disconnected the set is all there is to change: the next socket
      // never learns the dir.
      unwatch: (path: string) => {
        const rel = path || "";
        if (!held.delete(rel)) return;
        sub.sendIfOpen(frame("unwatch", rel));
      },
      replay,
      resume: sub.resume,
      close: sub.close,
    };
  }

  // Open ONE persistent `/ws/tree` run-snapshot subscription for a project (#300,
  // ADR-0047 §9): `runs.watch` holds the repo's `.ralphy/runstate` dir and each
  // `runs.dirty` push invokes `onDirty()`, which re-reads `runs.list`. A snapshot
  // is STATE, so the read is idempotent — hence the extra `onDirty()` on every
  // RE-open, the catch-up read that recovers a daemon restart with no operator
  // action. The FIRST open deliberately skips it: a subscription is only ever
  // mounted by a caller that reads the same snapshot itself in the same tick, so
  // catching up there re-read state nobody had yet missed — and this read spawns
  // a CLI, so the duplicate was the dominant cost of opening a project. Catching
  // up is for what arrived while we were DISCONNECTED, which the first connection
  // has no window for.
  function subscribeRuns(repo: string, onDirty: () => void) {
    const sub = persistentSocket("/ws/tree", {
      onOpen: (ws: WebSocket, reopened: boolean) => {
        ws.send(encodeCommand({ id: 0, verb: "runs.watch", payload: { repo, path: "" } }));
        if (reopened) onDirty();
      },
      onMessage: (ev: MessageEvent) => {
        if (commandFrame(ev)?.verb === "runs.dirty") onDirty();
      },
    });
    return { resume: sub.resume, close: sub.close };
  }

  // Open ONE persistent `/ws/tree` run-completion subscription for a project
  // (#310, ADR-0036 amendment). Unlike `subscribeRuns` this sends NO watch frame:
  // `changes.dirty` is not watcher-fed, so every connection receives every nudge
  // and the repo filter is the caller's (`WBChanges.shouldReload`). Each decoded
  // frame is handed over whole — a RE-open synthesizes one for this repo as the
  // catch-up read that recovers a daemon restart. The first open skips it for the
  // reason given on `subscribeRuns`: the mounting caller reads `changes.list` and
  // `sync.status` itself, and each of those spawns the `ralphy` CLI, which spawns
  // `git` — so the synthetic frame doubled the two most expensive reads of
  // opening a project.
  function subscribeChanges(repo: string, onFrame: (frame: PushFrame) => void) {
    const sub = persistentSocket("/ws/tree", {
      onOpen: (_ws: WebSocket, reopened: boolean) => {
        if (reopened) onFrame({ verb: "changes.dirty", payload: { repo } });
      },
      onMessage: (ev: MessageEvent) => {
        const frame = commandFrame(ev);
        if (frame) onFrame(frame);
      },
    });
    return { resume: sub.resume, close: sub.close };
  }

  // Open ONE persistent `/ws` presence subscription (#204): the daemon pushes a
  // `[0x03][JSON]` heartbeat every ~2s carrying `{name, avatar, uptime_secs}`.
  // Invoke `onPresence(payload)` per tick; the reconnect re-lights the topbar
  // after a daemon restart without a page reload. The heartbeat this socket
  // carries IS the shell's staleness signal, so a resume here is what re-arms
  // the probe every other resume depends on.
  // The same socket carries the daemon's pushes for the shown facts it owns
  // (`sessions.dirty`, `desk.dirty`, `repos.dirty`, `peers.dirty`; ADR-0070
  // D2): each `[0x02]` frame goes to `onPush(verb, payload)`, and every open to
  // `onOpen(reopened)`, so a reopen reads again what a lost push would have said.
  function subscribePresence(onPresence: (presence: Presence) => void, { onPush, onOpen }: PresenceHandlers = {}) {
    const sub = persistentSocket("/ws", {
      onOpen: (_ws: WebSocket, reopened: boolean) => onOpen?.(reopened),
      onMessage: (ev: MessageEvent) => {
        const a = new Uint8Array(ev.data);
        if (a[0] === TAG_COMMAND) {
          const f = commandFrame(ev);
          if (f?.verb) onPush?.(f.verb, f.payload || {});
          return;
        }
        if (a[0] !== TAG_PRESENCE) return;
        try {
          onPresence(JSON.parse(new TextDecoder().decode(a.subarray(1))));
        } catch {}
      },
    });
    return { resume: sub.resume, close: sub.close };
  }

  // Turn a daemon-bound `workbench:action` into a Spawn call. `project`→`repo`
  // (the handler reads `payload.repo`); run params ride the payload as closed-enum
  // values the daemon validates.
  document.addEventListener("workbench:action", (e) => {
    // A `workbench:action` always carries its `action`; a bare event names none.
    const d = e.detail || ({} as WorkbenchAction);
    const verb = ACTION_TO_VERB[d.action] || (d.action === "command" ? d.verb : null);
    if (!verb) return;
    const payload =
      d.action === "run-start"
        ? { repo: d.project, agent: d.agent, planAgent: d.planAgent, branchMode: d.branchMode }
        : { repo: d.project };
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
        window.getShell()?._flashAction?.(msg);
        window.getShell()?.runVerbFailed?.(msg);
      } else if (s.status === "exited") {
        window.getShell()?.runVerbFailed?.(WBRun.exitNote(verb, s.code, finalLine()));
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
