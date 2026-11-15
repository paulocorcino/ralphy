/* ---------------------------------------------------------------------------
   The command palette (ADR-0087 D6): a modal that lists the commands that
   can run now, from the list of `wb-commands.ts`, each with its keys.

   Alt+Shift+P opens it, from a terminal too. Typing filters, Up and Down
   move, Enter runs, Escape closes (the modal stack), and the focus goes back
   where it was. It reads nothing of `shell()`. `main.ts` registers it.
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { IS_MAC, bindCommand, keysText, paletteRows, runRow } from "./wb-commands.ts";
import type { PaletteRow } from "./wb-commands.ts";
import { sendWindow } from "./wb-events.ts";

/** The rows whose label holds every word of `query`, in the list's order. */
export function filterRows(rows: readonly PaletteRow[], query: string) {
  const words = query.toLowerCase().split(/\s+/u).filter(Boolean);
  return rows.filter((r) => words.every((w) => r.label.toLowerCase().includes(w)));
}

/** The next active row for an arrow key, wrapping at the ends. */
export function stepActive(active: number, count: number, key: string) {
  if (count === 0) return 0;
  if (key === "ArrowDown") return (active + 1) % count;
  if (key === "ArrowUp") return (active - 1 + count) % count;
  return active;
}

export function wbPalette() {
  return component([], {
    paletteOpen: false,
    query: "",
    active: 0,
    // The rows of the moment the palette opened: a command that can run now.
    all: [] as PaletteRow[],

    init() {
      bindCommand("palette.open", {
        can: () => !this.paletteOpen,
        run: () => {
          this.openPalette();
        },
      });
      bindCommand("settings.open", {
        can: () => true,
        run: () => {
          sendWindow(window, "workbench:menus-close");
          sendWindow(window, "workbench:settings-open");
        },
      });
    },

    openPalette() {
      this.all = paletteRows();
      this.query = "";
      this.active = 0;
      this.paletteOpen = true;
      void this.$nextTick(() => {
        this.$refs["paletteInput"]?.focus();
      });
    },
    closePalette() {
      this.paletteOpen = false;
    },
    rows() {
      return filterRows(this.all, this.query);
    },
    keysOf(row: PaletteRow) {
      return row.keys ? keysText(row.keys, IS_MAC) : "";
    },
    paletteKey(e: KeyboardEvent) {
      const rows = this.rows();
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        this.active = stepActive(this.active, rows.length, e.key);
        return;
      }
      if (e.key !== "Enter") return;
      e.preventDefault();
      const row = rows[this.active];
      if (row) this.pick(row);
    },
    // Closed first: the modal stack gives the focus back, and the command
    // then moves it where it goes (ADR-0087 D5).
    pick(row: PaletteRow) {
      this.closePalette();
      void this.$nextTick(() => {
        runRow(row);
      });
    },
  });
}
