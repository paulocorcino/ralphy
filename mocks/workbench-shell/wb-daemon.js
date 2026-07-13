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
    const ws = new WebSocket("ws://" + location.host + "/ws/command");
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
    spawn(verb, payload, (s) => {
      if (s.status === "output") window.WBRuns?.output?.(s.chunk || "");
      else if (s.status === "error") getShell()?._flashAction?.(s.message || "refused");
    });
  });

  return { spawn, encodeCommand, ACTION_TO_VERB, TAG_TERMINAL, TAG_COMMAND, TAG_PRESENCE };
})();
