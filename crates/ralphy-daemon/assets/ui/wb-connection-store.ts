/* ---------------------------------------------------------------------------
   The connection of this page to the daemon: the Alpine store `connection`
   (#621; ADR-0073 D6 and the amendment of 2026-10-08, decisions 3 and 6).

   The store is the one owner of the session (whether this browser may read),
   the security fact and log off, the presence socket and its heartbeat, the uptime, the daemon's name and
   mark, the page build and the build skew, whether the tab is hidden, and
   the write locks. Its fields change only through its own methods.
   `shell()` code reads `this.$store.connection`, the markup reads
   `$store.connection`, a component reads it through the `$store` magic, and
   code outside Alpine reads `Alpine.store("connection")`.

   It reads no `shell()` member and calls no component; it reads the stores
   `projects` and `runs`. It tells the page these facts with an event on
   `window`, and the page decides what each one means for it:
   - `workbench:auth-read`: the daemon said whether this browser may read,
     and its policy;
   - `workbench:log-off`: the operator logs off;
   - `workbench:repos-reread`, `workbench:sessions-reread`,
     `workbench:desk-reread`, `workbench:release-reread`,
     `workbench:attention-reread` and `workbench:logins-reread`: a push, a
     reopened socket or the tab coming back asks to read that fact again;
   - `workbench:board-refresh`, `workbench:panels-reread` and
     `workbench:sockets-resume`.
   The Changes view (wb-changes-panel.ts) tells it which change sets are not
   current (`changesCurrent`), so the write lock asks no component.

   `main.ts` registers it as `connection`, after `projects`. `connectionStore`
   builds it without Alpine, so `node --test` can test it (ADR-0073 D3, D6).
   --------------------------------------------------------------------------- */
import { apiFetch } from "./wb-api.ts";
import { WBChanges } from "./wb-changes.ts";
import { WBDeskSink } from "./wb-desk-sink.ts";
import { WBFail } from "./wb-fail.ts";
import { WBRun } from "./wb-runs.ts";
import { sendWindow } from "./wb-events.ts";
import type { Messages } from "./wb-messages.ts";
import type { SecurityFact, Subscription, Timer } from "./wb-shell-types.d.ts";

/** The detached file windows, as `wire` (app.ts) keeps them. */
export type DetachedWindows = { dirty: () => boolean; close: () => void };

/** What `main.ts` gives the store. */
export type ConnectionStoreDeps = {
  /** The door for operator messages. */
  messages: Messages;
  /** The detached file windows: an unsaved edit in one holds the build reload. */
  detached: DetachedWindows;
};

// Whether this tab holds work that only it can save. A commit draft is not
// counted: the skew lock refuses the commit, so it is work the tab could never
// save. A detached file window is: it saves through this tab, and a reloaded
// tab no longer hears it.
function unsavedWork(detached: DetachedWindows) {
  return !!(window.WBViewer?.anyDirty?.() || window.WBNotes?.anyDirty?.() || detached.dirty());
}

// Hold the desk changes of this tab, or let them go out.
function holdDesk(on: boolean) {
  WBDeskSink?.setHold?.(on);
}

// `deps` is the shape every store takes; the connection tells the operator
// nothing itself (the notice and the login gate are markup).
export function connectionStore(deps: ConnectionStoreDeps) {
  const { detached } = deps;
  return {
    // The build this page was served with (`<meta name="ralphy-build">`);
    // "" with no such tag, and then the page never reloads for a build.
    pageBuild: document.querySelector<HTMLMetaElement>('meta[name="ralphy-build"]')?.content || "",
    // Live presence + identity (#204): uptime from the `/ws` heartbeat,
    // name/avatar from `/api/identity`.
    uptimeText: "",
    identityName: "",
    identityAvatar: "",
    _lastHeartbeat: 0,
    // Staleness is DERIVED on a clock (`_clockTick`), not in the binding: the
    // event that makes the daemon stale — ticks STOPPING — never re-renders a
    // binding on `_lastHeartbeat`. Writing the same boolean is inert under
    // Alpine, so the steady state costs one comparison a second.
    presenceStale: false,
    _presenceSub: null as Subscription | null, // the `/ws` heartbeat subscription, kept so a resume can re-open it
    _clockTick: null as Timer,
    LIVE_SETTLE_MS: 250,
    _liveTimer: null as ReturnType<typeof setTimeout> | null,

    // Alpine calls this once, when `main.ts` registers the store. `main.ts`
    // builds `WBDaemon` and `WBConsole` first.
    init() {
      void this.probeSession();
      this.subscribePresence();
      void this.loadIdentity();
      // The board's two time-driven refresh triggers (#301), registered ONCE;
      // the predicate (wb-kanban.ts) decides.
      document.addEventListener("visibilitychange", () => {
        if (document.visibilityState === "visible") this.onTabVisible();
      });
      // A tablet resumes on a different link; its sockets died without a close.
      window.addEventListener("online", () => this.resumeSockets(true));
      // The console module cannot know whether THIS document's connection is
      // alive; hand it the heartbeat verdict, or its hidden-time fallback
      // resets every console after a minute on another tab.
      window.WBConsole?.setStaleProbe?.(() => this.socketsAreStale());
      // The presence half of the one-second clock; the phase clock's half is
      // the shell's (app.ts).
      this._clockTick = setInterval(() => {
        // Unconditional: the flag must already be true when the menu opens.
        const was = this.presenceStale;
        this.presenceStale = this.socketsAreStale();
        if (!was && this.presenceStale) this.onHeartbeatLost();
      }, 1000);
    },

    // The daemon's mark, in ONE place (rail puck, account menu, About card). The
    // fallback is a picture too: a blank 26px circle reads as a failed load.
    identityMark() {
      return this.identityAvatar || "🤖";
    },
    // Name + avatar for the account menu. A 404 (un-baptized) or a thrown
    // fetch leaves the fields empty.
    async loadIdentity() {
      try {
        const r = await apiFetch("GET /api/identity");
        if (r.ok) {
          const id = await r.json();
          this.identityName = id.name || "";
          this.identityAvatar = id.avatar || "";
        }
      } catch {}
    },

    // Three missed ~2s heartbeats: this document's connection is gone. The
    // shell's whole staleness signal — a suspended tablet runs no JS, so on
    // return the stamp is old; a desktop tab switch leaves it fresh.
    socketsAreStale() {
      return !this._lastHeartbeat || Date.now() - this._lastHeartbeat > 6000;
    },

    // The heartbeat stopped while the page stays visible: a desktop that slept
    // with the tab in front may fire neither `visibilitychange` nor `online`,
    // and its sockets read OPEN with no byte to come (#751). One resume per
    // loss, so a daemon that is down is not dialled every second. A page that
    // never had a heartbeat lost nothing; a hidden one resumes on its return.
    onHeartbeatLost() {
      if (!this._lastHeartbeat || this.tabHidden()) return;
      this.resumeSockets(true);
      window.WBConsole?.resumeAll?.(true);
    },

    // Bring the long-lived subscriptions back after a suspend. Presence, the
    // tree, the runs and the changes share the page's one socket, so one
    // resume of it brings them all back; it decides for itself
    // (`resumeDecision`) and debounces.
    resumeSockets(stale?: boolean) {
      const verdict = stale === undefined ? this.socketsAreStale() : stale;
      this._presenceSub?.resume?.(verdict);
      // The files and the Changes view hear it (wb-files.ts,
      // wb-changes-panel.ts). Their resume is the page socket's too, and its
      // debounce makes the three calls one.
      sendWindow(window, "workbench:sockets-resume", { stale: verdict });
    },

    // The `/ws` presence heartbeat (daemon mode). Each tick stamps
    // `_lastHeartbeat`, refreshes uptime and carries name/avatar once
    // baptized. It reads nothing: the shown facts are read again on the
    // daemon's pushes, which ride the same socket (ADR-0070 D2).
    subscribePresence() {
      if (!window.WBDaemon?.subscribePresence) return;
      this._presenceSub = window.WBDaemon.subscribePresence(
        (p) => {
          this._lastHeartbeat = Date.now();
          this.uptimeText = "Running for " + this.fmtUptime(p.uptime_secs);
          if (p.name) this.identityName = p.name;
          if (p.avatar) this.identityAvatar = p.avatar;
          if (p.build && this.pageBuild && p.build !== this.pageBuild) this.onBuildSkew();
        },
        {
          onPush: (verb, payload) => this.onPresencePush(verb, payload),
          onOpen: (reopened) => this.onPresenceOpen(reopened),
        },
      );
    },

    // A hidden tab reads no shown fact; `onTabVisible` reads them all again
    // (ADR-0070 D2), so a push dropped here is not lost.
    tabHidden() {
      return document.visibilityState === "hidden";
    },

    // A push from the daemon for a fact it owns (ADR-0070 D2 event 1).
    onPresencePush(verb: string, payload: PushPayload) {
      if (this.tabHidden()) return;
      if (verb === "sessions.dirty") {
        // A spawn and its first agent state arrive together: one read.
        clearTimeout(this._liveTimer);
        this._liveTimer = setTimeout(() => {
          if (this.tabHidden()) return;
          sendWindow(window, "workbench:sessions-reread");
          // The attention list follows the consoles' agent state.
          sendWindow(window, "workbench:attention-reread");
        }, this.LIVE_SETTLE_MS);
      } else if (verb === "desk.dirty") {
        // This tab's own write: it already holds the result.
        if (payload?.tab && payload.tab === WBDeskSink?.tabId?.()) return;
        sendWindow(window, "workbench:desk-reread");
      } else {
        this.onFactPush(verb);
      }
    },
    // A push that asks to read one fact again, with nothing to settle.
    onFactPush(verb: string) {
      if (verb === "repos.dirty" || verb === "peers.dirty") {
        // The change set and the branch have their own pushes.
        sendWindow(window, "workbench:repos-reread", { git: false });
      } else if (verb === "attention.dirty") {
        sendWindow(window, "workbench:attention-reread");
      } else if (verb === "logins.dirty") {
        sendWindow(window, "workbench:logins-reread");
      }
    },

    // The presence socket opened again: a push may have been lost while it
    // was down (ADR-0070 D2 event 2). The first open reads nothing: `init`
    // already did.
    onPresenceOpen(reopened: boolean) {
      if (!reopened || this.tabHidden()) return;
      sendWindow(window, "workbench:repos-reread", { git: true });
      sendWindow(window, "workbench:desk-reread");
      sendWindow(window, "workbench:attention-reread");
      sendWindow(window, "workbench:logins-reread");
      sendWindow(window, "workbench:board-refresh", { trigger: "reopen" });
      if (window.Alpine.store("projects").openSlug) void window.Alpine.store("runs").hydrateRuns();
    },

    // Seconds → a compact `1d 2h`, `2h 14m`, `5m`, `12s` uptime string.
    fmtUptime(secs: number | undefined) {
      const s = Math.max(0, Math.floor(secs || 0));
      const d = Math.floor(s / 86400);
      const h = Math.floor((s % 86400) / 3600);
      const m = Math.floor((s % 3600) / 60);
      if (d) return `${d}d ${h}h`;
      if (h) return `${h}h ${m}m`;
      if (m) return `${m}m`;
      return `${s}s`;
    },

    // --- login gate -------------------------------------------------------
    // An opaque overlay covers the shell while locked (`body.locked`).
    authed: true,

    // Ask the daemon whether this browser is authorized. A thrown fetch
    // keeps `authed` at its default.
    async probeSession() {
      try {
        const r = await apiFetch("GET /api/session");
        if (r.ok) {
          const s = await r.json();
          this.authed = s.authed;
          // The login card's mark: `/api/identity` is gated, so this pre-login
          // leg is the only source. `loadIdentity` overwrites it after login.
          if (s.avatar) this.identityAvatar = s.avatar;
          this.securityChanged({ policy: s.policy });
          // The login gate (wb-login.ts) and the restored view (app.ts) are
          // not the store's.
          sendWindow(window, "workbench:auth-read", {
            authed: s.authed,
            password: s.password,
            policy: s.policy,
          });
        }
      } catch {
        // No restore: `authed` is still at its `true` default, and a restore
        // against a dead daemon closes every tab whose read fails — and
        // `closeTab` persists.
      }
    },
    // The login gate opened or closed: a login the daemon accepted, or a
    // log-off.
    authChanged(authed: boolean) {
      this.authed = authed;
    },

    // --- security ---------------------------------------------------------
    // The daemon auth model (ADR-0032): an opt-in access token, an optional
    // password, and TOTP whose secret is shown exactly once.
    security: {
      tokenSet: true, // a networked daemon always has one; localhost needs none
      passwordSet: false,
      totpEnrolled: false,
      requireLogin: false, // opt-in: mimics a non-loopback bind with TOTP
      // Opt-in: markdown may load images from other websites (the CSP
      // `img-src https:`, ADR-0032 amendment §F).
      remoteImages: false,
      policy: "session", // overwritten by probeSession()
    },
    // The Security dialog (wb-security-dialog.ts) changes the security fact
    // only here (ADR-0073 D4). `patch` holds fields of `security`.
    securityChanged(patch: Partial<SecurityFact>) {
      Object.assign(this.security, patch);
    },

    // The account menu (wb-account.ts) and the login gate (wb-login.ts) hear
    // `workbench:log-off` and reset their own state.
    async logOff() {
      sendWindow(window, "workbench:log-off");
      // The session cookie is HttpOnly — only the server can clear it. The
      // route needs a live session (audit F5): a 401 here means the cookie
      // was already invalid, which is the same place this lands anyway.
      try {
        await fetch("/api/logout", { method: "POST" });
      } catch {}
      // Localhost/Bearer have no login gate to drop to (#205).
      if (this.security.policy === "session") {
        this.authChanged(false);
      }
      window.WB.emit("logoff");
    },

    // A time of day, `14:02`, for "Read at …" (`WBFail.clock`).
    fmtClock(ms: number) {
      return WBFail.clock(ms);
    },

    // The tab became visible (ADR-0070 D2 event 3): every open panel's facts
    // are read again, because a hidden tab read nothing. The project read
    // also reads the sessions, the fleet, the change set and the branch.
    onTabVisible() {
      sendWindow(window, "workbench:board-refresh", { trigger: "visible" });
      sendWindow(window, "workbench:repos-reread", { git: true });
      sendWindow(window, "workbench:desk-reread");
      sendWindow(window, "workbench:attention-reread");
      // Not only with the Runs panel open: the runs lock the writes.
      void window.Alpine.store("runs").hydrateRuns();
      this.rereadOpenPanels();
      this.resumeSockets();
      sendWindow(window, "workbench:release-reread");
    },
    // The panels whose facts have no push: read again on visible and after
    // login while they are open (fact index: settings 3, 4; usage 3, 4). The
    // Settings dialog hears the event and reads again only when it is open
    // (ADR-0073 D5).
    rereadOpenPanels() {
      sendWindow(window, "workbench:panels-reread");
    },

    // ---- write controls (#318) ------------------------------------------
    // Disabled from the open repo's LIVE RUN list (`runs.list`, ADR-0047 §9).
    // A HINT, not the authority: the CLI's `guard_run_lock` refuses
    // unconditionally, and a `ralphy triage` holding the lock writes no run
    // snapshot, so a click can still be refused while these look enabled.
    //
    // A write is also locked while the fact it acts on is not current
    // (ADR-0070 D3): a failed read after a good one, or a failed first read.
    // A fact never read yet is not "not current": the panel is loading.
    writeLocked() {
      return !!this.writeLockReason();
    },
    writeLockReason() {
      const slug = window.Alpine.store("projects").openKey();
      if (this.buildSkew) return this.BUILD_SKEW_LOCK;
      // The Changes view (wb-changes-panel.ts) says whether its reads are current.
      if (this.changesNotCurrent[slug]) {
        return "The changes shown are not current. Wait for the next read, or reload the page.";
      }
      if (window.Alpine.store("runs").runsRead[slug]?.current === false) {
        return "The runs shown are not current. Wait for the next read, or reload the page.";
      }
      return WBChanges.writeLockReason(window.Alpine.store("runs").runsByProject[slug]);
    },
    // Per project, true while the change set or the branch the Changes view
    // shows is not current. Only `changesCurrent` writes it.
    changesNotCurrent: {} as Record<string, boolean>,
    // The Changes view read the change set or the branch of `slug` again.
    changesCurrent(slug: string, current: boolean) {
      this.changesNotCurrent[slug] = !current;
    },
    BUILD_SKEW_LOCK:
      "This page is older than Ralphy. Save your work, and the page loads the new version.",
    // The daemon runs another build than this page (ADR-0070 D6). With no
    // unsaved work the tab reloads. With unsaved work it waits: the notice
    // stays, only saving that work is allowed, and each heartbeat asks again.
    // A console never holds the reload back: the daemon owns its PTY.
    onBuildSkew() {
      const unsaved = unsavedWork(detached);
      // A hidden tab waits: reloaded now, it would read every fact unseen.
      // The next heartbeat after it becomes visible asks again.
      if (!unsaved && !this.tabHidden()) {
        // The desk changes held back while the work was saved (a new note's
        // path among them) go out with the page's last write; the daemon
        // merges them per record.
        holdDesk(false);
        // A detached file window would outlive the reload with no tab that
        // hears its Save; closing it sends the file home as a tab.
        detached.close();
        window.location.reload();
        return;
      }
      // A hidden tab writes no desk until it reloads: its JavaScript may not
      // know the daemon's desk.
      if (!unsaved) {
        holdDesk(true);
        return;
      }
      if (!this.buildSkew) {
        this.buildSkew = true;
        holdDesk(true);
      }
    },
    // True while this tab runs an older build than the daemon and holds unsaved
    // work (ADR-0070 D6); set by `onBuildSkew`.
    buildSkew: false,
    // The run verbs reuse the Changes derivation LITERALLY (#331). CAVEAT:
    // `guard_run_lock` is called by changes/config/mutate/sync only; `ralphy
    // run`, `ralphy triage` and `push` do NOT refuse on a live lock (runlock.rs:
    // "a signal, never a mutex"), so for those this `disabled` is the only gate.
    verbLocked() {
      return this.writeLocked();
    },
    verbTitle(verb: string) {
      return WBRun.verbLockTitle(verb, this.writeLockReason());
    },
    // The flash after a no-arg verb is sent: `Triage requested.`
    verbRequestedText(verb: string) {
      return `${verb.charAt(0).toUpperCase()}${verb.slice(1)} requested.`;
    },
  };
}

/** The Alpine store `connection`, as `$store.connection` gives it. */
export type ConnectionStore = ReturnType<typeof connectionStore>;
