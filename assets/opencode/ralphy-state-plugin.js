// Ralphy's agent-state plugin for OpenCode.
//
// Ralphy writes this file next to a console's or a run's status file and
// names it in OPENCODE_CONFIG_CONTENT, so OpenCode loads it with the
// operator's own config and plugins. It appends one JSON line per state
// event to $RALPHY_STATUS_FILE, in the same shape as `ralphy hook status`
// writes for the other vendors. Ralphy maps the lines to a state; this file
// does not decide a state.
//
// Rules this file keeps (measured with OpenCode 1.18.4 and 1.19.0):
// - It never writes to stdout or stderr: in the TUI that text is drawn over
//   the screen.
// - It never throws: a throw does not stop OpenCode, but in `opencode run`
//   each one prints a stack trace on stderr.
// - Its first line is written when it loads: a plugin that did not load says
//   nothing at all, so that line is the only proof that it runs.
// - It imports only `node:fs`: a plain file with no package loads with no
//   install.
import { appendFileSync } from "node:fs";

// The bus events that end a wait, by the name OpenCode gives them.
const WAIT_ENDS = ["permission.replied", "question.replied", "question.rejected"];

export const RalphyAgentState = async () => {
  const out = process.env.RALPHY_STATUS_FILE;
  // sessionID -> parentID, for each session that has a parent (a subagent).
  const parents = new Map();

  const write = (line) => {
    if (!out) return;
    try {
      appendFileSync(out, JSON.stringify({ ...line, ts: new Date().toISOString() }) + "\n");
    } catch {
      // A failed write must not reach the agent: the state is lost, the
      // agent's work is not.
    }
  };

  // The D8 fields of a line for an event of session `id`: a subagent's
  // session is named as the agent too.
  const of = (id) => ({ session_id: id, agent_id: parents.has(id) ? id : undefined });

  const handle = (event) => {
    const type = event && event.type;
    const p = (event && event.properties) || {};
    if (type === "session.created" || type === "session.updated") {
      const info = p.info || {};
      if (info.parentID) {
        parents.set(info.id, info.parentID);
      } else if (type === "session.created" && info.id) {
        // A new lead session: the TUI's own `/new`, or the first prompt.
        write({ event: type, session_id: info.id });
      }
    } else if (type === "session.status") {
      const status = p.status || {};
      if (status.type === "busy") write({ event: type, ...of(p.sessionID) });
    } else if (type === "session.idle") {
      write({ event: type, ...of(p.sessionID) });
    } else if (type === "permission.asked") {
      write({ event: type, ...of(p.sessionID), tool_name: p.permission, tool_use_id: p.id });
    } else if (type === "question.asked") {
      const first = Array.isArray(p.questions) ? p.questions[0] : undefined;
      const question = first && typeof first.question === "string" ? first.question : undefined;
      write({ event: type, ...of(p.sessionID), tool_name: "question", tool_use_id: p.id, question });
    } else if (WAIT_ENDS.includes(type)) {
      write({ event: type, ...of(p.sessionID), tool_use_id: p.requestID });
    }
  };

  write({ event: "plugin.loaded" });
  return {
    event: async (input) => {
      try {
        handle(input && input.event);
      } catch {
        // A failure here must not reach the agent: OpenCode would print it.
      }
    },
  };
};
