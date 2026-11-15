// The ambient types of a session row of `/api/sessions`. The page
// instances, the event maps and the daemon replies are in `globals.d.ts`.

/** A live session of `/api/sessions` (the daemon's `HostedSessionInfo`). */
type HostedSession = {
  id: number;
  repo: string;
  agent: string;
  /** `agent` or `console`. */
  kind: string;
  started_at: number;
  daemon_id: string;
  environment: string;
  name?: string;
  /** The worktree; absent on the primary tree. */
  checkout?: string;
  /** The desk record the session serves. */
  record?: string;
  /** The agent's own state, from its hooks. */
  agent_state?: AgentStateBlock;
};

/**
 * The agent's block on a session row. A console whose hooks sent nothing has
 * `source: "silent"`, a `reason`, and no `state`. A console launched with no
 * state source on purpose has `source: "none"` and a `reason`.
 */
type AgentStateBlock = {
  state?: string;
  source?: "silent" | "none";
  reason?: string;
  since: string;
  detail?: string;
};
