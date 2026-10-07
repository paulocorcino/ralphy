/* ---------------------------------------------------------------------------
   The entry module of `index.html` (ADR-0075 D5): the page's one module tag.

   It imports the parts of the page, registers each Alpine component and
   directive, and starts Alpine once. A module tag runs after the classic
   scripts, so every classic name is in place when Alpine starts.
   --------------------------------------------------------------------------- */
import Alpine from "./vendor/alpine.esm.js";
// In the order of the page's old module tags. A module sets the `window` name
// that a classic script still reads (D9), so a bare import is not dead code.
import "./wb-fail.ts";
import "./wb-agents.ts";
import "./wb-changes.ts";
import "./wb-fleet.ts";
import "./wb-project.ts";
import "./wb-file-search.ts";
import { devices } from "./wb-devices.ts";
import "./wb-split.ts";
import "./wb-settings.ts";
import { settingsDialog } from "./wb-settings-dialog.ts";
import "./wb-desk-history.ts";
import "./wb-runs.ts";
import "./wb-kanban.ts";
import "./wb-spend.ts";
import "./wb-release.ts";
import { releaseDialogs } from "./wb-release-dialogs.ts";
import { hostsDialog } from "./wb-hosts-dialog.ts";
import "./wb-add-project.ts";
import { addProjectDialog } from "./wb-add-project-dialog.ts";
import { securityDialog } from "./wb-security-dialog.ts";
import "./wb-columns.ts";
import "./wb-monaco.ts";
import { createDaemon } from "./wb-daemon.ts";
import { WBDevice } from "./wb-device.ts";
import { iconDirective, shell, wire } from "./app.ts";

window.Alpine = Alpine;
// The daemon door and the device facts, before `wire`: `app.ts` reads
// `WBDaemon`, and the facts go out once per page load.
window.WBDaemon = createDaemon(window, document, location);
WBDevice.report(window);
wire(window, document);
Alpine.directive("icon", iconDirective);
// Each name is the `x-data` of one element in `index.html`.
Alpine.data("shell", shell);
Alpine.data("wbSettingsDialog", settingsDialog);
Alpine.data("wbDevices", devices);
Alpine.data("wbSecurityDialog", securityDialog);
Alpine.data("wbReleaseDialogs", releaseDialogs);
Alpine.data("wbAddProjectDialog", addProjectDialog);
Alpine.data("wbHostsDialog", hostsDialog);
Alpine.start();
