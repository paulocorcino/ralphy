/* ---------------------------------------------------------------------------
   The console's fence list: the fence records on the stage, their count,
   columns and lock chrome, the fence verbs that change the desk (create,
   rename, lock, remove), the refusal said on a fence, the detach glyph, and
   the focused fence.

   `createFenceList(deps)` returns the functions the console, its chrome, its
   fences, its desk, its view and the `WBConsole` API use (ADR-0075 D7). They
   read the console only through `deps`, and `FenceListDeps` lists every read,
   so `tsc` refuses a read outside it. `wb-console.ts` creates one per console,
   before its view, and keeps the fence records (`fences`, `notes`), the desk
   and the lifecycle channel. The focused fence is per-client state and lives
   here.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import * as WBConsoleInput from "./wb-console-input.ts";
import * as WBDeskFolds from "./wb-desk-folds.ts";
import type { PopupRegistry } from "./wb-console-popups.ts";
import type { ConsoleWin, DeskFence, DeskNote, ExtentOpts, NoteCard, Rect } from "./wb-types.d.ts";

const { PHONE_MAX_WIDTH } = WBConsoleInput;
const { fenceSpawnRect, rectsOverlap, fenceOf, fenceHolds } = WBGeometry;
const { fenceSummaries, nextFenceSlot, nextFenceName, fenceCycle, FENCE_MAX } = WBDeskFolds;

// A fence element: `buildFence` always sets its `data-fence-id`.
export type FenceElement = HTMLElement & { dataset: { fenceId: string } };

// The refusal span of a fence, with the timer that clears it.
type FenceNotice = HTMLElement & { _noticeTimer?: ReturnType<typeof setTimeout> };

// What the fence list reads from the console, and nothing else.
export type FenceListDeps = {
  // The console's page: the note cards (`WBNotes`).
  window: Window;
  // The page: a fence name is not written while it has the focus.
  document: Document;
  // The console's options: the torn-off fence window boots without
  // `autoBoot`, and derives no lock from a fence.
  OPTS: { autoBoot?: boolean };
  // The fences this tab detached: a detached fence shows no columns, keeps
  // its count from the registry and cannot be removed.
  popups: PopupRegistry;
  // The fence records and the note cards; the console reassigns both lists,
  // so they are read at each use.
  fences: () => DeskFence[];
  notes: () => DeskNote[];
  // The plane and the viewport; null before the page has them.
  stage: () => HTMLElement | null;
  workspace: () => HTMLElement | null;
  // An element's rect read from the DOM.
  restoreRect: (el: HTMLElement) => Rect;
  // Grows or fits the stage to the windows on it.
  applyExtent: (opts?: ExtentOpts) => void;
  // The viewport width the columns rule reads.
  columnMeasure: () => { viewport: number };
  // An element under a gesture of the operator keeps its rect.
  inGesture: (el: HTMLElement) => boolean;
  // A new fence id.
  newFenceId: () => string;
  // Paints a console's lock glyph (its own lock or its fence's).
  paintLockGlyph: (win: ConsoleWin) => void;
  // Writes the fence records to the desk.
  saveFences: (next: DeskFence[]) => void;
  // Slides the viewport to a fence. The view is built after the fence list,
  // so this is a lazy arrow.
  jumpToFence: (id: string) => HTMLElement | null;
  // Builds a fence element. The fences are built after the fence list, so
  // this is a lazy arrow.
  buildFence: (f: DeskFence) => HTMLElement;
};

export function createFenceList(deps: FenceListDeps) {
  const {
    window,
    document,
    OPTS,
    popups,
    fences,
    notes,
    stage,
    workspace,
    restoreRect,
    applyExtent,
    columnMeasure,
    inGesture,
    newFenceId,
    paintLockGlyph,
    saveFences,
    jumpToFence,
    buildFence,
  } = deps;
  const { isDetached, detachedMembers } = popups;

  // ---- the fence floor ---------------------------------------------------------
  // The fence element, its two gestures, tiling and detaching are
  // `wb-console-fences.ts`.
  const FENCE_NAME_MAX = 60;

  // The refusal flash `createFence` schedules. Module scope so a gesture
  // starting inside its 600 ms window can CANCEL it: otherwise the timer strips
  // a `fence-invalid` the gesture put there, and with the cursor at rest no
  // move re-adds it.
  let fenceFlash: { el: HTMLElement; timer: ReturnType<typeof setTimeout> } | null = null;
  function clearFenceFlash() {
    if (fenceFlash == null) return;
    clearTimeout(fenceFlash.timer);
    fenceFlash.el.classList.remove("fence-invalid");
    fenceFlash = null;
  }

  function fenceEl(id: string | null) {
    const st = stage();
    if (!st) return null;
    for (const el of st.querySelectorAll<FenceElement>(".fence")) {
      if (el.dataset.fenceId === id) return el;
    }
    return null;
  }

  // Re-derive every fence's count readout from the stage (#342). Membership is
  // never stored, so this folds the LIVE rects — from `renderFences`,
  // `setWin` and `forgetRecord`. NOT from `applyExtent`: it fires per
  // mousemove, and `offsetLeft` on a `.tiling` window is the INTERPOLATED
  // value mid-transition.
  function refreshFenceChrome() {
    const st = stage();
    // A hidden tab measures 0 and this fold reads MEASURED rects: refreshing
    // there writes `0 consoles` onto every fence. `refitAll` calls this again
    // on the first frame that can measure.
    if (!st || !st.offsetWidth || !st.offsetHeight) return;
    const els = new Map<string, FenceElement>();
    for (const el of st.querySelectorAll<FenceElement>(".fence")) els.set(el.dataset.fenceId, el);
    // A DETACHED fence's consoles are in a popup, so the membership fold
    // answers zero — but the fence is emptied, not empty (ADR-0051 §7a). Take
    // the count from the registry for those; the fold stays pure.
    const away = detachedMembers();
    for (const s of fenceSummaries(readFenceRects(st), readWindowRects(st))) {
      const el = els.get(s.id);
      if (!el) continue;
      const n = away[s.id] ? away[s.id].length : s.count;
      // Parenthesised: it trails the name field and reads as an aside to it.
      const count = el.querySelector<HTMLElement>(".fence-count");
      if (count) count.textContent = `(${n} console${n === 1 ? "" : "s"})`;
      const cols = el.querySelector<HTMLButtonElement>(".fence-columns");
      if (cols) cols.disabled = s.count === 0;
    }
    paintFenceColumns();
    // A console HELD by a locked fence wears the fence's lock (the class drops
    // its bands and grab cursor). Derived here with membership, from live rects.
    for (const w of st.querySelectorAll<ConsoleWin>(".session-window")) {
      w.classList.toggle("held", !w._deskLocked && heldByFence(w));
      paintLockGlyph(w);
    }
    // A card held by a locked fence is read-only for the same reason, and by
    // the same derivation (ADR-0064 §8) — the record is not rewritten. NOT in
    // the popup: `mountDetached` re-origins its members' rects into this
    // window while `fences` still holds the shell's stage coordinates, so the
    // derivation there would match a card to whatever fence happens to cover
    // the translated point.
    for (const el of OPTS.autoBoot === false ? [] : st.querySelectorAll<NoteCard>(".note-card")) {
      const own = notes().find((n: DeskNote) => n.id === el.dataset.noteId);
      const held = !own?.locked && !!fenceOf(fences(), restoreRect(el))?.locked;
      el.classList.toggle("held", held);
      window.WBNotes?.applyLock(el, !!own?.locked || held);
    }
  }

  // A detached fence's consoles are in the popup, and a phone paints one
  // console: no columns there (ADR-0051 §5, 2026-09-30). Also called from
  // `applyColumns`, which runs on every resize.
  function paintFenceColumns() {
    const narrow = columnMeasure().viewport <= PHONE_MAX_WIDTH;
    for (const el of stage()?.querySelectorAll<FenceElement>(".fence") || []) {
      const btn = el.querySelector<HTMLButtonElement>(".fence-columns");
      if (btn) btn.hidden = narrow || isDetached(el.dataset.fenceId);
    }
  }

  // The two DOM reads `refreshFenceChrome` and `fenceList` share: the stage is
  // where a fence and a window ARE, and membership is derived from those live
  // rects, never from `fences` or `wins`.
  function readFenceRects(st: HTMLElement) {
    return [...st.querySelectorAll<FenceElement>(".fence")].map((el) => ({
      id: el.dataset.fenceId,
      name: el.querySelector<HTMLInputElement>(".fence-name")?.value || "",
      rect: restoreRect(el),
    }));
  }

  function readWindowRects(st: HTMLElement) {
    return [...st.querySelectorAll<ConsoleWin>(".session-window")].map((w) => ({
      id: w._deskId,
      repo: w._deskRepo,
      rect: restoreRect(w),
    }));
  }

  // The fence list the toolbar picker shows (#343) — the same fold the fence
  // chrome reads. A SNAPSHOT at menu open, like `list()`.
  // The fence RECORDS (id, name, rect, locked), not the chrome summaries: the
  // note card derives its lock from the fence holding it and needs the rects.
  function fenceRecords() {
    return fences().map((f: DeskFence) => ({ ...f }));
  }

  // The cards, rendered by `wb-notes.ts`. Called wherever `renderFences` is —
  // the two collections go on the plane together or the extent is folded over
  // half of them.
  function renderNotes() {
    window.WBNotes?.render();
  }

  function fenceList() {
    const st = stage();
    if (!st) return [];
    return fenceSummaries(readFenceRects(st), readWindowRects(st));
  }

  // A fence's own lock, and a console held by a locked fence: the same
  // `fenceOf` fold as membership.
  function fenceLocked(id: string | null) {
    return !!fences().find((f: DeskFence) => f.id === id)?.locked;
  }
  function heldByFence(el: HTMLElement) {
    return fenceHolds(fences(), restoreRect(el), OPTS.autoBoot === false);
  }
  // A fence's lock, painted: the class, the glyph, and the tile button, which is a
  // no-op on a locked fence and says so by being disabled.
  function paintFenceLock(el: HTMLElement, locked: boolean) {
    el.classList.toggle("locked", !!locked);
    const btn = el.querySelector<HTMLButtonElement>(".fence-lock");
    if (btn) {
      btn.innerHTML = locked ? '<i class="bi bi-lock-fill"></i>' : '<i class="bi bi-unlock"></i>';
      btn.title = locked ? "Unlock this fence" : "Lock this fence in place";
      btn.setAttribute("aria-pressed", locked ? "true" : "false");
    }
    const tile = el.querySelector<HTMLButtonElement>(".fence-arrange");
    if (tile) tile.disabled = !!locked;
  }
  function setFenceLock(id: string, locked: boolean) {
    saveFences(fences().map((x: DeskFence) => (x.id === id ? { ...x, locked: !!locked } : x)));
    renderFences();
  }

  // A refused fence verb, said ON the fence. Cleared on a timer so a stale
  // refusal cannot outlive the gesture that caused it.
  function fenceNotice(id: string, text: string) {
    const el = fenceEl(id)?.querySelector<FenceNotice>(".fence-notice");
    if (!el) return;
    el.textContent = text;
    clearTimeout(el._noticeTimer);
    el._noticeTimer = setTimeout(() => {
      el.textContent = "";
    }, 2600);
  }

  function showDetachGlyph(id: string, on: boolean) {
    const away = fenceEl(id)?.querySelector<HTMLElement>(".fence-detached");
    if (away) away.hidden = !on;
  }

  // The verb the shortcut calls: walk one step and jump. Returns the id landed
  // on, or null when there is no fence (so the shell leaves the key unswallowed).
  function stepFence(step: number) {
    const st = stage();
    if (!st) return null;
    const id = fenceCycle(readFenceRects(st), focusedFence, step);
    if (id == null) return null;
    return jumpToFence(id) ? id : null;
  }

  // Upsert the DOM against `fences`. The rect is re-applied unless the fence is
  // under a gesture; the NAME is not written while the operator is typing in it
  // (an in-flight GET would yank the caret to a stale value).
  function renderFences() {
    const st = stage();
    if (!st) return;
    // Index the DOM by id rather than building an attribute SELECTOR: an id is
    // daemon data (a hand-edited `desk.toml` can carry any string), and one
    // quote in it would throw a SyntaxError out of the whole restore.
    const nodes = new Map<string, FenceElement>();
    for (const el of st.querySelectorAll<FenceElement>(".fence")) nodes.set(el.dataset.fenceId, el);
    const seen = new Set<string>();
    for (const f of fences()) {
      seen.add(f.id);
      const el = nodes.get(f.id) || buildFence(f);
      const r = f.rect || {};
      if (!inGesture(el)) {
        el.style.left = (r.left || 0) + "px";
        el.style.top = (r.top || 0) + "px";
        el.style.width = (r.width || 0) + "px";
        el.style.height = (r.height || 0) + "px";
      }
      const name = el.querySelector<HTMLInputElement>(".fence-name");
      if (name && name !== document.activeElement) name.value = f.name || "";
      paintFenceLock(el, !!f.locked);
    }
    for (const [id, el] of nodes) {
      if (!seen.has(id)) el.remove();
    }
    // A focused fence that is gone must not leave a dangling id: the birth path
    // resolves it, and a stale one would place the next console nowhere (#343).
    if (focusedFence && !seen.has(focusedFence)) clearFenceFocus();
    // The class rides the ELEMENT, and `buildFence` makes a fresh one for a
    // fence that arrived after the focus was taken.
    else if (focusedFence) focusFence(focusedFence);
    refreshFenceChrome();
  }

  // The next default name, from the numbers ALREADY on the plane, not the
  // count: `fences.length + 1` freezes at FENCE_MAX, so every fence past the
  // cap was born "Fence 13" (MEASURED), and the fence list IS the plane's map.
  // Only `Fence <n>` counts: a rename to "backend" must not move the next
  // default, and "Fence 99" is a number the operator chose.
  function atFenceCap() {
    return fences().length >= FENCE_MAX;
  }
  function createFence() {
    // AT THE CAP, REFUSE: the daemon refuses a fence past it too.
    // Refusing and SAYING SO are two jobs: this module knows no Alpine, so it
    // answers `false` and `newFence()` in app.ts does the talking; `atFenceCap`
    // is exported so the row can be disabled BEFORE the click.
    if (atFenceCap()) return false;
    const ws = workspace();
    const offset = { left: ws?.scrollLeft || 0, top: ws?.scrollTop || 0 };
    const viewport = { width: ws?.clientWidth || 0, height: ws?.clientHeight || 0 };
    const slot = nextFenceSlot(
      fences().map((f: DeskFence) => f.rect),
      offset,
      viewport,
    );
    // The whole scanned band is full. REFUSE — do not nudge the new fence into
    // a gap the operator never chose.
    if (slot < 0) {
      const blocked = fenceSpawnRect(offset, viewport, 0);
      const hit = fences().find((x: DeskFence) => rectsOverlap(blocked, x.rect || {}));
      const el = hit && fenceEl(hit.id);
      if (el) {
        clearFenceFlash();
        el.classList.add("fence-invalid");
        fenceFlash = { el, timer: setTimeout(clearFenceFlash, 600) };
      }
      return;
    }
    const spawn = fenceSpawnRect(offset, viewport, slot);
    const id = newFenceId();
    saveFences(
      fences().concat([
        {
          id,
          // Numbered from the names on the plane, not by the slot taken.
          name: nextFenceName(fences()),
          rect: spawn,
          locked: false,
        },
      ]),
    );
    renderFences();
    applyExtent();
    // A slot below the fold is still a fence the operator asked for, so travel
    // to it; a creation the screen does not acknowledge reads as a no-op. One
    // already on screen is left alone.
    const onScreen =
      spawn.left >= offset.left &&
      spawn.top >= offset.top &&
      spawn.left + spawn.width <= offset.left + viewport.width &&
      spawn.top + spawn.height <= offset.top + viewport.height;
    if (!onScreen) jumpToFence(id);
    return true;
  }

  function renameFence(id: string, name: string | null | undefined) {
    saveFences(
      fences().map((f: DeskFence) =>
        f.id === id
          ? { ...f, name: String(name == null ? "" : name).slice(0, FENCE_NAME_MAX) }
          : f,
      ),
    );
    renderFences();
  }

  function removeFence(id: string) {
    // Removing a DETACHED fence would destroy the glyph that brings its consoles
    // home (ADR-0051 §7a) while the registry kept a `DETACH_MAX` slot. Refuse.
    if (isDetached(id)) {
      fenceNotice(id, "Return this fence's consoles to this window first");
      WB.emit("fence-remove-refused", { fence: id, reason: "detached" });
      return;
    }
    saveFences(fences().filter((f: DeskFence) => f.id !== id));
    renderFences();
    applyExtent();
  }

  // ---- navigating the plane ----------------------------------------------------
  // ---- the fence list is the map (issue #343, ADR-0051 §7) ---------------------
  // The focused fence is PER-CLIENT transient state: never written to the desk,
  // never to `WBView`. The desk is shared last-write-wins (ADR-0051 §8), so a
  // stored focus would move where the OTHER operator's next console is born.
  let focusedFence: string | null = null;

  function focusedFenceId() {
    return focusedFence;
  }

  function focusFence(id: string | null) {
    focusedFence = id;
    const st = stage();
    if (!st) return;
    for (const el of st.querySelectorAll<FenceElement>(".fence")) {
      el.classList.toggle("is-focused", el.dataset.fenceId === id);
    }
  }

  function clearFenceFocus() {
    focusFence(null);
  }

  return {
    fenceLocked,
    heldByFence,
    setFenceLock,
    clearFenceFlash,
    fenceEl,
    refreshFenceChrome,
    paintFenceColumns,
    readFenceRects,
    readWindowRects,
    fenceRecords,
    renderNotes,
    fenceList,
    stepFence,
    renderFences,
    atFenceCap,
    createFence,
    renameFence,
    removeFence,
    focusedFenceId,
    focusFence,
    clearFenceFocus,
    fenceNotice,
    showDetachGlyph,
  };
}

export type FenceList = ReturnType<typeof createFenceList>;
