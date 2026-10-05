/* ---------------------------------------------------------------------------
   The Hosts dialog: the hosts paired over SSH, the form that adds one, and
   Remove in each row. The daemon runs `ralphy host …` on this computer, and
   `wb-hosts.js` is the fold that holds the dialog's state.

   `hostsDialog` is the Alpine component `wbHostsDialog`. It reaches `shell()`
   only through the names in `uses` (ADR-0073 D4), and it opens on the
   `workbench:hosts-open` event (ADR-0073 D5).

   Load order: after `wb-hosts.js`, before `app.js` and before Alpine, on
   `index.html` only.
   --------------------------------------------------------------------------- */
function hostsDialog() {
  return {
    // Every `shell()` member this component's code or markup reads or calls.
    // `loadComponent` in ui-tests/harness.mjs fails on any other name.
    uses: [
      "scrim",
      "osOf",
      "peerIcon",
      "peerStateWord",
      "peerFault",
      "fleetPeers",
      "loadRepos",
      "_flashAction",
      "hostRemoved",
    ],
    // The Add a host dialog (#497): its whole state is the wb-hosts.js fold.
    addHost: window.WBHosts.initial(),
    // The eye button of the password field. Hidden again on each open.
    hostSecretShown: false,
    // Remove in a row of the Hosts dialog (#497): the host being removed.
    removeHost: { open: false, daemon: "", name: "", rotate: false, busy: false, lines: [], failure: null },
    // --- Add a host (ADR-0067 §11, #497) ----------------------------------
    // Thin calls: every state change goes through `WBHosts.next`, and the
    // daemon runs `ralphy host …` on this computer.
    addHostStep(ev) {
      this.addHost = window.WBHosts.next(this.addHost, ev);
    },
    addHostFailed(message) {
      this.addHostStep({ type: "event", event: { event: "failed", kind: "other", message } });
    },
    // Opens on the list when a host is paired over SSH, else on the form.
    openAddHost() {
      const tab = this.sshHosts().length ? "hosts" : "add";
      this.addHost = Object.assign(window.WBHosts.initial(), { open: true, tab });
      this.hostSecretShown = false;
      this.removeHost.open = false;
      window.WBDaemon.observe("host.aliases", {})
        .then((reply) => {
          if (reply?.status === "ok") this.addHostStep({ type: "aliases", aliases: reply.aliases });
        })
        // Without the list the operator types the address.
        .catch((e) => console.warn("host aliases:", e));
    },
    closeAddHost() {
      this.addHostStep({ type: "close" });
      this.removeHost.open = false;
    },
    // The hosts paired over SSH. A WSL daemon is paired by its setup instead.
    sshHosts() {
      return (this.fleetPeers || []).filter((p) => p.tunnel);
    },
    // With no host left there is no list to show.
    hostTab() {
      return this.sshHosts().length ? this.addHost.tab : "add";
    },
    addHostTab(tab) {
      if (this.addHost.busy) return;
      this.removeHost.open = false;
      this.addHostStep({ type: "tab", tab });
    },
    // Cancel leaves an edit for the list it came from.
    addHostCancel() {
      if (this.addHost.editing) this.addHostTab("hosts");
      else this.closeAddHost();
    },
    addHostEdit(h) {
      if (this.addHost.busy) return;
      this.removeHost.open = false;
      this.addHostStep({ type: "edit", host: h });
    },
    // A host row's state glyph and its tooltip, as in the group header.
    hostRowGroup(h) {
      return { name: h.name, state: h.state, diagnosis: h.diagnosis, local: false };
    },
    // The row already prints the name, so the tooltip holds only the state.
    // A fault keeps the daemon's diagnosis: it says what to do.
    hostRowTitle(h) {
      if (h.state === "reachable") return "Connected";
      return h.diagnosis || window.WBFleet.groupTitle({ state: h.state, local: false });
    },
    addHostPick(alias) {
      this.addHostStep({ type: "pick", alias });
    },
    addHostType(field, value) {
      this.addHostStep({ type: "type", field, value });
    },
    addHostHost(value) {
      this.addHostStep({ type: "host", value });
    },
    // A browser offers to save what is typed in an `<input type="password">`,
    // and `autocomplete="off"` does not stop it. A text field drawn with
    // `-webkit-text-security` hides the characters and is not offered.
    // Without that property the field falls back to a password field.
    // The eye button shows the characters: a plain text field.
    hostSecretType() {
      if (this.hostSecretShown) return "text";
      const css = window.CSS;
      return css?.supports?.("-webkit-text-security", "disc") ? "text" : "password";
    },
    hostView() {
      return window.WBHosts.view(this.addHost);
    },
    hostPrimary() {
      return window.WBHosts.primary(this.addHost);
    },
    addHostPayload() {
      const s = this.addHost;
      const payload = { destination: window.WBHosts.destination(s) };
      if (s.signIn === "key" && s.keyFile.trim()) payload.identity = s.keyFile.trim();
      if (s.name.trim()) payload.name = s.name.trim();
      if (s.signIn === "config" && s.password && this.hostPasswordAllowed()) payload.password = s.password;
      return payload;
    },
    // The daemon takes a password only over https or from this computer.
    hostPasswordAllowed() {
      const { protocol, hostname } = window.location;
      return protocol === "https:" || ["localhost", "127.0.0.1", "[::1]", "::1"].includes(hostname);
    },
    hostReady() {
      return !this.addHost.busy && window.WBHosts.ready(this.addHost);
    },
    hostNeedsName() {
      return window.WBHosts.needsName(this.addHost);
    },
    hostHelpTabs() {
      return window.WBHosts.helpTabs();
    },
    hostHelpSteps() {
      return this.hostHelpTabs().find((t) => t.id === this.addHost.helpTab)?.steps || [];
    },
    hostWrongAddress() {
      return window.WBHosts.WRONG_ADDRESS;
    },
    hostCheckIcon(c) {
      return (
        {
          pass: "bi-check-circle",
          fix: "bi-wrench",
          copy: "bi-exclamation-circle",
          warn: "bi-exclamation-triangle",
          pending: "bi-hourglass",
        }[c.status] || "bi-dot"
      );
    },
    // Step 1 → 2 or 3: read the host key before anything signs in.
    async addHostNext() {
      const destination = window.WBHosts.destination(this.addHost);
      if (!destination || this.addHost.busy) return;
      this.addHostStep({ type: "check-again" });
      this.addHostStep({ type: "busy", value: true });
      let reply;
      try {
        reply = await window.WBDaemon.observe("host.key", { destination });
      } catch {
        reply = null;
      }
      this.addHostStep({ type: "busy", value: false });
      if (reply?.status !== "ok") {
        this.addHostFailed(window.WBFail.failed(reply, "Could not read the host key: the daemon did not answer."));
        return;
      }
      this.addHostStep({ type: "key", key: reply.key });
      if (this.addHost.step === "checks") this.addHostCheck();
    },
    async addHostTrust() {
      const destination = window.WBHosts.destination(this.addHost);
      const fingerprint = this.addHost.keys[0]?.fingerprint;
      if (!fingerprint || this.addHost.busy) return;
      this.addHostStep({ type: "busy", value: true });
      let reply;
      try {
        reply = await window.WBDaemon.observe("host.trust", { destination, fingerprint });
      } catch {
        reply = null;
      }
      this.addHostStep({ type: "busy", value: false });
      if (reply?.status !== "ok") {
        this.addHostFailed(window.WBFail.failed(reply, "Could not trust the host key: the daemon did not answer."));
        return;
      }
      this.addHostStep({ type: "trusted" });
      this.addHostCheck();
    },
    addHostCancelTrust() {
      this.addHostStep({ type: "cancel-trust" });
    },
    addHostCheck() {
      this._runHostVerb("host.check");
    },
    addHostCheckAgain() {
      this.addHostStep({ type: "check-again" });
      this.addHostCheck();
    },
    // Back to the fields from the checks, to correct the connection.
    addHostBack() {
      if (this.addHost.busy) return;
      this.addHostStep({ type: "back" });
    },
    addHostConnect() {
      if (!this.hostReady()) return;
      this._runHostVerb("host.add");
    },
    hostNeedsInstall() {
      return window.WBHosts.needsInstall(this.addHost);
    },
    hostAddedText() {
      return window.WBHosts.addedText(this.addHost);
    },
    hostInstallText() {
      return window.WBHosts.installText(this.addHost);
    },
    // The operator's click is the permission to install on the host.
    addHostInstall() {
      if (this.addHost.busy || !this.hostNeedsInstall()) return;
      this._runHostVerb("host.install");
    },
    // Stream a `host check|add|install` run: its output is the CLI's JSON lines.
    _runHostVerb(verb) {
      this.addHostStep({ type: "busy", value: true });
      let buf = "";
      window.WBDaemon.spawn(verb, this.addHostPayload(), (st) => {
        if (st.status === "output") {
          const fed = window.WBHosts.feed(buf, st.chunk);
          buf = fed.rest;
          for (const event of fed.events) this.addHostStep({ type: "event", event });
        } else if (st.status === "exited") {
          this.addHostStep({ type: "exit", verb, code: st.code });
          // `loadRepos`, not `loadFleet`: the latter CONCATENATES peer rows.
          // An add that saved the host (`done`) but ended non-zero (the tunnel is not
          // up) still made a row, so the list shows it and offers Edit.
          if (verb === "host.add" && (st.code === 0 || this.addHost.done)) this.loadRepos();
          if (verb === "host.install" && st.code === 0) this.addHostCheckAgain();
          if (verb === "host.check" && st.code === 0 && window.WBHosts.wantsAutoInstall(this.addHost)) {
            this.addHostStep({ type: "auto-tried" });
            this.addHostInstall();
          }
        } else if (st.status === "error") {
          this.addHostFailed(window.WBFail.failed(st, "Could not reach the host: the daemon did not start the command."));
          this.addHostStep({ type: "busy", value: false });
        }
      });
    },
    // Remove asks in the host's own row of the list.
    openRemoveHost(h) {
      if (this.removeHost.busy) return;
      this.removeHost = { open: true, daemon: h.daemon_id, name: h.name, rotate: false, busy: false, lines: [], failure: null };
      // The list scrolls: the last row's question opens below the visible part.
      this.$nextTick(() =>
        document
          .querySelector('.host-item[data-host="' + CSS.escape(h.daemon_id) + '"]')
          ?.scrollIntoView({ block: "nearest" }),
      );
    },
    closeRemoveHost() {
      this.removeHost.open = false;
    },
    // The daemon id, not the name: `host remove` accepts either, and the id
    // cannot name a second host.
    confirmRemoveHost() {
      if (this.removeHost.busy) return;
      Object.assign(this.removeHost, { busy: true, lines: [], failure: null });
      let s = window.WBHosts.initial();
      window.WBDaemon.spawn(
        "host.remove",
        { host: this.removeHost.daemon, rotate_token: !!this.removeHost.rotate },
        (st) => {
          if (st.status === "output") {
            const fed = window.WBHosts.feed(s.buf, st.chunk);
            s = Object.assign({}, s, { buf: fed.rest });
            for (const event of fed.events) s = window.WBHosts.next(s, { type: "event", event });
          } else if (st.status === "exited") {
            s = window.WBHosts.next(s, { type: "exit", verb: "host.remove", code: st.code });
            if (st.code === 0) {
              this.hostRemoved(this.removeHost.daemon);
              this.removeHost.open = false;
              this.loadRepos();
            }
          } else if (st.status === "error") {
            s = window.WBHosts.next(s, {
              type: "event",
              event: {
                event: "failed",
                kind: "other",
                message: window.WBFail.failed(st, "Could not remove the host: the daemon did not start the command."),
              },
            });
          }
          Object.assign(this.removeHost, {
            lines: s.lines,
            failure: s.failure,
            busy: st.status !== "exited" && st.status !== "error",
          });
        },
      );
    },
    async copyHostCommand(command, done = "Copied the command.") {
      try {
        await navigator.clipboard.writeText(command);
      } catch (e) {
        // No clipboard off a secure origin; the command stays on screen to type.
        console.warn("copy host command:", e);
        return;
      }
      this._flashAction(done);
    },
  };
}

window.WBHostsDialog = { component: hostsDialog };

if (typeof document !== "undefined" && document.addEventListener) {
  document.addEventListener("alpine:init", () => window.Alpine.data("wbHostsDialog", hostsDialog));
}
