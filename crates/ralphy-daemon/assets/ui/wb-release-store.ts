/* ---------------------------------------------------------------------------
   The release watch of this page: the Alpine store `release` (#621; ADR-0073
   D6 and the amendment of 2026-10-08, decisions 3 and 6).

   The store is the one owner of the release view the daemon computed, whether
   the operator dismissed it, and whether the last read is current. Its fields
   change only through its own methods. The account menu (wb-account.ts) and
   the About, What's new and update dialogs (wb-release-dialogs.ts) read it
   through the `$store` magic, and the dialogs change it only through
   `markReleaseSeen` and `releaseWatchChanged`.

   It reads no `shell()` member. Its `init()` reads the release view and hears
   `workbench:release-reread`, which the store `connection` sends when the tab
   comes back.

   `main.ts` registers it as `release`, after `checkouts`. `releaseStore`
   builds it without Alpine, so `node --test` can test it (ADR-0073 D3, D6).
   --------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import { WBRelease } from "./wb-release.ts";
import type { Messages } from "./wb-messages.ts";
import type { ReadState } from "./wb-shell-types.d.ts";

/** What `main.ts` gives the store. */
export type ReleaseStoreDeps = {
  /** The door for operator messages. */
  messages: Messages;
};

// `deps` is the shape every store takes; the release watch tells the operator
// nothing itself (the rail dot and the menu row are markup).
export function releaseStore(_deps: ReleaseStoreDeps) {
  return {
    // The release view the daemon computed (ADR-0056 §7), seeded empty.
    release: (WBRelease && WBRelease.EMPTY) || {
      current: "",
      channel: "rc",
      standing: "unknown",
      severity: "none",
      latest: null,
      gap: [],
      disabled: false,
      can_update: false,
    },
    // Dismissed by opening the panel — except urgent news.
    releaseSeen: false,

    // Alpine calls this once, when `main.ts` registers the store.
    init() {
      // Read at load and again on every return to the tab (below): the daemon
      // polls releases on its own six-hour clock, and a tab left open for days
      // would otherwise never show what that poll found.
      void this.loadRelease();
      window.addEventListener("workbench:release-reread", () => void this.loadRelease());
    },

    get releaseHasNews() {
      return !!WBRelease && WBRelease.hasNews(this.release);
    },
    // What the rail draws: an urgent release ignores the dismissal.
    get releaseUnread() {
      if (!this.releaseHasNews) return false;
      return !this.releaseSeen || WBRelease.isSticky(this.release);
    },
    get releaseSummary() {
      return WBRelease ? WBRelease.gapSummary(this.release) : "";
    },

    releaseRead: null as ReadState,
    releaseStale() {
      return WBFail.notCurrent(this.releaseRead, (ms) => window.Alpine.store("connection").fmtClock(ms));
    },
    async loadRelease() {
      if (!WBRelease) return;
      const view = await WBRelease.read();
      // A failed read keeps what the page last knew (ADR-0056 §6), marked
      // not current (ADR-0070 D3).
      this.releaseRead = WBFail.readFold(this.releaseRead, {
        ok: !!view,
        value: true,
        reason: "the daemon did not answer",
        at: Date.now(),
      });
      if (!view) return;
      // A dismissal is for the release that was shown. A newer one is news again.
      if (view.latest !== this.release.latest) this.releaseSeen = false;
      this.release = view;
    },
    // The What's new dialog was opened: the release it shows is dismissed.
    markReleaseSeen() {
      this.releaseSeen = true;
    },
    // The operator turned the release watch on or off.
    releaseWatchChanged(enable: boolean) {
      this.release = { ...this.release, disabled: !enable };
    },
  };
}

/** The Alpine store `release`, as `$store.release` gives it. */
export type ReleaseStore = ReturnType<typeof releaseStore>;
