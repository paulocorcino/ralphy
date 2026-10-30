/* ---------------------------------------------------------------------------
   The Security dialog: the daemon auth model (ADR-0032), an opt-in access
   token, an optional password, and TOTP whose secret is shown exactly once.

   `securityDialog` is the Alpine component `wbSecurityDialog`. It reaches
   `shell()` only through the names in `uses` (ADR-0073 D4). The security fact
   (`security`) and log off are the Alpine store `connection`
   (wb-connection-store.ts), because the boot, the login gate and log off read
   it too: the dialog reads it, and changes it only through
   `securityChanged`. The dialog opens on the `workbench:security-open` event,
   and log off closes it with the `workbench:log-off` event (ADR-0073 D5).

   `main.ts` registers it as `wbSecurityDialog` (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import type { AlpineMagics } from "./wb-alpine.ts";
import { apiFetch } from "./wb-api.ts";
import { wbQr } from "./wb-settings.ts";

/** The change of a toggle: the checkbox, which the handler puts back in sync. */
type CheckboxChange = { target: HTMLInputElement | null };

/** The members of the dialog that the login switch calls. */
type LoginDialog = {
  $store: AlpineMagics["$store"];
  securityForm: { requireLoginError: string };
  stepUpBody(fields: Record<string, string>, code: string): string;
  noteStepUpRefusal(r: Response): void;
  closeSecurity(): void;
};

// `:checked` won't re-sync when the bound value did not change.
function syncCheckbox(ev: CheckboxChange | undefined, checked: boolean) {
  if (ev?.target) ev.target.checked = checked;
}

// Ask the daemon to put the login gate on or off. True when it did. A refusal
// to lift the gate is a step-up refusal; any other failure is said under the
// switch.
async function postRequireLogin(dialog: LoginDialog, want: boolean, code: string) {
  try {
    const r = await fetch("/api/security/require-login", {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: dialog.stepUpBody({ enable: String(want) }, code),
    });
    if (!r.ok) noteLoginRefusal(dialog, want, r);
    return r.ok;
  } catch {
    dialog.securityForm.requireLoginError = "Cannot reach the daemon. Try again.";
    return false;
  }
}

function noteLoginRefusal(dialog: LoginDialog, want: boolean, r: Response) {
  if (want) dialog.securityForm.requireLoginError = `The daemon refused (${r.status}).`;
  else dialog.noteStepUpRefusal(r);
}

// What follows a change of the gate that the daemon took.
async function afterRequireLogin(dialog: LoginDialog, want: boolean) {
  if (want) {
    // The gate applies to THIS bind, loopback included (ADR-0032 §A); the
    // daemon invalidated sessions, so drop to the login screen.
    dialog.$store.connection.securityChanged({ policy: "session" });
    dialog.closeSecurity();
    void dialog.$store.connection.logOff();
  } else {
    // Gate lifted — re-sync authed/policy from the server.
    await dialog.$store.connection.probeSession();
  }
}

export function securityDialog() {
  // Every `shell()` member this component's code or markup reads or calls.
  // `loadComponent` in ui-tests/harness.mjs fails on any other name, and the
  // type check fails on a name the code reads.
  return component([], {
    securityOpen: false,
    // The dialog's own form state. The fact it shows is `security` in the store
    // `connection`.
    securityForm: {
      passwordDraft: "",
      passwordConfirm: "",
      // Set only in the moment after enrolling; the daemon shows it once.
      secret: "",
      otpauthUri: "",
      qrHtml: "",
      pendingEnroll: false, // QR shown, awaiting the confirm code (ADR-0032 §C)
      confirmCode: "",
      totpError: "",
      // A page keeps the image policy (`security.remoteImages`) it loaded
      // with, so a change asks for a reload.
      remoteImagesReload: false,
      // The enrolled password, typed once to change or remove it (step-up,
      // ADR-0032 amendment E). Never kept after the request.
      passwordCurrent: "",
      // The last step-up refusal, shown under the card that asked.
      stepUpError: "",
      // Why the login switch did not change, shown under it.
      requireLoginError: "",
    },
    // The step-up prompt (ADR-0032 amendment E): lowering the auth posture —
    // rotating the token, lifting the login gate, revoking TOTP — costs a
    // fresh authenticator code once a seed is armed. One prompt serves all
    // three; `label` says which, `_stepUpResolve` hands the code back to the
    // action that asked.
    stepUp: { open: false, code: "", label: "" },
    _stepUpResolve: null as ((code: string | null) => void) | null,

    async openSecurity() {
      this.securityOpen = true;
      // The REAL daemon auth state (GET /api/security/state).
      try {
        const r = await apiFetch("GET /api/security/state");
        if (r.ok) {
          const s = await r.json();
          this.$store.connection.securityChanged({
            tokenSet: s.token_set,
            passwordSet: s.password_set,
            totpEnrolled: s.totp_enrolled,
            requireLogin: s.require_login,
            remoteImages: s.remote_images,
          });
        }
      } catch {}
    },
    closeSecurity() {
      this.securityOpen = false;
      // Drop the one-time secret when leaving.
      this.securityForm.secret = "";
      this.securityForm.otpauthUri = "";
      this.securityForm.qrHtml = "";
      // The pending seed survives server-side (mint-once).
      this.securityForm.pendingEnroll = false;
      this.securityForm.confirmCode = "";
      this.securityForm.totpError = "";
      this.securityForm.passwordCurrent = "";
      this.securityForm.stepUpError = "";
      this.securityForm.requireLoginError = "";
      this.cancelStepUp();
    },

    // Whether the daemon will demand a fresh code for a posture downgrade: a
    // live TOTP seed is armed. A pending (unconfirmed) enrolment never counts.
    stepUpNeeded() {
      return this.$store.connection.security.totpEnrolled === true;
    },
    // Ask the operator for the current 6-digit code before `label`. Resolves
    // to the code, to `""` when no seed is armed (nothing to ask), or to
    // `null` when they cancel.
    askFreshCode(label: string) {
      if (!this.stepUpNeeded()) return Promise.resolve("");
      this.cancelStepUp();
      this.securityForm.stepUpError = "";
      this.stepUp = { open: true, code: "", label };
      void this.$nextTick?.(() => document.querySelector<HTMLElement>(".step-up input")?.focus());
      return new Promise<string | null>((resolve) => {
        this._stepUpResolve = resolve;
      });
    },
    submitStepUp() {
      const code = this.stepUp.code.trim();
      if (code.length !== 6) return;
      const resolve = this._stepUpResolve;
      this._stepUpResolve = null;
      this.stepUp = { open: false, code: "", label: "" };
      resolve?.(code);
    },
    cancelStepUp() {
      const resolve = this._stepUpResolve;
      this._stepUpResolve = null;
      this.stepUp = { open: false, code: "", label: "" };
      resolve?.(null);
    },
    // The form body for a step-up-guarded mutation: the base fields plus the
    // code, only when there is one to send (the daemon treats an absent code
    // as "nothing armed", and a stray empty field would read as a wrong code).
    stepUpBody(fields: Record<string, string>, code: string) {
      const p = new URLSearchParams(fields);
      if (code) p.set("code", code);
      return p.toString();
    },
    // Turn a step-up refusal into the line under the card. `Retry-After` is
    // the throttle (amendment §D) — the same brake the login has.
    noteStepUpRefusal(r: Response) {
      if (r.status === 429) {
        const wait = r.headers?.get?.("Retry-After") || "a few";
        this.securityForm.stepUpError = `Too many attempts — wait ${wait} s and try again.`;
      } else if (r.status === 401) {
        this.securityForm.stepUpError = "Code rejected. Enter the current code from your authenticator app.";
      } else {
        this.securityForm.stepUpError = `The daemon refused (${r.status}).`;
      }
    },

    async enrollTotp() {
      // A PENDING seed (mint-once); NOT armed until `confirmTotp()` proves
      // possession.
      try {
        const r = await apiFetch("POST /api/security/totp/enroll");
        if (!r.ok) return;
        const { uri } = await r.json();
        this.securityForm.pendingEnroll = true;
        this.securityForm.totpError = "";
        this.securityForm.confirmCode = "";
        this.securityForm.otpauthUri = uri;
        this.securityForm.secret = (uri.split("secret=")[1] || "").split("&")[0] ?? "";
        this.securityForm.qrHtml = wbQr(uri);
      } catch {}
    },

    async confirmTotp() {
      // Verifies against the pending seed and arms it (ADR-0032 §C).
      const code = this.securityForm.confirmCode.trim();
      if (code.length !== 6) return;
      try {
        const r = await apiFetch("POST /api/security/totp/confirm", {
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: "code=" + encodeURIComponent(code),
        });
        const ok = r.ok && (await r.json()).confirmed;
        if (ok) {
          this.$store.connection.securityChanged({ totpEnrolled: true });
          this.securityForm.pendingEnroll = false;
          this.securityForm.secret = "";
          this.securityForm.otpauthUri = "";
          this.securityForm.qrHtml = "";
          this.securityForm.confirmCode = "";
          this.securityForm.totpError = "";
        } else {
          this.securityForm.totpError = "Wrong code. Enter the current code from your authenticator app.";
        }
      } catch {
        this.securityForm.totpError = "Cannot reach the daemon. Try again.";
      }
    },

    async cancelEnroll() {
      // Abandon an in-flight enrolment: drop the pending seed server-side too.
      // A pending seed never gated anything, so no code is asked (the route
      // only demands one for a LIVE seed).
      try {
        await fetch("/api/security/totp/revoke", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: "",
        });
      } catch {}
      this.securityForm.pendingEnroll = false;
      this.securityForm.secret = "";
      this.securityForm.otpauthUri = "";
      this.securityForm.qrHtml = "";
      this.securityForm.confirmCode = "";
      this.securityForm.totpError = "";
    },

    async revokeTotp() {
      // POST /api/security/totp/revoke deletes the live AND pending seeds. With
      // a live seed armed it costs a fresh code (step-up): on a gated loopback
      // bind this is the whole gate going away.
      const code = await this.askFreshCode("revoke two-factor");
      if (code === null) return;
      try {
        const r = await fetch("/api/security/totp/revoke", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: this.stepUpBody({}, code),
        });
        if (!r.ok) {
          this.noteStepUpRefusal(r);
          return;
        }
      } catch {
        return;
      }
      this.$store.connection.securityChanged({ totpEnrolled: false });
      this.securityForm.pendingEnroll = false;
      this.securityForm.secret = "";
      this.securityForm.otpauthUri = "";
      this.securityForm.qrHtml = "";
      this.securityForm.confirmCode = "";
      this.securityForm.totpError = "";
      // revoking the seed removes the session factor → login can't be required
      this.$store.connection.securityChanged({ requireLogin: false });
    },

    // The `/api/security/password` body: the new password (empty = remove)
    // and, once one is enrolled, the current one — the step-up the daemon
    // demands before it changes or removes the factor. `password` is ALWAYS
    // present: an absent field is a 400, never a clear.
    passwordBody(pw: string) {
      const p = new URLSearchParams({ password: pw });
      if (this.$store.connection.security.passwordSet) p.set("current", this.securityForm.passwordCurrent);
      return p.toString();
    },
    async savePassword() {
      const pw = this.securityForm.passwordDraft.trim();
      // Require a matching confirmation before the value ever leaves the field.
      if (!pw || this.securityForm.passwordDraft !== this.securityForm.passwordConfirm) return;
      if (this.$store.connection.security.passwordSet && !this.securityForm.passwordCurrent) return;
      this.securityForm.stepUpError = "";
      try {
        const r = await apiFetch("POST /api/security/password", {
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: this.passwordBody(pw),
        });
        if (!r.ok) {
          this.notePasswordRefusal(r);
          return;
        }
        this.$store.connection.securityChanged({ passwordSet: (await r.json()).password_set });
      } catch {
        return;
      } finally {
        this.securityForm.passwordCurrent = "";
      }
      this.securityForm.passwordDraft = "";
      this.securityForm.passwordConfirm = "";
    },
    async clearPassword() {
      if (this.$store.connection.security.passwordSet && !this.securityForm.passwordCurrent) return;
      this.securityForm.stepUpError = "";
      try {
        const r = await fetch("/api/security/password", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: this.passwordBody(""),
        });
        if (!r.ok) {
          this.notePasswordRefusal(r);
          return;
        }
      } catch {
        return;
      } finally {
        this.securityForm.passwordCurrent = "";
      }
      this.$store.connection.securityChanged({ passwordSet: false });
      this.securityForm.passwordDraft = "";
      this.securityForm.passwordConfirm = "";
    },
    notePasswordRefusal(r: Response) {
      if (r.status === 401) {
        this.securityForm.stepUpError = "Current password rejected.";
      } else {
        this.noteStepUpRefusal(r);
      }
    },
    async remintToken() {
      // Rotates the token AND bumps the session epoch (ADR-0032 amendment §B):
      // every cookie, this browser's included, is invalidated IMMEDIATELY.
      // Costs a fresh code once TOTP is armed (step-up): the session must not
      // be able to rotate the key it rides on.
      const code = await this.askFreshCode("rotate the access token");
      if (code === null) return;
      try {
        const r = await fetch("/api/security/token/remint", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: this.stepUpBody({}, code),
        });
        if (!r.ok) {
          this.noteStepUpRefusal(r);
          return;
        }
      } catch {
        return;
      }
      if (this.$store.connection.security.policy === "session") void this.$store.connection.logOff();
    },

    async toggleRequireLogin(ev?: CheckboxChange) {
      // Only meaningful once TOTP is enrolled; the server refuses (400) an
      // enable with no seed, the client guard just avoids the round-trip.
      const want = !this.$store.connection.security.requireLogin;
      this.securityForm.requireLoginError = "";
      if (want && !this.$store.connection.security.totpEnrolled) {
        this.$store.connection.securityChanged({ requireLogin: false });
        syncCheckbox(ev, false);
        this.securityForm.requireLoginError = "Enroll two-factor above first.";
        return;
      }
      // Turning the gate OFF is the gate itself being lowered: it costs a
      // fresh code (step-up). Turning it on stays free.
      const code = want ? "" : await this.askFreshCode("turn login off");
      if (code === null) {
        syncCheckbox(ev, this.$store.connection.security.requireLogin);
        return;
      }
      const ok = await postRequireLogin(this, want, code);
      if (ok) this.$store.connection.securityChanged({ requireLogin: want });
      syncCheckbox(ev, this.$store.connection.security.requireLogin);
      if (!ok) return;
      await afterRequireLogin(this, want);
    },

    async toggleRemoteImages(ev?: CheckboxChange) {
      // Turning it ON loosens the CSP, so that direction costs a fresh code
      // once a seed is armed; turning it off stays free.
      const want = !this.$store.connection.security.remoteImages;
      const code = want ? await this.askFreshCode("show remote images") : "";
      if (code === null) {
        if (ev?.target) ev.target.checked = this.$store.connection.security.remoteImages;
        return;
      }
      let ok = false;
      try {
        const r = await fetch("/api/security/remote-images", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: this.stepUpBody({ enable: String(want) }, code),
        });
        ok = r.ok;
        if (!ok) this.noteStepUpRefusal(r);
      } catch {
        ok = false;
      }
      if (ok) {
        this.$store.connection.securityChanged({ remoteImages: want });
        this.securityForm.remoteImagesReload = true;
      }
      // `:checked` won't re-sync when the bound value did not change.
      if (ev?.target) ev.target.checked = this.$store.connection.security.remoteImages;
    },
  });
}

// `openFlag` is the path of the open flag that the dialog gives to `scrim()`.
// Code outside the component asks the modal stack with it (`modalOpen`), and
// never reads the flag (ADR-0073 D5).
export const WBSecurityDialog = { openFlag: "securityOpen" };
