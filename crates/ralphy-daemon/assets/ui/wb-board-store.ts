/* ---------------------------------------------------------------------------
   The board's issues of each project: the Alpine store `board` (#691;
   ADR-0073 D6 and the amendment of 2026-10-08, decisions 3 and 6).

   The store is the one owner of the board's issues and labels, the last
   failure of their read, and the read state. Its fields change only through
   its own methods. The board (wb-board.ts) reads `board.list` and
   `issue.show` and gives each reply to a method here; the markup reads
   `$store.board`, a component reads it through the `$store` magic, and code
   outside Alpine reads `Alpine.store("board")`.

   It reads no `shell()` member and hears no event.

   `main.ts` registers it as `board`, after `runs`. `boardStore` builds it
   without Alpine, so `node --test` can test it (ADR-0073 D3, D6).
   --------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import type { Messages } from "./wb-messages.ts";
import type { IssueDetail as Detail } from "./wb-kanban.ts";
import type { BoardIssue, Read } from "./wb-shell-types.d.ts";

/** What `main.ts` gives the store. */
export type BoardStoreDeps = {
  /** The door for operator messages. */
  messages: Messages;
};

/** The fields of an `issue.show` reply that the board keeps. */
export type IssueDetail = Pick<Detail, "body" | "comments" | "blocked_by">;

// `deps` is the shape every store takes; the board tells the operator nothing
// itself, because the board component flashes each failure.
export function boardStore(_deps: BoardStoreDeps) {
  return {
    // Fed by `board.list` (#198): rows adapted to the issue shape, and the
    // repo's name→color label map. Empty until a read resolves.
    boardIssues: {} as Record<string, BoardIssue[]>,
    boardLabels: {} as Record<string, Map<string, string>>,
    // A `board.list` failure (#207): a broken tracker connection must never
    // read as "no work to do".
    boardError: {} as Record<string, string | null>,
    boardRead: {} as Record<string, Read>,

    // The open project's issues. Empty until a read of it resolves.
    openIssues() {
      return this.boardIssues[window.Alpine.store("projects").openKey()] || [];
    },

    // A good `board.list` of `slug`.
    boardReadOk(slug: string, issues: BoardIssue[], labels: Map<string, string>) {
      this.boardIssues[slug] = issues;
      this.boardLabels[slug] = labels;
      this.boardError[slug] = null;
      this.boardRead[slug] = WBFail.readFold(this.boardRead[slug], { ok: true, value: true, at: Date.now() });
    },

    // A failed `board.list` (ADR-0070 D3). After a good read the cards stay,
    // under a banner that says they are not current; before one, there are no
    // cards and the banner says why. Moving a card is locked meanwhile.
    boardReadFailed(slug: string, msg: string) {
      const read = WBFail.readFold(this.boardRead[slug], { ok: false, reason: msg, at: Date.now() });
      this.boardRead[slug] = read;
      if (read.goodAt) {
        this.boardError[slug] = WBFail.notCurrent(read, (ms) => window.Alpine.store("connection").fmtClock(ms));
        return;
      }
      this.boardIssues[slug] = [];
      this.boardError[slug] = msg;
    },

    // An `issue.show` reply: the fold rows carry no body or comments. Only a
    // field of the right type is taken. `false` when the row is not held.
    issueDetailRead(slug: string, number: number, detail: IssueDetail) {
      const iss = (this.boardIssues[slug] || []).find((i) => i.number === number);
      if (!iss) return false;
      if (typeof detail.body === "string") iss.body = detail.body;
      if (Array.isArray(detail.comments)) iss.comments = detail.comments;
      if (Array.isArray(detail.blocked_by)) iss.blockedBy = detail.blocked_by;
      return true;
    },

    // The labels of one issue: the board's edit, or its revert.
    issueLabelsSet(slug: string, number: number, labels: string[]) {
      const iss = (this.boardIssues[slug] || []).find((i) => i.number === number);
      if (iss) iss.labels = labels;
    },
  };
}

export type BoardStore = ReturnType<typeof boardStore>;
