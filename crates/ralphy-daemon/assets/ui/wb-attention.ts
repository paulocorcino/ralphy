/* ---------------------------------------------------------------------------
   The attention list: the requests that wait for the operator, as the daemon
   computed them (`GET /api/attention`, ADR-0085 D10, D13).

   `wbAttention` is the Alpine component of the rail button, its count and its
   panel. It holds the last list the daemon sent and whether the panel is
   open. It reads the list when it starts, and again on
   `workbench:attention-reread`, which the store `connection` sends on a
   `sessions.dirty` push, a reopened presence socket and the tab coming back,
   and on `workbench:logged-in`. The page computes no request of its own: the
   list is the daemon's, joined across peers by the daemon the page talks to.

   The count is shown on the rail and in the tab title. A row has one action:
   a console's row has "Go to the console", which shows the console the way
   the Go-to picker does (`revealWindow` in wb-consoles-tab.ts); a run's row
   has "Open the run", which shows the run in the Runs panel
   (`workbench:run-select`, heard by wb-runs-panel.ts). The store
   `connection` also sends `workbench:attention-reread` on an
   `attention.dirty` push.

   A request the daemon marks `dismissable` (an issue a finished run left for
   the operator, a console that finished a turn it was marked for) has a
   "Dismiss" button: `POST /api/attention/dismiss` hides it on every device,
   and the daemon of a peer's request stores it. A console's `console-done`
   request also ends when the operator opens that console: the focus moving
   into its window, or "Go to the console", dismisses it.

   "Notify me in this browser" is a switch of this device (the per-client
   view, `wb-client-view.ts`), off until the operator turns it on, which is when the browser is asked for
   permission. With it on, a request id this page did not know before shows
   one browser notification while the tab is hidden.

   `main.ts` registers it as `wbAttention`.
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { apiFetch } from "./wb-api.ts";
import { sendWindow } from "./wb-events.ts";
import { WBView } from "./wb-client-view.ts";
import { WBFleet } from "./wb-fleet.ts";
import { WBProject } from "./wb-project.ts";
import type { ConsoleWin } from "./wb-types.d.ts";

/** The kind of a request (`attention::Kind`). */
export type AttentionKind = "console-waiting" | "console-silent" | "console-done" | "run-waiting" | "run-finished";

/** What a row's one action opens: "none" for a finished run, which is no
 * longer in the Runs panel. */
export type AttentionAction = "console" | "run" | "none";

/** A request of `GET /api/attention` (`AttentionRequest` in `attention.rs`).
 * A peer's request has `id`, `console` and `repo` prefixed `<daemon_id>/`. */
export type AttentionRequest = {
  id: string;
  kind: AttentionKind;
  vendor: string;
  console?: string;
  repo?: string;
  run?: string;
  issue?: string;
  since: string;
  detail?: string;
  dismissable: boolean;
};

/** The reply of `GET /api/attention`. */
export type AttentionReply = { requests: AttentionRequest[] };

/** The tab title with no request open (`<title>` in `index.html`). */
export const PAGE_TITLE = "Ralphy";

/** The tab title for `count` open requests. */
export function attentionTitle(count: number) {
  return count > 0 ? `(${count}) ${PAGE_TITLE}` : PAGE_TITLE;
}

/** What a row says the request is. */
export function kindText(kind: AttentionKind) {
  switch (kind) {
    case "console-waiting":
    case "run-waiting":
      return "Waits for you";
    case "console-silent":
      return "No state from this agent";
    case "console-done":
      return "The agent is done";
    case "run-finished":
      return "A run stopped on this issue";
    default: {
      const unknown: never = kind;
      return unknown;
    }
  }
}

/** What the row of a request of `kind` opens. */
export function actionOf(kind: AttentionKind): AttentionAction {
  switch (kind) {
    case "console-waiting":
    case "console-silent":
    case "console-done":
      return "console";
    case "run-waiting":
      return "run";
    case "run-finished":
      return "none";
    default: {
      const unknown: never = kind;
      return unknown;
    }
  }
}

/** The saved choice; off when nothing is saved or the store cannot be read. */
function savedNotify() {
  return WBView.read()?.notify === true;
}

/** Keep the choice on this device. A store that cannot be written keeps the
 * switch working until the page is closed. */
function saveNotify(on: boolean) {
  WBView.patch({ notify: on });
}

function canNotify() {
  return typeof Notification !== "undefined";
}

/** Ask the browser for permission to notify, unless it already answered. */
async function askPermission() {
  if (!canNotify()) return "denied";
  if (Notification.permission !== "default") return Notification.permission;
  return Notification.requestPermission();
}

function postDismiss(id: string) {
  return apiFetch("POST /api/attention/dismiss", {
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ id }),
  });
}

/** How long ago `since` (RFC 3339) was, at `now` (ms); "" when it is not a time. */
export function ageText(since: string, now: number) {
  const at = Date.parse(since);
  if (Number.isNaN(at)) return "";
  const minutes = Math.floor(Math.max(0, now - at) / 60000);
  if (minutes < 1) return "Less than a minute";
  const count = (n: number, unit: string) => `${n} ${unit}${n === 1 ? "" : "s"}`;
  if (minutes < 60) return count(minutes, "minute");
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return count(hours, "hour");
  return count(Math.floor(hours / 24), "day");
}

/** The session a console request names: the number after the last `/` of
 * `console`, under the same repo ref (a peer's ref carries the peer). */
export function sessionOf(request: AttentionRequest, sessions: readonly HostedSession[] | null | undefined) {
  if (!request.console) return null;
  const id = Number(request.console.slice(request.console.lastIndexOf("/") + 1));
  return (sessions || []).find((s) => s.id === id && s.repo === request.repo) || null;
}

export function wbAttention() {
  // It reads no `shell()` member.
  return component([], {
    open: false,
    requests: [] as AttentionRequest[],
    // The clock the ages are read at: set at each read, and each minute.
    now: Date.now(),
    _seq: 0,
    // The switch of this device, and the ids the last list held (`null`
    // before the first list: what is on the page at load is not new).
    notifyOn: false,
    _known: null as Set<string> | null,
    _dismissing: new Set<string>(),

    init() {
      this.notifyOn = savedNotify() && canNotify() && Notification.permission === "granted";
      this.read();
      window.addEventListener("workbench:attention-reread", () => this.read());
      window.addEventListener("workbench:logged-in", () => this.read());
      window.addEventListener("workbench:menus-close", () => this.close());
      document.addEventListener("focusin", (e) => this.focused(e.target));
      window.setInterval(() => {
        this.now = Date.now();
      }, 60000);
    },

    count() {
      return this.requests.length;
    },

    // Reads close together can answer out of order: the newest owns the list.
    // A failed read keeps the last list.
    async read() {
      const seq = ++this._seq;
      try {
        const r = await apiFetch("GET /api/attention");
        if (!r.ok || seq !== this._seq) return;
        const reply = await r.json();
        if (seq !== this._seq) return;
        this.show(reply.requests);
      } catch {
        // A daemon that does not answer: the last list stays, and the next
        // push or reopened socket reads again.
      }
    },

    show(requests: AttentionRequest[]) {
      const known = this._known;
      this._known = new Set(requests.map((r) => r.id));
      this.requests = requests;
      this.now = Date.now();
      document.title = attentionTitle(this.requests.length);
      if (known && this.notifyOn && document.hidden) {
        for (const request of requests) if (!known.has(request.id)) this.announce(request);
      }
    },

    // One browser notification for a new request. Its tag is the request id,
    // so the browser shows a repeat of the same request once.
    announce(request: AttentionRequest) {
      if (!canNotify() || Notification.permission !== "granted") return;
      const detail = request.detail ? `: ${request.detail}` : "";
      const note = new Notification(this.nameOf(request), {
        body: `${this.kindOf(request)}${detail}`,
        tag: request.id,
      });
      note.onclick = () => window.focus();
    },

    canNotify() {
      return canNotify();
    },

    // Turn the switch on or off. On: the browser is asked for permission,
    // and the switch stays off when the answer is no.
    async setNotify(on: boolean) {
      if (!on) {
        this.notifyOn = false;
        saveNotify(false);
        return;
      }
      this.notifyOn = (await askPermission()) === "granted";
      saveNotify(this.notifyOn);
      if (!this.notifyOn) this.$store.modals.flash("The browser did not allow notifications.");
    },

    toggle() {
      const was = this.open;
      sendWindow(window, "workbench:menus-close");
      this.open = !was;
    },
    close() {
      this.open = false;
    },

    // The console's name when the daemon has one, else its project's name
    // (a remoteless repo's slug is a hash key, not a name).
    nameOf(request: AttentionRequest) {
      const projects = this.$store.projects;
      const session = sessionOf(request, projects.liveSessions);
      if (session?.name) return session.name;
      const project = projects.projects.find((p) => projects.repoRef(p) === request.repo);
      return project ? WBProject.projectName(project) : WBFleet.refSlug(request.repo);
    },
    kindOf(request: AttentionRequest) {
      return kindText(request.kind);
    },
    ageOf(request: AttentionRequest) {
      return ageText(request.since, this.now);
    },
    actionOf(request: AttentionRequest) {
      return actionOf(request.kind);
    },
    // Whether the row has "Dismiss": the daemon says which requests may be
    // hidden, and a live one may not.
    canDismiss(request: AttentionRequest) {
      return request.dismissable;
    },

    // Hide `request` on every device, then read the list again. A refusal
    // keeps the row and says so.
    async dismiss(request: AttentionRequest) {
      try {
        const r = await postDismiss(request.id);
        if (!r.ok) this.$store.modals.flash("Could not dismiss the request.");
      } catch {
        this.$store.modals.flash("Could not dismiss the request: the daemon did not answer.");
      }
      await this.read();
    },

    // The operator opened the console that holds `target`: its `console-done`
    // requests end.
    focused(target: EventTarget | null) {
      const win = (target as Element | null)?.closest?.<ConsoleWin>(".session-window");
      if (!win?._deskId) return;
      for (const request of this.requests) {
        const session = sessionOf(request, this.$store.projects.liveSessions);
        if (request.kind === "console-done" && session?.record === win._deskId) this.finish(request);
      }
    },
    // Dismiss `request` with nothing said when the daemon refuses: the row
    // stays, with its button.
    async finish(request: AttentionRequest) {
      if (this._dismissing.has(request.id)) return;
      this._dismissing.add(request.id);
      try {
        await postDismiss(request.id);
      } catch {
        // The daemon did not answer; the next read shows the row again.
      } finally {
        this._dismissing.delete(request.id);
      }
      await this.read();
    },

    // Show the run of `request` in the Runs panel.
    openRun(request: AttentionRequest) {
      this.open = false;
      if (!request.repo || !request.run) return;
      sendWindow(window, "workbench:run-select", { repo: request.repo, runid: request.run });
    },

    // Show the console of `request`, as the Go-to picker shows a window: its
    // own tab when it has one, else the Consoles tab, panned to it.
    goTo(request: AttentionRequest) {
      this.open = false;
      if (request.kind === "console-done") this.finish(request);
      const id = sessionOf(request, this.$store.projects.liveSessions)?.record;
      if (!id) return;
      const tab = this.$store.tabs.tabOf(id);
      if (tab) {
        this.$store.tabs.activate(tab.id);
        return;
      }
      if (this.$store.tabs.active !== "consoles") this.$store.tabs.activate("consoles");
      this.$nextTick(() => WBConsole.reveal(id));
    },
  });
}
