// The note card's drop-down menus and the Markdown cheat sheet: the file menu,
// the heading index, and what to type (ADR-0064 §§6, 10, 11). `createNotes`
// builds it with the card's names as typed deps; the text of each function is
// the one it had in `wb-notes.ts`.
import { anchorsOf, veiledOf } from "./wb-notes-folds.ts";
import type { NoteCard, NoteSource } from "./wb-types.d.ts";

/** What `createNotes` hands the menus: the card's and the editor's names. */
export type NoteMenusDeps = {
  /** The page's window: the reduced-motion setting. */
  window: Window;
  /** The page's document. */
  document: Document;
  /** The desk record of a card, by id. */
  recordOf: (id: string | null | undefined) => NoteSource | null;
  /** Pull the editor's text into the card's document. */
  syncFromEditor: (el: NoteCard) => void;
  /** Close the open palette, if any. */
  closePalette: () => void;
};

export function createNoteMenus(deps: NoteMenusDeps) {
  const { window, document, recordOf, syncFromEditor, closePalette } = deps;

  // ---- the drop-down menus (ADR-0064 §11) ------------------------------------------

  // The head's dropdowns — the `⋯` file menu and the `☰` index. ONE open at a
  // time across the whole plane, closed by the next press anywhere else: the
  // card is a small surface and a menu left open over the text is in the way
  // of the thing it belongs to. They share this opener because they share that
  // rule; two independent close-on-outside listeners left one hanging when the
  // other opened.
  let openMenu: HTMLElement | null = null;
  let openMenuTrigger = "";
  function openDrop(el: NoteCard, popSelector: string, triggerSelector: string, fill: (pop: HTMLElement) => boolean | void) {
    const pop = el.querySelector<HTMLElement>(popSelector);
    if (!pop) return;
    const wasOpen = !pop.hidden;
    closeMenu();
    closePalette();
    if (wasOpen) return;
    if (fill && fill(pop) === false) return;
    pop.hidden = false;
    openMenu = pop;
    openMenuTrigger = triggerSelector;
    document.addEventListener("pointerdown", closeOnOutside, true);
  }
  function toggleMenu(el: NoteCard) {
    openDrop(el, ".note-menu-card:not(.note-anchors)", ".note-more", (menu) => {
      const record = recordOf(el.dataset.noteId);
      // Only a saved note in the PRIMARY tree has file actions: a worktree note
      // cannot be renamed or deleted yet (ADR-0064, amendment: `note.write` is
      // the only Write verb that crosses the worktree gate).
      const writable = !!record?.path && !record?.checkout;
      const missing = el.classList.contains("missing");
      // Renaming and deleting need a file that is there.
      for (const item of menu.querySelectorAll<HTMLButtonElement>(".note-menu-file")) {
        item.disabled = !writable || missing;
      }
      // Only on a note that IS marked: hiding is the eye's, and this is the
      // one door out of it.
      const mark = menu.querySelector<HTMLButtonElement>(".note-menu-mark");
      if (mark) {
        mark.hidden = !veiledOf(el._noteMarkdown);
        mark.disabled = missing;
      }
      // Re-aiming is the MISSING card's verb and the only one it has, so it is
      // not shown at all until the card is in that state.
      const point = menu.querySelector<HTMLButtonElement>(".note-menu-point");
      if (point) {
        point.hidden = !missing;
        point.disabled = !record;
      }
    });
  }

  // The index (ADR-0064 §10), rebuilt on every open from the live document.
  function toggleIndex(el: NoteCard) {
    openDrop(el, ".note-anchors", ".note-index", (pop) => {
      syncFromEditor(el);
      const found = anchorsOf(el._noteMarkdown);
      pop.textContent = "";
      if (!found.length) return false;
      for (const a of found) {
        const row = document.createElement("button");
        row.className = "note-menu-item";
        row.type = "button";
        row.textContent = a.text;
        row.title = a.text;
        row.addEventListener("click", () => {
          closeMenu();
          scrollToAnchor(el, a.index);
        });
        pop.append(row);
      }
      return true;
    });
  }

  // Show the index button only when there is an index. A card whose note has
  // no `##` would otherwise offer a control that opens onto nothing.
  function paintIndex(el: NoteCard) {
    const btn = el.querySelector<HTMLElement>(".note-index");
    if (!btn) return;
    btn.hidden = anchorsOf(el._noteMarkdown).length === 0;
  }

  // Scroll this card's body to its `index`-th `##`. By ORDINAL and not by
  // text, for `jump`'s reason: a note may hold two sections with one name.
  //
  // The BODY is scrolled, and nothing else. `heading.scrollIntoView()` walks
  // every scrollable ancestor and moves each one — so picking a section also
  // panned the plane under the card, which is the operator's report: the
  // canvas moved when only the note should have. The offset is computed
  // against the body's own box, which is the one box that must move.
  function scrollToAnchor(el: NoteCard, index: number) {
    const body = el.querySelector<HTMLElement>(".note-body");
    const heading = body?.querySelectorAll?.("h2")?.[index];
    if (!body || !heading) return;
    const top =
      heading.getBoundingClientRect().top - body.getBoundingClientRect().top + body.scrollTop;
    body.scrollTo({ top: Math.max(0, top), behavior: reducedMotion() ? "auto" : "smooth" });
  }

  function closeMenu() {
    if (!openMenu) return;
    openMenu.hidden = true;
    openMenu = null;
    openMenuTrigger = "";
    document.removeEventListener("pointerdown", closeOnOutside, true);
  }
  function closeOnOutside(ev: PointerEvent) {
    if (openMenu && !openMenu.contains(ev.target as Node | null) && !(ev.target as Element | null)?.closest?.(openMenuTrigger)) {
      closeMenu();
    }
  }

  // ---- the cheat sheet ----------------------------------------------------------

  // WHAT TO TYPE. The editor is hybrid WYSIWYG (ADR-0064 §6) — the markup
  // dissolves the moment it is recognised — which is exactly why the marks
  // themselves are not discoverable: there is nothing left on screen to read
  // them off. The `/` menu answers the same question for BLOCKS and nothing
  // answered it for marks.
  //
  // The flavour is CommonMark + GFM, because that is what the editor's two
  // presets parse; every row below was typed into the real bundle and its
  // markdown read back (2026-09-22), so nothing here is a guess at upstream's
  // rules. Three of them are this bundle's own additions, because upstream had
  // no rule at all: `[ ] ` on a plain line, `[text](url)` (commonmark ships an
  // input rule for an IMAGE and none for a link), and the `@@path` shorthand.
  const MARKDOWN_HELP = [
    ["**bold**", "Bold"],
    ["*italic*", "Italic"],
    ["`code`", "Inline code"],
    ["~~struck~~", "Strikethrough"],
    ["[text](url)", "A link"],
    ["@@path/to/file", "A link to that file"],
    ["#", "Large heading"],
    ["##", "Heading. It is also listed in the heading index."],
    ["-", "A bullet"],
    ["1.", "A numbered item"],
    ["[ ]", "A task. [x] marks it as done."],
    [">", "A quote"],
    ["|3x3|", "A 3×3 table"],
    ["---", "A horizontal line"],
    ["```", "A code block"],
    ["```mermaid", "A diagram. Click it to edit."],
    ["/", "The block menu"],
    ["Shift+Enter", "A line break inside the block"],
  ];
  // The trailing SPACE is what fires a block mark, and it cannot be seen in a
  // chip — so the table drops it and this line carries it instead.
  const MARKDOWN_HELP_NOTE = "Type a space after a mark to apply it.";

  // Borrowed from `wb-messages.ts`'s `askConfirm`: the shell's modal CLASSES
  // over DOM this module builds itself, so the card keeps working in the
  // detached-fence popup, which has no Alpine and no modal markup.
  function markdownHelp() {
    const scrim = document.createElement("div");
    scrim.className = "modal-scrim wb-note-help";
    const modal = document.createElement("div");
    modal.className = "modal note-help-modal";
    modal.setAttribute("role", "dialog");
    modal.setAttribute("aria-modal", "true");
    modal.setAttribute("aria-label", "Markdown in a note");
    const head = document.createElement("div");
    head.className = "modal-head";
    head.innerHTML = '<i class="bi bi-markdown"></i>';
    const heading = document.createElement("span");
    heading.className = "modal-title";
    heading.textContent = "Markdown in a note";
    const sub = document.createElement("span");
    sub.className = "modal-sub";
    sub.textContent = "CommonMark + GFM";
    head.append(heading, sub);
    const table = document.createElement("table");
    table.className = "note-help-table";
    for (const [type, gets] of MARKDOWN_HELP) {
      const tr = document.createElement("tr");
      const td1 = document.createElement("td");
      const code = document.createElement("code");
      code.textContent = type;
      td1.append(code);
      const td2 = document.createElement("td");
      td2.textContent = gets;
      tr.append(td1, td2);
      table.append(tr);
    }
    const note = document.createElement("p");
    note.className = "note-help-note";
    note.textContent = MARKDOWN_HELP_NOTE;
    const foot = document.createElement("div");
    foot.className = "modal-foot";
    const ok = document.createElement("button");
    ok.className = "btn accent";
    ok.type = "button";
    ok.textContent = "Close";
    foot.append(ok);
    modal.append(head, table, note, foot);
    scrim.append(modal);
    document.body.append(scrim);
    ok.focus();
    const done = () => {
      document.removeEventListener("keydown", onKey, true);
      scrim.remove();
    };
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape" || (ev.key === "Enter" && document.activeElement === ok)) {
        ev.stopPropagation();
        done();
      }
    };
    // CAPTURE, for `askConfirm`'s reason: the plane's accelerators and a
    // console's terminal both take Escape before a bubbling listener would.
    document.addEventListener("keydown", onKey, true);
    ok.addEventListener("click", done);
    return scrim;
  }

  function reducedMotion() {
    try {
      return !!window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches;
    } catch {
      return false;
    }
  }

  return { toggleMenu, toggleIndex, paintIndex, closeMenu, markdownHelp, MARKDOWN_HELP };
}
