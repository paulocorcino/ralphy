/* ---------------------------------------------------------------------------
   The detail of the open issue of the board (`loadIssueDetail` of
   wb-board.ts): the `issue.show` read, and the rule that only the newest read
   of the issue that is still open is applied.

   The step takes the board component as `board`.
   --------------------------------------------------------------------------- */
import type { AlpineMagics } from "./wb-alpine.ts";
import { shownIssue } from "./wb-board-read.ts";
import { WBFail } from "./wb-fail.ts";

/** What the detail read reads or writes on the board component. */
type DetailBoard = {
  $store: AlpineMagics["$store"];
  issueError: string | null;
  issueLoading: boolean;
  kanbanSel: number | null;
  _issueDetailGen: number | null;
};

export async function loadIssueDetail(board: DetailBoard, number: number) {
  const slug = board.$store.projects.openSlug;
  board.issueError = null;
  // Set BEFORE the first await so the markup never paints `_(empty)_` for
  // a body still on the wire; cleared only by the NEWEST fetch.
  board.issueLoading = true;
  // INVARIANT (#302): a reply — content OR error — is applied only by the
  // NEWEST fetch, and only while its project+issue is still the open
  // drawer. The number alone is not enough: the board fold re-fires this
  // on every refresh, and two projects routinely carry the same number.
  const gen = (board._issueDetailGen = (board._issueDetailGen || 0) + 1);
  const stale = () =>
    gen !== board._issueDetailGen ||
    board.$store.projects.openSlug !== slug ||
    board.kanbanSel !== number;
  const fail = (msg: string) => {
    if (stale()) return;
    board.issueError = msg;
    board.$store.modals.flash(msg);
  };
  try {
    const reply = await window.WBDaemon.observe("issue.show", { repo: slug, number });
    if (WBFail.isError(reply)) {
      fail(WBFail.failed(reply, "Could not load the issue."));
      return;
    }
    const detail = shownIssue(reply);
    if (!detail) {
      fail("Could not load the issue.");
      return;
    }
    if (stale() || !slug) return;
    if (!board.$store.board.issueDetailRead(slug, number, detail)) return;
    // Success owns the banner too: this one may land second.
    board.issueError = null;
  } catch {
    // Transport error: the drawer says so rather than reading as empty.
    fail("Could not load the issue.");
  } finally {
    if (!stale()) board.issueLoading = false;
  }
}
