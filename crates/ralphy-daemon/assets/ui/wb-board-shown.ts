/* ---------------------------------------------------------------------------
   What the board's markup shows, as the bodies of members of `wbBoard`
   (wb-board.ts), which takes the component as `board`. It also holds the
   label toggle and the plan discard, the writes of the board.
   --------------------------------------------------------------------------- */
import type { AlpineMagics } from "./wb-alpine.ts";
import { WBFail } from "./wb-fail.ts";
import type { BoardIssue, ReadyPlan } from "./wb-shell-types.d.ts";

/** What these members read on the board component. */
type BoardView = {
  $store: AlpineMagics["$store"];
  boardRefreshing: boolean;
  projectIssues(): BoardIssue[];
  selectedIssue(): BoardIssue | null;
  closeLabel(issue: BoardIssue): string;
  labelColor(label: string): string;
  labelInk(label: string): string;
  labelsLocked(): boolean;
  hasLabel(issue: BoardIssue | null, label: string): boolean;
  maybeRefreshBoard(trigger: string): void;
  openPlan(): ReadyPlan | null;
  closePlanModal(): void;
  loadPlan(slug: string | null): Promise<void>;
};

function openSlugOf(board: BoardView) {
  return board.$store.projects.openSlug ?? "";
}

export function scopeLabel(board: BoardView) {
  const slug = board.$store.projects.openSlug;
  return slug !== null && slug !== ""
    ? board.$store.projects.projectLabel(slug)
    : "No project open";
}

/** The note that the first read of an open project has not landed. */
export function refreshNoteShown(board: BoardView) {
  return openSlugOf(board) !== "" && board.boardRefreshing && board.projectIssues().length === 0;
}

/** The note that an open project has no issues: no read is out and none failed. */
export function emptyNoteShown(board: BoardView) {
  const slug = openSlugOf(board);
  const failed = (board.$store.board.boardError[slug] ?? "") !== "";
  return slug !== "" && !board.boardRefreshing && !failed && board.projectIssues().length === 0;
}

export function labelStyle(board: BoardView, label: string) {
  return `background:${board.labelColor(label)};color:${board.labelInk(label)}`;
}

export function issueStateClass(board: BoardView) {
  const iss = board.selectedIssue();
  if (iss === null || iss.state !== "closed") return "open";
  const reason = iss.reason ?? "";
  return reason === "" ? "completed" : reason;
}

export function issueStateIcon(board: BoardView) {
  const iss = board.selectedIssue();
  if (iss === null || iss.state !== "closed") return "bi-record-circle";
  return iss.reason === "not_planned" ? "bi-slash-circle" : "bi-check-circle-fill";
}

export function issueStateText(board: BoardView) {
  const iss = board.selectedIssue();
  return iss !== null && iss.state === "closed" ? board.closeLabel(iss) : "Open";
}

export function assigneeText(board: BoardView) {
  const names = (board.selectedIssue()?.assignees ?? []).join(", ");
  return names === "" ? "—" : names;
}

export function commentCountText(board: BoardView) {
  const n = board.selectedIssue()?.comments.length ?? 0;
  return `${n} comment${n === 1 ? "" : "s"}`;
}

/** Toggles one label of the issue at once, then writes it; a refusal reverts it. */
export function toggleIssueLabel(board: BoardView, iss: BoardIssue | null, label: string) {
  if (!iss) return;
  // Defence in depth: a `:disabled` button is still reachable by keyboard
  // in some browsers.
  if (board.labelsLocked()) return;
  const has = board.hasLabel(iss, label);
  const op = has ? "remove" : "add";
  const prev = [...iss.labels];
  const slug = board.$store.projects.openSlug;
  if (slug === null || slug === "") return;
  board.$store.board.issueLabelsSet(
    slug,
    iss.number,
    has ? prev.filter((l) => l !== label) : [...prev, label],
  );
  window.WB.emit("issue-label-change", { project: slug, number: iss.number, label, op });
  // The run-lock-aware `label.set` Mutate (#199): refusal → revert + flash.
  void (async () => {
    try {
      const reply = await window.WBDaemon.observe("label.set", {
        repo: slug,
        number: iss.number,
        label,
        op,
      });
      if (WBFail.isError(reply)) {
        board.$store.board.issueLabelsSet(slug, iss.number, prev);
        board.$store.modals.flash(
          WBFail.failed(reply, "Could not change the labels: the daemon gave no reason."),
        );
        return; // a refused write changed nothing to re-read
      }
      // Re-fold so the column reflects the server, not the optimistic
      // edit (#301).
      board.maybeRefreshBoard("label");
    } catch {
      // No daemon reachable — leave the optimistic edit in place.
    }
  })();
}

export async function discardPlan(board: BoardView) {
  const held = board.openPlan();
  if (!held || board.$store.connection.writeLocked()) return;
  const ok = await board.$store.modals.askConfirm({
    title: "Discard this plan?",
    message: `Deletes the plan for #${held.summary.issue}. The next run plans it again.`,
    confirmLabel: "Discard",
    danger: true,
  });
  if (!ok) return;
  const slug = board.$store.projects.openSlug;
  try {
    const reply = await window.WBDaemon.write("plan.discard", { repo: slug });
    if (WBFail.isError(reply)) {
      board.$store.modals.flash(WBFail.failed(reply, "Could not discard the plan."));
      return;
    }
    board.$store.modals.flash(`Plan for #${held.summary.issue} discarded.`);
    board.closePlanModal();
  } catch {
    board.$store.modals.flash("Could not discard the plan: the daemon is not connected.");
  } finally {
    // Re-read on EVERY path: the panel shows what is on disk now.
    await board.loadPlan(slug);
  }
}
