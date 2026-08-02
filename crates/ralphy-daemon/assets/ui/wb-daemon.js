/* ---------------------------------------------------------------------------
   ralphy workbench shell — the daemon call door (the run/triage/push verbs)

   The browser's `workbench:action` seam is verb-agnostic; this adapter maps the
   actions that reach the daemon (`ACTION_TO_VERB`) to a `Command {id, verb,
   payload}` and drives one `/ws/command` socket per Spawn call (the daemon's
   handler is one-command-per-connection with a streamed lifecycle). The client
   never composes a command line — it sends closed-enum params and the daemon's
   verb registry (dispatch.rs) builds the argv. Raw `status:"output"` chunks feed
   the Runs panel live (ADR-0032 §5, ADR-0036); the structured event fold stays on
   the ⚡ demo button.
--------------------------------------------------------------------------- */
window.WBDaemon = (function () {
  // The tagged-frame codec, mirrored from src/protocol.rs (see wb-console.js).
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
  const ACTION_TO_VERB = { "run-start": "run" };

  let nextId = 1;

  function encodeCommand({ id, verb, payload }) {
    const body = new TextEncoder().encode(JSON.stringify({ id, verb, payload }));
    const out = new Uint8Array(1 + body.length);
    out[0] = TAG_COMMAND;
    out.set(body, 1);
    return out;
  }

  // Open a fresh `/ws/command`, fire the verb, and stream each reply's `payload`
  // (which carries `status`) to `onStatus`; close on the terminal `exited`/`error`.
  function spawn(verb, payload, onStatus) {
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

  // Fire an Observe read (`tree.list`/`file.read`) and resolve with the single
  // reply payload — the daemon answers ONE frame on the same id and returns (no
  // spawn/stream). One socket per read, mirroring `spawn`'s per-call shape.
  function observe(verb, payload) {
    return new Promise((resolve, reject) => {
      const id = nextId++;
      const ws = new WebSocket(WS_ORIGIN + "/ws/command");
      ws.binaryType = "arraybuffer";
      ws.onopen = () => ws.send(encodeCommand({ id, verb, payload }));
      ws.onmessage = (ev) => {
        const a = new Uint8Array(ev.data);
        if (a[0] !== TAG_COMMAND) return;
        try {
          resolve(JSON.parse(new TextDecoder().decode(a.subarray(1))).payload);
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
  function readImage(repo, path, onRefused) {
    return observe("file.image", { repo, path }).then((reply) => {
      if (window.WBFail.isError(reply) || !reply.base64 || !reply.mediaType) {
        onRefused?.(window.WBFail.message(reply, "refused"));
        return null;
      }
      return `data:${reply.mediaType};base64,${reply.base64}`;
    });
  }

  // Fire a Write byte-op (`file.write`/`file.create`/`file.rename`/`file.copy`/
  // `file.delete`, #197/#362) and resolve with the single reply payload. Same one-socket-one-reply
  // shape as `observe` — the daemon answers ONE frame on the id and returns (no
  // spawn/stream); a confinement refusal comes back as `{status:"error",reason}`.
  function write(verb, payload) {
    return observe(verb, payload);
  }

  // Open ONE persistent `/ws/tree` subscription for a project (#196, ADR-0036 §4):
  // `watch`/`unwatch` a rel dir as the tree expands/collapses, and invoke
  // `onDirty(relPath)` for each `tree.dirty` push. Returns the control handle; the
  // caller closes it when the project closes (the daemon tears the watcher down on
  // the last release). Commands sent before the socket opens are queued.
  function subscribeTree(repo, onDirty) {
    const ws = new WebSocket(WS_ORIGIN + "/ws/tree");
    ws.binaryType = "arraybuffer";
    let open = false;
    const pending = [];
    const send = (verb, path) => {
      const frame = encodeCommand({ id: 0, verb, payload: { repo, path: path || "" } });
      if (open) ws.send(frame);
      else pending.push(frame);
    };
    ws.onopen = () => {
      open = true;
      while (pending.length) ws.send(pending.shift());
    };
    ws.onmessage = (ev) => {
      const a = new Uint8Array(ev.data);
      if (a[0] !== TAG_COMMAND) return;
      let frame;
      try {
        frame = JSON.parse(new TextDecoder().decode(a.subarray(1)));
      } catch {
        return;
      }
      if (frame.verb === "tree.dirty") onDirty((frame.payload && frame.payload.path) || "");
    };
    return {
      watch: (path) => send("watch", path),
      unwatch: (path) => send("unwatch", path),
      close: () => {
        try {
          ws.close();
        } catch {}
      },
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
  // has no window for. Reconnects on `close` ONLY, one fixed 3s timer (see
  // subscribePresence).
  function subscribeRuns(repo, onDirty) {
    let closed = false;
    let ws = null;
    let opened = false;
    const connect = () => {
      if (closed) return;
      ws = new WebSocket(WS_ORIGIN + "/ws/tree");
      ws.binaryType = "arraybuffer";
      ws.onopen = () => {
        ws.send(encodeCommand({ id: 0, verb: "runs.watch", payload: { repo, path: "" } }));
        if (opened) onDirty();
        opened = true;
      };
      ws.onmessage = (ev) => {
        const a = new Uint8Array(ev.data);
        if (a[0] !== TAG_COMMAND) return;
        let frame;
        try {
          frame = JSON.parse(new TextDecoder().decode(a.subarray(1)));
        } catch {
          return;
        }
        if (frame.verb === "runs.dirty") onDirty();
      };
      ws.onclose = () => {
        if (!closed) setTimeout(connect, 3000);
      };
    };
    connect();
    return {
      close: () => {
        // Set the flag BEFORE closing, so our own `close` never schedules a retry.
        closed = true;
        try {
          ws && ws.close();
        } catch {}
      },
    };
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
  // opening a project. Reconnects on `close` ONLY, one fixed 3s timer (the
  // subscribeRuns shape); no polling timer.
  function subscribeChanges(repo, onFrame) {
    let closed = false;
    let ws = null;
    let opened = false;
    const connect = () => {
      if (closed) return;
      ws = new WebSocket(WS_ORIGIN + "/ws/tree");
      ws.binaryType = "arraybuffer";
      ws.onopen = () => {
        if (opened) onFrame({ verb: "changes.dirty", payload: { repo } });
        opened = true;
      };
      ws.onmessage = (ev) => {
        const a = new Uint8Array(ev.data);
        if (a[0] !== TAG_COMMAND) return;
        let frame;
        try {
          frame = JSON.parse(new TextDecoder().decode(a.subarray(1)));
        } catch {
          return;
        }
        onFrame(frame);
      };
      ws.onclose = () => {
        if (!closed) setTimeout(connect, 3000);
      };
    };
    connect();
    return {
      close: () => {
        // Set the flag BEFORE closing, so our own `close` never schedules a retry.
        closed = true;
        try {
          ws && ws.close();
        } catch {}
      },
    };
  }

  // Open ONE persistent `/ws` presence subscription (#204): the daemon pushes a
  // `[0x03][JSON]` heartbeat every ~2s carrying `{name, avatar, uptime_secs}`.
  // Invoke `onPresence(payload)` per tick; reconnect after a fixed 3s backoff on
  // close/error so a daemon restart re-lights the topbar without a page reload.
  // A single fixed backoff (no exponential storm) is deliberate — one socket.
  function subscribePresence(onPresence) {
    let closed = false;
    let ws = null;
    const connect = () => {
      if (closed) return;
      ws = new WebSocket(WS_ORIGIN + "/ws");
      ws.binaryType = "arraybuffer";
      ws.onmessage = (ev) => {
        const a = new Uint8Array(ev.data);
        if (a[0] !== TAG_PRESENCE) return;
        try {
          onPresence(JSON.parse(new TextDecoder().decode(a.subarray(1))));
        } catch {}
      };
      // Reconnect on `close` ONLY — the spec guarantees `error` is always
      // followed by `close`, so scheduling on both would double the backoff
      // into a storm. One pending 3s timer per drop.
      ws.onclose = () => {
        if (!closed) setTimeout(connect, 3000);
      };
    };
    connect();
    return {
      close: () => {
        closed = true;
        try {
          ws && ws.close();
        } catch {}
      },
    };
  }

  // Turn a daemon-bound `workbench:action` into a Spawn call. `project`→`repo`
  // (the handler reads `payload.repo`); run params ride the payload as closed-enum
  // values the daemon validates.
  document.addEventListener("workbench:action", (e) => {
    const d = e.detail || {};
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
    const feedLines = (text) => {
      pending += text;
      const parts = pending.split(/\r\n|\n|\r/);
      pending = parts.pop();
      if (pending.length > PENDING_CAP) pending = pending.slice(-PENDING_CAP);
      for (const line of parts) if (line.trim() !== "") lastLine = line.trim();
    };
    const finalLine = () => {
      if (pending.trim() !== "") lastLine = pending.trim();
      pending = "";
      return lastLine;
    };
    spawn(verb, payload, (s) => {
      if (s.status === "output") {
        const chunk = s.chunk || "";
        window.WBRuns?.output?.(chunk);
        feedLines(chunk);
      } else if (window.WBFail.isError(s)) {
        const msg = window.WBFail.message(s, "refused");
        getShell()?._flashAction?.(msg);
        getShell()?.runVerbFailed?.(msg);
      } else if (s.status === "exited") {
        getShell()?.runVerbFailed?.(window.WBRun.exitNote(verb, s.code, finalLine()));
      }
    });
  });

  return {
    spawn,
    observe,
    readImage,
    write,
    subscribeTree,
    subscribeRuns,
    subscribeChanges,
    subscribePresence,
    encodeCommand,
    ACTION_TO_VERB,
    TAG_TERMINAL,
    TAG_COMMAND,
    TAG_PRESENCE,
  };
})();
