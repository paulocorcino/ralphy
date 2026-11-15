/* ---------------------------------------------------------------------------
   The help screen (ADR-0087 D7): a modal that shows the keyboard shortcuts.

   `?` opens it while the focus is outside a field, an editor and a
   terminal; Alt+Shift+/ opens it from anywhere; the palette and the account
   menu have a row for it. The page shortcuts come from the list of
   `wb-commands.ts`; the keys inside the components are `COMPONENT_KEYS`. It
   reads nothing of `shell()`. `main.ts` registers it.
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { COMMANDS, IS_MAC, bindCommand, commandKeysText } from "./wb-commands.ts";
import type { CommandGroup } from "./wb-commands.ts";

/** One line of the help screen: what it does, and its keys. */
export type HelpLine = { label: string; keys: string };
export type HelpSection = { title: string; lines: HelpLine[] };

/** The keys a component keeps for itself, on this system. */
export function componentKeys(mac: boolean): HelpLine[] {
  const mod = mac ? "⌘" : "Ctrl+";
  return [
    { label: "Move in a menu; close it", keys: "↑ ↓ Home End; Escape" },
    { label: "Move between tabs", keys: "← → Home End" },
    { label: "Close a dialog; the focus stays inside it", keys: "Escape; Tab" },
    { label: "Save or cancel a name being edited", keys: "Enter; Escape" },
    { label: "Save a note or a file", keys: `${mod}S` },
    { label: "Leave a note being edited", keys: "Escape" },
    { label: "Rename in the files tree; show the properties", keys: "F2; Alt+Enter" },
    {
      label: "Copy the selection of a terminal",
      keys: mac ? "⌘C" : "Ctrl+Insert",
    },
  ];
}

const GROUPS: readonly CommandGroup[] = ["Page", "Consoles and fences"];

/** The sections of the help screen, on this system. */
export function helpSections(mac: boolean): HelpSection[] {
  const sections: HelpSection[] = GROUPS.map((title) => ({
    title,
    lines: COMMANDS.filter((c) => c.group === title && c.keys.length > 0).map((c) => ({
      label: c.label,
      keys: commandKeysText(c, mac),
    })),
  }));
  sections.push({ title: "Inside the components", lines: componentKeys(mac) });
  return sections;
}

export function wbHelp() {
  return component([], {
    helpOpen: false,
    sections: helpSections(IS_MAC),
    // F1 to F12 are the screen and sound keys of a Mac until Fn is held.
    fnNote: IS_MAC,

    init() {
      bindCommand("help.open", {
        can: () => !this.helpOpen,
        run: () => {
          this.helpOpen = true;
        },
      });
      window.addEventListener("workbench:help-open", () => {
        this.helpOpen = true;
      });
    },
    closeHelp() {
      this.helpOpen = false;
    },
  });
}
