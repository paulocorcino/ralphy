/* ---------------------------------------------------------------------------
   A console in its own tab (ADR-0037 §3d): the tab's id, its stored shape,
   and the + menu at the end of the tab strip.

   The tab belongs to this device: the store `tabs` keeps it, and the view
   store (`WBView`) holds it as `consoleTabs`. Its console is maximized on this
   page only, through `WBConsole.tabMax`, which never writes the desk.

   `wbConsoleTabMenu` is the + menu. Its rows are the rows of the column list
   (`WBColumns.listFold` over `WBConsole.columnRoster()`), so the + lists the
   same consoles as the column menu of a phone. `main.ts` registers it.
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { WBColumns } from "./wb-columns.ts";
import { sendWindow } from "./wb-events.ts";
import type { ColumnGroup, ColumnRow } from "./wb-columns.ts";
import type { CanvasTab } from "./wb-shell-types.d.ts";

const PREFIX = "console:";

export function consoleTabId(deskId: string) {
  return PREFIX + deskId;
}

export function isConsoleTabId(id: string) {
  return id.startsWith(PREFIX);
}

// The tabs whose pane is the stage: the fixed Consoles tab and every console tab.
export function showsStage(id: string) {
  return id === "consoles" || isConsoleTabId(id);
}

export function consoleTab(deskId: string, title: string): CanvasTab {
  return { id: consoleTabId(deskId), kind: "console", title, icon: "bi bi-terminal", closable: true, deskId };
}

export function wbConsoleTabMenu() {
  return component([], {
    open: false,
    groups: [] as ColumnGroup[],
    menuAt: { top: 0, left: 0 },
    toggle(rect: DOMRect | undefined) {
      const was = this.open;
      sendWindow(window, "workbench:menus-close");
      if (was) return;
      // No console is the one the list opens from, and no column is open:
      // every live console of the stage is a row.
      this.groups = WBColumns.listFold({ ...WBConsole.columnRoster(), columns: [], from: null, full: false });
      this.menuAt = { top: Math.round((rect?.bottom || 0) + 4), left: Math.round(rect?.left || 0) };
      this.open = true;
    },
    rowLabel(r: ColumnRow) {
      return WBColumns.rowLabel(r, window.WBConsoleName.consoleLabel);
    },
    pick(r: ColumnRow) {
      if (!r.enabled) return;
      this.open = false;
      this.$store.tabs.openConsoleTab(r.id, this.rowLabel(r));
    },
    init() {
      window.addEventListener("workbench:menus-close", () => {
        this.open = false;
      });
    },
  });
}
