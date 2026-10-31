/* ---------------------------------------------------------------------------
   The login notices: a login of this daemon was used from another device
   (#717). The daemon owns them: it reads them from its audit log
   (`GET /api/audit/logins/notices`), the newest first, and the page keeps
   no copy of its own.

   `wbLoginNotice` is the Alpine component of the notices, shown on every
   open page. It reads the list when it starts, when the tab becomes visible,
   and again on `workbench:logins-reread`, which the store `connection` sends
   on a `logins.dirty` push and on a reopened page socket, and on
   `workbench:logged-in`.

   Each notice shows the facts of the other device, what the login did there
   and when, and two buttons. "End this login" revokes that login
   (`POST /api/security/logins/revoke`): every device that holds its cookie
   must sign in again. "This was me" dismisses the notice of that pair of
   login and device (`POST /api/audit/logins/dismiss`). After either one, the
   notice goes away; a refusal keeps it and says the act failed.

   `main.ts` registers it as `wbLoginNotice`.
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { apiFetch } from "./wb-api.ts";
import { deviceName, eventLine } from "./wb-devices.ts";
import type { Device } from "./wb-devices.ts";

/** A notice of `GET /api/audit/logins/notices` (`Notice` in
 * `audit/logins.rs`). `device` is `null` for a use that came with no device
 * cookie, and `facts` is `null` when that device's page never reported its
 * facts. */
export type LoginNotice = {
  login: string;
  device: string | null;
  did: string | null;
  at: string;
  facts: Device | null;
};

/** The reply of `GET /api/audit/logins/notices`. */
export type LoginNoticesReply = { notices: LoginNotice[] };

/** The key of a notice: its pair of login and device. */
export function noticeKey(notice: LoginNotice) {
  return `${notice.login}/${notice.device ?? ""}`;
}

/** The line that names the other device: its facts and its address. */
export function deviceText(notice: LoginNotice) {
  if (!notice.facts) return "A device that sent no facts";
  const ip = notice.facts.ip ? ` · ${notice.facts.ip}` : "";
  return `${deviceName(notice.facts)}${ip}`;
}

/** The line that says what the login did on that device, and when. `did`
 * is `console_launch` or the verb of a command (`audit_layer.rs`), named as
 * the Devices panel names the same audit line. */
export function didText(notice: LoginNotice) {
  const at = new Date(notice.at);
  const when = Number.isNaN(at.getTime()) ? notice.at : at.toLocaleString();
  let what = "Something it did not name";
  if (notice.did === "console_launch") what = eventLine({ at: notice.at, event: notice.did });
  else if (notice.did) what = eventLine({ at: notice.at, event: "command", verb: notice.did });
  return `${what} · ${when}`;
}

function postForm(
  route: "POST /api/security/logins/revoke" | "POST /api/audit/logins/dismiss",
  fields: Record<string, string>,
) {
  return apiFetch(route, {
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams(fields).toString(),
  });
}

/** No failed act yet. */
function noFailures(): Record<string, string> {
  return {};
}

export function wbLoginNotice() {
  // It reads no `shell()` member.
  return component([], {
    notices: [] as LoginNotice[],
    // The failed act of each notice, by `noticeKey`.
    failed: noFailures(),
    _seq: 0,

    init() {
      void this.read();
      const reread = () => {
        void this.read();
      };
      window.addEventListener("workbench:logins-reread", reread);
      window.addEventListener("workbench:logged-in", reread);
      document.addEventListener("visibilitychange", () => {
        if (document.visibilityState === "visible") reread();
      });
    },

    // Reads close together can answer out of order: the newest owns the list.
    // A failed read keeps the last list.
    async read() {
      const seq = ++this._seq;
      try {
        const r = await apiFetch("GET /api/audit/logins/notices");
        if (!r.ok || seq !== this._seq) return;
        const reply = await r.json();
        if (seq !== this._seq) return;
        this.notices = reply.notices;
      } catch {
        // A daemon that does not answer: the last list stays, and the next
        // push or reopened socket reads again.
      }
    },

    deviceOf(notice: LoginNotice) {
      return deviceText(notice);
    },
    didOf(notice: LoginNotice) {
      return didText(notice);
    },
    keyOf(notice: LoginNotice) {
      return noticeKey(notice);
    },
    failureOf(notice: LoginNotice) {
      return this.failed[noticeKey(notice)] || "";
    },

    // End the login of `notice`. The daemon lists no notice of a revoked
    // login, so every notice of that login goes away.
    async revoke(notice: LoginNotice) {
      const ok = await this.act(notice, "Could not end this login", () =>
        postForm("POST /api/security/logins/revoke", { login: notice.login }),
      );
      if (ok) this.notices = this.notices.filter((n) => n.login !== notice.login);
    },

    // The operator says the use was their own: the notice of that pair goes
    // away.
    async dismiss(notice: LoginNotice) {
      const key = noticeKey(notice);
      const ok = await this.act(notice, "Could not dismiss this notice", () =>
        postForm("POST /api/audit/logins/dismiss", {
          login: notice.login,
          device: notice.device ?? "",
        }),
      );
      if (ok) this.notices = this.notices.filter((n) => noticeKey(n) !== key);
    },

    // Send one act for `notice`; `false`, with the failure on the notice,
    // when the daemon refuses or does not answer.
    async act(notice: LoginNotice, failure: string, send: () => Promise<Response>) {
      const key = noticeKey(notice);
      const rest = { ...this.failed };
      delete rest[key];
      this.failed = rest;
      try {
        const r = await send();
        if (r.ok) return true;
        this.failed = { ...this.failed, [key]: `${failure}: the daemon refused (${r.status}).` };
      } catch {
        this.failed = { ...this.failed, [key]: `${failure}: the daemon did not answer.` };
      }
      return false;
    },
  });
}
