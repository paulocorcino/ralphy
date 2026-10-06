/* ---------------------------------------------------------------------------
   The Settings dialog: the data-driven knobs (schema in wb-settings.js), the
   daemon's `config.get`/`config.set`/`config.unset`, and the desk history.

   `settingsDialog` is the Alpine component `wbSettingsDialog`. It reaches
   `shell()` only through the names in `uses` (ADR-0073 D4). It opens on the
   `workbench:settings-open` event; log off closes it with `workbench:log-off`,
   and a tab that becomes visible or a login makes it read its settings again,
   when it is open, with `workbench:panels-reread` (ADR-0073 D5). The Devices
   section inside it is its own component (`wbDevices`), nested one level
   deeper.

   Load order: after wb-settings.js, before `app.js` and before Alpine, on
   `index.html` only.
   --------------------------------------------------------------------------- */
function settingsDialog() {
  return {
    // Every `shell()` member this component's code or markup reads or calls.
    // `loadComponent` in ui-tests/harness.mjs fails on any other name.
    uses: ["_flashAction", "askConfirm", "openSlug", "projectLabel", "scrim"],
    // Data-driven (schema in wb-settings.js); the daemon persists via
    // `config.set`/`config.unset`.
    SETTINGS: window.WB_SETTINGS,
    TRISTATE: window.WB_TRISTATE,
    settingsOpen: false,
    // The daemon (machine-wide) group first; per-project sections follow.
    settingsSection: "daemon",
    settings: window.wbSettingsDefaults(),

    // Keys held in this browser profile's view store (`scope: "client"`).
    CLIENT_KEYS: window.wbClientKeys(),

    openSettings() {
      this.settingsOpen = true;
      this.settingsError = "";
      // Client-scoped keys come from the view store; `config.get` has none.
      const view = window.WBView.read() || {};
      this.settings["consoles.relaunch_on_load"] = view.relaunch === true;
      this.settings["consoles.key_bar"] = view.keys ?? "unset";
      // The size the consoles show now: the key bar's A−/A+ write the same field.
      this.settings["consoles.font_size"] = window.WBConsole?.fontSize() ?? this.settings["consoles.font_size"];
      this.readSettings();
    },
    // The open repo's resolved config (`config.get`), merged over the schema
    // defaults; with no repo open the project groups are disabled.
    readSettings() {
      if (this.openSlug) {
        WBDaemon.observe("config.get", { repo: this.openSlug })
          .then((reply) => {
            const cfg = reply && reply.status === "ok" ? reply.config : null;
            // The defaults must not pass as the project's values (ADR-0070 D3).
            if (!cfg || typeof cfg !== "object") {
              this.settingsError = window.WBFail.failed(
                reply,
                "Could not read the settings: the daemon gave no reason.",
              );
            }
            if (cfg && typeof cfg === "object") {
              for (const k in cfg) {
                // Never round-trip the MASKED secret: a save would persist the mask.
                if (k === "events.token") continue;
                if (cfg[k] !== null && k in this.settings) this.settings[k] = cfg[k];
              }
            }
          })
          .catch(() => {
            this.settingsError = "Could not read the settings: the daemon did not answer";
          });
      }
    },
    // Why the open project's settings could not be read, or "". While set, a
    // project setting is not written: the panel shows defaults, not values.
    settingsError: "",
    closeSettings() {
      this.settingsOpen = false;
    },
    // The "Desk history" section (ADR-0050 amendment 2026-10-04): read each
    // time it opens. `loaded` is set only by a list the daemon served, so a
    // failed read never shows as an empty history (ADR-0070 D3).
    deskHistory: { loaded: false, rows: [], error: "" },
    showSettingsSection(id) {
      this.settingsSection = id;
      if (id === "desk-history") this.loadDeskHistory();
    },
    async loadDeskHistory() {
      this.deskHistory.error = "";
      try {
        const r = await fetch("/api/desk/history");
        const reply = await r.json().catch(() => null);
        if (!r.ok || !Array.isArray(reply)) {
          this.deskHistory.loaded = false;
          this.deskHistory.rows = [];
          this.deskHistory.error = `Could not read the desk history: ${reply?.error || "the daemon gave no reason"}.`;
          return;
        }
        const when = (ms) => new Date(ms).toLocaleString();
        this.deskHistory.rows = window.WBDeskHistory.rows(reply, when);
        this.deskHistory.loaded = true;
      } catch {
        this.deskHistory.loaded = false;
        this.deskHistory.rows = [];
        this.deskHistory.error = "Could not read the desk history: the daemon did not answer.";
      }
    },
    // A restore reloads every open page, this one too: a page never moves a
    // live window from the desk, so a reload is how the restored layout shows.
    async postDeskRestore(body, failLine) {
      try {
        const r = await fetch("/api/desk/history", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        });
        if (r.ok) {
          window.WBConsole?.reloadForRestoredDesk();
          return;
        }
        const reply = await r.json().catch(() => null);
        this.deskHistory.error = `${failLine}: ${reply?.error || "the daemon gave no reason"}.`;
      } catch {
        this.deskHistory.error = `${failLine}: the daemon did not answer.`;
      }
    },
    askDeskRestore(title) {
      return this.askConfirm({
        title,
        message: "Consoles that run now stay open. Every open page reloads.",
        confirmLabel: "Restore",
      });
    },
    async restoreDeskVersion(row) {
      if (!(await this.askDeskRestore("Restore this desk layout?"))) return;
      await this.postDeskRestore({ id: row.id }, "Could not restore the desk layout");
    },
    async downloadDeskVersion(row) {
      try {
        const r = await fetch("/api/desk/history?id=" + encodeURIComponent(row.id));
        const version = await r.json().catch(() => null);
        if (!r.ok || !version) {
          this.deskHistory.error = `Could not download the desk layout: ${version?.error || "the daemon gave no reason"}.`;
          return;
        }
        const blob = new Blob([JSON.stringify(version, null, 2)], { type: "application/json" });
        const url = URL.createObjectURL(blob);
        const a = document.createElement("a");
        a.href = url;
        a.download = window.WBDeskHistory.fileName(version);
        document.body.append(a);
        a.click();
        a.remove();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
      } catch {
        this.deskHistory.error = "Could not download the desk layout: the daemon did not answer.";
      }
    },
    pickDeskUpload() {
      document.getElementById("desk-upload-input")?.click();
    },
    async uploadDeskVersion(event) {
      const file = event.target.files?.[0];
      event.target.value = "";
      if (!file) return;
      this.deskHistory.error = "";
      const parsed = window.WBDeskHistory.parseUpload(await file.text());
      if (parsed.cause) {
        this.deskHistory.error = `Could not upload the desk layout: ${parsed.cause}.`;
        return;
      }
      if (!(await this.askDeskRestore("Restore the desk layout from this file?"))) return;
      await this.postDeskRestore({ version: parsed.version }, "Could not upload the desk layout");
    },

    async saveSetting(key, value) {
      this.settings[key] = value;
      // A client-scoped key is this browser's preference: view store, never
      // `config.set`.
      if (this.CLIENT_KEYS.has(key)) {
        if (key === "consoles.relaunch_on_load") window.WBView.patch({ relaunch: value === true });
        // "unset" is the ABSENCE of a preference: written as null.
        if (key === "consoles.key_bar")
          window.WBView.patch({ keys: value === "on" || value === "off" ? value : null });
        // Held to the range the key bar steps through; an emptied field is
        // the default size. `setFont` writes the store and refits every console.
        if (key === "consoles.font_size") {
          const px = window.WBConsole.stepFont(value === "" ? NaN : Number(value), 0);
          this.settings[key] = window.WBConsole.setFont(px);
        }
        WB.emit("setting-change", { project: null, key, value });
        return;
      }
      // The run-lock-aware config Mutates; an empty/"unset" value clears the
      // key. `observe` (not `spawn`) so a run-lock refusal surfaces (#207).
      if (this.openSlug && this.settingsError) {
        this._flashAction("Could not change the setting: the settings were not read. Open the settings again.");
        return;
      }
      if (this.openSlug) {
        const empty = value === "" || value === "unset" || value == null;
        try {
          const reply = await window.WBDaemon.observe(empty ? "config.unset" : "config.set", {
            repo: this.openSlug,
            key,
            value: String(value),
          });
          if (window.WBFail.isError(reply)) {
            this._flashAction(window.WBFail.failed(reply, "Could not change the setting: the daemon gave no reason."));
          }
        } catch {
          // No daemon reachable — leave the optimistic setting in place.
        }
      }
      WB.emit("setting-change", { project: this.openSlug, key, value });
    },
  };
}

// `openFlag` is the path of the open flag that the dialog gives to `scrim()`.
// Code outside the component asks the modal stack with it (`modalOpen`), and
// never reads the flag (ADR-0073 D5).
window.WBSettingsDialog = { component: settingsDialog, openFlag: "settingsOpen" };

if (typeof document !== "undefined" && document.addEventListener) {
  document.addEventListener("alpine:init", () => window.Alpine.data("wbSettingsDialog", settingsDialog));
}
