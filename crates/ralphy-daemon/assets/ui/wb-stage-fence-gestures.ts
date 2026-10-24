/* ---------------------------------------------------------------------------
   The fence gestures: moving a fence with the consoles and cards it carries,
   and resizing a fence (issue #341, ADR-0075 phase 5).

   `createFenceGestures(deps)` returns `startFenceMove` and `startFenceResize`.
   `createFences` in `wb-stage-fences.ts` builds it and hands the two
   functions to the fence element. They read the console only through `deps`.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import * as WBConsoleInput from "./wb-console-input.ts";
import { DIRS } from "./wb-stage-chrome.ts";
import type { Gestures } from "./wb-stage-chrome.ts";
import type { ConsoleWin, DeskFence, DeskWindowFields, ExtentOpts, NoteCard, Point, Rect } from "./wb-types.d.ts";

const { dragThreshold, dragBegins } = WBConsoleInput;
const { FENCE_MIN, fenceMembership, fenceFits, fenceMoveDelta, resizeRect } = WBGeometry;

// A surface a fence carries: a console window or a note card. A card on top
// floats elsewhere, and its place is the `_noteShadow`.
type Carried = {
  el: HTMLElement & { _noteShadow?: HTMLElement | null };
  id: string;
  rect: Rect;
} & ({ kind: "window"; el: ConsoleWin } | { kind: "note"; el: NoteCard });

// What the fence gestures read from the console, and nothing else.
export type FenceGestureDeps = {
  // The console's page: a gesture ends on a `blur` of it, and a drop writes the
  // cards through its `WBNotes`.
  window: Window;
  // The page the gestures listen on.
  document: Document;
  // The elements under a gesture; shared with the window chrome.
  gestures: Gestures;
  // The fence records; the console reassigns the list, so it is read when a
  // gesture needs it.
  fences: () => DeskFence[];
  // Whether a fence refuses a drag.
  fenceLocked: (id: string) => boolean;
  // Clears the refusal flash of a fence.
  clearFenceFlash: () => void;
  // The render of the fences.
  renderFences: () => void;
  // The plane; null before the page has it.
  stage: () => HTMLElement | null;
  // The console windows a fence may hold: the stage's and the console tabs'.
  fenceWindows: (st: HTMLElement) => ConsoleWin[];
  // Grows or fits the stage to the windows on it.
  applyExtent: (opts?: ExtentOpts) => void;
  // The auto-pan loop of a drag at a viewport edge.
  autoPan: (
    node: HTMLElement,
    place: (pointer: Point) => void,
  ) => { follow: (pointer: Point) => void; stop: () => void };
  // Writes the fence records to the desk.
  saveFences: (next: DeskFence[]) => void;
  // An element's rect read from the DOM.
  restoreRect: (el: HTMLElement) => Rect;
  // Writes a window's fields to the desk.
  setWin: (win: ConsoleWin, fields: DeskWindowFields) => void;
};

export function createFenceGestures(deps: FenceGestureDeps) {
  const {
    window,
    document,
    gestures,
    fences,
    fenceLocked,
    clearFenceFlash,
    renderFences,
    stage,
    fenceWindows,
    applyExtent,
    autoPan,
    saveFences,
    restoreRect,
    setWin,
  } = deps;

  // ---- the fence gestures (issue #341) -----------------------------------------
  // Both gestures read the fence's rect from the DOM, never from the captured
  // `f`: `f.rect` goes stale the first time the fence moves. Only `f.id` is
  // taken from the closure.
  //
  // INVARIANT, on EVERY exit path (mouseup, the `ev.buttons === 0` recovery,
  // `window` blur): the document listeners are removed and the gesture is
  // finalized EXACTLY ONCE, accepted or refused. `done` makes a doubled exit
  // a no-op.

  function startFenceMove(el: HTMLElement, f: DeskFence) {
    return (e: PointerEvent) => {
      if (e.button !== 0 || !e.isPrimary) return; // primary button only — see makeDraggable
      if (fenceLocked(f.id)) return;
      const st = stage();
      if (!st) return;
      const pointerId = e.pointerId;
      const start = restoreRect(el);
      // Membership is computed ONCE, at mousedown, and frozen for the gesture:
      // recomputing per move makes windows join and leave under the cursor as
      // the fence sweeps the plane, and the drop would carry a set nobody chose.
      // Windows AND cards: a fence carries every surface whose centre it holds
      // (ADR-0064 §8). The ids are NAMESPACED because the two collections are
      // keyed independently and `fenceMembership` sees one flat list.
      // A console in its own tab is carried too: its shadow is its place.
      const all = fenceWindows(st)
        .map((w): Carried => ({ el: w, id: "w:" + w._deskId, kind: "window", rect: restoreRect(w) }))
        .concat(
          [...st.querySelectorAll<NoteCard>(".note-card")].map((el): Carried => ({
            el,
            id: "n:" + el.dataset.noteId,
            kind: "note",
            rect: restoreRect(el),
          })),
        );
      // The FULL fence list, not a singleton: the fold's `break` decides an
      // overlapping pair (reachable via a hand-edited `desk.toml`), and a
      // singleton bypasses it.
      const live = fences().map((x) => (x.id === f.id ? { id: x.id, rect: start } : x));
      const ids = new Set(fenceMembership(live, all)[f.id] || []);
      const carried = all.filter((m) => ids.has(m.id));
      gestures.begin(el);
      for (const m of carried) gestures.begin(m.el);
      const startX = e.clientX;
      const startY = e.clientY;
      // The plane's origin AT MOUSEDOWN: auto-pan scrolls the viewport
      // mid-gesture and slides the stage under a stationary cursor, so every
      // delta is taken against the LIVE origin (as `makeDraggable`'s `place`).
      const origin0 = st.getBoundingClientRect();
      let delta = { dx: 0, dy: 0 };
      let fits = true;
      let done = false;
      // A press with no movement is a CLICK, not a drop: it changes nothing,
      // so it writes nothing. `armed` is the same rule with a width
      // (`dragThreshold`).
      let moved = false;
      const threshold = dragThreshold(e.pointerType);
      let armed = false;
      clearFenceFlash();
      const place = (pointer: Point) => {
        const origin = st.getBoundingClientRect();
        const d = fenceMoveDelta(
          {
            dx: pointer.x - startX + (origin0.left - origin.left),
            dy: pointer.y - startY + (origin0.top - origin.top),
          },
          start,
          carried.map((m) => m.rect),
        );
        const rect = { ...start, left: start.left + d.dx, top: start.top + d.dy };
        fits = fenceFits(fences(), { id: f.id, rect });
        el.classList.toggle("fence-invalid", !fits);
        el.style.left = rect.left + "px";
        el.style.top = rect.top + "px";
        // A refused position previews the FENCE (that is the feedback) but never
        // the members: dragging over a neighbour must not shuffle its windows.
        if (d.dx || d.dy) moved = true;
        if (fits) {
          delta = d;
          for (const m of carried) {
            m.el.style.left = m.rect.left + d.dx + "px";
            m.el.style.top = m.rect.top + d.dy + "px";
            // A card on top floats elsewhere; its place is the shadow.
            if (m.el._noteShadow) {
              m.el._noteShadow.style.left = m.el.style.left;
              m.el._noteShadow.style.top = m.el.style.top;
            }
          }
        }
        applyExtent({ grow: true });
      };
      // Holding the fence against a viewport edge scrolls the plane under it. A
      // fence re-rendered or removed mid-drag ends the loop.
      const pan = autoPan(el, place);
      const onMove = (ev: PointerEvent) => {
        if (ev.pointerId !== pointerId) return; // a second finger is not this gesture
        if (ev.buttons === 0) {
          onUp();
          return;
        }
        const last = { x: ev.clientX, y: ev.clientY };
        if (!armed) {
          if (!dragBegins({ x: startX, y: startY }, last, threshold)) return;
          armed = true;
        }
        place(last);
        pan.follow(last);
      };
      const onUp = () => {
        if (done) return;
        done = true;
        pan.stop();
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        // A touch gesture the system takes over ends in `pointercancel`.
        document.removeEventListener("pointercancel", onUp);
        window.removeEventListener("blur", onUp);
        el.classList.remove("fence-invalid");
        gestures.end(el);
        for (const m of carried) gestures.end(m.el);
        // Refuse, do NOT snap: the fence and everything it carries go back to
        // where the gesture began and nothing is persisted.
        if (!fits || !moved) {
          el.style.left = start.left + "px";
          el.style.top = start.top + "px";
          for (const m of carried) {
            m.el.style.left = m.rect.left + "px";
            m.el.style.top = m.rect.top + "px";
            if (m.el._noteShadow) {
              m.el._noteShadow.style.left = m.el.style.left;
              m.el._noteShadow.style.top = m.el.style.top;
            }
          }
          applyExtent();
          return;
        }
        saveFences(
          fences().map((x) =>
            x.id === f.id
              ? { ...x, rect: { ...start, left: start.left + delta.dx, top: start.top + delta.dy } }
              : x,
          ),
        );
        renderFences();
        // Each member is written EXACTLY ONCE, here, at its computed place —
        // a write per mousemove would send N changes per frame for a gesture
        // with one outcome. The cards go in ONE call for the same reason.
        for (const m of carried) {
          if (m.kind === "note") continue;
          setWin(m.el, { rect: { ...m.rect, left: m.rect.left + delta.dx, top: m.rect.top + delta.dy } });
        }
        const cards = carried.filter((m) => m.kind === "note").map((m) => m.el);
        if (cards.length) window.WBNotes?.persistCards(cards);
        applyExtent();
      };
      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("pointercancel", onUp);
      window.addEventListener("blur", onUp);
      e.preventDefault();
      e.stopPropagation();
    };
  }

  // Resize moves the FENCE only — never a member: a window whose centre falls
  // outside the new rect stops being reported by `fenceMembership`. That holds
  // for WEST and NORTH too — the opposite edge is anchored, so it is a resize,
  // not the §6 move that carries members.
  function startFenceResize(el: HTMLElement, f: DeskFence, dir: string) {
    const way = DIRS.includes(dir) ? dir : "se";
    return (e: PointerEvent) => {
      if (e.button !== 0 || !e.isPrimary) return;
      if (fenceLocked(f.id)) return;
      const st = stage();
      if (!st) return;
      const pointerId = e.pointerId;
      const start = restoreRect(el);
      // Captured ONCE (see `startResize`): a live re-read feeds the extent this
      // gesture grows back in as its own bound.
      const bounds = { width: st.offsetWidth, height: st.offsetHeight };
      const startX = e.clientX;
      const startY = e.clientY;
      let out = start;
      let fits = true;
      let done = false;
      let sized = false; // a click is not a resize — see `moved` in startFenceMove
      const threshold = dragThreshold(e.pointerType);
      let armed = false;
      clearFenceFlash();
      gestures.begin(el);
      const onMove = (ev: PointerEvent) => {
        if (ev.pointerId !== pointerId) return; // a second finger is not this gesture
        if (ev.buttons === 0) {
          onUp();
          return;
        }
        if (!armed) {
          if (!dragBegins({ x: startX, y: startY }, { x: ev.clientX, y: ev.clientY }, threshold))
            return;
          armed = true;
        }
        const next = resizeRect(
          way,
          start,
          { dx: ev.clientX - startX, dy: ev.clientY - startY },
          FENCE_MIN,
          bounds,
        );
        fits = fenceFits(fences(), { id: f.id, rect: next });
        if (next.width !== start.width || next.height !== start.height) sized = true;
        if (fits) out = next;
        el.classList.toggle("fence-invalid", !fits);
        el.style.left = next.left + "px";
        el.style.top = next.top + "px";
        el.style.width = next.width + "px";
        el.style.height = next.height + "px";
        applyExtent({ grow: true });
      };
      const onUp = () => {
        if (done) return;
        done = true;
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        // A touch gesture the system takes over ends in `pointercancel`.
        document.removeEventListener("pointercancel", onUp);
        window.removeEventListener("blur", onUp);
        el.classList.remove("fence-invalid");
        gestures.end(el);
        const rect = fits && sized ? out : start;
        el.style.left = rect.left + "px";
        el.style.top = rect.top + "px";
        el.style.width = rect.width + "px";
        el.style.height = rect.height + "px";
        if (!fits || !sized) {
          applyExtent();
          return;
        }
        saveFences(fences().map((x) => (x.id === f.id ? { ...x, rect } : x)));
        renderFences();
        applyExtent();
      };
      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("pointercancel", onUp);
      window.addEventListener("blur", onUp);
      e.preventDefault();
      e.stopPropagation();
    };
  }

  return { startFenceMove, startFenceResize };
}
