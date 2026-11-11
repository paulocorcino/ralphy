/* ---------------------------------------------------------------------------
   ralphy workbench shell — file viewers (the closable tabs)

   Four flavours, each opening as its own tab after the fixed Consoles tab:
     • source code — Monaco: syntax highlight, in-place editing, and its own
       find widget. Binaries never reach here (app.ts refuses them).
     • Markdown  — rendered with `marked`, sanitized with DOMPurify, mermaid
       fences drawn as diagrams (Cursor-style), a heading outline to jump around,
       an in-page find, and an edit/preview toggle over the raw source. A
       Mermaid file (`.mmd`) uses the same pane, drawn as one diagram
       (`wb-viewer-render.ts` fills the body by kind).
     • image     — an allowlisted image the daemon verified and served as a
       `data:` URL (ADR-0049), fit to the pane or shown 1:1. Read-only.
     • diff      — HEAD against the working tree, side by side. Read-only.

   Editing is allowed but never touches disk: a Save emits a `save` intent on the
   `workbench:action` seam carrying the new content, for a backend to persist.

   This module owns the panes. The records and their guards are
   `wb-file-viewer-records.ts`, the code pane is `wb-file-viewer-code.ts`, the
   split and the mirror are `wb-file-viewer-slot.ts`, and the diff, image and
   refused panes are `wb-file-viewer-read-only.ts`. The Markdown pane, the
   find, the encoding menu and the save steps are its siblings
   `wb-viewer-markdown.ts`, `-find.ts`, `-encoding.ts` and `-save.ts`; it
   builds them in `createViewer`.
--------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import { WBFleet } from "./wb-fleet.ts";
import { sendDocument } from "./wb-events.ts";
import { need } from "./wb-dom-need.ts";
import type { Messages } from "./wb-messages.ts";
import { isRendered } from "./wb-viewer-render.ts";
import { createViewerEncoding, encodingLabel, refreshEncodingPill } from "./wb-viewer-encoding.ts";
import { createViewerFind } from "./wb-viewer-find.ts";
import { createViewerMarkdown, linkTarget, remoteImageNotice } from "./wb-viewer-markdown.ts";
import { createViewerSave } from "./wb-viewer-save.ts";
import { createViewerSlot } from "./wb-file-viewer-slot.ts";
import { createViewerReadOnly } from "./wb-file-viewer-read-only.ts";
import { createViewerCode } from "./wb-file-viewer-code.ts";
import type { ViewerMarkdown } from "./wb-viewer-markdown.ts";
import type { ReopenBytes } from "./wb-viewer-save.ts";
import type { ViewerSlot } from "./wb-viewer-panes.ts";
import type { MermaidWindow } from "./wb-dbml-draw.ts";
import type {
  BytesRecord,
  OpenSpec,
  PaneRecord,
  RefusedRecord,
  ViewerRecord,
} from "./wb-file-viewer-records.ts";
import { codeEditor, diffEditor } from "./wb-file-viewer-records.ts";

export { isFileDescriptor, isOpenRequest } from "./wb-file-viewer-records.ts";
export type { BytesRecord, RefusedRecord, ViewerRecord } from "./wb-file-viewer-records.ts";
/** What the entry module hands the file pane. */
export type ViewerDeps = {
  /** The page's door for operator messages. */
  messages: Messages;
};

export function createViewer(window: MermaidWindow, document: Document, deps: ViewerDeps) {
  const { messages } = deps;
  const viewers = document.getElementById("viewers");
  const map = new Map<string, ViewerRecord>(); // tab id → viewer record

  // Monaco boots through an AMD loader, so an editor is created
  // asynchronously. INVARIANTS across that gap: `rec.content` is the single
  // source of truth until `rec.ed` exists, and a tab closed mid-boot must
  // never mount an orphan editor. Liveness is `map.get(rec.id) === rec`, NOT
  // `has`: tab ids are stable per file, so a close+reopen inside the boot
  // window puts a DIFFERENT record under the same key.
  const alive = (rec: ViewerRecord) => map.get(rec.id) === rec;

  // The viewer's parts, built in the order of what each one reads. The save
  // steps read `renderMarkdown` and `drawMermaid`, which are built after
  // them, so they take those two late.
  const find = createViewerFind({ document, alive });
  const { findInEditor, findInMarkdown } = find;
  const saver = createViewerSave({
    window,
    messages,
    reopenRefused,
    renderMarkdown: (rec) => {
      markdown.renderMarkdown(rec);
    },
    drawMermaid: (rec) => {
      markdown.drawMermaid(rec);
    },
  });
  const { showSaveError, clearSaveError, descOf, reloadFile, applyFresh, showDiskBadge } = saver;
  const viewerEncoding = createViewerEncoding({ window, document, messages, saver, reopenRefused });
  const { buildCode, mountEditor, isNarrow, watchNarrow, bindSave } = createViewerCode({
    window,
    document,
    viewers,
    messages,
    alive,
    findInEditor,
    mirrors: (id) => mirrors(id),
    refresh: () => {
      refresh();
    },
    setPathLabel,
    detachBtnHtml,
    mirrorBtnHtml,
    detachClick,
    saver,
    encoding: viewerEncoding,
  });
  const markdown: ViewerMarkdown = createViewerMarkdown({
    window,
    document,
    viewers,
    setPathLabel,
    setCaption,
    detachBtnHtml,
    detachClick,
    mountEditor,
    saver,
    encoding: viewerEncoding,
    find,
  });
  const { buildMarkdown, drawMermaid, jumpTo } = markdown;
  const { refresh, show, mirrors, mirrored, closeMirror } = createViewerSlot({
    window,
    document,
    viewers,
    map,
    alive,
    findInEditor,
    drawMermaid,
    setPathLabel,
    setCaption,
    isNarrow,
    watchNarrow,
    bindSave,
  });
  const { buildDiff, buildImage, buildRefused } = createViewerReadOnly({
    window,
    document,
    viewers,
    messages,
    alive,
    setPathLabel,
    setCaption,
    detachBtnHtml,
    detachClick,
    reloadFile,
    isNarrow,
    watchNarrow,
  });

  function disposeEditor(rec: ViewerRecord) {
    // The mirror sits over THIS pane's model: its editor goes before the model
    // does, on every path — an editor over a disposed model throws on render.
    if (mirrored() === rec) closeMirror();
    rec.ro?.disconnect();
    rec.ro = undefined;
    rec.saveKey?.dispose();
    rec.saveKey = undefined;
    if (!rec.ed) return;
    disposeEditorAndModels(rec, rec.ed);
    rec.ed = undefined;
  }

  function disposeEditorAndModels(rec: ViewerRecord, ed: NonNullable<ViewerRecord["ed"]>) {
    if (rec.kind === "diff") {
      // A diff editor holds TWO models and BOTH must be disposed on EVERY
      // path — disposing the editor does NOT (the leak is only visible in
      // `monaco.editor.getModels()`, which wb_diff_311.py counts). The EDITOR
      // goes first: a model disposed while attached raises a page error (#407).
      const m = diffEditor(ed)?.getModel();
      ed.dispose();
      m?.original.dispose();
      m?.modified.dispose();
    } else {
      codeEditor(ed)?.getModel()?.dispose();
      ed.dispose();
    }
  }

  // A pane that opened refused now has bytes: rebuild it as the pane its
  // kind deserves. `open` returns early on a known id, so the record is
  // replaced under the same id — the tab in the shell is untouched.
  function reopenRefused(rec: RefusedRecord, bytes: ReopenBytes) {
    const { id, project, label, path, kind, checkout, detached } = rec;
    API.close(id);
    API.open({
      id,
      project,
      label,
      path,
      ftype: kind,
      checkout,
      detached,
      content: bytes.content,
      encoding: bytes.encoding,
      bom: bytes.bom,
    });
    if (rec.visible) refresh();
    window.WB.emit("reload", { project: rec.project, path: rec.path });
  }

  // The Detach/Re-attach button. A file tab detaches into a standalone popup
  // (watch an agent in the main window, read the file in another); a detached
  // pane folds back in. wb-file-viewer only *requests* it — the shell (app.ts) opens
  // the popup, and the popup (detached.html) folds back — so this module stays
  // agnostic to windows/tabs.
  function detachBtnHtml(rec: ViewerRecord) {
    return rec.detached
      ? '<button class="vbtn" data-act="detach" title="Re-attach" aria-label="Re-attach">' +
          '<i class="bi bi-box-arrow-in-down-left"></i>' +
          '<span class="vbtn-label">Re-attach</span></button>'
      : '<button class="vbtn" data-act="detach" title="Detach" aria-label="Detach">' +
          '<i class="bi bi-box-arrow-up-right"></i>' +
          '<span class="vbtn-label">Detach</span></button>';
  }

  // The mirror toggle (ADR-0037 §3c) is the attached canvas's: a detached
  // popup has one pane and no slot to put a second editor in.
  function mirrorBtnHtml(rec: ViewerRecord) {
    return rec.detached
      ? ""
      : '<button class="vbtn" data-act="mirror" title="Mirror" aria-label="Mirror">' +
          '<i class="bi bi-files"></i><span class="vbtn-label">Mirror</span></button>';
  }

  // What the toolbar says a pane IS: the path only (the tab and the sidebar
  // already name the file and the repo). A DETACHED pane keeps the full label;
  // the full form always rides the `title`. `dir` / `file` are split so the
  // CSS ellipsizes the directory first.
  function pathLabel(rec: ViewerRecord) {
    const suffix = rec.kind === "diff" ? " ↔ HEAD" : "";
    const full = `${rec.label} / ${rec.path}${suffix}`;
    const cut = rec.path.lastIndexOf("/") + 1;
    const head = rec.detached ? `${rec.label} / ` : "";
    return { dir: head + rec.path.slice(0, cut), file: rec.path.slice(cut) + suffix, full };
  }

  function setPathLabel(el: HTMLElement, rec: ViewerRecord) {
    const span = need(el, ".viewer-path");
    const { dir, file, full } = pathLabel(rec);
    span.textContent = "";
    const d = document.createElement("span");
    d.className = "viewer-dir";
    d.textContent = dir;
    const f = document.createElement("span");
    f.className = "viewer-file";
    f.textContent = file;
    span.append(d, f);
    span.title = full;
  }

  // A caption swap keeps the button's shape: icon, then the label span a narrow
  // pane hides, and the same words in `title`/`aria-label` for when it does.
  function setCaption(btn: HTMLElement, icon: string, caption: string) {
    btn.innerHTML = `<i class="bi ${icon}"></i><span class="vbtn-label"></span>`;
    need(btn, ".vbtn-label").textContent = caption;
    btn.title = caption;
    btn.setAttribute("aria-label", caption);
  }

  function detachClick(rec: BytesRecord) {
    const evt = rec.detached ? "workbench:reattach-request" : "workbench:detach-request";
    sendDocument(document, evt, descOf(rec));
  }

  // --- public API ---------------------------------------------------------
  let uidSeq = 0;
  // The fields every pane has, from the shell's open request.
  function paneRecord(spec: OpenSpec): PaneRecord {
    const { id, project, label, path, ftype, original, detached, checkout, encoding, bom } = spec;
    return {
      id,
      project,
      label: (label ?? "") || WBFleet.refSlug(project),
      path,
      kind: ftype,
      original,
      uid: ++uidSeq,
      editing: false,
      visible: false,
      detached: Boolean(detached),
      checkout: (checkout ?? "") || null,
      encoding: (encoding ?? "") || "UTF-8",
      bom: Boolean(bom),
    };
  }
  const API = {
    // Whether any open file tab holds an edit not yet saved: the work a
    // reload onto a new build must not lose (ADR-0070 D6).
    anyDirty() {
      return [...map.values()].some((rec) => rec.dirty === true);
    },
    // `original` is the diff's HEAD side. `project` is the IDENTITY (tab id,
    // wire field, model URI) and stays the full ref; `label` is the human
    // form, derived here when the caller supplies none. `checkout` pins the
    // pane to the worktree its bytes came from (#406), `null` for the primary.
    // `encoding`/`bom` are what `file.read` reported (ADR-0036 amendment
    // 2026-09-22) and ride the record so a save gives the bytes back the way
    // they came; absent (a diff) is UTF-8 without a BOM. `refused` is
    // the daemon's reason for serving nothing: the pane says so and keeps the
    // tab, instead of the tab closing under the click.
    open(spec: OpenSpec) {
      const { id, ftype } = spec;
      if (map.has(id)) return;
      const pane = paneRecord(spec);
      if (spec.refused !== undefined) {
        const refused: RefusedRecord = { ...pane, refused: spec.refused };
        map.set(id, refused);
        buildRefused(refused);
        return;
      }
      const rec: BytesRecord = { ...pane, content: spec.content, refused: null };
      map.set(id, rec);
      if (isRendered(ftype)) buildMarkdown(rec);
      else if (ftype === "diff") buildDiff(rec);
      else if (ftype === "image") buildImage(rec);
      else buildCode(rec);
    },

    // The shell's `file.write` answered. `saveDone` is the ack the dirty mark
    // waits for; `saveFailed` puts the mark back and names the reason in the
    // pane. `reply` rides along so an `unencodable` refusal can say which
    // character (`char_index`) the encoding could not take.
    saveDone(id: string) {
      const rec = map.get(id);
      if (!rec) return;
      rec.dirty = false;
      rec.saveBtn?.classList.remove("dirty");
      clearSaveError(rec);
    },
    saveFailed(id: string, reason: string, reply?: WriteReply) {
      const rec = map.get(id);
      if (!rec) return;
      rec.dirty = true;
      rec.saveBtn?.classList.add("dirty");
      const text =
        reason === "unencodable"
          ? `Could not save: character ${(reply?.char_index ?? 0) + 1} ` +
            `cannot be written in ${rec.encoding}.`
          : WBFail.failed(reply ?? { reason }, "Could not save: the daemon gave no reason.");
      showSaveError(rec, text);
    },

    // The encoding a pane will save with, and the change of it (a "save
    // with…" or the `unencodable` dialog's "save as UTF-8").
    encodingOf(id: string) {
      const rec = map.get(id);
      return rec ? { encoding: rec.encoding, bom: rec.bom } : null;
    },
    setEncoding(id: string, encoding: string, bom: boolean) {
      const rec = map.get(id);
      if (!rec) return;
      rec.encoding = encoding;
      rec.bom = bom;
      rec.dirty = true;
      rec.saveBtn?.classList.add("dirty");
      refreshEncodingPill(rec);
    },

    // Exposed for its test: the pill's text for a daemon-named encoding.
    encodingLabel,

    // The shell never calls it — a detach goes through the pane's own button.
    // detached.html reads it on unload, to send the edited bytes home. A
    // refused pane has no bytes to describe.
    descOf(id: string) {
      const rec = map.get(id);
      return rec && rec.refused === null ? descOf(rec) : null;
    },

    // Show one pane (or none, when the Consoles tab is active), and beside it
    // the slot the shell resolved (ADR-0037 §3c): `{ id, mirror, focus, ratio }`
    // or nothing. One argument is the single pane every caller had; the
    // detached popup never passes a second.
    setActive(id: string | null, slot?: ViewerSlot | "" | null) {
      show(id, slot === "" || slot === undefined ? null : slot);
    },

    // The canvas width the split decision is made against.
    width() {
      return viewers?.clientWidth ?? 0;
    },

    // The file behind an OPEN pane moved. Re-key the record instead of
    // close+open: reopening refetches the bytes and would discard the
    // operator's unsaved edits, and `open` returns early on a known id, so a
    // naive reopen is a silent no-op. A diff tab is skipped — its id comes from
    // the changes panel, not from this path.
    repath(oldId: string, { id, path }: { id: string; path: string }) {
      const rec = map.get(oldId);
      if (!rec || map.has(id) || rec.kind === "diff") return;
      map.delete(oldId);
      rec.id = id;
      rec.path = path;
      map.set(id, rec);
      if (rec.el) rec.el.dataset["tabId"] = id;
      if (rec.el) setPathLabel(rec.el, rec);
    },

    close(id: string) {
      const rec = map.get(id);
      if (!rec) return;
      // Delete from the map FIRST: a pending mountEditor() checks membership
      // before touching the DOM, so a tab closed mid-boot mounts nothing.
      map.delete(id);
      disposeEditor(rec);
      rec.el?.remove();
    },

    // Scroll an open markdown pane to a `#fragment`, once the bytes landed.
    jumpTo(id: string, fragment: string) {
      const rec = map.get(id);
      if (rec && isRendered(rec.kind)) jumpTo(rec, fragment);
    },

    // Land a tab on the first occurrence of `term` (a content-search open).
    // A code pane whose editor has not mounted yet remembers the term and
    // acts once it has; a rendered markdown pane uses its own find bar; a
    // diff or an image has nothing to find in.
    find(id: string, term: string) {
      const rec = map.get(id);
      if (!rec || !term) return;
      if (rec.kind === "diff" || rec.kind === "image") return;
      if (isRendered(rec.kind) && !rec.editing) {
        findInMarkdown(rec, term);
        return;
      }
      if (rec.ed) findInEditor(rec, term);
      else rec.pendingFind = term;
    },

    // Exposed for the decision table's test; `linkClick` is the only caller.
    linkTarget,
    // Exposed for its test; `setPathLabel` is the only caller.
    pathLabel,
    // Exposed for its test; `blockedImage` is the only caller.
    remoteImageNotice,

    // An external write to this file's bytes landed. A CLEAN tab
    // auto-refreshes; a DIRTY tab stashes them and shows the "changed on disk"
    // badge, NEVER clobbering unsaved edits. Equal bytes are a no-op (our own
    // save round-trips through the same nudge).
    externalChange(id: string, content: string) {
      const rec = map.get(id);
      if (!rec) return;
      // A diff tab never auto-refreshes: it is a two-sided read, and a
      // single-side update would silently misrepresent the comparison. A
      // refused pane has no bytes to compare; its Reload is the retry.
      if (rec.kind === "diff" || rec.refused !== null) return;
      if (content === rec.content) return;
      if (rec.dirty === true) {
        rec.pendingDisk = content;
        showDiskBadge(rec);
      } else {
        applyFresh(rec, content);
      }
    },
  };
  return API;
}
