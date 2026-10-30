// The pane's bytes and what happens to them: Save, Reload, an external write,
// and the "changed on disk" badge. A pane over a diff or an image never saves.
import { WBFail } from "./wb-fail.ts";
import { refreshEncodingPill } from "./wb-viewer-encoding.ts";
import { isRendered } from "./wb-viewer-render.ts";
import type { MonacoEditor } from "./wb-monaco.ts";
import type { Messages } from "./wb-messages.ts";
import type { BytesRecord, RefusedRecord, ViewerRecord } from "./wb-file-viewer.ts";

/** What a refused pane is rebuilt from once the daemon serves its bytes. */
export type ReopenBytes = { content: string; encoding?: string; bom?: boolean };

/** Rebuild a refused pane as the pane its kind deserves. */
export type ReopenRefused = (rec: RefusedRecord, bytes: ReopenBytes) => void;

/** What `createViewer` hands the save steps. */
export type ViewerSaveDeps = {
  window: Window;
  /** The page's door for operator messages. */
  messages: Messages;
  reopenRefused: ReopenRefused;
  /** Paint a rendered pane again from its bytes. */
  renderMarkdown: (rec: BytesRecord) => void;
  /** Draw the Mermaid holders a pane is waiting to draw. */
  drawMermaid: (rec: ViewerRecord) => void;
};

export type ViewerSave = ReturnType<typeof createViewerSave>;

export function createViewerSave(deps: ViewerSaveDeps) {
  const { window, messages, reopenRefused, renderMarkdown, drawMermaid } = deps;

  function save(rec: ViewerRecord) {
    // A diff and an image are read-only: a `save` intent from either would be a
    // mutation those surfaces forbid, so it never reaches the action seam. (An
    // image's `content` is a `data:` URL, not the file's bytes — saving it would
    // write the URL over the image.) A refused pane has no bytes to save.
    if (rec.kind === "diff" || rec.kind === "image" || rec.refused !== null) return;
    const content = contentOf(rec);
    rec.content = content;
    hideDiskBadge(rec);
    clearSaveError(rec);
    // The dirty mark is cleared by `saveDone`, on the daemon's ack — not here:
    // a refused write (`unencodable`, a denylisted path) must leave the pane
    // looking unsaved, because it is.
    // `checkout` is the tab's PIN (#406): the write goes to the tree the bytes
    // came from, whatever the project's selection is by now. `encoding`/`bom`
    // are what the read reported (or a "save with…" chose), so the bytes go
    // back the way they came (ADR-0036 amendment 2026-09-22).
    window.WB.emit("save", {
      project: rec.project,
      path: rec.path,
      bytes: content.length,
      content,
      checkout: rec.checkout,
      encoding: rec.encoding,
      bom: !!rec.bom,
    });
    if (isRendered(rec.kind) && !rec.editing) renderMarkdown(rec); // keep preview fresh
  }

  // The inline "not saved" line beside the Save button: a write refusal must
  // be read from the pane it concerns, not from a flash in another panel.
  function showSaveError(rec: ViewerRecord, text: string) {
    const el = rec.el?.querySelector<HTMLElement>(".viewer-save-err");
    if (!el) return;
    el.textContent = text;
    el.style.display = "";
  }
  function clearSaveError(rec: ViewerRecord) {
    const el = rec.el?.querySelector<HTMLElement>(".viewer-save-err");
    if (el) el.style.display = "none";
  }

  // The pane's current bytes, whether shown as source (Monaco) or as a rendered
  // markdown preview. Before Monaco finishes booting `rec.ed` is undefined and
  // `rec.content` is still authoritative.
  function contentOf(rec: BytesRecord) {
    if (rec.kind === "diff") return rec.content;
    if (rec.ed && (rec.kind === "code" || rec.editing)) return (rec.ed as MonacoEditor).getValue();
    return rec.content;
  }

  // A portable descriptor — enough to reopen this file anywhere (a tab or a
  // detached popup), carrying the *current* (possibly edited) content.
  // The pin rides the descriptor (#406): a detached pane saves and reloads
  // against the tree its bytes came from, and re-attaches pinned to it.
  function descOf(rec: BytesRecord): FileDescriptor {
    return {
      project: rec.project,
      label: rec.label,
      path: rec.path,
      ftype: rec.kind,
      content: contentOf(rec),
      checkout: rec.checkout ?? null,
      encoding: rec.encoding,
      bom: !!rec.bom,
    };
  }

  // Reload discards local edits and reloads from source. Daemon-backed repos
  // re-read the REAL file via `file.read` (#197). The apply step is shared via
  // `applyFresh`. With no daemon client loaded (a unit test) it does nothing.
  function reloadFile(rec: ViewerRecord) {
    const daemonBacked = !!window.WBDaemon?.observe;
    if (daemonBacked) {
      // Daemon mode: a non-ok reply or a transport drop must NOT regenerate
      // synthetic bytes (C1). The tab stays — the operator's bytes are still
      // the best answer the pane has — and the reason lands in the pane.
      const fail = (reply?: DaemonReply) => {
        showSaveError(
          rec,
          WBFail.failed(reply, "Could not reload the file: the daemon gave no reason."),
        );
        messages.flash("Could not reload the file.");
      };
      // An image reloads through its own verb (ADR-0049): `file.read` refuses
      // its bytes, so routing it here would turn every image Reload into a
      // "reload failed".
      if (rec.kind === "image") {
        WBDaemon.readImage(rec.project, rec.path, undefined, rec.checkout)
          .then((url) => settleImage(rec, url, fail))
          .catch(() => fail());
        return;
      }
      readWith(rec, rec.encoding)
        .then((reply) => {
          if (!reply || reply.status !== "ok") return fail(reply);
          rec.bom = !!reply.bom;
          if (reply.encoding) rec.encoding = reply.encoding;
          refreshEncodingPill(rec);
          if (rec.refused !== null) return reopenRefused(rec, reply);
          applyFresh(rec, reply.content);
        })
        .catch(() => fail());
    }
  }

  // The fresh bytes of an image: the reload failed when there are none.
  function settleImage(rec: ViewerRecord, url: string | null | undefined, fail: () => void) {
    if (!url) return fail();
    if (rec.refused !== null) return reopenRefused(rec, { content: url });
    return applyFresh(rec, url);
  }

  // `file.read` for this pane, with `encoding` as the hint: a tab's encoding is
  // sticky once opened (the daemon's detection, or a "reopen with…"), so a
  // reload never silently re-detects it.
  function readWith(rec: ViewerRecord, encoding: string | undefined) {
    const payload = WBDaemon.withCheckout({ repo: rec.project, path: rec.path }, rec.checkout);
    if (encoding) payload.encoding = encoding;
    return WBDaemon.observe("file.read", payload);
  }

  function applyFresh(rec: BytesRecord, fresh: string) {
    // A diff pane has no single "fresh bytes" to apply: reloading it means
    // re-resolving BOTH sides, which is a reopen, not a refresh.
    if (rec.kind === "diff") return;
    // `rec.content` FIRST: bytes that land before Monaco boots are picked up by
    // the pending `create()`, and setValue fires the change event, so the dirty
    // flag is cleared *after* the update, not before.
    rec.content = fresh;
    paintFresh(rec, fresh);
    rec.dirty = false;
    rec.saveBtn?.classList.remove("dirty");
    hideDiskBadge(rec);
    window.WB.emit("reload", { project: rec.project, path: rec.path });
  }

  // Show the fresh bytes in the pane of their kind.
  function paintFresh(rec: BytesRecord, fresh: string) {
    if (rec.kind === "image") {
      // A fresh `data:` URL repaints the pane; the `onload` handler re-reads the
      // intrinsic size, which an overwritten image may well have changed.
      const img = rec.el?.querySelector<HTMLImageElement>(".img-canvas");
      if (img) img.src = fresh;
    } else if (rec.kind === "code" || rec.editing) {
      if (rec.ed) (rec.ed as MonacoEditor).setValue(fresh);
      // In the read-only fallback there is no editor to update, and leaving the
      // <pre> stale would show bytes that no longer match rec.content.
      else if (rec.fallbackEl) rec.fallbackEl.textContent = fresh;
    } else {
      renderMarkdown(rec);
      if (rec.visible) drawMermaid(rec);
    }
  }

  // The "changed on disk" badge: shown when an EXTERNAL write lands on a DIRTY
  // tab (never auto-applied — the operator's unsaved edits win until they click).
  function showDiskBadge(rec: ViewerRecord) {
    const b = rec.el?.querySelector<HTMLElement>(".viewer-disk-badge");
    if (b) b.style.display = "";
  }
  function hideDiskBadge(rec: ViewerRecord) {
    rec.pendingDisk = undefined;
    const b = rec.el?.querySelector<HTMLElement>(".viewer-disk-badge");
    if (b) b.style.display = "none";
  }

  return {
    save,
    showSaveError,
    clearSaveError,
    descOf,
    reloadFile,
    readWith,
    applyFresh,
    showDiskBadge,
    hideDiskBadge,
  };
}
