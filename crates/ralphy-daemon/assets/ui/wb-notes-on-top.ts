// The note card on top: one card floats in front of the windows, in this tab
// only (ADR-0064, 2026-09-26 amendment). `createNotes` builds it with the
// card's closure names as typed deps; the text of each function is the one it
// had in `wb-notes.ts`.
import { WBGeometry } from "./wb-geometry.ts";
import { NOTE_DEFAULT, NOTE_MIN, onTopClamp, onTopRect, titleOf } from "./wb-notes-folds.ts";
import type { FloatBox } from "./wb-notes-folds.ts";
import { dragThreshold, dragBegins } from "./wb-console-input.ts";
import type { Stack } from "./wb-stage-stack.ts";
import type { CardHost, DeskFence, NoteCard, NoteSource } from "./wb-types.d.ts";

/** What `createNotes` hands the card on top: the card's closure names. */
export type NoteOnTopDeps = {
  /** The page's window: its resize event and `ResizeObserver`. */
  window: Window & { ResizeObserver?: typeof ResizeObserver };
  /** The page's document. */
  document: Document;
  /** The page's consoles; null where a test has none. */
  consoleHost: CardHost | null;
  /** The document's z stack; null where a test has none. */
  stack: Stack | null;
  /** The stage element, or null before the page has one. */
  stage: () => HTMLElement | null;
  /** The card of a note, by id. */
  cardEl: (id: string | null | undefined) => NoteCard | null;
  /** The desk record of a card, by id. */
  recordOf: (id: string | null | undefined) => NoteSource | null;
  /** True for a card whose fence is detached, so it is in the popup. */
  isAway: (record: Pick<NoteSource, "rect"> | null | undefined, fences: DeskFence[] | null | undefined) => boolean;
  /** Give a sleeping card its editor back. */
  wakeCard: (el: NoteCard) => void;
  /** True in a detached fence's popup, where the document holds a fragment of the plane. */
  fragment: () => boolean;
};

export function createNoteOnTop(deps: NoteOnTopDeps) {
  const { window, document, consoleHost, stack, stage, cardEl, recordOf, isAway, wakeCard } = deps;
  const fragment = deps.fragment;

  // The one card on top in THIS tab, or null. Memory only: the desk, the
  // per-client view and a reload never see it, so a reload finds the card in
  // its place.
  let onTopId: string | null = null;

  function onTop(el: NoteCard) {
    return el.classList.contains("on-top");
  }

  function viewportSize() {
    const ws = document.getElementById("workspace");
    return { width: ws?.clientWidth || 0, height: ws?.clientHeight || 0 };
  }

  // The floating box lives in CSS variables and NEVER in the inline rect: the
  // inline rect stays the desk rect, as a maximized window's does, so a fence
  // move, membership and `persistCards` keep reading the card's place.
  // `--ws-*` is the viewport's box on the screen: the card is `position:
  // fixed` (no ancestor of the stage has a transform, a filter or `contain`),
  // so a scroll of the stage never has to move it and it does not shake
  // during a pan.
  function placeOnTop(el: NoteCard, box: FloatBox) {
    const ws = document.getElementById("workspace");
    if (ws) {
      const r = ws.getBoundingClientRect();
      el.style.setProperty("--ws-x", r.left + ws.clientLeft + "px");
      el.style.setProperty("--ws-y", r.top + ws.clientTop + "px");
      el.style.setProperty("--ws-w", ws.clientWidth + "px");
      el.style.setProperty("--ws-h", ws.clientHeight + "px");
    }
    el.classList.toggle("band", !!box.band);
    if (box.band) {
      el._noteOnTop = null;
      return;
    }
    el._noteOnTop = { left: box.left, top: box.top, width: box.width, height: box.height };
    el.style.setProperty("--ot-x", box.left + "px");
    el.style.setProperty("--ot-y", box.top + "px");
    el.style.setProperty("--ot-w", box.width + "px");
    el.style.setProperty("--ot-h", box.height + "px");
  }

  // The card's place while it floats: the desk rect, the tone and the title,
  // and no editor. A click on it puts the card back.
  function paintShadow(el: NoteCard) {
    let sh = el._noteShadow;
    if (!sh) {
      sh = document.createElement("div");
      sh.className = "note-shadow";
      sh.title = "Kept on top. Click to put back.";
      const name = document.createElement("span");
      name.className = "note-shadow-title";
      sh.append(name);
      sh.addEventListener("click", () => putBack());
      stage()?.append(sh);
      el._noteShadow = sh;
    }
    for (const side of ["left", "top", "width", "height"] as const) sh.style[side] = el.style[side];
    sh.style.setProperty("--note-tone", getComputedStyle(el).getPropertyValue("--note-tone"));
    sh.firstChild!.textContent = titleOf(el._noteMarkdown, "Untitled note");
  }

  // Float the card in front of the windows (decisions 1–4). One card at a
  // time: another card on top goes back first. Refused for a card that is in
  // a detached fence's popup (`isAway`). Inside the popup itself the card is
  // on top of that window (ADR-0064 §7, amended 2026-10-05); its record may be
  // the orphan `mountDetached` kept, when this window's desk does not hold it.
  function keepOnTop(id: string | null | undefined) {
    const el = cardEl(id);
    const record = recordOf(id) || (fragment() ? el?._noteOrphan : null);
    if (!record || !el) return false;
    if (isAway(record, consoleHost?.fenceRecords?.() || [])) return false;
    if (onTopId === id) return true;
    putBack();
    wakeCard(el);
    onTopId = id!;
    el.classList.add("on-top");
    placeOnTop(el, onTopRect(record.rect || NOTE_DEFAULT, viewportSize()));
    paintShadow(el);
    watchViewport();
    stack?.focusWin(el);
    return true;
  }

  // Back to the desk rect, which the inline style never stopped holding.
  function putBack() {
    const id = onTopId;
    onTopId = null;
    const el = id ? cardEl(id) : null;
    if (!el) return;
    el.classList.remove("on-top", "band");
    for (const v of ["--ot-x", "--ot-y", "--ot-w", "--ot-h", "--ws-x", "--ws-y", "--ws-w", "--ws-h"]) {
      el.style.removeProperty(v);
    }
    el._noteOnTop = null;
    el._noteShadow?.remove();
    el._noteShadow = null;
    viewportWatch?.disconnect();
    viewportWatch = null;
    // Back on the plane the card keeps the z it was last focused with, which
    // is above a maximized console that covered its place before it floated.
    consoleHost?.raiseMaximized?.();
  }

  function onTopNow() {
    return onTopId;
  }

  // A change of the viewport's size — a window resize, a side panel that
  // opens, the tab coming back — keeps the floating card inside the view and
  // moves it in and out of the band. Observed only while a card is on top.
  let viewportWatch: ResizeObserver | null = null;
  function watchViewport() {
    if (viewportWatch || typeof window.ResizeObserver !== "function") return;
    const ws = document.getElementById("workspace");
    if (!ws) return;
    viewportWatch = new ResizeObserver(refitOnTop);
    viewportWatch.observe(ws);
  }
  function refitOnTop() {
    const el = onTopId ? cardEl(onTopId) : null;
    if (!el) return;
    const vp = viewportSize();
    // A hidden Consoles tab measures 0×0, which would read as a phone.
    if (!vp.width || !vp.height) return;
    const record = recordOf(onTopId);
    placeOnTop(
      el,
      el._noteOnTop ? onTopClamp(el._noteOnTop, vp) : onTopRect(record?.rect || NOTE_DEFAULT, vp),
    );
  }
  // The observer sees the viewport's SIZE; a window resize can also move it.
  window.addEventListener?.("resize", refitOnTop);

  // Drag (`dir` null) or resize the floating box. The plane's own gestures
  // cannot do this: they write the inline rect and persist it. Nothing here is
  // persisted — the floating box is thrown away when the card goes back.
  function floatGesture(el: NoteCard, dir: string | null) {
    return (e: PointerEvent) => {
      if (!onTop(el) || el.classList.contains("band") || !el._noteOnTop) return;
      if (e.button !== 0 || !e.isPrimary) return;
      if (!dir && (e.target as Element).closest("button, input")) return;
      const start = { ...el._noteOnTop };
      const vp = viewportSize();
      const from = { x: e.clientX, y: e.clientY };
      const pointerId = e.pointerId;
      const threshold = dragThreshold(e.pointerType);
      let armed = false;
      const onMove = (ev: PointerEvent) => {
        if (ev.pointerId !== pointerId) return;
        if (ev.buttons === 0) {
          onUp();
          return;
        }
        const at = { x: ev.clientX, y: ev.clientY };
        if (!armed) {
          if (!dragBegins(from, at, threshold)) return;
          armed = true;
        }
        const delta = { dx: at.x - from.x, dy: at.y - from.y };
        const box = dir
          ? WBGeometry.resizeRect(dir, start, delta, NOTE_MIN, vp)
          : onTopClamp({ ...start, left: start.left + delta.dx, top: start.top + delta.dy }, vp);
        placeOnTop(el, box);
      };
      const onUp = () => {
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        document.removeEventListener("pointercancel", onUp);
        window.removeEventListener("blur", onUp);
      };
      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("pointercancel", onUp);
      window.addEventListener("blur", onUp);
      e.preventDefault();
      if (dir) e.stopPropagation();
    };
  }

  return { onTop, keepOnTop, putBack, onTopNow, paintShadow, floatGesture };
}
