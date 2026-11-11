// The note card's dormancy: a card far from the view gives its editor back and
// wakes when it returns (ADR-0064 §14). `createNotes` builds it with the card's
// names as typed deps; the text of each function is the one it had in
// `wb-notes.ts`.
import { DORMANT_MARGIN_PX, dormantAfterMs } from "./wb-console-gpu.ts";
import { noteDormancyDecision, titleOf } from "./wb-notes-folds.ts";
import type { NoteCard, NoteSource } from "./wb-types.d.ts";

/** What `createNotes` hands the dormancy: the card's names. */
export type NoteDormancyDeps = {
  /** The page's window: where `IntersectionObserver` is looked for. */
  window: Window & { IntersectionObserver?: typeof IntersectionObserver };
  /** The page's document. */
  document: Document;
  /** The desk record of a card, by id. */
  recordOf: (id: string | null | undefined) => NoteSource | null;
  /** Read the note's file into the card. */
  loadInto: (el: NoteCard, record: NoteSource) => unknown;
  /** The id of the card on top, or null. */
  onTopNow: () => string | null;
  /** Is this card showing nothing right now? */
  veiledNow: (el: NoteCard) => boolean;
};

export function createNoteDormancy(deps: NoteDormancyDeps) {
  const { window, document, recordOf, loadInto, onTopNow, veiledNow } = deps;

  // Dormancy (ADR-0064 §14). Its OWN observer, not the consoles': a console
  // sleeps by dropping a socket, a card by destroying an editor, and the two
  // have different refusals — a card with unsaved text never sleeps.
  let observer: IntersectionObserver | null = null;
  let sweeper: ReturnType<typeof setInterval> | null = null;
  const watched = new Map<NoteCard, { visible: boolean; since: number }>();
  const SWEEP_MS = 5000;
  function trackDormancy(el: NoteCard) {
    if (typeof window.IntersectionObserver !== "function") return;
    const root = document.getElementById("workspace");
    if (!root) return;
    observer ??= new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- it watches only cards
          const card = entry.target as NoteCard;
          const seen = watched.get(card);
          if (!seen) continue;
          seen.visible = entry.isIntersecting;
          seen.since = Date.now();
          if (entry.isIntersecting) wakeCard(card);
        }
      },
      { root, rootMargin: `${DORMANT_MARGIN_PX}px` },
    );
    sweeper ??= setInterval(sweepDormancy, SWEEP_MS);
    watched.set(el, { visible: true, since: Date.now() });
    observer.observe(el);
  }
  function untrackDormancy(el: NoteCard) {
    watched.delete(el);
    observer?.unobserve(el);
    // Nothing left to watch: a page that once showed a note must not keep a
    // 5 s timer and an observer alive for the document's life — a popup that
    // reattached its fence would carry both forever.
    if (watched.size === 0) {
      if (sweeper != null) clearInterval(sweeper);
      sweeper = null;
      observer?.disconnect();
      observer = null;
    }
  }

  function sweepDormancy() {
    const after = dormantAfterMs();
    for (const [el, seen] of [...watched]) {
      if (!el.isConnected) {
        untrackDormancy(el);
        continue;
      }
      const verdict = noteDormancyDecision({
        visible: seen.visible,
        onTop: el.dataset.noteId === onTopNow(),
        dirty: el._noteDirty,
        inFlight: el._noteInFlight,
        asleep: el._noteAsleep === true,
        elapsed: Date.now() - seen.since,
        after,
      });
      if (verdict === "sleep") sleepCard(el);
    }
  }
  function sleepCard(el: NoteCard) {
    if (el._noteAsleep === true || el._noteDirty || el._noteInFlight) return;
    // A veiled card has already given its editor back and is already showing
    // the one line it is allowed to show; putting it to sleep would replace
    // that with the "asleep" line and lose what the card is saying.
    if (veiledNow(el)) return;
    el._noteAsleep = true;
    const editor = el._noteEditor;
    el._noteEditor = null;
    try {
      editor?.destroy();
    } catch {}
    const body = el.querySelector<HTMLElement>(".note-body");
    if (body) {
      body.textContent = "";
      const p = document.createElement("p");
      p.className = "note-asleep";
      p.textContent = titleOf(el._noteMarkdown, "Untitled note");
      body.append(p);
    }
  }
  function wakeCard(el: NoteCard) {
    if (el._noteAsleep !== true) return;
    el._noteAsleep = false;
    const record = recordOf(el.dataset.noteId);
    // Re-READ rather than remount the text held here: another page may have
    // written the file while this card slept, and the file is the note.
    if (record) loadInto(el, record);
  }

  return { trackDormancy, untrackDormancy, wakeCard };
}
