/* ---------------------------------------------------------------------------
   The entry module of `index.html` (ADR-0075 D5): the page's one module tag.

   It imports the parts of the page, registers each Alpine component and
   directive, and starts Alpine once. A module tag runs after the classic
   vendor scripts, so every vendor name is in place when Alpine starts.
   --------------------------------------------------------------------------- */
import Alpine from "./vendor/alpine.esm.js";
// In the order of the page's old module tags. The two bare imports set the
// `window` names `app.ts` and browser checks read (D9): not dead code.
import { devices } from "./wb-devices.ts";
import { settingsDialog } from "./wb-settings-dialog.ts";
import "./wb-kanban.ts";
import "./wb-spend.ts";
import { releaseDialogs } from "./wb-release-dialogs.ts";
import { hostsDialog } from "./wb-hosts-dialog.ts";
import { addProjectDialog } from "./wb-add-project-dialog.ts";
import { securityDialog } from "./wb-security-dialog.ts";
import { wbColumns, wbConsoleMenus } from "./wb-consoles-tab.ts";
import { createDaemon } from "./wb-daemon.ts";
import { WBDevice } from "./wb-device.ts";
import { createViewer } from "./wb-viewer.ts";
import { createNotes } from "./wb-notes.ts";
import { createConsole } from "./wb-console.ts";
import { iconDirective, shell, wire } from "./app.ts";
import { projectsStore } from "./wb-projects-store.ts";

window.Alpine = Alpine;
// The consoles first, before `wire`: `app.ts` and the markup read `WBConsole`.
// Creating them starts the desk read; `boot` comes once Alpine has started.
window.WBConsole = createConsole(window, document, location, {});
// The daemon door and the device facts, before `wire`: `app.ts` reads
// `WBDaemon`, and the facts go out once per page load.
window.WBDaemon = createDaemon(window, document, location);
WBDevice.report(window);
// The file pane, before `wire`: `app.ts` reads `WBViewer`.
window.WBViewer = createViewer(window, document);
// The note cards, before `wire`: `wb-console.ts` reads `WBNotes` inside functions.
window.WBNotes = createNotes(window, document);
wire(window, document);
Alpine.directive("icon", iconDirective);
// The open project, before the components that read it (ADR-0073 D6).
Alpine.store("projects", projectsStore());
// Each name is the `x-data` of one element in `index.html`.
Alpine.data("shell", shell);
Alpine.data("wbSettingsDialog", settingsDialog);
Alpine.data("wbDevices", devices);
Alpine.data("wbSecurityDialog", securityDialog);
Alpine.data("wbReleaseDialogs", releaseDialogs);
Alpine.data("wbAddProjectDialog", addProjectDialog);
Alpine.data("wbHostsDialog", hostsDialog);
Alpine.data("wbConsoleMenus", wbConsoleMenus);
Alpine.data("wbColumns", wbColumns);
Alpine.start();
// The consoles boot on a page whose shell Alpine has built (ADR-0075 D7).
window.WBConsole.boot();
