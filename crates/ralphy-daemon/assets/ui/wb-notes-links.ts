// The links inside a note card (ADR-0064 §12). `createNotes` builds the
// installer with the card's names as typed deps; the text of the handler is
// the one it had in `wb-notes.ts`.
import { isSafeScheme, popupLinkTarget } from "./wb-notes-folds.ts";
import { sendDocument } from "./wb-events.ts";
import type { NoteCard, NoteSource } from "./wb-types.d.ts";

/** What `createNotes` hands the links: the page's names and the card's record. */
export type NoteLinksDeps = {
  /** The page's window: the viewer's rule for where a link goes. */
  window: Window;
  /** The page's document: where an open request is sent. */
  document: Document;
  /** The desk record of a card, by id. */
  recordOf: (id: string | null | undefined) => NoteSource | null;
};

/** The installer of the click rule for one card. */
export function installLinks(deps: NoteLinksDeps) {
  const { window, document, recordOf } = deps;

  // The anchor a click landed on, when it is a link of this card.
  function linkAnchor(ev: Event, el: NoteCard) {
    const a = (ev.target as Element | null)?.closest?.<HTMLAnchorElement>("a[href]");
    return a && el.contains(a) ? a : null;
  }

  // Resolved against the CHECKOUT ROOT, not the note's own directory: a
  // note lands in `.ralphy/notes/` by default, and a path written there is
  // the operator's path into their repo, not into the run-state directory.
  function linkTargetOf(href: string | null) {
    return window.WBViewer?.linkTarget?.("", href) || popupLinkTarget(href);
  }

  // The open REQUEST to the shell for a path inside the repo.
  function requestOpen(record: NoteSource | null, target: { path: string; fragment: string }) {
    sendDocument(document, "workbench:open-request", {
      project: record?.repo || null,
      path: target.path,
      fragment: target.fragment,
      checkout: record?.checkout ?? null,
    });
  }

  // A link inside a note obeys the viewer's rule (ADR-0064 §12): an external
  // one opens in a new tab, a repo-relative one is an open REQUEST to the
  // shell. A raw `href` would navigate the whole workbench away.
  function followLink(ev: Event, el: NoteCard) {
    const a = linkAnchor(ev, el);
    if (!a) return;
    const href = a.getAttribute("href");
    const target = linkTargetOf(href);
    if (!target || target.kind === "fragment") {
      ev.preventDefault();
      return;
    }
    if (target.kind === "external") {
      // The SCHEME is an allowlist, and that is a security control, not
      // tidiness. The viewer's `linkTarget` answers "external" for ANY
      // scheme and leaves it to the browser — safe there only because the
      // viewer's markdown went through DOMPurify first (wb-file-viewer.ts), which
      // drops a `javascript:` href. A note's markdown never meets that pass:
      // Crepe renders the link straight into the document, so a `.note` file
      // — repo bytes, which an agent may have written — could otherwise run
      // script on the daemon's own origin with one click.
      if (!isSafeScheme(href)) {
        ev.preventDefault();
        return;
      }
      a.target = "_blank";
      a.rel = "noopener";
      return;
    }
    ev.preventDefault();
    requestOpen(recordOf(el.dataset.noteId), target);
  }

  return function installOn(el: NoteCard) {
    el.addEventListener("click", (ev) => followLink(ev, el));
  };
}
