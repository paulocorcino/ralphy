/* ---------------------------------------------------------------------------
   The replies the board reads (`boardFold`, `readFold`, `rowToIssue` and
   `shownIssue` of wb-board.ts): the `board.list` fold with its timeout, the
   issues and label colors it holds, a fold row as an issue, and the issue of
   an `issue.show` reply.

   Pure functions of their arguments, except `boardFold`, which asks the daemon.
   --------------------------------------------------------------------------- */
import type { BoardLabel } from "./wb-kanban.ts";
import type { BoardRow } from "./wb-shell-types.d.ts";

// The `board.list` reply for the repo `slug`, or a rejection after `timeoutMs`.
export function boardFold(slug: string, timeoutMs: number) {
  return Promise.race([
    window.WBDaemon.observe("board.list", { repo: slug }),
    new Promise<never>((_, rej) =>
      setTimeout(() => rej(new Error("board fold timed out")), timeoutMs),
    ),
  ]);
}

// The issues of a board fold, in the shape that `toIssue` makes, and the
// colors of its labels.
export function readFold<T>(
  board: DaemonReplies["board.list"]["board"],
  toIssue: (row: BoardRow) => T,
) {
  const b = board || {};
  return { issues: (b.issues || []).map(toIssue), colors: labelColors(b.labels) };
}

// The repo's label colors by label name. A Map, so a label named
// `constructor` reads as missing. A blank color is skipped: a bare "#" is
// truthy and masks `labelColor`'s fallback.
function labelColors(labels: BoardLabel[] | undefined) {
  const colors = new Map<string, string>();
  for (const l of labels || []) {
    if (!l.color) continue;
    colors.set(l.name, "#" + String(l.color).replace(/^#/, ""));
  }
  return colors;
}

// The issues that block a fold row, under either name the fold uses.
function rowBlockers(row: BoardRow) {
  return row.blocked_by || row.blockedBy || [];
}

// A CLI fold row → the issue shape `wb-kanban.ts` expects. Body + comments
// are absent from the fold (`issue.show` fills them on open).
export function rowToIssue(row: BoardRow) {
  return {
    number: row.number,
    title: row.title || "",
    state: row.state || "open",
    reason: row.reason ?? row.state_reason ?? null,
    labels: row.labels || [],
    assignees: row.assignees || [],
    blockedBy: rowBlockers(row),
    created: row.created || "",
    updated: row.updated || "",
    body: "",
    comments: [],
  };
}

// The issue of an `issue.show` reply, or null when the reply carries none.
export function shownIssue(reply: ReplyOf<"issue.show">) {
  if (reply.status !== "ok" || !reply.issue || typeof reply.issue !== "object") {
    return null;
  }
  return reply.issue;
}
