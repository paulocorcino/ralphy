/* ---------------------------------------------------------------------------
   The Consoles tab's own state: two Alpine components, one for each place of
   the page that shows it.

   `wbConsoleMenus` is on the toolbar of the tab bar: the
   New-console menu with its Run field, the Fence and Note menus, and the
   Alt+Shift+<digit>, Alt+Shift+R and Alt+Shift+F<n> keys. `wbColumns` is
   inside the Consoles tab: the columns beside a maximized console
   (ADR-0051 §5), their list, the count and stage pills of the footer, the
   console module's events, and the Alt+Shift+arrow keys.

   Each reaches `shell()` only through the names in its `uses` (ADR-0073 D4).
   One dropdown at a time: every menu trigger sends `workbench:menus-close`,
   and `shell()` and both components hear it and close their own menus.

   `main.ts` registers both (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { WBAgents } from "./wb-agents.ts";
import { WBColumns } from "./wb-columns.ts";
import { WBFleet } from "./wb-fleet.ts";
import { WBProject } from "./wb-project.ts";

export function wbConsoleMenus() {
  // Every `shell()` member this component's code or markup reads or calls.
  return component(["active", "activate", "_flashAction", "roster", "liveSessions", "sessionsError", "consoleShortcutsBlocked"], {
    agentMenu: false,
    // The Go-to picker (#337) and the fence picker (#343): SNAPSHOTS taken
    // when the menu opens, because the windows and fences live in the DOM.
    windowMenu: false,
    windowList: [],
    fenceMenu: false,
    fenceItems: [],
    // The note picker (ADR-0064 §§9–10): a SNAPSHOT on open, like the two
    // above — the cards live in the DOM and the desk, not in Alpine state.
    noteMenu: false,
    noteItems: [],
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
        roster: this.roster,
        sessions: this.liveSessions,
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
    openConsoleItem(item: any, opts: any = {}) {
      if (!WBAgents.consoleIntent(item, opts)) return;
      if (item.plain) this.newPlainConsole(item.command);
      else this.newConsole(item.kind);
      this.agentMenu = false;
    },

    newConsole(agent: any) {
      // The accelerator path calls this directly: refuse with no repo here too.
      if (!this.$store.projects.openSlug) return;
      if (this.active !== "consoles") this.activate("consoles");
      // Always the primary: only the console's own title switcher moves it
      // (ADR-0063, amendment 2026-09-16 b).
      WBConsole.open({ repo: this.$store.projects.openSlug, agent, checkout: null });
    },
    // a bare shell in the repo dir (no agent) — the daemon's per-repo console;
    // with `command`, the shell runs it instead of a prompt and the session
    // ends with it (the console row's "Run…" field)
    newPlainConsole(command: any) {
      if (this.active !== "consoles") this.activate("consoles");
      WBConsole.open({ repo: this.$store.projects.openSlug, plain: true, command: command || undefined });
    },
    openConsoleRun() {
      this.consoleRunOpen = true;
      this.$nextTick(() => this.$refs.consoleRun?.focus());
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
      if (this.active !== "consoles") this.activate("consoles");
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
      if (this.active !== "consoles") this.activate("consoles");
      // The menu closes BEFORE the fence is drawn, or its list goes stale.
      this.fenceMenu = false;
      // The module decides; `false` is its refusal at the cap, and saying so is
      // this layer's job (`wb-console.ts` reaches no shell).
      if (WBConsole.createFence() === false) this._flashAction(this.fenceCapMessage());
    },

    // ONE dropdown at a time: each trigger sends `workbench:menus-close`
    // before toggling its own. The shell, this toolbar and the columns list
    // each hear it and close their own menus.
    closeMenus() {
      window.dispatchEvent(new CustomEvent("workbench:menus-close"));
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
    revealWindow(id: any) {
      if (this.active !== "consoles") this.activate("consoles");
      this.windowMenu = false;
      // AFTER the tab is laid out: a `display:none` tab measures 0.
      this.$nextTick(() => WBConsole.reveal(id));
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
      if (!this.$store.projects.openSlug) return;
      if (this.active !== "consoles") this.activate("consoles");
      this.noteMenu = false;
      // AFTER the tab is laid out, as `revealWindow`: a `display:none` tab
      // measures a 0×0 viewport and every card would land at the origin.
      this.$nextTick(() => {
        const ws = document.getElementById("workspace");
        window.WBNotes.create({
          repo: this.$store.projects.openSlug,
          checkout: window.WBConsole.checkoutOf(this.$store.projects.openSlug),
          viewport: { width: ws?.clientWidth || 0, height: ws?.clientHeight || 0 },
          offset: { left: ws?.scrollLeft || 0, top: ws?.scrollTop || 0 },
        });
      });
    },

    // The note list is the map too (ADR-0064 §10): the row slides the plane to
    // the card.
    jumpNote(id: any) {
      if (this.active !== "consoles") this.activate("consoles");
      this.noteMenu = false;
      // As `revealWindow`: a `display:none` tab measures a 0×0 viewport.
      this.$nextTick(() => window.WBNotes.jump(id));
    },
    // Keep a card on top, or put it back (ADR-0064, 2026-09-26 amendment).
    // The menu closes on the way on top so the card is in view; putting back
    // keeps it open and redraws the rows.
    toggleOnTop(n: any) {
      if (n.away) return;
      if (n.onTop) {
        window.WBNotes.putBack();
        this.noteItems = window.WBNotes.list();
        return;
      }
      if (this.active !== "consoles") this.activate("consoles");
      this.noteMenu = false;
      // As `jumpNote`: a `display:none` tab measures a 0×0 viewport.
      this.$nextTick(() => window.WBNotes.keepOnTop(n.id));
    },

    // The fence list is the map (#343). Snapshot on open, like the Go-to picker.
    toggleFenceMenu() {
      this.fenceItems = WBConsole.fenceList();
      const was = this.fenceMenu;
      this.closeMenus();
      this.fenceMenu = !was;
    },
    jumpFence(id: any) {
      if (this.active !== "consoles") this.activate("consoles");
      this.fenceMenu = false;
      // As `revealWindow`: a `display:none` tab measures 0.
      this.$nextTick(() => WBConsole.jumpToFence(id));
    },
    // Alt+Shift+F<n> → the n-th fence, up to F12 (the fence cap). MEASURED
    // (Chromium 148): Alt+Shift+F10/F11/F12 all reach the document —
    // `Shift+F10`, F11 and F12 want their exact combo, and Alt takes this out
    // of their way. The ROW carries only its own key; the modifier pair is a
    // legend in the head.
    fenceShortcutLabel(n: any) {
      return `F${n}`;
    },
    fenceShortcutHint() {
      return this.isMac ? "⌥⇧F<n>" : "Alt+Shift+F<n>";
    },

    // Ordinal, not id: the row's position in `fenceList()`, read LIVE (the
    // menu's snapshot may be stale). Returns whether it landed.
    jumpFenceAt(n: any) {
      if (this.active !== "consoles") return false;
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
        if (this.consoleShortcutsBlocked(true)) return;
        if (e.code === "KeyR") {
          e.preventDefault();
          this.openConsoleRunMenu();
          return;
        }
        const row = this.consoleItems().find((it: any) => e.code === "Digit" + it.digit);
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
        if (this.consoleShortcutsBlocked()) return;
        if (!this.jumpFenceAt(Number(e.code.slice(1)))) return;
        e.preventDefault();
      });
    },
  });
}

export function wbColumns() {
  // Every `shell()` member this component's code or markup reads or calls.
  return component(["active", "_flashAction", "consoleShortcutsBlocked"], {
    // Columns beside a maximized console (ADR-0051 §5): per-client view
    // state, never desk state. The ids left to right; empty whenever fewer
    // than two remain, so a lone survivor is an ordinary maximize again.
    columns: [],
    columnMenu: false,
    consoleCount: 0,
    // The stage extent, for the footer pill (#338).
    stageW: 0,
    stageH: 0,
    columnDir: "right",
    _columnsRestored: false,
    _paintedKey: "",
    columnGroups: [] as any[],
    columnFilter: "",
    columnFrom: null,
    columnMenuAt: { top: 0, right: 0, maxWidth: 400, maxHeight: 400 },
    // INVARIANT: the shell never writes `max` itself. Each `applyColumns` call
    // passes `persist`, so `setMax` writes it for the first console in reading
    // order (`true`) or one that stopped being first (`false`). `columns` is
    // the grid: a list of columns, each a list of ids (`wb-columns.ts`).
    columnIds() {
      return WBColumns.flat(this.columns);
    },
    columnCap() {
      return WBColumns.cap(WBConsole.columnMeasure().viewport, WBConsole.PHONE_MAX_WIDTH);
    },
    // The ONE writer of `columns`. The view store is written only when the grid
    // changes: `paintColumns` runs on every `consoles-changed` during boot with
    // an empty grid, and an unconditional write would erase the stored grid
    // before `restoreColumns` reads it.
    setColumns(next: any) {
      if (JSON.stringify(next) === JSON.stringify(this.columns)) return;
      this.columns = next;
      window.WBView?.patch({ columns: WBColumns.toStored(next) });
    },
    // Once, on `workbench:desk-restored`. A list the desk does not confirm is
    // ignored and cleared from the store; a kept one is painted with `persist`.
    restoreColumns() {
      if (this._columnsRestored) return;
      this._columnsRestored = true;
      const stored = window.WBView?.read();
      this.columnDir = WBColumns.dirOf(stored?.columnDir);
      const raw = stored?.columns ?? null;
      const next = WBColumns.fromStored(raw, WBConsole.deskRecords());
      this.setColumns(next);
      // `setColumns` writes only a change; an ignored grid meets an empty one.
      // A flat list stored before rows is written back as a grid.
      if (next.length && JSON.stringify(raw) !== JSON.stringify(next)) {
        window.WBView?.patch({ columns: WBColumns.toStored(next) });
      }
      if (!next.length && raw !== null) window.WBView?.patch({ columns: null });
      if (next.length) this.paintColumns({ raise: true });
    },
    effectiveColumns(fromId: any) {
      return this.columnIds().includes(fromId) ? this.columns : [[fromId]];
    },
    // The Right | Down choice at the top of the list, kept in this browser.
    setColumnDir(dir: any) {
      this.columnDir = WBColumns.dirOf(dir);
      window.WBView?.patch({ columnDir: this.columnDir });
    },
    // Re-derive what is painted from the list and the current cap. A console
    // that left the stage (closed, detached) leaves the list.
    paintColumns(opts?: any) {
      const byId = new Map(
        [...document.querySelectorAll<any>("#stage .session-window")].map((w) => [w._deskId, w]),
      );
      const head = this.columnIds()[0];
      const headWin = head ? byId.get(head) : null;
      // At a cap of 1 the first console is painted as a plain maximize, so its
      // Restore took the maximize path. It is still a column restore.
      if (headWin && !headWin.classList.contains("maximized") && !headWin.classList.contains("column")) {
        this.restoreColumn(head);
        return;
      }
      // A first console off the stage for a relaunch comes back under the same id.
      if (head && !headWin && WBConsole.isRelaunching(head)) return;
      const kept = WBColumns.keep(this.columns, new Set(byId.keys()));
      const keptIds = WBColumns.flat(kept);
      // The first console left the stage and one is left: it takes the maximize.
      if (head && !headWin && keptIds.length === 1) {
        const cap = this.columnCap();
        this.setColumns([]);
        WBConsole.applyColumns(WBColumns.painted(kept, cap), { cap, unmax: null, persist: true });
        return;
      }
      // The KEPT grid is stored, never the painted slice: a console the cap
      // hides comes back when the cap grows again.
      this.setColumns(keptIds.length >= 2 ? kept : []);
      const left =
        this.columnIds()[0] ?? document.querySelector<any>("#stage .session-window.maximized")?._deskId;
      // A hidden consoles tab measures 0 wide, which reads as a cap of 1: keep
      // the painted columns as they are until the tab shows again.
      if (left && !WBConsole.columnMeasure().viewport) return;
      const cap = left ? this.columnCap() : 1;
      const before = WBConsole.focusedId();
      const painted = WBColumns.painted(this.columns, cap);
      const ids = painted.map((p: any) => p.id);
      const key = ids.join(" ");
      // A column that stops being painted falls back to its plane rect with its
      // old z-index; raising the painted ones keeps it behind them.
      const moved = this.columnIds().length >= 2 && key !== this._paintedKey;
      this._paintedKey = key;
      WBConsole.applyColumns(painted, { cap, unmax: null, raise: !!opts?.raise || moved, persist: true });
      // Only when the keys are not somewhere else (a search box, a modal).
      const el = document.activeElement;
      const keysFree = !el || el === document.body || !!el.closest?.(".session-window");
      if (this.active === "consoles" && keysFree && this.columnIds().includes(before)) {
        const want = WBColumns.focusAfter(ids, before);
        if (want && (want !== before || moved)) WBConsole.focusColumn(want);
      }
    },
    // A fence detached to a popup takes its consoles out of the columns.
    leaveColumns(ids: any) {
      const r = WBColumns.external(this.columns, { type: "detached", ids });
      if (!r.changed) return;
      const cap = r.columns.length ? this.columnCap() : 1;
      this.setColumns(r.ended ? [] : r.columns);
      WBConsole.applyColumns(WBColumns.painted(r.columns, cap), { cap, unmax: r.unmax, raise: true, persist: true });
    },
    // Consoles whose records another client removed (`ids`, from the console
    // module after a desk read) leave the columns, then the stage. A session
    // that ended, a remote maximize and a remote rect or fence change need
    // nothing here: `WBColumns.external` names them as no-ops.
    checkColumnDesk(ids: any) {
      const gone = this.columnIds().filter((id: any) => ids.includes(id));
      const r = WBColumns.external(this.columns, { type: "closed", ids: gone });
      if (r.changed) {
        const cap = r.columns.length ? this.columnCap() : 1;
        this.setColumns(r.ended ? [] : r.columns);
        // Painted BEFORE the drops, so a lone survivor is maximized first.
        WBConsole.applyColumns(WBColumns.painted(r.columns, cap), { cap, unmax: null, raise: true, persist: true });
      }
      for (const id of ids) WBConsole.dropClosedElsewhere(id);
      if (r.changed) this.paintColumns();
    },
    toggleColumnMenu(id: any, rect: any) {
      const was = this.columnMenu && this.columnFrom === id;
      const ids = WBColumns.flat(this.effectiveColumns(id));
      this.columnGroups = WBColumns.listFold({
        ...WBConsole.columnRoster(),
        columns: ids,
        from: id,
        full: ids.length >= this.columnCap(),
      });
      this.columnFrom = id;
      const top = Math.round((rect?.bottom || 0) + 4);
      const right = Math.round(rect?.right || 0);
      // Right edge on the button's right edge, and no larger than the room
      // left of it and under it: the list grows with its longest row, and a
      // fixed guess at its width pushed it past the edge of the window.
      this.columnMenuAt = {
        top,
        right: Math.max(8, window.innerWidth - right),
        maxWidth: Math.max(200, right - 8),
        maxHeight: Math.max(120, window.innerHeight - top - 8),
      };
      this.columnFilter = "";
      this.closeMenus();
      this.columnMenu = !was;
      if (this.columnMenu) this.$nextTick(() => this.$refs.columnFilter?.focus());
    },
    columnFilterShown() {
      return this.columnGroups.reduce((n, g) => n + g.rows.length, 0) >= WBColumns.FILTER_MIN;
    },
    // `owner/repo` without the environment: the operator already knows where
    // each console runs, and the list is about telling the consoles apart.
    columnRepoLabel(ref: any) {
      const row = this.$store.projects.projects.find((p) => this.$store.projects.repoRef(p) === ref);
      return row ? WBProject.projectName(row) : WBFleet.refLabel(ref);
    },
    columnView() {
      return WBColumns.filterGroups(this.columnGroups, this.columnFilter, (ref: any) => this.columnRepoLabel(ref));
    },
    columnRowLabel(r: any) {
      return WBColumns.rowLabel(r, window.WBConsoleName.consoleLabel);
    },
    // Enter in the filter opens the first row that can be opened; at the cap,
    // it swaps in the first row that can be swapped.
    openFirstColumn() {
      const rows = this.columnView().flatMap((g: any) => g.rows);
      const open = rows.find((r: any) => r.enabled);
      if (open) return this.openColumn(open.id);
      const swap = rows.find((r: any) => r.swappable);
      if (swap) this.swapColumn(swap.id);
    },
    openColumn(id: any) {
      const from = this.columnFrom;
      if (!from) return;
      const cols = this.effectiveColumns(from);
      const out = WBColumns.open(cols, from, id, this.columnCap(), this.columnDir);
      if (!out.ok) {
        if (out.reason) this._flashAction(out.reason);
        return;
      }
      this.setColumns(out.columns);
      this.columnMenu = false;
      this.paintColumns({ raise: true });
      WBConsole.focusColumn(id);
    },
    // Put `id` in the row that opened the list (ADR-0051 §5, swap).
    swapColumn(id: any) {
      const from = this.columnFrom;
      if (!from) return;
      const r = WBColumns.swap(this.effectiveColumns(from), from, id);
      if (!r.ok) return;
      const cap = this.columnCap();
      this.setColumns(r.ended ? [] : r.columns);
      this.columnMenu = false;
      WBConsole.applyColumns(WBColumns.painted(r.columns, cap), { cap, unmax: r.unmax, raise: true, persist: true });
      this.paintColumns();
      WBConsole.focusColumn(id);
    },
    restoreColumn(id: any) {
      const r = WBColumns.restore(this.columns, id);
      const cap = r.columns.length ? this.columnCap() : 1;
      this.setColumns(r.ended ? [] : r.columns);
      // It may promote a lone survivor to the maximize.
      WBConsole.applyColumns(WBColumns.painted(r.columns, cap), { cap, unmax: r.unmax, raise: true, persist: true });
      this.paintColumns();
    },
    // A fence opened as columns (ADR-0051 §5, 2026-09-30): the grid follows the
    // members' stage rects and replaces the columns that are open.
    columnsFromFence(items: any) {
      const grid = WBColumns.fromRects(items);
      const ids = WBColumns.flat(grid);
      if (!ids.length) return;
      const old =
        this.columnIds()[0] ?? document.querySelector<any>("#stage .session-window.maximized")?._deskId;
      const cap = ids.length >= 2 ? this.columnCap() : 1;
      this.setColumns(ids.length >= 2 ? grid : []);
      WBConsole.applyColumns(WBColumns.painted(grid, cap), {
        cap,
        unmax: old && !ids.includes(old) ? old : null,
        raise: true,
        persist: true,
      });
      this.paintColumns();
      WBConsole.focusColumn(ids[0]);
    },

    // Alt+Shift+←/→. Returns the fence landed on, or null when the plane has
    // none (the shortcut decides whether to swallow the key). Runs against the
    // LIVE stage.
    stepFence(step: any) {
      if (this.active !== "consoles") return null;
      return WBConsole.stepFence(step);
    },
    // Alt+Shift+arrows among the painted consoles: "x" across the columns,
    // "y" along the rows of one column. Returns whether it applied.
    stepColumn(axis: any, step: any) {
      if (this.active !== "consoles" || this.columnIds().length < 2) return false;
      const painted = WBColumns.painted(this.columns, this.columnCap());
      const to = WBColumns.focusMove(painted, WBConsole.focusedId(), axis, step);
      if (to) WBConsole.focusColumn(to);
      return true;
    },
    // The columns while they are open, the fences otherwise (ADR-0051 §5). The
    // fences have no "y": ↑/↓ with no columns open applies nothing.
    arrowStep(axis: any, step: any) {
      if (this.columnIds().length >= 2) return this.stepColumn(axis, step);
      return axis === "x" && !!this.stepFence(step);
    },

    closeOwnMenus() {
      this.columnMenu = false;
    },
    // ONE dropdown at a time (see `wbConsoleMenus`).
    closeMenus() {
      window.dispatchEvent(new CustomEvent("workbench:menus-close"));
    },

    // The listeners, added once when Alpine builds the consoles tab.
    init() {
      window.addEventListener("workbench:menus-close", () => this.closeOwnMenus());
      // A console whose record another client removed leaves the columns
      // before it leaves the stage.
      window.WBConsole?.setDeskGoneHook?.((ids: any) => this.checkColumnDesk(ids));

      // The Alpine mirror of the live console count.
      document.addEventListener("workbench:consoles-changed", (e: any) => {
        this.consoleCount = e.detail.count;
        this.paintColumns();
      });

      // A console's title bar asked for the columns list, or to restore a column;
      // or something changed the cap (maximize, first measurable frame).
      document.addEventListener("workbench:column-open", (e: any) => {
        this.toggleColumnMenu(e.detail.id, e.detail.rect);
      });
      document.addEventListener("workbench:column-restore", (e: any) => {
        this.restoreColumn(e.detail.id);
      });
      document.addEventListener("workbench:columns-stale", () => {
        this.paintColumns();
      });
      document.addEventListener("workbench:columns-leave", (e: any) => {
        this.leaveColumns(e.detail.ids);
      });
      document.addEventListener("workbench:fence-columns", (e: any) => {
        this.columnsFromFence(e.detail.items);
      });
      document.addEventListener("workbench:desk-restored", () => {
        this.restoreColumns();
      });
      // A narrower or wider viewport changes the cap. One repaint per frame.
      let columnsFrame = 0;
      window.addEventListener("resize", () => {
        if (columnsFrame) return;
        columnsFrame = requestAnimationFrame(() => {
          columnsFrame = 0;
          this.paintColumns();
        });
      });

      // …and of the stage extent, for the footer pill (#338).
      document.addEventListener("workbench:stage-extent", (e: any) => {
        this.stageW = e.detail.width;
        this.stageH = e.detail.height;
      });

      // Alt+Shift+arrows → walk the columns (←/→) and the rows of a column (↑/↓)
      // while two or more consoles are open, from inside a column's terminal too;
      // otherwise ←/→ walk the fences in reading order (`fenceCycle`) — ADR-0051 §5.
      // With nothing to walk the key is left UNSWALLOWED.
      const ARROW_STEPS = {
        ArrowRight: ["x", 1],
        ArrowLeft: ["x", -1],
        ArrowDown: ["y", 1],
        ArrowUp: ["y", -1],
      };
      document.addEventListener("keydown", (e) => {
        if (!e.altKey || !e.shiftKey || e.ctrlKey || e.metaKey) return;
        const move = ARROW_STEPS[e.code as keyof typeof ARROW_STEPS];
        if (!move) return;
        if (this.consoleShortcutsBlocked(this.columnIds().length >= 2)) return;
        if (!this.arrowStep(move[0], move[1])) return;
        e.preventDefault();
      });
    },
  });
}
