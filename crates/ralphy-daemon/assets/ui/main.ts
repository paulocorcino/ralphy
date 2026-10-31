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
import { wbColumns } from "./wb-consoles-tab.ts";
import { wbConsoleMenus } from "./wb-consoles-tab-menus.ts";
import { wbConsoleTabMenu } from "./wb-console-tabs.ts";
import { wbFiles } from "./wb-files.ts";
import { wbMoveDialog } from "./wb-move-dialog.ts";
import { wbUploadDialog } from "./wb-files-upload.ts";
import { wbPropertiesDialog } from "./wb-properties-dialog.ts";
import { wbRuns } from "./wb-runs-panel.ts";
import { wbBoard } from "./wb-board.ts";
import { wbChanges } from "./wb-changes-panel.ts";
import { wbBranchDialog } from "./wb-branch-dialog.ts";
import { wbSpend } from "./wb-spend-panel.ts";
import { wbAccount } from "./wb-account.ts";
import { wbLogin } from "./wb-login.ts";
import { wbAttention } from "./wb-attention.ts";
import { wbLoginNotice } from "./wb-login-notice.ts";
import { createDaemon } from "./wb-daemon.ts";
import { startPresence } from "./wb-presence.ts";
import { WBDevice } from "./wb-device.ts";
import { createViewer } from "./wb-file-viewer.ts";
import { wireDiagramZoom } from "./wb-zoom-overlay.ts";
import { createNotes } from "./wb-notes.ts";
import { createConsole } from "./wb-console.ts";
import { createMessages } from "./wb-messages.ts";
import { createStack, createGestures } from "./wb-stage-stack.ts";
import { shell } from "./app.ts";
import { wire } from "./wb-wire.ts";
import { iconDirective } from "./wb-icon.ts";
import { projectsStore } from "./wb-projects-store.ts";
import { connectionStore } from "./wb-connection-store.ts";
import { runsStore } from "./wb-runs-store.ts";
import { boardStore } from "./wb-board-store.ts";
import { checkoutsStore } from "./wb-checkouts-store.ts";
import { releaseStore } from "./wb-release-store.ts";
import { modalsStore } from "./wb-modals-store.ts";
import { tabsStore } from "./wb-tabs-store.ts";
import { uploadsStore } from "./wb-uploads-store.ts";

window.Alpine = Alpine;
// The door for operator messages first: the consoles, the cards and the
// modules after them tell the operator through it. Its flash and its Alpine
// confirm reach the store `modals` at each call, so the store may be
// registered later.
const messages = createMessages(window, document);
// The z stack and the gestures, once for this document: the consoles and the
// cards share them.
const stack = createStack(document);
const gestures = createGestures();
// The consoles, before `wire`: `app.ts` and the markup read `WBConsole`.
// Creating them starts the desk read; `boot` comes once Alpine has started.
window.WBConsole = createConsole(window, document, location, { messages, stack, gestures });
// The daemon door and the device facts, before `wire`: `app.ts` reads
// `WBDaemon`, and the facts go out once per page load.
window.WBDaemon = createDaemon(window, document, location, { messages });
WBDevice.report(window);
// The operator's presence rides the daemon's `/ws` socket, which the store
// `connection` opens.
const daemon = window.WBDaemon;
startPresence(window, document, { send: daemon.reportPresence, onOpen: daemon.onPresenceOpen });
// The file pane, before `wire`: `app.ts` reads `WBViewer`.
window.WBViewer = createViewer(window, document, { messages });
wireDiagramZoom(window, document);
// The note cards, before `wire`: `wb-console.ts` reads `WBNotes` inside functions.
window.WBNotes = createNotes(window, document, {
  console: window.WBConsole,
  messages,
  stack,
  gestures,
});
// `wire` hands back the detached file windows: the store `connection` asks
// them about unsaved work before a build reload, and the store `tabs` hands
// them a file it detaches.
const detached = wire(window, document, { messages });
Alpine.directive("icon", iconDirective);
// The modal stack, the confirm, the prompt and the flash line, first: the
// door and every store and component after it flash and ask through it. It
// has no `init`, and it never calls the door.
Alpine.store("modals", modalsStore({ messages }));
// The open project, the fleet, the agents and the live sessions, before the
// components that read them (ADR-0073 D6); its `init` reads the projects and
// the agents, and starts the peer read.
Alpine.store("projects", projectsStore({ messages }));
// The session, the presence socket and the write locks; its `init` reads the
// session, opens the presence socket and starts the presence clock.
Alpine.store("connection", connectionStore({ messages, detached }));
// The runs of each project; its `init` hears the open project change.
Alpine.store("runs", runsStore({ messages }));
// The board's issues of each project; the board component reads them.
Alpine.store("board", boardStore({ messages }));
// The selected checkout of each project; its `init` takes the `unknown
// checkout` hook and the desk's checkouts once the desk lands.
Alpine.store("checkouts", checkoutsStore({ messages }));
// The release view; its `init` reads it and hears the tab come back.
Alpine.store("release", releaseStore({ messages }));
// The canvas tabs, the slot and the split; its `init` hears a component ask
// for a tab. `detachFile` watches a popup through the detached file windows.
Alpine.store("tabs", tabsStore({ messages, detached }));
// The files the operator uploads; the upload dialog adds them, and the card
// at the bottom of the files panel shows them.
Alpine.store("uploads", uploadsStore({ messages }));
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
Alpine.data("wbConsoleTabMenu", wbConsoleTabMenu);
Alpine.data("wbFiles", wbFiles);
Alpine.data("wbMoveDialog", wbMoveDialog);
Alpine.data("wbUploadDialog", wbUploadDialog);
Alpine.data("wbPropertiesDialog", wbPropertiesDialog);
Alpine.data("wbRuns", wbRuns);
Alpine.data("wbBoard", wbBoard);
Alpine.data("wbChanges", wbChanges);
Alpine.data("wbBranchDialog", wbBranchDialog);
Alpine.data("wbSpend", wbSpend);
Alpine.data("wbAccount", wbAccount);
Alpine.data("wbLogin", wbLogin);
Alpine.data("wbAttention", wbAttention);
Alpine.data("wbLoginNotice", wbLoginNotice);
Alpine.start();
// The consoles boot on a page whose shell Alpine has built (ADR-0075 D7).
window.WBConsole.boot();
