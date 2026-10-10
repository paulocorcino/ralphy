/* ---------------------------------------------------------------------------
   The Settings dialog: the data-driven knobs (schema in wb-settings.ts), the
   daemon's `config.get`/`config.set`/`config.unset`, and the desk history.

   `settingsDialog` is the Alpine component `wbSettingsDialog`. It reaches
   `shell()` only through the names in `uses` (ADR-0073 D4). It opens on the
   `workbench:settings-open` event; log off closes it with `workbench:log-off`,
   and a tab that becomes visible or a login makes it read its settings again,
   when it is open, with `workbench:panels-reread` (ADR-0073 D5). The Devices
   section inside it is its own component (`wbDevices`), nested one level
   deeper.

   `main.ts` registers it as `wbSettingsDialog` (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { apiFetch } from "./wb-api.ts";
import { WBDeskHistory } from "./wb-desk-history.ts";
import { WBFail } from "./wb-fail.ts";
import { WB_SETTINGS, WB_TRISTATE, wbClientKeys, wbSettingsDefaults } from "./wb-settings.ts";

/** One row of the Desk history section (`WBDeskHistory.rows`). */
type DeskRow = ReturnType<typeof WBDeskHistory.rows>[number];

export function settingsDialog() {
  // Every `shell()` member this component's code or markup reads or calls.
  // `loadComponent` in ui-tests/harness.mjs fails on any other name, and the
  // type check fails on a name the code reads.
  return component(["flash", "askConfirm", "scrim"], {
    // Data-driven (schema in wb-settings.ts); the daemon persists via
    // `config.set`/`config.unset`.
    SETTINGS: WB_SETTINGS,
    TRISTATE: WB_TRISTATE,
    settingsOpen: false,
    // The daemon (machine-wide) group first; per-project sections follow.
    settingsSection: "daemon",
    settings: wbSettingsDefaults(),

    // Keys held in this browser profile's view store (`scope: "client"`).
    CLIENT_KEYS: wbClientKeys(),

    openSettings() {
      this.settingsOpen = true;
      this.settingsError = "";
      // Client-scoped keys come from the view store; `config.get` has none.
      const view = window.WBView.read();
      this.settings["consoles.relaunch_on_load"] = view?.relaunch === true;
      this.settings["consoles.key_bar"] = view?.keys ?? "unset";
      // The size the consoles show now: the key bar's A−/A+ write the same field.
      this.settings["consoles.font_size"] = window.WBConsole?.fontSize() ?? this.settings["consoles.font_size"];
      this.readSettings();
    },
    // The open repo's resolved config (`config.get`), merged over the schema
    // defaults; with no repo open the project groups are disabled.
    readSettings() {
      if (this.$store.projects.openSlug) {
        window.WBDaemon.observe("config.get", { repo: this.$store.projects.openSlug })
          .then((reply) => {
            const cfg = reply && reply.status === "ok" ? reply.config : null;
            // The defaults must not pass as the project's values (ADR-0070 D3).
            if (!cfg || typeof cfg !== "object") {
              this.settingsError = WBFail.failed(
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
    deskHistory: { loaded: false, rows: [] as DeskRow[], error: "" },
    showSettingsSection(id: string) {
      this.settingsSection = id;
      if (id === "desk-history") this.loadDeskHistory();
    },
    async loadDeskHistory() {
      this.deskHistory.error = "";
      try {
        const r = await apiFetch("GET /api/desk/history");
        const reply = r.ok ? await r.json().catch(() => null) : null;
        if (!Array.isArray(reply)) {
          const refusal = r.ok ? null : await r.json().catch(() => null);
          this.deskHistory.loaded = false;
          this.deskHistory.rows = [];
          this.deskHistory.error = `Could not read the desk history: ${refusal?.error || "the daemon gave no reason"}.`;
          return;
        }
        const when = (ms: number) => new Date(ms).toLocaleString();
        this.deskHistory.rows = WBDeskHistory.rows(reply, when);
        this.deskHistory.loaded = true;
      } catch {
        this.deskHistory.loaded = false;
        this.deskHistory.rows = [];
        this.deskHistory.error = "Could not read the desk history: the daemon did not answer.";
      }
    },
    // A restore reloads every open page, this one too: a page never moves a
    // live window from the desk, so a reload is how the restored layout shows.
    async postDeskRestore(body: object, failLine: string) {
      try {
        const r = await apiFetch("POST /api/desk/history", {
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
    askDeskRestore(title: string) {
      return this.askConfirm({
        title,
        message: "Consoles that run now stay open. Every open page reloads.",
        confirmLabel: "Restore",
      });
    },
    async restoreDeskVersion(row: DeskRow) {
      if (!(await this.askDeskRestore("Restore this desk layout?"))) return;
      await this.postDeskRestore({ id: row.id }, "Could not restore the desk layout");
    },
    async downloadDeskVersion(row: DeskRow) {
      try {
        const r = await apiFetch("GET /api/desk/history?id", { query: { id: row.id } });
        const version = r.ok ? await r.json().catch(() => null) : null;
        if (!version) {
          const refusal = r.ok ? null : await r.json().catch(() => null);
          this.deskHistory.error = `Could not download the desk layout: ${refusal?.error || "the daemon gave no reason"}.`;
          return;
        }
        const blob = new Blob([JSON.stringify(version, null, 2)], { type: "application/json" });
        const url = URL.createObjectURL(blob);
        const a = document.createElement("a");
        a.href = url;
        a.download = WBDeskHistory.fileName(version);
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
    async uploadDeskVersion(event: Event & { target: HTMLInputElement }) {
      const file = event.target.files?.[0];
      event.target.value = "";
      if (!file) return;
      this.deskHistory.error = "";
      const parsed = WBDeskHistory.parseUpload(await file.text());
      if (parsed.cause) {
        this.deskHistory.error = `Could not upload the desk layout: ${parsed.cause}.`;
        return;
      }
      if (!(await this.askDeskRestore("Restore the desk layout from this file?"))) return;
      await this.postDeskRestore({ version: parsed.version }, "Could not upload the desk layout");
    },

    async saveSetting(key: string, value: unknown) {
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
        window.WB.emit("setting-change", { project: null, key, value });
        return;
      }
      // The run-lock-aware config Mutates; an empty/"unset" value clears the
      // key. `observe` (not `spawn`) so a run-lock refusal surfaces (#207).
      if (this.$store.projects.openSlug && this.settingsError) {
        this.flash("Could not change the setting: the settings were not read. Open the settings again.");
        return;
      }
      if (this.$store.projects.openSlug) {
        const empty = value === "" || value === "unset" || value == null;
        try {
          const reply = await window.WBDaemon.observe(empty ? "config.unset" : "config.set", {
            repo: this.$store.projects.openSlug,
            key,
            value: String(value),
          });
          if (WBFail.isError(reply)) {
            this.flash(WBFail.failed(reply, "Could not change the setting: the daemon gave no reason."));
          }
        } catch {
          // No daemon reachable — leave the optimistic setting in place.
        }
      }
      window.WB.emit("setting-change", { project: this.$store.projects.openSlug, key, value });
    },
  });
}

// `openFlag` is the path of the open flag that the dialog gives to `scrim()`.
// Code outside the component asks the modal stack with it (`modalOpen`), and
// never reads the flag (ADR-0073 D5).
export const WBSettingsDialog = { openFlag: "settingsOpen" };
