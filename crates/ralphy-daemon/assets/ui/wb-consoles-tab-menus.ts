/* ---------------------------------------------------------------------------
   The toolbar of the Consoles tab: the Alpine component `wbConsoleMenus`,
   on the toolbar of the tab bar. It holds the New-console menu with its Run
   field, the Go-to, Fence and Note menus, and the Alt+Shift+<digit>,
   Alt+Shift+R and Alt+Shift+F<n> keys. The columns inside the tab are
   `wbColumns` in `wb-consoles-tab.ts`.

   It reaches `shell()` only through the names in its `uses` (ADR-0073 D4).
   One dropdown at a time: every menu trigger sends `workbench:menus-close`,
   and `shell()` and both components hear it and close their own menus.

   `main.ts` registers it (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { WBAgents } from "./wb-agents.ts";
import type { MenuRow } from "./wb-agents.ts";
import { WBFleet } from "./wb-fleet.ts";
import { WBProject } from "./wb-project.ts";
import { sendWindow } from "./wb-events.ts";

/** A row of the Note menu (`WBNotes.list`). */
type NoteItem = ReturnType<Window["WBNotes"]["list"]>[number];

export function wbConsoleMenus() {
  // Every `shell()` member this component's code or markup reads or calls.
  return component([], {
    agentMenu: false,
    // The Go-to picker (#337) and the fence picker (#343): SNAPSHOTS taken
    // when the menu opens, because the windows and fences live in the DOM.
    windowMenu: false,
    windowList: [] as ReturnType<typeof WBConsole.list>,
    fenceMenu: false,
    fenceItems: [] as ReturnType<typeof WBConsole.fenceList>,
    // The note picker (ADR-0064 §§9–10): a SNAPSHOT on open, like the two
    // above — the cards live in the DOM and the desk, not in Alpine state.
    noteMenu: false,
    noteItems: [] as NoteItem[],
    // Which notes have their `##` sections open in the menu, by id. Collapsed
    // is the default: a note is a document, and every heading of every note at
    // once is a wall, not a map.
    // The console row's "Run…" field: one command line for ONE new console.
    // Never stored — the next console from the row or Alt+Shift+0 is a plain
    // shell again.
    consoleRunOpen: false,
    consoleRunText: "",
    // The "New console" menu (wb-agents.ts): the roster folded against the
    // live sessions, plus a plain console pinned LAST. Each row carries an
    // Alt+Shift+<digit> accelerator, matched by physical key (e.code) so it
    // fires regardless of layout. Console is Alt+Shift+0; Alt+Shift+R opens the
    // menu with the console row's command field focused.
    consoleItems() {
      return WBAgents.menuRows({
        roster: this.$store.projects.roster,
        sessions: this.$store.projects.liveSessions,
        openSlug: this.$store.projects.openSlug,
      });
    },
    isMac: /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || ""),
    // The accelerator pattern, stated ONCE in the menu head (the Fence menu's
    // rule): each row carries only its digit.
    consoleShortcutHint() {
      return this.isMac ? "⌥⇧<n>" : "Alt+Shift+<n>";
    },
    // The menu head names the repo by its bare name — the owner and the
    // environment are the sidebar's to say, and neither changes which agent to
    // pick. The full ref rides the head's title.
    consoleMenuRepoName() {
      if (!this.$store.projects.openSlug) return "";
      const row = this.$store.projects.projects.find((p) => this.$store.projects.repoRef(p) === this.$store.projects.openSlug);
      const name = row ? WBProject.projectName(row) : WBFleet.refSlug(this.$store.projects.openSlug);
      return name.split("/").pop() || name;
    },
    // Every row is a launch (the menu is "New console"); `opts.tryAnyway` is
    // the unavailable row's escape hatch.
    openConsoleItem(item: MenuRow & { command?: string }, opts: { tryAnyway?: boolean } = {}) {
      if (!WBAgents.consoleIntent(item, opts)) return;
      if (item.plain) this.newPlainConsole(item.command);
      else this.newConsole(item.kind);
      this.agentMenu = false;
    },

    newConsole(agent: string) {
      // The accelerator path calls this directly: refuse with no repo here too.
      if (!this.$store.projects.openSlug) return;
      if (this.$store.tabs.active !== "consoles") this.$store.tabs.activate("consoles");
      // Always the primary: only the console's own title switcher moves it
      // (ADR-0063, amendment 2026-09-16 b).
      WBConsole.open({ repo: this.$store.projects.openSlug, agent, checkout: null });
    },
    // a bare shell in the repo dir (no agent) — the daemon's per-repo console;
    // with `command`, the shell runs it instead of a prompt and the session
    // ends with it (the console row's "Run…" field)
    newPlainConsole(command: string | null | undefined) {
      if (this.$store.tabs.active !== "consoles") this.$store.tabs.activate("consoles");
      WBConsole.open({ repo: this.$store.projects.openSlug, plain: true, command: command || undefined });
    },
    openConsoleRun() {
      this.consoleRunOpen = true;
      void this.$nextTick(() => this.$refs.consoleRun?.focus());
    },
    // Cancel keeps the menu open: Esc undoes only the field.
    closeConsoleRun() {
      this.consoleRunOpen = false;
      this.consoleRunText = "";
    },
    // A blank line is not a launch: the field stays open for the typing.
    runConsoleCommand() {
      const command = WBAgents.runCommand(this.consoleRunText);
      if (!command) return;
      this.newPlainConsole(command);
      this.agentMenu = false;
      this.closeConsoleRun();
    },
    // Alt+Shift+R: the menu, open (never toggled shut), with the field focused.
    // The menu lives in the Consoles tab's toolbar: on any other tab it would
    // open hidden.
    openConsoleRunMenu() {
      if (this.$store.tabs.active !== "consoles") this.$store.tabs.activate("consoles");
      this.closeMenus();
      this.agentMenu = true;
      this.openConsoleRun();
    },

    // The fence cap, stated before the click. Read off `fenceItems` (the
    // snapshot `toggleFenceMenu` takes), NOT `WBConsole.atFenceCap()`:
    // MEASURED, the module's fence array is not Alpine state, so a binding on
    // it never re-evaluated. The module stays the AUTHORITY for the gesture
    // (`newFence`), so a stale snapshot can never create a thirteenth fence.
    fenceAtCap() {
      return this.fenceItems.length >= window.WBConsole.FENCE_MAX;
    },
    fenceCapMessage() {
      return `Maximum of ${window.WBConsole.FENCE_MAX} fences. Remove one to add another.`;
    },
    fenceCapReason() {
      return this.fenceAtCap() ? this.fenceCapMessage() : "Draw a named fence on the stage";
    },
    newFence() {
      if (this.$store.tabs.active !== "consoles") this.$store.tabs.activate("consoles");
      // The menu closes BEFORE the fence is drawn, or its list goes stale.
      this.fenceMenu = false;
      // The module decides; `false` is its refusal at the cap, and saying so is
      // this layer's job (`wb-console.ts` reaches no shell).
      if (WBConsole.createFence() === false) this.$store.modals.flash(this.fenceCapMessage());
    },

    // ONE dropdown at a time: each trigger sends `workbench:menus-close`
    // before toggling its own. The shell, this toolbar and the columns list
    // each hear it and close their own menus.
    closeMenus() {
      sendWindow(window, "workbench:menus-close");
    },
    closeOwnMenus() {
      this.agentMenu = false;
      this.closeConsoleRun();
      this.windowMenu = false;
      this.fenceMenu = false;
      this.noteMenu = false;
    },
    toggleAgentMenu() {
      const was = this.agentMenu;
      this.closeMenus();
      this.agentMenu = !was;
    },
    toggleWindowMenu() {
      this.windowList = WBConsole.list();
      const was = this.windowMenu;
      this.closeMenus();
      this.windowMenu = !was;
    },
    revealWindow(id: string) {
      this.windowMenu = false;
      // A console in its own tab is shown in its tab (ADR-0037 §3d).
      const tab = this.$store.tabs.tabOf(id);
      if (tab) {
        this.$store.tabs.activate(tab.id);
        return;
      }
      if (this.$store.tabs.active !== "consoles") this.$store.tabs.activate("consoles");
      // AFTER the tab is laid out: a `display:none` tab measures 0.
      void this.$nextTick(() => WBConsole.reveal(id));
    },

    // The note cap, stated before the click — `fenceAtCap`'s shape, and for
    // the same reason: the module's array is not Alpine state, so a binding on
    // it would never re-evaluate.
    noteAtCap() {
      return this.noteItems.length >= 32;
    },
    noteCapReason() {
      if (!this.$store.projects.openSlug) return "Open a project first. A note is saved in its checkout.";
      if (this.noteAtCap()) return "Maximum of 32 notes. Close one to add another.";
      return "Write a note on the stage";
    },
    toggleNoteMenu() {
      this.noteItems = window.WBNotes.list();
      const was = this.noteMenu;
      this.closeMenus();
      this.noteMenu = !was;
    },
    // A new note (ADR-0064 §9): no dialog. The card lands in the middle of
    // what the operator is looking at, in the project's selected checkout —
    // where the file will be written is a field in the card's own footer.
    newNote() {
      const slug = this.$store.projects.openSlug;
      if (!slug) return;
      if (this.$store.tabs.active !== "consoles") this.$store.tabs.activate("consoles");
      this.noteMenu = false;
      // AFTER the tab is laid out, as `revealWindow`: a `display:none` tab
      // measures a 0×0 viewport and every card would land at the origin.
      void this.$nextTick(() => {
        const ws = document.getElementById("workspace");
        window.WBNotes.create({
          repo: slug,
          checkout: window.WBConsole.checkoutOf(slug),
          viewport: { width: ws?.clientWidth || 0, height: ws?.clientHeight || 0 },
          offset: { left: ws?.scrollLeft || 0, top: ws?.scrollTop || 0 },
        });
      });
    },

    // The note list is the map too (ADR-0064 §10): the row slides the plane to
    // the card.
    jumpNote(id: string) {
      if (this.$store.tabs.active !== "consoles") this.$store.tabs.activate("consoles");
      this.noteMenu = false;
      // As `revealWindow`: a `display:none` tab measures a 0×0 viewport.
      void this.$nextTick(() => window.WBNotes.jump(id));
    },
    // Keep a card on top, or put it back (ADR-0064, 2026-09-26 amendment).
    // The menu closes on the way on top so the card is in view; putting back
    // keeps it open and redraws the rows.
    toggleOnTop(n: NoteItem) {
      if (n.away) return;
      if (n.onTop) {
        window.WBNotes.putBack();
        this.noteItems = window.WBNotes.list();
        return;
      }
      if (this.$store.tabs.active !== "consoles") this.$store.tabs.activate("consoles");
      this.noteMenu = false;
      // As `jumpNote`: a `display:none` tab measures a 0×0 viewport.
      void this.$nextTick(() => window.WBNotes.keepOnTop(n.id));
    },

    // The fence list is the map (#343). Snapshot on open, like the Go-to picker.
    toggleFenceMenu() {
      this.fenceItems = WBConsole.fenceList();
      const was = this.fenceMenu;
      this.closeMenus();
      this.fenceMenu = !was;
    },
    jumpFence(id: string) {
      if (this.$store.tabs.active !== "consoles") this.$store.tabs.activate("consoles");
      this.fenceMenu = false;
      // As `revealWindow`: a `display:none` tab measures 0.
      void this.$nextTick(() => WBConsole.jumpToFence(id));
    },
    // Alt+Shift+F<n> → the n-th fence, up to F12 (the fence cap). MEASURED
    // (Chromium 148): Alt+Shift+F10/F11/F12 all reach the document —
    // `Shift+F10`, F11 and F12 want their exact combo, and Alt takes this out
    // of their way. The ROW carries only its own key; the modifier pair is a
    // legend in the head.
    fenceShortcutLabel(n: number) {
      return `F${n}`;
    },
    fenceShortcutHint() {
      return this.isMac ? "⌥⇧F<n>" : "Alt+Shift+F<n>";
    },

    // Ordinal, not id: the row's position in `fenceList()`, read LIVE (the
    // menu's snapshot may be stale). Returns whether it landed.
    jumpFenceAt(n: number) {
      if (this.$store.tabs.active !== "consoles") return false;
      const f = WBConsole.fenceList()[n - 1];
      if (!f) return false;
      this.fenceMenu = false;
      return !!WBConsole.jumpToFence(f.id);
    },

    // The listeners, added once when Alpine builds the toolbar.
    init() {
      window.addEventListener("workbench:menus-close", () => this.closeOwnMenus());

      // Alt+Shift+<digit> → the menu row carrying that digit, through the SAME row
      // action as a click. Matched on `e.code` so layout does not matter. R is no
      // row: it opens the menu on the console row's command field, so the digits
      // stay a sequence of rows. They work from inside a terminal too: its xterm
      // hands them over (wb-console-terminal.ts).
      document.addEventListener("keydown", (e) => {
        if (!e.altKey || !e.shiftKey || e.ctrlKey || e.metaKey) return;
        if (!/^(?:Digit\d|KeyR)$/.test(e.code)) return;
        if (this.$store.modals.consoleShortcutsBlocked(true)) return;
        if (e.code === "KeyR") {
          e.preventDefault();
          this.openConsoleRunMenu();
          return;
        }
        const row = this.consoleItems().find((it) => e.code === "Digit" + it.digit);
        // No row, or a disabled one: inert, and the key is not swallowed.
        if (!row || row.disabled) return;
        e.preventDefault();
        this.openConsoleItem(row);
      });

      // Alt+Shift+F<n> → the n-th fence, F1..F12 (the fence cap). None of the
      // reserved neighbours is hit — Alt+F4, Shift+F10, F11, F12 each want their
      // exact combo (MEASURED: with Alt+Shift held, F10–F12 reach the document).
      // With no fence at that ordinal the key is left UNSWALLOWED.
      document.addEventListener("keydown", (e) => {
        if (!e.altKey || !e.shiftKey || e.ctrlKey || e.metaKey) return;
        if (!/^F(?:[1-9]|1[0-2])$/.test(e.code)) return;
        if (this.$store.modals.consoleShortcutsBlocked()) return;
        if (!this.jumpFenceAt(Number(e.code.slice(1)))) return;
        e.preventDefault();
      });
    },
  });
}
