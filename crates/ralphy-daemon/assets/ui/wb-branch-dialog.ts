/* ---------------------------------------------------------------------------
   The branch dialog and the checkout menu (#621): an Alpine component around
   the Switch branch modal.

   The branch chip of a project row sends `workbench:branch-open`; the
   checkout chips of the Files bar and of the Changes view send
   `workbench:checkout-menu`. The dialog switches and creates branches through
   the daemon's `branch.*` verbs, and the checkout menu picks a checkout or
   removes a worktree. A refused branch act is held in the store `checkouts`
   (`branchError`), and the files show it under the Files bar.

   The selected checkouts and the worktree listings are the Alpine store
   `checkouts` (wb-checkouts-store.ts). It reaches `shell()` only through the
   names in its `uses` (ADR-0073 D4). `main.ts` registers it (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { WBFail } from "./wb-fail.ts";
import { WBProject } from "./wb-project.ts";
import { WBSessionRoute } from "./wb-session-route.ts";
import { sendWindow } from "./wb-events.ts";
import type { CheckoutRow, Project } from "./wb-shell-types.d.ts";

export function wbBranchDialog() {
  // Every `shell()` member this component's code or markup reads or calls.
  return component(["scrim", "flash", "askConfirm"], {
    // The branch chip opens a filtered picker; switching or creating goes
    // through the daemon's `branch.*` verbs. The header reflects the pick
    // optimistically.
    branchOpen: false,
    branchModal: {
      slug: null as string | null,
      filter: "",
      branches: [] as string[],
      current: "",
      primaryBranch: "",
      dirty: false,
      checkoutDirty: false,
    },
    // A `worktree.remove` in flight, per repo ref: the chip's menu greys the
    // row and a second click is ignored until the re-read lands.
    worktreeRemoving: {} as Record<string, string>,

    init() {
      window.addEventListener("workbench:branch-open", (e) => this.openBranchModal(e.detail.project));
      window.addEventListener("workbench:checkout-menu", (e) => this.openCheckoutChip(e.detail.project, e.detail.anchor));
    },

    openBranchModal(p: Project) {
      if (!this.$store.checkouts.canSwitchBranch(p)) return;
      // Reaching for the picker IS the next branch act.
      this.$store.checkouts.branchRefused("");
      const ref = this.$store.projects.repoRef(p);
      // Under a selection "current" is the WORKTREE's branch (#407).
      const ck = this.$store.checkouts.checkoutOf(ref);
      const wt = ck ? (this.$store.checkouts.worktreeListings[ref]?.worktrees || []).find((w) => w && w.name === ck) : null;
      this.branchModal = {
        slug: ref,
        filter: "",
        branches: [...(p.branches || [p.branch])],
        current: wt ? wt.branch || "HEAD" : p.branch,
        primaryBranch: p.branch,
        dirty: !!p.dirty,
        // The dirty warning is about the tree the switch will hit (#407);
        // `dirty` stays the primary's for its picker row.
        checkoutDirty: wt ? wt.dirty === true : !!p.dirty,
      };
      this.branchOpen = true;
      this.loadBranches(ref);
      // The chip's dirty dot and the `current` seed above read the listing.
      this.$store.checkouts.ensureWorktreeListing(ref);
      this.$nextTick(() => {
        this.$refs.branchFilter?.focus();
      });
    },

    // The repo's real local branches via `branch.list` (#199). A failed read
    // empties the list (M5).
    async loadBranches(slug: string) {
      try {
        const reply = await window.WBDaemon.observe(
          "branch.list",
          window.WBDaemon.withCheckout({ repo: slug }, this.$store.checkouts.checkoutOf(slug)),
        );
        if (this.branchModal.slug !== slug) return; // modal moved on — leave it
        if (!reply || reply.status !== "ok") {
          this.branchModal.branches = [];
          this.flash("Could not load the branches.");
          return;
        }
        // The daemon nests the CLI's `{current, branches:[]}` JSON under the
        // `branches` field (lib.rs Query reply), same as `reply.board.*` /
        // `reply.issue.*` — read one level deeper, not the top level.
        const data = reply.branches || {};
        if (Array.isArray(data.branches)) this.branchModal.branches = data.branches;
        if (data.current) this.branchModal.current = data.current;
      } catch {
        if (this.branchModal.slug === slug) {
          this.branchModal.branches = [];
          this.flash("Could not load the branches.");
        }
      }
    },

    closeBranchModal() {
      this.branchOpen = false;
    },

    // Filtered (case-insensitive substring), current pinned to the top.
    branchList() {
      const q = this.branchModal.filter.trim().toLowerCase();
      const all = this.branchModal.branches;
      const hit = q ? all.filter((b) => b.toLowerCase().includes(q)) : all.slice();
      const cur = this.branchModal.current;
      return hit.sort((a, b) => (a === cur ? -1 : b === cur ? 1 : a.localeCompare(b)));
    },

    openCheckoutChip(p: Project, anchor: HTMLElement) {
      const ref = this.$store.projects.repoRef(p);
      const listing = this.$store.checkouts.worktreeListings[ref] || null;
      const mine = (this.$store.projects.liveSessions || []).filter((s) => WBSessionRoute.matchesRepo(s, ref));
      window.WBConsole.checkoutMenu({
        anchor,
        host: document.body,
        rows: window.WBConsole.checkoutMenuRows(listing, this.$store.checkouts.checkoutOf(ref), mine, p.branch, !!p.dirty),
        onPick: (row) => this.$store.checkouts.setCheckout(ref, row.primary ? null : row.name),
        onRemove: (row) => this.removeWorktree(ref, row),
      });
    },

    // The create row shows only when the typed name matches no existing branch.
    canCreateBranch() {
      const name = this.branchModal.filter.trim();
      if (!name) return false;
      return !this.branchModal.branches.some((b) => b.toLowerCase() === name.toLowerCase());
    },

    // Enter = act on the top match, else create the typed branch (quick-pick).
    branchEnter() {
      const list = this.branchList();
      if (list.length) this.switchBranch(list[0]);
      else if (this.canCreateBranch()) this.createBranch();
    },

    // Under a selected worktree (#407) the act lands on THAT tree's HEAD:
    // `p.branch` is the primary's and must not move, so no optimistic update —
    // the chip converges from `_mutateBranch`'s forced `worktree.list` re-read.
    switchBranch(name: string) {
      if (this.$store.connection.writeLocked()) {
        this.flash(this.$store.connection.writeLockReason());
        this.closeBranchModal();
        return;
      }
      if (name !== this.branchModal.current) {
        const slug = this.branchModal.slug;
        const checkout = this.$store.checkouts.checkoutOf(slug);
        const p = checkout ? null : this.$store.projects.projects.find((x) => this.$store.projects.repoRef(x) === slug);
        const prev = p ? p.branch : null;
        if (p) p.branch = name; // optimistic — the chip updates immediately
        window.WB.emit("branch-switch", { project: slug, branch: name, checkout });
        // The run-lock-aware `branch.switch` Mutate (#199): refusal → revert.
        this._mutateBranch("branch.switch", slug, name, () => {
          if (p) p.branch = prev!;
        });
      }
      this.closeBranchModal();
    },

    createBranch() {
      if (!this.canCreateBranch()) return;
      if (this.$store.connection.writeLocked()) {
        this.flash(this.$store.connection.writeLockReason());
        return;
      }
      const name = this.branchModal.filter.trim();
      const from = this.branchModal.current;
      const slug = this.branchModal.slug;
      const checkout = this.$store.checkouts.checkoutOf(slug);
      const p = checkout ? null : this.$store.projects.projects.find((x) => this.$store.projects.repoRef(x) === slug);
      const prevBranch = p ? p.branch : null;
      const prevBranches = p ? [...(p.branches || [])] : null;
      if (p) {
        p.branches = [...(p.branches || []), name];
        p.branch = name; // a fresh branch is checked out onto
      }
      window.WB.emit("branch-create", { project: slug, name, from, checkout });
      this._mutateBranch("branch.create", slug, name, () => {
        if (p) {
          p.branch = prevBranch!;
          p.branches = prevBranches!;
        }
      });
      this.closeBranchModal();
    },

    // The chip menu's trash action: `worktree.remove` (#409). The listing is
    // the truth on every path (a `branch kept` reply is an error whose
    // directory is gone), so the selection resets from the re-read
    // (`checkoutAfterListing`), never from the reply's status. A refusal lands
    // verbatim in a notice with one OK: the menu it came from has closed.
    // A cancel makes NO daemon call.
    async removeWorktree(slug: string, w: CheckoutRow) {
      if (!slug || !w || w.primary || this.worktreeRemoving[slug]) return;
      const ok = await this.askConfirm({
        title: `Delete worktree ${w.name}?`,
        message:
          "The worktree folder is deleted from the disk. " +
          "Its branch is deleted too, unless it has commits that are not on the base branch.",
        confirmLabel: "Delete",
        danger: true,
      });
      if (!ok || this.worktreeRemoving[slug]) return;
      this.worktreeRemoving = { ...this.worktreeRemoving, [slug]: w.name };
      const refused = (message: string) =>
        window.WBConsole.askNotice({ title: `Could not delete worktree ${w.name}`, message });
      try {
        const reply = await window.WBDaemon.observe("worktree.remove", { repo: slug, name: w.name });
        if (WBFail.isError(reply)) {
          refused(WBFail.cause(reply, "The daemon refused to delete the worktree."));
        } else {
          this.flash(`Worktree ${w.name} deleted`);
        }
      } catch {
        refused("Could not reach the daemon. Check whether the worktree was deleted.");
      } finally {
        await this.$store.checkouts.ensureWorktreeListing(slug, true);
        const ck = this.$store.checkouts.checkoutOf(slug);
        if (ck && WBProject.checkoutAfterListing(ck, this.$store.checkouts.worktreeListings[slug]) === null) {
          this.$store.checkouts.checkoutGone(slug, ck);
        }
        // Cleared last: no entry means the re-read landed too.
        const next = { ...this.worktreeRemoving };
        delete next[slug];
        this.worktreeRemoving = next;
      }
    },

    // Await a `branch.*` Mutate; on a `{status:"error"}` refusal (a held
    // run.lock, ADR-0036 §6) run `revert` and report the verbatim message under
    // the chip AND in the runs flash (the aside is closed by default). Carries
    // the selected checkout (#407): the worktree's HEAD moves, never the
    // primary's.
    async _mutateBranch(verb: string, slug: string | null, name: string, revert: () => void) {
      try {
        const reply = await window.WBDaemon.observe(
          verb,
          WBDaemon.withCheckout({ repo: slug, name }, this.$store.checkouts.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          revert();
          this._branchRefused(
            WBFail.failed(
              reply,
              verb === "branch.create"
                ? "Could not create the branch: the daemon gave no reason."
                : "Could not switch branch: the daemon gave no reason.",
            ),
          );
        }
      } catch {
        // A transport throw is NOT a refusal, so the optimistic update STAYS —
        // the verb may have landed.
        this._branchRefused("Could not reach the daemon. Check whether the branch changed.");
      } finally {
        // Re-read the listing on every path (an unconfirmed switch may have
        // landed); a moved HEAD also changes the working tree and sync row.
        this.$store.checkouts.ensureWorktreeListing(slug, true);
        if (slug) sendWindow(window, "workbench:changes-read", { ref: slug });
      }
    },
    // The Projects panel's counterpart to `_changesRefused` (wb-changes-panel.ts).
    _branchRefused(msg: string) {
      this.$store.checkouts.branchRefused(msg);
      this.flash(msg);
    },
  });
}
