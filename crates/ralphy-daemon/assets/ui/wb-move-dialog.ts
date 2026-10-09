/* ---------------------------------------------------------------------------
   The move destination picker (#364): an Alpine component around its modal.

   The files send `workbench:move-open` with the full rel path of a row; the
   picker browses real directories one level at a time through `tree.list`,
   in the checkout the tree shows, and answers with `workbench:move-confirmed`
   (`from`, `to`), which the files carry out (`performMove`, wb-files.ts).

   It reaches `shell()` only through the names in its `uses` (ADR-0073 D4).
   `main.ts` registers it (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { WBFail } from "./wb-fail.ts";
import { isProtectedDir, parentRel } from "./wb-file-paths.ts";
import { sendWindow } from "./wb-events.ts";

export function wbMoveDialog() {
  // Every `shell()` member this component's code or markup reads or calls.
  return component(["checkoutOf", "scrim"], {
    // The move destination picker (#364): browses one level at a time through
    // `tree.list`. `from` is the FULL rel path; `dir` the browsed directory
    // ("" is the repo root).
    movePick: { open: false, from: "", isFolder: false, dir: "", entries: [], busy: false, error: "" } as any,
    _movePickSeq: 0,
    // The destination is PICKED, never typed: the picker browses real
    // directories through `tree.list`. On `workbench:move-open`, which the
    // files send with the full rel path of the row.
    openMove(from: any) {
      this.movePick = {
        open: true,
        from,
        dir: "",
        entries: [],
        busy: false,
        error: "",
      };
      this._movePickSeq = (this._movePickSeq || 0) + 1;
      this.movePickLoad(parentRel(from));
    },

    async movePickLoad(dir: any) {
      // Stamp the request: two quick clicks would settle out of order.
      const seq = (this._movePickSeq = (this._movePickSeq || 0) + 1);
      this.movePick.busy = true;
      this.movePick.error = "";
      // Same checkout as the tree the row came from (#406).
      const listing = await WBDaemon.observe(
        "tree.list",
        WBDaemon.withCheckout({ repo: this.$store.projects.openSlug, path: dir }, this.checkoutOf(this.$store.projects.openSlug)),
      ).catch(() => null);
      if (seq !== this._movePickSeq) return;
      this.movePick.busy = false;
      // A refused listing is a REASON, not an empty folder.
      if (!listing || WBFail.isError(listing) || !Array.isArray(listing.entries)) {
        this.movePick.entries = [];
        this.movePick.error = WBFail.failed(listing, "Could not list the folder: the daemon gave no reason.");
        return;
      }
      const from = this.movePick.from;
      this.movePick.dir = dir;
      this.movePick.entries = listing.entries
        .filter((e) => e.dir)
        // A folder cannot move into itself or its own subtree.
        .filter((e) => {
          const rel = dir ? `${dir}/${e.name}` : e.name;
          return rel !== from && !rel.startsWith(`${from}/`);
        })
        // Never a destination the Write path refuses (`fswrite::PROTECTED_DIRS`);
        // `tree` still LISTS `.ralphy` so the run artifacts stay watchable.
        .filter((e) => !isProtectedDir(e.name));
    },

    movePickInto(name: any) {
      this.movePickLoad(this.movePick.dir ? `${this.movePick.dir}/${name}` : name);
    },

    movePickUp() {
      if (!this.movePick.dir) return;
      this.movePickLoad(parentRel(this.movePick.dir));
    },

    movePickCancel() {
      this.movePick.open = false;
    },

    // "Move here" is dead while the browsed directory IS the source's parent
    // (a no-op the daemon reports as `exists`) and while the listing FAILED
    // (`dir` still names the last directory that loaded).
    movePickBlocked() {
      return (
        this.movePick.busy ||
        !!this.movePick.error ||
        this.movePick.dir === parentRel(this.movePick.from)
      );
    },

    movePickConfirm() {
      const from = this.movePick.from;
      const dir = this.movePick.dir;
      const leaf = from.slice(parentRel(from) ? parentRel(from).length + 1 : 0);
      // A no-op the daemon would report as a bare `exists`: name the reason.
      if (dir === parentRel(from)) {
        this.movePick.error = "it is already in that folder";
        return;
      }
      this.movePick.open = false;
      // The files move it (`performMove`, wb-files.ts).
      const to = dir ? `${dir}/${leaf}` : leaf;
      sendWindow(window, "workbench:move-confirmed", { from, to });
    },
  });
}
