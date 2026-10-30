import type { Shell } from "./app.ts";
import { shell } from "./app.ts";
import { WBFail } from "./wb-fail.ts";
import { classify, fileTabId, newEntryTitle, parentRel } from "./wb-file-paths.ts";
import { WBReleaseDialogs } from "./wb-release-dialogs.ts";
import { WBSecurityDialog } from "./wb-security-dialog.ts";
import { WBSettingsDialog } from "./wb-settings-dialog.ts";
import { WBSplit } from "./wb-split.ts";
import { isFilePopupMessage } from "./wb-detached.ts";
import type { FilePopupMessage } from "./wb-detached.ts";
import { createEmitter, forwardAction, sendWindow } from "./wb-events.ts";
import type { SavePayload, WireDeps } from "./wb-shell-types.d.ts";

// Everything the page wires at load: the window names other code reads,
// the document and window listeners, and the state of detached windows.
// `window` and `document` are parameters so a test passes its own stubs, and
// each call starts from fresh state (ADR-0075 D7).
export function wire(window: Window, document: Document, deps: WireDeps) {
  const { messages } = deps;
  // The one exit point: every gesture becomes a `workbench:action` event.
  // Each page sets its own `window.WB`, and its modules read it (ADR-0075 D9).
  window.WB = createEmitter(document);
  window.shell = shell;

  // The live Alpine component instance. On `window` explicitly: the
  // terminal checks that it exists, and a bare declaration would reach it only
  // by accident of global scope.
  window.getShell = function getShell() {
    const root = document.querySelector<HTMLElement & { _x_dataStack?: Shell[] }>("[x-data]");
    return root?._x_dataStack?.[0] ?? null;
  };

  // A viewer asked to detach → open the popup and close the tab.
  document.addEventListener("workbench:detach-request", (e) =>
    window.Alpine.store("tabs").detachFile(e.detail),
  );

  // A rendered markdown link asked for a repo file → open (or focus) its tab.
  document.addEventListener("workbench:open-request", (e) =>
    window.Alpine.store("tabs").openLink(e.detail),
  );

  // The divider between the two panes was dragged: the ratio is view state.
  document.addEventListener("workbench:split-ratio", (e) => {
    const sh = window.Alpine.store("tabs");
    sh.splitRatioChanged(e.detail.ratio);
    // Re-told, not only stored: the viewer repaints `--wb-split` from the ratio
    // it was last given, and a repaint can come before the next activation.
    sh.syncViewer();
    sh.persistView();
  });

  // The canvas resized. Only a crossing of the split's width floor changes what
  // is painted, so the fold reruns on the crossing alone — not per pixel.
  let wbCanvasWide: boolean | null = null;
  document.addEventListener("workbench:canvas-resize", (e) => {
    const sh = window.Alpine.store("tabs");
    const wide = WBSplit.available(e.detail.width);
    if (wide === wbCanvasWide) return;
    wbCanvasWide = wide;
    if (sh.slot) sh.syncViewer();
  });

  // The popups this shell opened. Membership is the authorisation for every
  // message below.
  const detachedWindows = new Map<Window, FileDescriptor>();

  // A popup that closes sends its bytes home on unload (`wb-reattach`). One that
  // dies without an unload event (a crashed or killed renderer) is found by this
  // poll and comes home with the descriptor it was detached with. The poll acts
  // on the SECOND tick that sees it closed: the unload message carries the
  // edited bytes and must win over the detach-time copy.
  const detachedClosedSeen = new Set<Window>();
  let detachedPoll: number | null = null;

  function watchDetached(win: Window, desc: FileDescriptor) {
    detachedWindows.set(win, desc);
    if (!detachedPoll) detachedPoll = window.setInterval(pollDetached, 500);
  }

  function pollDetached() {
    for (const [win, desc] of [...detachedWindows]) {
      if (!win.closed) continue;
      if (detachedClosedSeen.has(win)) reattachFile(win, desc);
      else detachedClosedSeen.add(win);
    }
  }

  // Whether a detached file window holds an edit not yet saved. The popups
  // are same-origin windows this shell opened, so it asks their viewer directly.
  function detachedDirty() {
    for (const win of detachedWindows.keys()) {
      if (!win.closed && win.WBViewer?.anyDirty?.()) return true;
    }
    return false;
  }

  // Close every detached file window; each one's unload sends its file home.
  function closeDetached() {
    for (const win of detachedWindows.keys()) if (!win.closed) win.close();
  }

  const detached = { watch: watchDetached, dirty: detachedDirty, close: closeDetached };

  // The one way a detached file comes home: the button, the popup's unload and
  // the poll all end here. Closing the popup matters after an F5 inside it: the
  // unload sent the file home, and the reloaded page has nothing left to show.
  function reattachFile(win: Window, desc: FileDescriptor) {
    // The pin comes home with the bytes (#406): explicit `null` is the primary.
    window.Alpine.store("tabs").openTab({
      project: desc.project,
      path: desc.path,
      title: desc.path.split("/").pop() ?? desc.path,
      ftype: desc.ftype,
      content: desc.content,
      checkout: desc.checkout ?? null,
      encoding: desc.encoding,
      bom: desc.bom,
    });
    detachedWindows.delete(win);
    detachedClosedSeen.delete(win);
    if (!detachedWindows.size) {
      window.clearInterval(detachedPoll);
      detachedPoll = null;
    }
    if (!win.closed) win.close();
  }

  // Messages from detached popups. Both guards matter: `e.origin` refuses a
  // page on another origin, `e.source` a same-origin window we did not open.
  // Without them this listener accepted `file.write` from anyone holding a
  // handle to this window.
  window.addEventListener("message", (e) => {
    if (e.origin !== window.location.origin) return;
    if (!detachedWindows.has(e.source as Window)) return;
    const m: unknown = e.data;
    if (!isFilePopupMessage(m)) return;
    popupMessage(m, e);
  });

  // One message of a detached file window this page opened.
  function popupMessage(m: FilePopupMessage, e: MessageEvent) {
    if (m.type === "wb-detach-ready") {
      // The popup booted and is asking for its file.
      (e.source as Window).postMessage(
        { type: "wb-detach-open", desc: detachedWindows.get(e.source as Window) },
        window.location.origin,
      );
    } else if (m.type === "wb-emit") {
      // `fromWindow` lets a save's answer reach the pane that sent it.
      forwardAction(window.WB, m.action, m.detail, e.source as Window);
    } else if (m.type === "wb-open-request" && m.detail) {
      // A link clicked inside a detached pane; `openLink` re-classifies, so the
      // popup decides nothing about what opens.
      window.Alpine.store("tabs").openLink({
        project: m.detail.project,
        path: m.detail.path,
        fragment: m.detail.fragment,
        checkout: m.detail.checkout ?? null,
      });
    } else if (m.type === "wb-reattach" && m.desc) {
      // A second `wb-reattach` from the same popup (the button, then its own
      // unload) never gets here: the guard above drops a window no longer held.
      reattachFile(e.source as Window, m.desc);
    }
  }

  // --- Write byte-ops (#197): the workspace-mutating seam actions go to the
  // daemon's confined `file.*` verbs; a refusal is flashed. The browser composes
  // the full rel path.
  (function wireWriteVerbs() {
    const daemonBacked = () => !!window.WBDaemon?.write;
    const flash = (msg: string) => messages.flash(msg);
    const call = (verb: string, payload: CommandPayload, okMsg?: string) => {
      WBDaemon.write(verb, payload)
        .then((reply) => {
          if (WBFail.isError(reply))
            flash(WBFail.failed(reply, "Could not rename: the daemon gave no reason."));
          else if (okMsg) flash(okMsg);
        })
        .catch(() => flash("Could not rename: the daemon did not answer."));
    };

    // Where a write is aimed: the project, and the checkout it carries.
    type Aim = {
      repo: string;
      checkout: string | null;
      aimed: (payload: CommandPayload) => CommandPayload;
    };
    type ActionOf<A extends WorkbenchAction["action"]> = Extract<WorkbenchAction, { action: A }>;

    // The aim of an action, or null where it has no project. Every Write carries
    // the checkout it is aimed at (#406): a Save says its tab's PIN (explicit
    // `null` = the primary, never the selection), a tree gesture says the
    // current selection.
    function actionTarget(d: WorkbenchAction | undefined): Aim | null {
      if (!d || !("project" in d)) return null;
      const repo = d.project;
      if (!repo) return null;
      const checkout =
        "checkout" in d && d.checkout !== undefined
          ? d.checkout
          : window.Alpine.store("checkouts").checkoutOf(repo);
      return { repo, checkout, aimed: (payload) => WBDaemon.withCheckout(payload, checkout) };
    }

    // The pane's encoding rides the write (ADR-0036 amendment 2026-09-22)
    // and the pane hears the answer: its dirty mark waits for the ack, and
    // a refusal is read where the bytes are, not in a flash elsewhere.
    // A detached window's pane is `detached` in its own viewer; a tab's is
    // its tab id in this one.
    function actSave(d: ActionOf<"save">, { repo, checkout, aimed }: Aim) {
      const viewer = () => (d.fromWindow ? d.fromWindow.WBViewer : window.WBViewer);
      const id = d.fromWindow ? "detached" : fileTabId(repo, d.path, checkout);
      const payload: SavePayload = { repo, path: d.path, content: d.content || "" };
      if (d.encoding) payload.encoding = d.encoding;
      if (d.bom) payload.bom = true;
      const send = (p: SavePayload): Promise<void> =>
        WBDaemon.write("file.write", aimed(p))
          .then((reply) => {
            if (!WBFail.isError(reply)) return viewer()?.saveDone?.(id);
            const reason = WBFail.message(reply, "the daemon gave no reason");
            viewer()?.saveFailed?.(id, reason, reply);
            // UTF-8 represents everything; a refusal under it is not a
            // conversion question, and asking again would loop.
            if (reason === "unencodable" && !/^utf-?8$/i.test(p.encoding || "utf-8")) {
              return offerUtf8(p, reply);
            }
            flash(WBFail.failed(reply, "Could not save: the daemon gave no reason."));
          })
          .catch(() => {
            viewer()?.saveFailed?.(id, "the daemon did not answer");
            flash("Could not save: the daemon did not answer.");
          });
      // The daemon wrote nothing (a round-trip or a refusal, never a `?`):
      // the one repair the browser can offer is a DELIBERATE conversion,
      // named to the operator and made only on their yes.
      const offerUtf8 = (p: SavePayload, reply: WriteReply) => {
        const at = Number(reply?.char_index ?? 0) + 1;
        const ask = messages.askInShell({
          title: `Could not save as ${p.encoding}`,
          message: `Character ${at} is not representable in ${p.encoding}. Save the file as UTF-8 instead?`,
          confirmLabel: "Save as UTF-8",
        });
        // No shell, no dialog: the pane already says "Could not save" and why.
        if (!ask) return;
        return ask.then((ok) => {
          if (!ok) return;
          viewer()?.setEncoding?.(id, "UTF-8", false);
          const { bom: _bom, ...rest } = p;
          return send({ ...rest, encoding: "utf-8" });
        });
      };
      void send(payload);
    }

    // A console's switcher cut a worktree: re-read the listing, flash what the
    // add had to say.
    function actWorktreeCreated(d: ActionOf<"worktree-created">, { repo }: Aim) {
      void window.Alpine.store("checkouts").ensureWorktreeListing(repo, true);
      if (d.message) flash(d.message.split("\n").map(WBFail.sentence).filter(Boolean).join(" "));
    }

    // The name of a new entry: the prompt dialog where the page has a shell,
    // else the browser's prompt.
    async function askEntryName(shellPage: boolean, folder: boolean, dir: string) {
      if (!shellPage) return window.prompt(folder ? "New folder name" : "New file name");
      return window.Alpine.store("modals").askPrompt({
        // No placeholder: a plausible filename in an empty field reads as
        // a name already chosen, and operators pressed Enter on it.
        title: newEntryTitle(folder ? "folder" : "file", dir),
        message: "",
        placeholder: "",
      });
    }

    // `create` carries the target DIRECTORY and no name: ask for it, then
    // open a created file so the operator lands in it.
    async function actCreate(d: ActionOf<"create">, { repo, aimed }: Aim) {
      const folder = d.kind === "folder";
      const c = window.getShell();
      const name = await askEntryName(!!c, folder, d.path);
      if (!name) return;
      const path = d.path ? `${d.path}/${name}` : name;
      const reply = await WBDaemon.write("file.create", aimed({ repo, path, dir: folder })).catch(
        () => null,
      );
      if (!reply) return flash(`Could not create ${name}: the daemon did not answer.`);
      if (WBFail.isError(reply))
        return flash(WBFail.failed(reply, `Could not create ${name}: the daemon gave no reason.`));
      flash(`${name} created.`);
      if (!folder && c)
        window.Alpine.store("tabs").openTab({
          project: repo,
          path,
          title: name,
          ftype: classify(name),
        });
      // The files re-list the level, then reveal the new entry (wb-files.ts).
      sendWindow(window, "workbench:tree-dirty", { rel: d.path || "", reveal: path });
    }

    // Irreversible (a folder removes recursively): confirm first.
    async function actDelete(d: ActionOf<"delete">, { repo, aimed }: Aim) {
      const name = d.title || d.path.split("/").pop() || d.path;
      const message = d.isFolder
        ? `Delete folder “${name}” and its contents? This cannot be undone.`
        : `Delete “${name}”? This cannot be undone.`;
      const ask = messages.askInShell({
        title: "Delete",
        message,
        confirmLabel: "Delete",
        danger: true,
      });
      const ok = ask ? await ask : window.confirm(message);
      if (!ok) return;
      const reply = await WBDaemon.write("file.delete", aimed({ repo, path: d.path })).catch(
        () => null,
      );
      if (!reply) return flash("Could not delete: the daemon did not answer.");
      if (!WBFail.isError(reply)) return flash(`${name} deleted.`);
      const reason = WBFail.message(reply, "the daemon gave no reason");
      flash(WBFail.failed(reply, "Could not delete: the daemon gave no reason."));
      // "not found" on a delete says the ROW is the lie: re-list the parent
      // so the ghost ends up off the screen.
      if (/not found/i.test(reason)) {
        sendWindow(window, "workbench:tree-dirty", { rel: parentRel(d.path) });
      }
    }

    async function runAction(d: WorkbenchAction) {
      if (!daemonBacked()) return;
      const aim = actionTarget(d);
      if (!aim) return;
      switch (d.action) {
        case "save":
          actSave(d, aim);
          break;
        case "worktree-created":
          actWorktreeCreated(d, aim);
          break;
        case "create":
          await actCreate(d, aim);
          break;
        case "rename": {
          // `from`/`to` are FULL rel paths (shared with the move gesture).
          call("file.rename", aim.aimed({ repo: aim.repo, path: d.from, to: d.to }));
          break;
        }
        case "delete":
          await actDelete(d, aim);
          break;
        case "branch-create":
        case "branch-switch":
        case "command":
        case "console-close":
        case "console-open":
        case "console-restart":
        case "console-switch-checkout":
        case "copy-path":
        case "detach":
        case "detach-blocked":
        case "fence-detach":
        case "fence-detach-blocked":
        case "fence-detach-refused":
        case "fence-focus":
        case "fence-reattach":
        case "fence-remove-refused":
        case "issue-label-change":
        case "kanban-toggle":
        case "login":
        case "logoff":
        case "open":
        case "open-diff":
        case "open-refused":
        case "reload":
        case "run-issue-focus":
        case "run-start":
        case "setting-change":
          // The other gestures change nothing on the daemon's tree.
          break;
      }
    }

    document.addEventListener("workbench:action", (e) => {
      runAction(e.detail).catch(() => flash("Could not finish that action."));
    });
  })();

  // Dismiss the context menu on any outside interaction.
  const hideContextMenu = () => {
    const menu = document.getElementById("ctxmenu");
    if (menu) menu.style.display = "none";
  };
  document.addEventListener("click", hideContextMenu);
  document.addEventListener("scroll", hideContextMenu, true);

  // `/` → the search on screen: FILES while a project is open, projects
  // otherwise.
  document.addEventListener("keydown", (e) => {
    if (e.key !== "/" || e.ctrlKey || e.metaKey || e.altKey) return;
    const c = window.getShell();
    if (!c || window.Alpine.store("modals").consoleShortcutsBlocked()) return;
    e.preventDefault();
    if (window.Alpine.store("projects").openSlug) sendWindow(window, "workbench:file-search-open");
    else c.focusProjectSearch();
  });

  // Ctrl/Cmd+Shift+F → the FILES search. NOT `consoleShortcutsBlocked`: the
  // editor is exactly where "find in files" is reached for.
  document.addEventListener("keydown", (e) => {
    if (!(e.ctrlKey || e.metaKey) || !e.shiftKey || e.altKey) return;
    if (e.key !== "F" && e.key !== "f") return;
    if (!filesSearchOffered() || dialogOverFiles()) return;
    e.preventDefault();
    sendWindow(window, "workbench:file-search-open");
  });

  // The FILES search needs the shell, a login and an open project.
  function filesSearchOffered() {
    return (
      !!window.getShell() &&
      window.Alpine.store("connection").authed &&
      !!window.Alpine.store("projects").openSlug
    );
  }

  // A dialog that the FILES search must not open over.
  function dialogOverFiles() {
    const modals = window.Alpine.store("modals");
    return [
      WBSettingsDialog.openFlag,
      WBSecurityDialog.openFlag,
      "runOpen",
      "branchOpen",
      WBReleaseDialogs.whatsNewFlag,
    ].some((flag) => modals.modalOpen(flag));
  }

  window.WBRuns = {
    // A raw output chunk: the Runs panel (wb-runs-panel.ts) appends it.
    output(text) {
      sendWindow(window, "workbench:run-output", { text });
    },
  };

  // The detached file windows: the Alpine store `connection` asks them about
  // unsaved work before a build reload (wb-connection-store.ts).
  return detached;
}
