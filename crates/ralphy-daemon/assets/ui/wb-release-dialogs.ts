/* ---------------------------------------------------------------------------
   The About, What's new and update dialogs: the product card from
   `/api/about`, the releases between this build and the newest (ADR-0056 §7),
   and the update the page asks for (ADR-0056 §11).

   `releaseDialogs` is the Alpine component `wbReleaseDialogs`. It reaches
   `shell()` only through the names in `uses` (ADR-0073 D4). The release fact
   (`release`, `releaseSeen`) stays in `shell()`, because the sidebar reads it:
   the dialogs read it, and change it only through `markReleaseSeen` and
   `releaseWatchChanged`. The account menu opens the dialogs with the
   `workbench:whats-new-open` and `workbench:about-open` events (ADR-0073 D5).

   `main.ts` registers it as `wbReleaseDialogs` (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { WBRelease } from "./wb-release.ts";

export function releaseDialogs() {
  // Every `shell()` member this component's code or markup reads or calls.
  // `loadComponent` in ui-tests/harness.mjs fails on any other name, and the
  // type check fails on a name the code reads.
  return component(["release", "releaseSummary", "releaseStale", "markReleaseSeen", "releaseWatchChanged", "refreshLive", "localSessions", "peerSessions", "fleetPeers", "projectLabel", "identityMark", "scrim"], {
    // --- about (read-only) ------------------------------------------------
    // The product card from `/api/about`; these defaults show until it answers.
    aboutOpen: false,
    about: {
      name: "ralphy",
      version: "",
      license: "GPL-3.0-or-later",
      repository: "https://github.com/paulocorcino/ralphy",
      creator: "Paulo Corcino",
      error: "",
    },
    releaseCmdCopied: false,
    whatsNewOpen: false,
    // The update the page asks for (ADR-0056 §11). `phase` goes idle →
    // confirm → running → restarting, or to error. `consoles` are the ones that
    // close with the daemon; `peers` are the WSL peers updated after it;
    // `needCode` is a live TOTP seed.
    relUpdate: {
      phase: "idle",
      code: "",
      needCode: false,
      consoles: [] as string[],
      peers: [] as (string | undefined)[],
      peerConsoles: 0,
      error: "",
    },
    // `ralphy update` in a terminal restarts only this daemon, so only its own
    // consoles close.
    get releaseConsoleWarning() {
      const n = this.localSessions().length;
      if (!n) return "";
      if (n === 1) return "1 console is open. The update closes it and stops the agent in it.";
      return n + " consoles are open. The update closes them and stops the agents in them.";
    },
    async copyReleaseCommand() {
      try {
        await navigator.clipboard.writeText("ralphy update");
      } catch (e) {
        // No clipboard off a secure origin; the command stays on screen to type.
        console.warn("copy ralphy update:", e);
        return;
      }
      this.releaseCmdCopied = true;
      setTimeout(() => (this.releaseCmdCopied = false), 2000);
    },
    openWhatsNew() {
      this.whatsNewOpen = true;
      this.markReleaseSeen();
      // The warning counts the consoles open now, not at the last poll.
      this.refreshLive();
    },
    closeWhatsNew() {
      this.whatsNewOpen = false;
      // A running update goes on without the panel; a question does not.
      if (this.relUpdate.phase === "confirm" || this.relUpdate.phase === "error") this.cancelUpdate();
    },
    async beginUpdate() {
      let needCode = false;
      try {
        const r = await fetch("/api/security/state");
        if (r.ok) needCode = (await r.json()).totp_enrolled === true;
      } catch (e) {
        // The daemon asks for the code anyway; the page then shows its refusal.
        console.warn("security state:", e);
      }
      // The list as it is now, not as the last poll left it.
      await this.refreshLive();
      const consoles = this.localSessions().map(
        (s) => s.name || (s.repo && s.repo !== "~" ? this.projectLabel(s.repo) : "home"),
      );
      // A peer that can be woken through `wsl.exe` is the one the update takes
      // after this daemon (ADR-0056 §11). Its consoles close only if it takes a
      // new version, so they are counted apart.
      const wsl = (this.fleetPeers || []).filter((p) => p.nudgeable);
      const peers = wsl.map((p) => p.name || p.environment);
      const peerConsoles = wsl.reduce((n, p) => n + this.peerSessions(p.daemon_id).length, 0);
      this.relUpdate = { phase: "confirm", code: "", needCode, consoles, peers, peerConsoles, error: "" };
    },
    // The line about the WSL peers in the update question.
    get updatePeerText() {
      const u = this.relUpdate;
      if (!u.peers || !u.peers.length) return "";
      const names = u.peers.join(", ");
      const n = u.peerConsoles || 0;
      if (!n) return `Then Ralphy updates the WSL copy too: ${names}.`;
      const consoles = n === 1 ? "its 1 console closes" : `its ${n} consoles close`;
      return `Then Ralphy updates the WSL copy too: ${names}. If it takes a new version, ${consoles} as well.`;
    },
    cancelUpdate() {
      this.relUpdate = { phase: "idle", code: "", needCode: false, consoles: [], peers: [], peerConsoles: 0, error: "" };
    },
    async confirmUpdate() {
      const u = this.relUpdate;
      const code = u.code.trim();
      if (u.needCode && code.length !== 6) return;
      this.relUpdate = { ...u, phase: "running", error: "" };
      let result;
      try {
        result = await WBRelease.update(u.needCode ? code : "");
      } catch (e) {
        result = { ok: false, status: 0, message: "" };
      }
      if (!result.ok) {
        this.relUpdate = { ...u, phase: "confirm", code: "", error: this.updateRefusal(result) };
        return;
      }
      this.relUpdate = { ...u, phase: "restarting", code: "" };
      this.awaitNewBuild(this.release.current);
    },
    // The line under the question when the daemon refused or the update failed.
    updateRefusal(result: any) {
      if (result.status === 401) return "Code rejected. Enter the current code from your authenticator app.";
      if (result.status === 429) {
        return `Too many attempts. Wait ${result.retryAfter || "a few"} seconds and try again.`;
      }
      if (result.status === 0) return "Could not reach Ralphy. Try again.";
      return result.message || `The update did not start (${result.status}).`;
    },
    // Read the release view until a different build answers, then load the
    // page again: the new build brings its own workbench. The same build
    // after a gap means the new one did not start and the old one is back.
    async awaitNewBuild(from: string, pause = 2000) {
      const deadline = Date.now() + 120000;
      let sawGap = false;
      while (Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, pause));
        const view = await WBRelease.read();
        if (!view) {
          sawGap = true;
          continue;
        }
        if (view.current !== from) {
          window.location.reload();
          return;
        }
        if (sawGap) {
          this.relUpdate = {
            ...this.relUpdate,
            phase: "error",
            error: "The new version did not start, so the previous version runs again. The file .ralphy/update.log in your home folder says why.",
          };
          return;
        }
      }
      this.relUpdate = {
        ...this.relUpdate,
        phase: "error",
        error: "Ralphy did not come back in two minutes. The file .ralphy/update.log in your home folder says why.",
      };
    },
    async setReleaseWatch(enable: boolean) {
      if (!WBRelease) return;
      try {
        await WBRelease.setWatch(enable);
        this.releaseWatchChanged(enable);
      } catch (e) {
        // A preference: the next read reports what actually took.
        console.warn("release watch:", e);
      }
    },

    async openAbout() {
      this.aboutOpen = true;
      this.about.error = "";
      try {
        const r = await fetch("/api/about");
        if (r.ok) {
          const data = await r.json();
          // Merge onto the seed so any missing field keeps its fallback.
          this.about = { ...this.about, ...data, error: "" };
        } else {
          this.about.error = "Could not load the version details: the daemon did not answer.";
        }
      } catch {
        this.about.error = "Could not load the version details: the daemon did not answer.";
      }
    },
    closeAbout() {
      this.aboutOpen = false;
    },
    // The current year for the copyright line (client clock is fine here).
    aboutYear() {
      return new Date().getFullYear();
    },
  });
}

// `whatsNewFlag` is the path of the What's new open flag that the dialog gives
// to `scrim()`. Code outside the component asks the modal stack with it
// (`modalOpen`), and never reads the flag (ADR-0073 D5).
export const WBReleaseDialogs = { whatsNewFlag: "whatsNewOpen" };
