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
   the operator) has a "Dismiss" button instead: `POST /api/attention/dismiss`
   hides it on every device, and the daemon of a peer's request stores it.

   `main.ts` registers it as `wbAttention`.
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { apiFetch } from "./wb-api.ts";
import { sendWindow } from "./wb-events.ts";
import { WBFleet } from "./wb-fleet.ts";
import { WBProject } from "./wb-project.ts";

/** The kind of a request (`attention::Kind`). */
export type AttentionKind = "console-waiting" | "console-silent" | "run-waiting" | "run-finished";

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

    init() {
      this.read();
      window.addEventListener("workbench:attention-reread", () => this.read());
      window.addEventListener("workbench:logged-in", () => this.read());
      window.addEventListener("workbench:menus-close", () => this.close());
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
      this.requests = requests;
      this.now = Date.now();
      document.title = attentionTitle(this.requests.length);
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
        const r = await apiFetch("POST /api/attention/dismiss", {
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ id: request.id }),
        });
        if (!r.ok) this.$store.modals.flash("Could not dismiss the request.");
      } catch {
        this.$store.modals.flash("Could not dismiss the request: the daemon did not answer.");
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
