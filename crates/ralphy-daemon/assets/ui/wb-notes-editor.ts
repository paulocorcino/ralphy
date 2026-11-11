// The note card's editor: Crepe mounted over the body, and the read that fills
// it (ADR-0064 §§6, 11). The save is in `wb-notes-save.ts`. `createNotes` builds
// it with the card's closure names as typed deps; the text of each function is
// the one it had in `wb-notes.ts`.
import { WBFail } from "./wb-fail.ts";
import { removeNote } from "./wb-desk-folds.ts";
import {
  ADD_TITLE,
  EMPTY_TITLE_HINT,
  REMOVE_TITLE,
  applyLook,
  bodyOf,
  savedLabel,
  styleOf,
  titleFieldOf,
  titleOf,
  veiledOf,
  withHeader,
  withVeil,
} from "./wb-notes-folds.ts";
import { createNoteSave, dress } from "./wb-notes-save.ts";
import { sendDocument } from "./wb-events.ts";
import type { CardHost, DeskNote, NoteCard, NoteEditor, NoteSource } from "./wb-types.d.ts";

// The one surface `vendor-build/crepe/entry.js` exports. It is set by a script
// the shell and the detached fence page load, so it may not be there.
export type CrepeLean = {
  create(options: {
    root: HTMLElement;
    value: string;
    readonly: boolean;
    placeholder: string;
    titleLabels: { add: string; remove: string; heading: string };
    onChange: (markdown: string) => void;
  }): Promise<NoteEditor>;
};

/** What `createNotes` hands the editor: the card's closure names. */
export type NoteEditorDeps = {
  /** The page's window: `CrepeLean` and the daemon's `observe` and `write`. */
  window: Window & { CrepeLean?: CrepeLean };
  /** The page's document. */
  document: Document;
  /** The page's consoles; null where a test has none. */
  consoleHost: CardHost | null;
  /** The desk record of a card, by id. */
  recordOf: (id: string | null | undefined) => NoteSource | null;
  /** Write one card's record back, keeping the rest of the collection. */
  patch: (id: string | null | undefined, fields: Partial<DeskNote>) => void;
  /** Put the desk's cards on the stage. */
  render: () => void;
  /** Repaint the card's index from its document. */
  paintIndex: (el: NoteCard) => void;
  /** The veil's panel, shown in place of an editor. */
  veilPanel: () => HTMLElement;
  /** Paint the veil on one card. */
  paintVeil: (el: NoteCard) => void;
  /** True in a detached fence's popup, where the document holds a fragment of the plane. */
  fragment: () => boolean;
};

export function createNoteEditor(deps: NoteEditorDeps) {
  const { window, document, consoleHost, recordOf, render, paintIndex, veilPanel, paintVeil } =
    deps;
  const { syncFromEditor, markDirty, flush } = createNoteSave({
    window,
    document,
    recordOf,
    patch: deps.patch,
    fragment: deps.fragment,
    paintTitle,
    paintState,
    paintPath,
  });

  // Mount Crepe over the card's body with `markdown` (front matter stripped —
  // the look is chrome, not text the operator edits). Re-entrant: a retitle
  // hands the editor a document it did not write, so the live view is given
  // back FIRST — two ProseMirror views over one body is the leak dormancy
  // exists to prevent, arriving by another door.
  function mountEditor(el: NoteCard, markdown: string): Promise<NoteEditor | null> {
    const body = el.querySelector<HTMLElement>(".note-body");
    if (!body || !window.CrepeLean) return Promise.resolve(null);
    const previous = el._noteEditor;
    el._noteEditor = null;
    try {
      previous?.destroy();
    } catch {}
    el._noteMarkdown = markdown;
    // A card with an editor is AWAKE by definition — `wakeCard` is not the
    // only door any more, a retitle remounts too, and a card left flagged
    // asleep with a live view would never be given back again.
    el._noteAsleep = false;
    const style = styleOf(markdown);
    applyLook(el, style);
    body.textContent = "";
    // A VEILED CARD MOUNTS NO EDITOR (ADR-0064 §8 as amended). Not a blur and
    // not `display: none`: the text is never put into the document at all, and
    // the markdown held here is cut down to its header, so the tab does not
    // carry the body either. Revealing RE-READS the file — which is also what
    // waking from dormancy does, and for the same reason.
    if (veiledOf(markdown) && !el._noteRevealed) {
      el._noteMarkdown = withVeil(withHeader("", titleFieldOf(markdown) ?? "", style), true);
      body.append(veilPanel());
      paintVeil(el);
      paintTitle(el);
      paintState(el, "");
      return Promise.resolve(null);
    }
    paintVeil(el);
    // Marked here and cleared on teardown: `create()` resolves a tick or two
    // later, and a card closed, detached or evicted in between would otherwise
    // leave a live ProseMirror view with its listeners on a detached node —
    // the exact leak dormancy exists to prevent.
    el._noteGone = false;
    return window.CrepeLean.create({
      root: body,
      value: bodyOf(markdown),
      // Never read-only from the lock: a locked card is pinned, not frozen.
      readonly: false,
      placeholder: "Write a note…",
      titleLabels: { add: ADD_TITLE, remove: REMOVE_TITLE, heading: EMPTY_TITLE_HINT },
      onChange: (next) => {
        // Milkdown reports its own value back on mount too; a change that is
        // not a change must not mark the card dirty, or every card would
        // autosave itself once per reload.
        if (next === bodyOf(el._noteMarkdown)) return;
        el._noteMarkdown = dress(el, next);
        markDirty(el);
        paintTitle(el);
      },
    })
      .then((editor) => {
        if (el._noteGone === true) {
          try {
            editor.destroy();
          } catch {}
          return null;
        }
        el._noteEditor = editor;
        paintTitle(el);
        // The footer says when this note last landed, from the moment it
        // opens — not only after the card has saved it once itself.
        if (!el._noteDirty) paintState(el, "");
        if (el._noteFocusOnMount === true) {
          el._noteFocusOnMount = false;
          try {
            editor.dom()?.focus();
          } catch {}
        }
        return editor;
      })
      .catch((err: unknown) => {
        paintState(
          el,
          "Could not start the editor: " +
            String(err instanceof Error && err.message ? err.message : err),
        );
        return null;
      });
  }

  // Read the note's file into the card. A missing file is a STATE, not a
  // disappearance: the card stays, says so, and — §11 as amended 2026-09-22 —
  // keeps an editor, because the way back from that state is the operator
  // writing in it.
  function loadInto(el: NoteCard, record: NoteSource) {
    if (record.path === undefined || record.path === "") {
      return mountEditor(el, dress(el, ""));
    }
    return window.WBDaemon.observe(
      "note.read",
      window.WBDaemon.withCheckout({ repo: record.repo, path: record.path }, record.checkout),
    )
      .then((reply) => {
        if (WBFail.isError(reply)) return readRefused(el, record, reply);
        el.classList.remove("missing");
        // After a reload the card remembers no save of its own, and this is
        // the only place the answer exists (`note.read` carries it).
        el._noteSavedAt = Number(reply.modified) || null;
        return mountEditor(el, reply.markdown ?? "");
      })
      .catch((err: unknown) => {
        paintMissing(
          el,
          WBFail.cause(
            { message: err instanceof Error ? err.message : undefined },
            "The daemon did not answer.",
          ),
        );
        return null;
      });
  }

  // A `note.read` the daemon refused: a file that is not a note leaves the
  // stage, any other refusal is a missing file the card keeps.
  function readRefused(el: NoteCard, record: NoteSource, reply: ReplyOf<"note.read">) {
    const reason = WBFail.message(reply, "Could not read the file.");
    // A file that is not ours is not a missing note — it is someone
    // else's file under our extension (ADR-0064 §11). The card goes and
    // the shell says so; keeping it would put an editor over bytes and
    // the first autosave would overwrite them.
    if (reason === "not a note" && record.path !== undefined && record.path !== "") {
      consoleHost?.saveNotes(removeNote(consoleHost.notes(), record.id));
      render();
      sendDocument(document, "workbench:open-request", {
        project: record.repo ?? null,
        path: record.path,
        checkout: record.checkout ?? null,
        as: "bytes",
      });
      return null;
    }
    // The reason in the footer, beside the path that footer already
    // shows — and the whole of it on hover, because `.note-state` is a
    // narrow box and "not found" alone is the half that matters.
    const said = WBFail.why(reply, "the file could not be read");
    paintMissing(el, said, `${record.path} — ${said}`);
    // An editor over what the card still holds — the unsaved text if
    // there is any, and an empty document if the read is all this card
    // ever had. Typing in it makes the card dirty, and a dirty card
    // writes its file back. Without this the missing state is a dead
    // end: a red line where the note was and no way to put it back.
    if (el._noteEditor) return null;
    // The line is repainted AFTER the mount: mounting an editor ends by
    // clearing the footer, and the card would go translucent while
    // saying nothing about why.
    return mountEditor(el, el._noteMarkdown || dress(el, "")).then((mounted) => {
      paintMissing(el, said, `${record.path} — ${said}`);
      return mounted;
    });
  }

  // The card says its file is gone — in the FOOTER, and in the dashed border
  // the `missing` class draws. The body is left alone: a read that fails must
  // not take the text with it, and emptying it is the one irreversible thing
  // this state can do. Measured 2026-09-22 from the operator's screenshot — a
  // card in a fence went translucent and blank while pointing at a path no
  // file was ever written to, and what it blanked had never reached the disk.
  function paintMissing(el: NoteCard, reason: string, full?: string) {
    el.classList.add("missing");
    paintState(el, reason);
    const state = el.querySelector<HTMLElement>(".note-state");
    if (state) state.title = full === undefined || full === "" ? reason : full;
  }

  // The head, repainted from the document: both the name and whether there is
  // an index are folds of `_noteMarkdown`, so every caller that has one has
  // the other.
  function paintTitle(el: NoteCard) {
    const title = el.querySelector<HTMLElement>(".note-title");
    if (title) title.textContent = titleOf(el._noteMarkdown, "Untitled note");
    paintIndex(el);
  }

  // `text` is the momentary word — "…", "Saved", a refusal. EMPTY means idle,
  // and idle is where the last-save stamp lives: the footer is the one place
  // that can answer "when did this land" without opening the file's
  // properties (asked 2026-09-22).
  function paintState(el: NoteCard, text?: string) {
    const state = el.querySelector<HTMLElement>(".note-state");
    if (!state) return;
    if (text !== undefined && text !== "") {
      state.textContent = text;
      state.title = "";
      return;
    }
    state.textContent = savedLabel(el._noteSavedAt);
    state.title =
      el._noteSavedAt !== null && el._noteSavedAt !== 0
        ? new Date(el._noteSavedAt).toLocaleString()
        : "";
  }

  function paintPath(el: NoteCard, path: string | null | undefined) {
    const field = el.querySelector<HTMLElement>(".note-path");
    if (field) {
      field.textContent = path ?? "";
      field.title =
        path === null || path === undefined || path === "" ? "This note is not saved yet" : path;
    }
    const dir = el.querySelector<HTMLInputElement>(".note-dir");
    if (dir) dir.hidden = (path ?? "") !== "";
  }

  return {
    mountEditor,
    loadInto,
    paintTitle,
    paintState,
    syncFromEditor,
    markDirty,
    flush,
    paintPath,
  };
}
