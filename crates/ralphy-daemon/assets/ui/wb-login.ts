/* ---------------------------------------------------------------------------
   The login gate: the opaque card shown while this browser may not read, with
   one box per digit of the authenticator code, the password and "keep me
   signed in".

   `wbLogin` is the Alpine component of the gate (#621; ADR-0073 D2, D4). The
   session (`authed`) and the security fact are the Alpine store `connection`
   (wb-connection-store.ts); the gate reads them and keeps no copy. It hears
   `workbench:auth-read` for whether the login asks for a password, and
   `workbench:log-off` to reset its fields. After a login the daemon accepted,
   it reads the gated facts again, and sends `workbench:logged-in`. The shell
   (app.ts) hears it: it reads the desk again and restores the open tabs
   through the store `tabs`.

   `main.ts` registers it as `wbLogin`.
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { sendWindow } from "./wb-events.ts";

export function wbLogin() {
  // It reads no `shell()` member.
  return component([], {
    // `remember` is "keep me signed in" (ADR-0032 amendment 2026-09-16):
    // opt-in, reset on every log-off.
    login: { code: "", digits: ["", "", "", "", "", ""], password: "", remember: false, error: "", passwordRequired: false },

    init() {
      // The daemon said whether the login asks for a password (the store
      // `connection`).
      window.addEventListener("workbench:auth-read", (e) => {
        this.login.passwordRequired = e.detail.password;
      });
      // Localhost/Bearer have no login gate to drop to (#205).
      window.addEventListener("workbench:log-off", () => {
        if (this.$store.connection.security.policy === "session") {
          this.login = { code: "", digits: ["", "", "", "", "", ""], password: "", remember: false, error: "", passwordRequired: this.login.passwordRequired };
        }
      });
    },

    // Re-fetch the endpoints that returned 401 while gated; the presence
    // socket self-reconnects.
    rehydrateAfterAuth() {
      this.$store.projects.clearReposError();
      void this.$store.projects.loadRepos();
      void this.$store.connection.loadIdentity();
      // `/api/agents` is gated too.
      void this.$store.projects.loadAgents();
      // `/api/desk` and `file.read` are gated too: the shell reads the desk
      // and restores the open tabs (app.ts).
      sendWindow(window, "workbench:logged-in");
      // Every shown fact reads again after login (ADR-0070 D2 event 4);
      // `loadRepos` above covers the sessions, the change set and the branch.
      sendWindow(window, "workbench:board-refresh", { trigger: "login" });
      // Not only with the Runs panel open: the runs lock the writes.
      void this.$store.runs.hydrateRuns();
      this.$store.connection.rereadOpenPanels();
    },

    // --- TOTP digit boxes -------------------------------------------------
    // One input per digit; a paste or OTP autofill landing all 6 in the first
    // box is spread. `login.code` is the joined string.

    _otpBoxes(el: HTMLElement) {
      return el.closest(".login-otp")!.querySelectorAll("input");
    },

    _otpSync() {
      this.login.code = this.login.digits.join("");
    },

    // Once the 6th digit lands: the password field if required, else submit.
    _otpAdvancePastLast(el: HTMLElement) {
      if (this.$store.connection.security.passwordSet || this.login.passwordRequired) {
        this.$refs.loginPassword?.focus();
      } else {
        el.closest("form")?.querySelector<HTMLElement>(".login-btn")?.focus();
      }
    },

    _otpFill(text: string, el: HTMLElement) {
      const chars = text.replace(/\D/g, "").slice(0, 6).split("");
      for (let j = 0; j < 6; j++) this.login.digits[j] = chars[j] || "";
      this._otpSync();
      const boxes = this._otpBoxes(el);
      if (chars.length >= 6) this._otpAdvancePastLast(el);
      else boxes[chars.length].focus();
    },

    otpInput(i: number, e: Event & { target: HTMLInputElement }) {
      const v = e.target.value.replace(/\D/g, "");
      if (v.length > 1) {
        this._otpFill(v, e.target);
        return;
      }
      e.target.value = v;
      this.login.digits[i] = v;
      this._otpSync();
      if (!v) return;
      if (i < 5) this._otpBoxes(e.target)[i + 1].focus();
      else this._otpAdvancePastLast(e.target);
    },

    otpKeydown(i: number, e: KeyboardEvent & { target: HTMLInputElement }) {
      const boxes = this._otpBoxes(e.target);
      if (e.key === "Backspace" && !e.target.value && i > 0) {
        e.preventDefault();
        this.login.digits[i - 1] = "";
        this._otpSync();
        boxes[i - 1].focus();
      } else if (e.key === "ArrowLeft" && i > 0) {
        e.preventDefault();
        boxes[i - 1].focus();
      } else if (e.key === "ArrowRight" && i < 5) {
        e.preventDefault();
        boxes[i + 1].focus();
      }
    },

    otpPaste(e: ClipboardEvent & { target: HTMLInputElement }) {
      const text = e.clipboardData?.getData("text") || "";
      this._otpFill(text, e.target);
    },

    // The url-encoded `POST /api/login` body; `remember=true` only when
    // checked (an absent field is a standard session). Pure.
    loginBody() {
      const code = (this.login.code || "").trim();
      const body = new URLSearchParams({ code });
      if (this.login.passwordRequired || this.$store.connection.security.passwordSet) {
        body.set("password", this.login.password || "");
      }
      if (this.login.remember) body.set("remember", "true");
      return body.toString();
    },

    async submitLogin() {
      try {
        const res = await fetch("/api/login", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: this.loginBody(),
        });
        if (res.ok) {
          this.login.error = "";
          this.$store.connection.authChanged(true);
          this.forgetLoginSecrets();
          this.rehydrateAfterAuth();
          window.WB.emit("login");
        } else {
          this.login.error = "Invalid code or password.";
        }
      } catch {
        // Only the daemon authenticates: a thrown fetch never logs in.
        this.login.error = "Cannot reach the daemon. Try again.";
      }
    },

    // The code and the password are SPENT the moment the daemon accepts them:
    // the session is the cookie now, and neither is ever replayed. Dropping
    // them keeps a plaintext password out of a live `<input>` for the rest of
    // the session — which is also what stopped the browser's password manager
    // from pairing it with the next text field the operator typed into and
    // offering to save a note's title as a username (measured 2026-09-22).
    forgetLoginSecrets() {
      this.login.code = "";
      this.login.password = "";
      this.login.digits = ["", "", "", "", "", ""];
    },
  });
}
