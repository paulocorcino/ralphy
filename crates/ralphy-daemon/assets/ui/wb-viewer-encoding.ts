// The encoding pill and its menu, on a file pane that has bytes.
import { WBFail } from "./wb-fail.ts";
import type { Messages } from "./wb-messages.ts";
import type { ViewerRecord } from "./wb-file-viewer.ts";
import type { ReopenRefused, ViewerSave } from "./wb-viewer-save.ts";

/** The ENCODINGS rows: the label, the daemon's name, and whether a BOM is written. */
type Encoding = [label: string, name: string, bom: boolean];

// --- the encoding pill and its menu ---------------------------------------
// What the pane will save with, as the daemon named it (ADR-0036 amendment
// 2026-09-22), and the two things an operator can do about it: REOPEN the
// same bytes under another encoding (the daemon re-decodes; the operator
// judges by the glyphs), or SAVE the text under another one (a deliberate
// conversion — the daemon never converts on its own).
const ENCODINGS: Encoding[] = [
  ["UTF-8", "utf-8", false],
  ["UTF-8 BOM", "utf-8", true],
  ["UTF-16 LE", "utf-16le", true],
  ["UTF-16 BE", "utf-16be", true],
  ["Windows-1252", "windows-1252", false],
  ["ISO-8859-2", "iso-8859-2", false],
  ["ISO-8859-15", "iso-8859-15", false],
  ["Windows-1250", "windows-1250", false],
  ["Windows-1251", "windows-1251", false],
  ["KOI8-R", "koi8-r", false],
  ["Shift_JIS", "shift_jis", false],
  ["EUC-JP", "euc-jp", false],
  ["GBK", "gbk", false],
  ["Big5", "big5", false],
  ["EUC-KR", "euc-kr", false],
];

// The pill's text: the daemon's canonical name made readable (`UTF-16LE`
// → `UTF-16 LE`, `windows-1252` → `Windows-1252`), plus ` BOM` when one
// was read and will be written back.
export function encodingLabel(name: string | null | undefined, bom: boolean) {
  let text = String(name || "UTF-8");
  text = text.replace(/^utf-16(le|be)$/i, (_, e: string) => `UTF-16 ${e.toUpperCase()}`);
  text = text.replace(/^windows-/i, "Windows-");
  return bom ? `${text} BOM` : text;
}

export function encodingBtnHtml(rec: ViewerRecord) {
  return `<button class="vbtn viewer-enc" data-act="encoding" title="Encoding — reopen or save with another" aria-label="Encoding"><span class="viewer-enc-label">${encodingLabel(rec.encoding, rec.bom)}</span></button>`;
}

export function refreshEncodingPill(rec: ViewerRecord) {
  const label = rec.el?.querySelector<HTMLElement>(".viewer-enc-label");
  if (label) label.textContent = encodingLabel(rec.encoding, rec.bom);
}

function markCurrentEncoding(rec: ViewerRecord, menu: HTMLElement) {
  for (const item of menu.querySelectorAll<HTMLElement>(".enc-item")) {
    const i = Number(item.dataset.reopen ?? item.dataset.savewith);
    const [, name, bom] = ENCODINGS[i];
    const current =
      String(rec.encoding).toLowerCase() === name && !!rec.bom === bom;
    item.classList.toggle("current", current);
  }
}

/** What `createViewer` hands the encoding menu. */
export type ViewerEncodingDeps = {
  window: Window;
  document: Document;
  /** The page's door for operator messages. */
  messages: Messages;
  /** The pane's own read, write and apply steps. */
  saver: Pick<ViewerSave, "readWith" | "showSaveError" | "applyFresh" | "save">;
  reopenRefused: ReopenRefused;
};

export type ViewerEncoding = ReturnType<typeof createViewerEncoding>;

export function createViewerEncoding(deps: ViewerEncodingDeps) {
  const { window, document, messages, saver, reopenRefused } = deps;
  const { readWith, showSaveError, applyFresh, save } = saver;

  function wireEncodingMenu(rec: ViewerRecord, el: HTMLElement) {
    const btn = el.querySelector<HTMLElement>('[data-act="encoding"]');
    if (!btn) return;
    const menu = document.createElement("div");
    menu.className = "enc-menu";
    menu.style.display = "none";
    const group = (head: string, act: string) =>
      `<div class="dropdown-head">${head}</div>` +
      ENCODINGS.map(
        ([label, name, bom], i) =>
          `<button class="enc-item" data-${act}="${i}"><span>${label}</span></button>`,
      ).join("");
    menu.innerHTML = group("Reopen with", "reopen") + group("Save with", "savewith");
    el.append(menu);
    rec.encMenu = menu;
    const close = () => {
      menu.style.display = "none";
      document.removeEventListener("click", onOutside, true);
      document.removeEventListener("keydown", onKey, true);
    };
    const onOutside = (ev: MouseEvent) => {
      if (!menu.contains(ev.target as Node) && ev.target !== btn) close();
    };
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") close();
    };
    btn.onclick = (ev: MouseEvent) => {
      ev.stopPropagation();
      if (menu.style.display !== "none") return close();
      markCurrentEncoding(rec, menu);
      menu.style.display = "";
      document.addEventListener("click", onOutside, true);
      document.addEventListener("keydown", onKey, true);
    };
    menu.onclick = (ev: MouseEvent) => {
      const item = (ev.target as Element).closest?.<HTMLElement>(".enc-item");
      if (!item) return;
      ev.stopPropagation();
      close();
      if (item.dataset.reopen != null) reopenWith(rec, ENCODINGS[Number(item.dataset.reopen)]);
      else if (item.dataset.savewith != null) saveWith(rec, ENCODINGS[Number(item.dataset.savewith)]);
    };
  }

  // Reopen: the same bytes, decoded as `name`. Unsaved edits would be lost
  // to the re-read, so a dirty pane asks first.
  function reopenWith(rec: ViewerRecord, [label, name]: Encoding) {
    const go = () => {
      readWith(rec, name)
        .then((reply) => {
          if (!reply || reply.status !== "ok") {
            showSaveError(rec, WBFail.failed(reply, `Could not reopen as ${label}: the daemon gave no reason.`));
            return;
          }
          rec.encoding = reply.encoding || name;
          rec.bom = !!reply.bom;
          refreshEncodingPill(rec);
          if (rec.refused !== null) return reopenRefused(rec, reply);
          applyFresh(rec, reply.content);
        })
        .catch(() => showSaveError(rec, `Could not reopen as ${label}: the daemon did not answer.`));
    };
    if (!rec.dirty) return go();
    // The design-system dialog where there is a shell; `window.confirm` only
    // in a detached popup, which has no shell and no other dialog.
    const ask =
      messages.askInShell({
        title: `Reopen as ${label}?`,
        message: "Unsaved changes in this tab are discarded.",
        confirmLabel: "Reopen",
        danger: true,
      }) ?? Promise.resolve(window.confirm(`Reopen as ${label}? Unsaved changes are discarded.`));
    ask.then((ok) => ok && go());
  }

  // Save with: the text as it is, written under `name` — the conversion the
  // operator asked for, by name.
  function saveWith(rec: ViewerRecord, [, name, bom]: Encoding) {
    rec.encoding = name;
    rec.bom = bom;
    refreshEncodingPill(rec);
    save(rec);
  }

  return { wireEncodingMenu };
}
