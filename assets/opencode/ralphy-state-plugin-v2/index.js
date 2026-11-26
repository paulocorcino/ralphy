// Ralphy's agent-state plugin for OpenCode 2.x.
//
// The 2.x twin of `../ralphy-state-plugin.js`: 2.x loads only a folder, and
// only a default export with an `id` and a `setup`. It appends to
// $RALPHY_STATUS_FILE the same JSON lines, with the same `event` names and
// fields, as the 1.x plugin, so Ralphy maps both with one table. The 2.x
// event names it reads are in docs/research/opencode-2-spike/plugin-v2.md.
//
// Rules this file keeps (measured with OpenCode 2.3.2):
// - It never writes to stdout or stderr, and it never throws.
// - Its first line is written when it loads: that line is the only proof
//   that it runs.
// - It imports only `node:fs`: a folder with no dependency loads with no
//   install.
// - One process may call `setup` more than once (a reload): the newest call
//   takes the event stream, so no line is written twice.
import { appendFileSync } from "node:fs";

// The 2.x events that end a turn: 1.x names this `session.idle`.
const TURN_ENDS = [
  "session.execution.succeeded",
  "session.execution.failed",
  "session.execution.interrupted",
];
// The 2.x events of a busy turn: 1.x writes `session.status` busy at these.
const TURN_BUSY = ["session.execution.started", "session.step.started", "session.step.ended"];
// The 2.x events that end a wait, with the field that names the wait.
const WAIT_ENDS = {
  "permission.replied": ["permission.replied", "requestID"],
  "form.replied": ["question.replied", "id"],
  "form.cancelled": ["question.rejected", "id"],
};
const SHARED = Symbol.for("ralphy.agent-state.v2");

const write = (line) => {
  const out = process.env.RALPHY_STATUS_FILE;
  if (!out) return;
  try {
    appendFileSync(out, JSON.stringify({ ...line, ts: new Date().toISOString() }) + "\n");
  } catch {
    // A failed write must not reach the agent: the state is lost, the
    // agent's work is not.
  }
};

const shared = () => {
  if (!globalThis[SHARED]) {
    globalThis[SHARED] = { parents: new Map(), leads: new Set(), stop: undefined };
    write({ event: "plugin.loaded" });
  }
  return globalThis[SHARED];
};

// A lead session is written once, when it is first seen: 2.x may create
// the first session before the plugin subscribes.
const seen = (state, id) => {
  if (!id || state.parents.has(id) || state.leads.has(id)) return;
  state.leads.add(id);
  write({ event: "session.created", session_id: id });
};

const handle = (state, event) => {
  const type = event && event.type;
  const d = (event && event.data) || {};
  const of = (id) => ({ session_id: id, agent_id: state.parents.has(id) ? id : undefined });
  if (type === "session.created") {
    if (d.parentID) state.parents.set(d.sessionID, d.parentID);
    else seen(state, d.sessionID);
  } else if (TURN_BUSY.includes(type)) {
    seen(state, d.sessionID);
    write({ event: "session.status", ...of(d.sessionID) });
  } else if (TURN_ENDS.includes(type)) {
    write({ event: "session.idle", ...of(d.sessionID) });
  } else if (type === "permission.asked") {
    write({ event: type, ...of(d.sessionID), tool_name: d.action, tool_use_id: d.id });
  } else if (type === "form.created") {
    const form = d.form || {};
    const question = typeof form.title === "string" ? form.title : undefined;
    write({
      event: "question.asked",
      ...of(form.sessionID),
      tool_name: "question",
      tool_use_id: form.id,
      question,
    });
  } else if (WAIT_ENDS[type]) {
    const [name, field] = WAIT_ENDS[type];
    write({ event: name, ...of(d.sessionID), tool_use_id: d[field] });
  }
};

const listen = async (ctx, state, signal) => {
  try {
    for await (const event of ctx.event.subscribe({ signal })) {
      try {
        handle(state, event);
      } catch {
        // A failure here must not reach the agent: OpenCode would log it.
      }
    }
  } catch {
    // The stream ends with an abort when the plugin unloads.
  }
};

export default {
  id: "ralphy-agent-state",
  async setup(ctx) {
    const state = shared();
    if (state.stop) state.stop();
    const controller = new AbortController();
    const stop = () => controller.abort();
    state.stop = stop;
    void listen(ctx, state, controller.signal);
    return () => {
      stop();
      if (state.stop === stop) state.stop = undefined;
    };
  },
};
