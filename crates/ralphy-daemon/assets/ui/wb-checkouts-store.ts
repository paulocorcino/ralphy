/* ---------------------------------------------------------------------------
   The selected checkout of each project and its worktree listing: the Alpine
   store `checkouts` (#621; ADR-0073 D6 and the amendment of 2026-10-08,
   decisions 3 and 6).

   The store is the one owner of this fact. Its fields change only through
   its own methods. `shell()` code reads `this.$store.checkouts`, the markup
   reads `$store.checkouts`, a component reads it through the `$store` magic,
   and code outside Alpine reads `Alpine.store("checkouts")`.

   It reads no `shell()` member. Its `init()` hears `workbench:project-changed`
   for the branch refusal line. It tells the page these facts with an event on
   `window`, and the page decides what each one means for it:
   - `workbench:checkout-changed`: the open project's checkout changed;
   - `workbench:changes-read` and `workbench:changes-forget`: the change set
     of a ref must be read again, or forgotten.
   It also holds the change count of each project, which the Changes view
   reads and the project rows show.

   The worktree listing has this one holder. The console reads it by ref
   (`listingOf`), asks for its read (`ensureWorktreeListing`), and hears of a
   change through `WBConsole.ingestWorktrees(ref)`. No copy of the listing is
   pushed to it, so a worktree whose delete is in flight is absent from every
   reader.

   The branch dialog and the checkout menu are the component `wbBranchDialog`
   (wb-branch-dialog.ts). `main.ts` registers the store as `checkouts`, after
   `runs`. `checkoutsStore` builds it without Alpine, so `node --test` can test
   it (ADR-0073 D3, D6).
   --------------------------------------------------------------------------- */
import { WBProject } from "./wb-project.ts";
import { sendWindow } from "./wb-events.ts";
import type { Messages } from "./wb-messages.ts";
import type { Listing, Project } from "./wb-project.ts";

/** What `main.ts` gives the store. */
export type CheckoutsStoreDeps = {
  /** The door for operator messages. */
  messages: Messages;
};

export function checkoutsStore(deps: CheckoutsStoreDeps) {
  const { messages } = deps;
  // The `worktree.list` read in flight per ref. Not in the returned object:
  // Alpine would wrap it.
  const reading = new Map<string, Promise<void>>();
  return {
    worktreeListings: {} as Record<string, Listing | null>,
    // The selected checkout per repo ref (#406, ADR-0063 §4): the REACTIVE copy
    // of `WBConsole`'s desk mirror (a closure variable there is invisible to
    // Alpine). `worktreeListings` is the last `worktree.list` reply per ref.
    checkouts: {} as Record<string, string>,
    // The newest `worktree.list` read of each ref: a reply of an older one is
    // dropped. One counter per ref, so a read of another repo never drops it.
    _listingSeq: {} as Record<string, number>,
    // Worktrees per ref whose delete is in flight: every listing leaves them
    // out until their own delete settles, so the re-read of another delete
    // does not bring them back.
    _hidden: {} as Record<string, string[]>,
    // A refused `branch.switch`/`branch.create`, held until the next branch act
    // or a project switch. Not `treeError` (the tree is fine) and not
    // `changesError` (the chip lives in THIS panel). The branch dialog
    // (wb-branch-dialog.ts) sets it, and the files show it under the Files bar.
    branchError: "",
    // Working-tree change count per slug (#307), of the checkout the Changes
    // view reads. `null` until a load succeeds, so a failed read never reads
    // like a clean tree. The Changes view (wb-changes-panel.ts) reads it and
    // writes it through `changeCountRead`; the project rows show it.
    changesCount: {} as Record<string, number | null>,

    // Alpine calls this once, when `main.ts` registers the store. `main.ts`
    // builds `WBDaemon` and `WBConsole` first.
    init() {
      // The selected checkouts (#406): the ONE hook for `unknown checkout`, and
      // the copy of the desk mirror once the boot desk lands.
      window.WBDaemon?.onUnknownCheckout?.(
        (repo, name) => repo != null && this.checkoutGone(repo, name),
      );
      void window.WBConsole?.whenDeskLoaded?.().then(() => this.adoptDeskCheckouts());
      // A branch refusal names an act against the project that WAS open.
      window.addEventListener("workbench:project-changed", () => (this.branchError = ""));
    },

    // The Changes view read the change count of `slug`; `null` is a read
    // that found no count.
    changeCountRead(slug: string, count: number | null) {
      this.changesCount[slug] = count;
    },

    // The branch dialog's refusal line; "" clears it.
    branchRefused(msg: string) {
      this.branchError = msg || "";
    },

    // Only when the daemon can reach the repo on disk. NOT gated on `remote`:
    // a local-only repo still has branches.
    canSwitchBranch(p: Project) {
      return WBProject.canSwitchBranch(p);
    },

    // The checkout chip of a project row: which tree Files, Changes and
    // search read, and that a click chooses another.
    checkoutTitle(p: Project) {
      const name = this.checkoutOf(window.Alpine.store("projects").repoRef(p));
      return name
        ? `Files, changes and search show worktree “${name}”. Click to choose another one.`
        : "Files, changes and search show the primary tree. Click to choose a worktree.";
    },

    branchChipTitle(p: Project) {
      const ref = window.Alpine.store("projects").repoRef(p);
      return WBProject.branchChipTitle(p, this.checkoutOf(ref), this.worktreeListings[ref] || null);
    },
    chipIcon(p: Project) {
      return WBProject.chipIcon(this.checkoutOf(window.Alpine.store("projects").repoRef(p)));
    },
    chipDirty(p: Project) {
      const ref = window.Alpine.store("projects").repoRef(p);
      return WBProject.chipDirty(p, this.checkoutOf(ref), this.worktreeListings[ref] || null);
    },

    // The Files bar's checkout chip (ADR-0063 amendment 2026-09-16 b): shown
    // once the repo has a worktree; a pick sets the #406 selection (what
    // Files, Changes, diff and Find show), never where a console is launched.
    hasWorktrees(p: Project) {
      return WBProject.hasWorktrees(
        this.worktreeListings[window.Alpine.store("projects").repoRef(p)] || null,
      );
    },

    checkoutOf(ref: string | null | undefined) {
      return (ref != null && this.checkouts[ref]) || null;
    },
    chipLabel(p: Project) {
      const ref = window.Alpine.store("projects").repoRef(p);
      return WBProject.chipLabel(p, this.checkoutOf(ref), this.worktreeListings[ref] || null);
    },
    // The reactive map is REPLACED so Alpine sees it; persistence goes to the
    // desk mirror; the files remount an open tree built for another checkout.
    setCheckout(ref: string, name: string | null) {
      const next = { ...this.checkouts };
      if (name) next[ref] = String(name);
      else delete next[ref];
      this.checkouts = next;
      window.WBConsole?.setCheckout?.(ref, name || null);
      if (window.Alpine.store("projects").openSlug === ref)
        sendWindow(window, "workbench:checkout-changed");
      // The Changes panel and the sync row are the SELECTED checkout's (#407).
      // Another tree's change set is not this tree's "last value" (ADR-0070
      // D3): its reads start over.
      sendWindow(window, "workbench:changes-forget", { ref });
      if (window.Alpine.store("projects").openSlug === ref) {
        sendWindow(window, "workbench:changes-read", { ref });
      }
    },
    // The daemon answered `unknown checkout` for `name`: drop the selection —
    // unless it already moved on, in which case a late reply says nothing.
    checkoutGone(ref: string, name: string) {
      if (this.checkoutOf(ref) !== name) return;
      this.setCheckout(ref, null);
      messages.flash(`Worktree ${name} no longer exists. Showing the primary tree.`);
    },
    // The listing of `ref` as every reader sees it: without a worktree whose
    // delete is in flight. `null` until a read lands.
    listingOf(ref: string | null | undefined): Listing | null {
      return (ref && this.worktreeListings[ref]) || null;
    },
    // The chip needs the worktree's BRANCH, which only `worktree.list` knows:
    // one read per ref, `force` re-reads (after a branch act the chip
    // converges from this reply, #407). A read of a ref already in flight is
    // that read, unless forced. A forced re-read that fails DROPS the cached
    // entry rather than showing the pre-act branch. Newest read of a ref wins.
    ensureWorktreeListing(ref: string | null | undefined, force = false): Promise<void> {
      if (!ref || ref === "~" || (this.worktreeListings[ref] && !force)) return Promise.resolve();
      const open = reading.get(ref);
      if (open && !force) return open;
      const read: Promise<void> = this._readListing(ref, force).finally(() => {
        if (reading.get(ref) === read) reading.delete(ref);
      });
      reading.set(ref, read);
      return read;
    },
    async _readListing(ref: string, force: boolean): Promise<void> {
      const seq = (this._listingSeq[ref] = (this._listingSeq[ref] || 0) + 1);
      const listing = await readWorktreeListing(ref);
      if (seq !== this._listingSeq[ref]) return; // superseded → the newer read owns it
      if (listing || force) this._storeListing(ref, listing);
    },
    _storeListing(ref: string, listing: Listing | null) {
      const hidden = this._hidden[ref] || [];
      const shown =
        listing && hidden.length
          ? { ...listing, worktrees: listing.worktrees.filter((w) => !hidden.includes(w.name)) }
          : listing;
      this.worktreeListings = { ...this.worktreeListings, [ref]: shown };
      window.WBConsole?.ingestWorktrees?.(ref);
    },
    // A delete of `name` started: the listing leaves it out at once.
    hideWorktree(ref: string, name: string) {
      this._hidden = { ...this._hidden, [ref]: [...(this._hidden[ref] || []), name] };
      const listing = this.worktreeListings[ref];
      if (listing) this._storeListing(ref, listing);
    },
    // The delete of `name` settled: the next listing shows it if it is there.
    unhideWorktree(ref: string, name: string) {
      this._hidden = {
        ...this._hidden,
        [ref]: (this._hidden[ref] || []).filter((n) => n !== name),
      };
    },
    // Copy the desk mirror's selections into the reactive map once the desk
    // has landed (boot, and again after a login under the `Session` policy),
    // and bring an already-open tree in line with what it now says.
    adoptDeskCheckouts() {
      const open = window.Alpine.store("projects").openSlug;
      const before = open ? this.checkoutOf(open) : null;
      this.checkouts = window.WBConsole?.checkouts?.() || {};
      if (open) sendWindow(window, "workbench:checkout-changed");
      if (open) {
        void this.ensureWorktreeListing(open);
        // The desk can land AFTER the open's own reads: re-read under the
        // restored selection, only when it differs (two git spawns otherwise).
        if (this.checkoutOf(open) !== before) {
          sendWindow(window, "workbench:changes-read", { ref: open });
        }
      }
    },
  };
}

export type CheckoutsStore = ReturnType<typeof checkoutsStore>;

// The worktree listing of `ref` from `worktree.list`, or null when the read
// fails or the reply has none.
async function readWorktreeListing(ref: string): Promise<Listing | null> {
  try {
    const reply = await window.WBDaemon.observe("worktree.list", { repo: ref });
    if (reply && reply.status === "ok") return reply.checkouts || null;
  } catch {
    // A transport error reads as no listing, as a refusal does.
  }
  return null;
}
