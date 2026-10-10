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
import { wbFiles } from "./wb-files.ts";
import { wbMoveDialog } from "./wb-move-dialog.ts";
import { createDaemon } from "./wb-daemon.ts";
import { WBDevice } from "./wb-device.ts";
import { createViewer } from "./wb-file-viewer.ts";
import { createNotes } from "./wb-notes.ts";
import { createConsole } from "./wb-console.ts";
import { createMessages } from "./wb-messages.ts";
import { iconDirective, shell, wire } from "./app.ts";
import { projectsStore } from "./wb-projects-store.ts";

window.Alpine = Alpine;
// The door for operator messages first: the consoles, the cards and the
// modules after them tell the operator through it. The shell is read at each
// call, so it may be built later.
const messages = createMessages(window, document, { shell: () => window.getShell?.() ?? null });
// The consoles, before `wire`: `app.ts` and the markup read `WBConsole`.
// Creating them starts the desk read; `boot` comes once Alpine has started.
window.WBConsole = createConsole(window, document, location, { messages });
// The daemon door and the device facts, before `wire`: `app.ts` reads
// `WBDaemon`, and the facts go out once per page load.
window.WBDaemon = createDaemon(window, document, location, { messages });
WBDevice.report(window);
// The file pane, before `wire`: `app.ts` reads `WBViewer`.
window.WBViewer = createViewer(window, document, { messages });
// The note cards, before `wire`: `wb-console.ts` reads `WBNotes` inside functions.
window.WBNotes = createNotes(window, document, { console: window.WBConsole, messages });
wire(window, document, { messages });
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
Alpine.data("wbFiles", wbFiles);
Alpine.data("wbMoveDialog", wbMoveDialog);
Alpine.start();
// The consoles boot on a page whose shell Alpine has built (ADR-0075 D7).
window.WBConsole.boot();
