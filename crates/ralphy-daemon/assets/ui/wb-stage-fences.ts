/* ---------------------------------------------------------------------------
   The console's fences: the fence element and its tools, the move and the
   resize gestures, tiling a fence's consoles, and detaching a fence into its
   own window (ADR-0075 phase 5).

   `createFences(deps)` returns `buildFence`, `startFenceMove`,
   `startFenceResize`, `arrangeFence` and `detachFence` for one console
   (ADR-0075 D7). They read the console only through `deps`, and `FenceDeps`
   lists every read, so `tsc` refuses a read outside it. `wb-console.ts`
   creates one per console and keeps the desk and the lifecycle channel; the
   fence list is `wb-stage-fence-list.ts`. A gesture begins and ends through the gestures owner,
   and a detach changes the popup registry only through its calls.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import * as WBConsoleInput from "./wb-console-input.ts";
import * as WBDeskFolds from "./wb-desk-folds.ts";
import { DIRS } from "./wb-stage-chrome.ts";
import { sendDocument } from "./wb-events.ts";
import type { Gestures } from "./wb-stage-chrome.ts";
import type { PopupEntry, PopupMember, PopupRegistry } from "./wb-desk-popups.ts";
import type { FenceList } from "./wb-stage-fence-list.ts";
import type { OpenerLink } from "./wb-desk-detach.ts";
import type { ConfirmOptions } from "./wb-console-title.ts";
import type { DetachReason } from "./wb-console-session.ts";
import type { ConsoleWin, DeskFence, DeskWindowFields, ExtentOpts, NoteCard, Point, Rect } from "./wb-types.d.ts";

const { dragThreshold, dragBegins } = WBConsoleInput;
const { FENCE_MIN, fenceMembership, fenceFits, fenceMoveDelta, tileIntoRect, resizeRect, WIN_MIN_W, WIN_MIN_H } =
  WBGeometry;
const { detachFold, DETACH_MAX } = WBDeskFolds;

// A surface a fence carries: a console window or a note card. A card on top
// floats elsewhere, and its place is the `_noteShadow`.
type Carried = {
  el: HTMLElement & { _noteShadow?: HTMLElement | null };
  id: string;
  rect: Rect;
} & ({ kind: "window"; el: ConsoleWin } | { kind: "note"; el: NoteCard });

// What the fences read from the console, and nothing else.
export type FenceDeps = {
  // The console's page: `detachFence` opens the popup from it, and a gesture
  // ends on a `blur` of it.
  window: Window;
  // The page the fences build their elements in and listen on.
  document: Document;
  // The elements under a gesture; shared with the window chrome.
  gestures: Gestures;
  // The fences this tab detached and their popup entries.
  popups: PopupRegistry;
  // The lifecycle channel to the popups: raises a popup this tab no longer
  // holds a handle to.
  link: OpenerLink;
  // The console windows on the stage.
  wins: Set<ConsoleWin>;
  // The fence records; the console reassigns the list, so it is read when a
  // gesture or a tiling needs it.
  fences: () => DeskFence[];
  // The fence list: a fence's element, lock and refusal, the refusal flash,
  // the detach glyph, the fences and windows as the stage has them now, the
  // fence chrome, the fence verbs that change the desk, and the render of
  // the fences and the cards.
  fenceFloor: FenceList;
  // The plane; null before the page has it.
  stage: () => HTMLElement | null;
  // Grows or fits the stage to the windows on it.
  applyExtent: (opts?: ExtentOpts) => void;
  // The console's own question dialog.
  askConfirm: (opts: ConfirmOptions) => Promise<unknown>;
  // The auto-pan loop of a drag at a viewport edge.
  autoPan: (
    node: HTMLElement,
    place: (pointer: Point) => void,
  ) => { follow: (pointer: Point) => void; stop: () => void };
  // What a popup is handed: the fence's members, measured on this stage.
  fenceSnapshot: (id: string) => PopupMember[];
  // Raises a window.
  focusWin: (win: HTMLElement) => void;
  // The detach glyph's click.
  glyphClick: (id: string) => void;
  // A new popup id.
  newPid: () => string;
  // Brings a detached fence's consoles home.
  reattachFence: (id: string, opts?: { force?: boolean }) => void;
  // Writes the fence records to the desk.
  saveFences: (next: DeskFence[]) => void;
  // An element's rect read from the DOM.
  restoreRect: (el: HTMLElement) => Rect;
  // Writes a window's fields to the desk.
  setWin: (win: ConsoleWin, fields: DeskWindowFields) => void;
  // Takes a member off the stage without closing its session.
  tearDownMember: (win: ConsoleWin, reason: DetachReason) => void;
};

export function createFences(deps: FenceDeps) {
  const {
    window,
    document,
    gestures,
    popups,
    link,
    wins,
    fences,
    fenceFloor,
    stage,
    applyExtent,
    askConfirm,
    autoPan,
    fenceSnapshot,
    focusWin,
    glyphClick,
    newPid,
    reattachFence,
    saveFences,
    restoreRect,
    setWin,
    tearDownMember,
  } = deps;
  const {
    clearFenceFlash,
    fenceEl,
    fenceLocked,
    fenceNotice,
    showDetachGlyph,
    readFenceRects,
    readWindowRects,
    refreshFenceChrome,
    removeFence,
    renameFence,
    setFenceLock,
    renderFences,
    renderNotes,
  } = fenceFloor;

  // Reuses `DIRS` so `resizeRect` answers all eight; its west/north legs clamp
  // at the pinned origin (no negative coordinate, ADR-0051 §2).
  const FENCE_DIRS = DIRS;

  // A fence is a stage child BELOW every window (`z-index: 1` against `Z_BASE`)
  // with `pointer-events: none`, so "never intercepts a window drag, resize or
  // focus click" holds by construction and `onFloorDown`'s `e.target !== st`
  // test keeps panning alive inside a fence. Only the name field, the two tool
  // buttons and the eight resize bands opt back in.
  function buildFence(f: DeskFence) {
    const el = document.createElement("div");
    el.className = "fence";
    el.dataset.fenceId = f.id;
    const head = document.createElement("div");
    head.className = "fence-head";
    // Two SMALL opt-in handles: a full-width interactive head would swallow the
    // floor's own pan (`onFloorDown` bails unless the press targets the stage).
    const grab = document.createElement("span");
    grab.className = "fence-grab";
    grab.title = "Move this fence";
    grab.textContent = "⠿";
    grab.addEventListener("pointerdown", startFenceMove(el, f));
    const name = document.createElement("input");
    name.className = "fence-name";
    name.setAttribute("aria-label", "Fence name");
    name.value = f.name || "";
    // READ-ONLY until double-clicked: the field lives in a bar the operator
    // also clicks to raise, focus and drag, and an always-live input turns a
    // slip into a rename. No `title`, no hover affordance (`.fence-name`): a
    // read-only field that lights up reads as editable.
    //
    // `pristine` is what a cancel returns to; `endEdit` is the ONE place an
    // edit ends. Committing on `change` (fires on blur) meant clicking away
    // SAVED with nothing to undo it. Now:
    //   Enter  → commit
    //   Escape → cancel, restoring the name
    //   a press anywhere else → cancel, restoring the name
    let pristine = name.value;
    let editing = false;
    // A document-level pointerdown, not just `blur`: the plane's pan handler
    // calls `preventDefault()` on mousedown, so pressing the stage does NOT move
    // focus (MEASURED). Capture phase, before the floor's handler swallows it.
    const stopOutside = (e: PointerEvent) => {
      if (e.target !== name) endEdit(false);
    };
    const endEdit = (commit: boolean) => {
      if (!editing) return;
      editing = false; // first: the `blur()` below re-enters through the handler
      document.removeEventListener("pointerdown", stopOutside, true);
      name.readOnly = true;
      if (!commit) name.value = pristine;
      // Collapse the selection `select()` made. MEASURED: neither `blur()` nor
      // re-assigning `.value` (same string = no-op) clears it, so after Escape
      // the name stayed highlighted on a field nobody was editing.
      name.setSelectionRange(0, 0);
      name.blur();
      // Last, because it re-renders the fence and replaces this very input.
      if (commit) renameFence(f.id, name.value);
    };
    name.readOnly = true;
    // A single click leaves NO trace: prevented, it neither focuses the field
    // nor starts a selection. `dblclick` still arrives (cancelling a mousedown
    // default does not cancel the click pair). Once editing, the guard steps
    // aside.
    name.addEventListener("mousedown", (e: MouseEvent) => {
      if (name.readOnly) e.preventDefault();
    });
    name.addEventListener("dblclick", () => {
      if (editing) return;
      pristine = name.value;
      editing = true;
      name.readOnly = false;
      name.focus();
      name.select();
      // Attached here, so the pointerdown that OPENED the edit is already past.
      document.addEventListener("pointerdown", stopOutside, true);
    });
    name.addEventListener("keydown", (e: KeyboardEvent) => {
      if (e.key !== "Enter" && e.key !== "Escape") return;
      // Held at the field so an Escape meant for this edit never also reaches the
      // plane's own key handlers.
      e.stopPropagation();
      endEdit(e.key === "Enter");
    });
    // A real focus loss (Tab, the window losing focus) cancels too — same rule, and
    // `endEdit` is idempotent, so the `blur()` inside it lands here harmlessly.
    name.addEventListener("blur", () => endEdit(false));
    // Name · count · arrange (#342). Filled by `refreshFenceChrome`. No repo
    // list in the chrome: a fence is not bound to a project (ADR-0051 §6);
    // `fenceSummaries` still folds it.
    const count = document.createElement("span");
    count.className = "fence-count";
    // A fence verb's refusal belongs on the fence (`WB.emit` only reaches the
    // console; no toast surface). Filled and cleared by `fenceNotice`.
    const notice = document.createElement("span");
    notice.className = "fence-notice";
    head.append(grab, name, count, notice);
    // Arrange and close take the fence's TOP-RIGHT corner, like every other
    // closable surface here; trailing the head made their position a function
    // of the name's length.
    const tools = document.createElement("div");
    tools.className = "fence-tools";
    const tile = document.createElement("button");
    tile.className = "fence-arrange";
    tile.type = "button";
    tile.title = "Tile this fence's consoles";
    tile.textContent = "⊞";
    tile.addEventListener("click", async () => {
      // A detached fence has nothing here to tile; `arrangeFence` says so, so
      // the question is not put to the operator.
      if (popups.isDetached(f.id)) return arrangeFence(f.id);
      const ok = await askConfirm({
        title: "Tile this fence?",
        message: `Rearranges the consoles in ${f.name || "this fence"}. Sessions keep running.`,
        confirmLabel: "Tile",
      });
      if (ok) arrangeFence(f.id);
    });
    const drop = document.createElement("button");
    drop.className = "fence-drop";
    drop.type = "button";
    drop.title = "Remove this fence";
    drop.textContent = "×";
    drop.addEventListener("click", async () => {
      // A DETACHED fence refuses removal and says why; no question first.
      if (popups.isDetached(f.id)) return removeFence(f.id);
      const ok = await askConfirm({
        title: "Remove this fence?",
        message: `Removes ${f.name || "this fence"}. Consoles stay where they are.`,
        confirmLabel: "Remove",
        danger: true,
      });
      if (ok) removeFence(f.id);
    });
    const detach = document.createElement("button");
    detach.className = "fence-detach";
    detach.type = "button";
    detach.title = "Detach this fence into its own window";
    detach.textContent = "⧉";
    detach.addEventListener("click", () => detachFence(f.id));
    // Lock in place: the fence and every console it holds refuse a drag. Glyph
    // painted by `paintFenceLock` from `renderFences`.
    const lock = document.createElement("button");
    lock.className = "fence-lock";
    lock.type = "button";
    lock.addEventListener("click", () => setFenceLock(f.id, !fenceLocked(f.id)));
    // Every member as columns, in a grid that follows the stage (ADR-0051 §5,
    // 2026-09-30). `wbColumns` (wb-consoles-tab.ts) owns the columns, so this
    // only names the members and their stage rects. `refreshFenceChrome`
    // paints disabled and hidden.
    const columns = document.createElement("button");
    columns.className = "fence-columns";
    columns.type = "button";
    columns.title = "Open this fence's consoles as columns";
    columns.innerHTML = '<i class="bi bi-layout-three-columns"></i>';
    columns.addEventListener("click", () => {
      const st = stage();
      if (!st || popups.isDetached(f.id)) return;
      const all = readWindowRects(st);
      const ids = new Set(fenceMembership(readFenceRects(st), all)[f.id] || []);
      const items = all.filter((w) => ids.has(w.id)).map((w) => ({ id: w.id, rect: w.rect }));
      if (!items.length) return;
      sendDocument(document, "workbench:fence-columns", { items });
    });
    // BETWEEN arrange and close: close stays the OUTERMOST control
    // (wb_fence_342.py asserts exactly that).
    tools.append(tile, columns, lock, detach, drop);
    // What tells an EMPTIED fence from an empty one (ADR-0051 §7a): a detached
    // fence keeps its name, rect and list entry and carries this glyph; clicking
    // it brings the consoles home. `hidden` here AND in the stylesheet: an
    // author `display` beats the UA's `[hidden]` rule.
    const away = document.createElement("div");
    away.className = "fence-detached";
    away.title = "Return this fence's consoles to this window";
    away.textContent = "⧉";
    away.hidden = true;
    away.addEventListener("click", () => glyphClick(f.id));
    // Every edge and corner resizes. Only the SE handle (`.fence-grip`) is
    // visible; the other seven announce themselves through the cursor.
    const handles = FENCE_DIRS.map((dir) => {
      const h = document.createElement("div");
      h.className = dir === "se" ? "fence-edge fence-grip" : "fence-edge";
      h.dataset.dir = dir;
      h.title = "Resize this fence";
      h.addEventListener("pointerdown", startFenceResize(el, f, dir));
      return h;
    });
    // ORDER IS THE HIT TEST: the bands overlap the head and the tools and all
    // take pointer events; later siblings win, so the interactive clusters go
    // last or the north band eats the name field and the close button.
    el.append(...handles, head, tools, away);
    stage()?.append(el);
    return el;
  }

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
      const all = [...st.querySelectorAll<ConsoleWin>(".session-window")]
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
    const way = FENCE_DIRS.includes(dir) ? dir : "se";
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

  function detachFence(id: string) {
    const out = detachFold(popups.detachedIds(), { type: "detach", fenceId: id });
    for (const effect of out.effects) {
      if (effect.type === "focus") {
        // Raising a popup that already holds this fence — the ONLY way to raise
        // one. The handle is the direct route; after a reload it died with the
        // document and the channel is the only one left.
        const live = popups.entry(id);
        if (live?.handle && !live.handle.closed) live.handle.focus();
        else link.post({ type: "origin-focus", tab: link.tab, fenceId: id, pid: live?.pid ?? undefined });
        WB.emit("fence-focus", { fence: id });
        return;
      }
      if (effect.type === "refuse") {
        fenceNotice(id, `Maximum of ${DETACH_MAX} detached fences`);
        WB.emit("fence-detach-refused", { fence: id, reason: effect.reason });
        return; // the registry is NOT committed
      }
    }
    if (!out.effects.some((e) => e.type === "open")) return;

    const st = stage();
    const fence = st ? readFenceRects(st).find((f) => f.id === id) : null;
    // INVARIANT: either the popup exists AND the members are torn down, or
    // neither. `window.open` therefore runs BEFORE a single window is touched —
    // a blocked popup must leave the fence exactly as it was.
    // `fence` is the daemon's route for the page (`Shell` in `assets.rs`).
    const handle = window.open("fence", "", "popup,width=900,height=700");
    if (!handle) {
      fenceNotice(id, "Could not detach: pop-up blocked");
      WB.emit("fence-detach-blocked", { fence: id });
      return; // the registry is NOT committed, nothing was torn down
    }
    // The fence's consoles leave the columns BEFORE the snapshot, so a leftmost
    // that left travels with `max: false` and no terminal shows in two places.
    // Synchronous: the popup document loads later.
    const leaving = st ? fenceMembership(readFenceRects(st), readWindowRects(st))[id] || [] : [];
    sendDocument(document, "workbench:columns-leave", { ids: leaving });
    const members = fenceSnapshot(id);

    const entry: PopupEntry & { handle: Window } = {
      handle,
      members,
      memberIds: members.map((m) => m.id).filter(Boolean),
      fence: fence || { id, name: "", rect: null },
      // This popup's identity, on every lifecycle message both ways (#476).
      pid: newPid(),
      poll: null,
      greeted: false,
      rescue: null,
      // Seeded NOW, not at the first `popup-beat`: the popup needs a page load
      // and a handshake before it can beat; `PEER_WINDOW_MS` is the grace.
      peer: { seen: Date.now(), lost: false },
    };
    // The entry lands BEFORE the commit, so `detachedMembers()` has the ids to
    // persist. Still nothing is torn down yet — the invariant above holds.
    popups.put(id, entry);
    popups.commitDetached(out.registry);
    // Armed BEFORE the teardown: a throw there would leave the entry with no
    // watcher, and a force-closed popup fires no `beforeunload`. The fold is
    // idempotent on a re-attach, so the doubled signal costs nothing.
    entry.poll = setInterval(() => {
      if (entry.handle.closed) reattachFence(id);
    }, 500);
    // A page that never completes the handshake (load failure, navigation, an
    // auth interstitial) is NOT closed, so the poll never fires. Bring the
    // consoles home instead.
    entry.rescue = setTimeout(() => {
      if (!entry.greeted && popups.entry(id) === entry) reattachFence(id);
    }, 5000);
    for (const m of members) {
      if (m.kind === "note") continue;
      const win = [...wins].find((w) => w._deskId === m.id);
      // The popup attaches each member again.
      if (win) tearDownMember(win, "reconnect");
    }
    // The cards leave the stage the same way: `renderNotes` drops every card
    // whose fence is now detached, and the records stay exactly where they are.
    renderNotes();
    showDetachGlyph(id, true);
    applyExtent();
    refreshFenceChrome();
    WB.emit("fence-detach", { fence: id });
  }

  // Tile ONE fence's members into its own rect (#342); windows animate via
  // the `.tiling` CSS transition. The grid is inset by the fence's OWN chrome:
  // the head band and the SE `.fence-grip` sit BELOW every window, so a member
  // parked on either makes the fence's controls unhittable.
  const FENCE_GRIP = 14;
  function arrangeFence(id: string) {
    // Detached: tiling the empty box would rewrite the rects the popup will
    // restore from (ADR-0051 §7a).
    if (popups.isDetached(id)) return;
    // A locked fence keeps its layout: tiling would rewrite every member's rect.
    if (fenceLocked(id)) return;
    const st = stage();
    const el = fenceEl(id);
    if (!st || !el) return;
    const rect = restoreRect(el);
    const all = [...st.querySelectorAll<ConsoleWin>(".session-window")].map((w) => ({
      el: w,
      id: w._deskId,
      rect: restoreRect(w),
    }));
    // The FULL fence list with this fence's LIVE rect: the fold's `break`
    // decides an overlapping pair, and a singleton bypasses it.
    const live = fences().map((x) => (x.id === id ? { id: x.id, rect } : x));
    const ids = new Set(fenceMembership(live, all)[id] || []);
    // A maximized console is NOT tiled: a tile rect written onto it is
    // invisible while it REPLACES the pre-maximize rect. Filtered before the
    // grid so it stays hole-free (#338). A LOCKED console is skipped too.
    const members = all
      .filter(
        (m) =>
          ids.has(m.id) &&
          !m.el.classList.contains("maximized") &&
          !m.el.classList.contains("column") &&
          !m.el._deskLocked,
      )
      .map((m) => m.el);
    // An empty fence is a NO-OP, not an error.
    if (!members.length) return;
    const headH = el.querySelector<HTMLElement>(".fence-head")?.offsetHeight || 28;
    const tiles = tileIntoRect(
      {
        left: rect.left,
        top: rect.top + headH,
        width: rect.width,
        height: Math.max(0, rect.height - headH - FENCE_GRIP),
      },
      members,
    );
    members.forEach((win, i) => {
      const t = tiles[i];
      // The computed tile, written now: a read after the transition would
      // race a hidden tab and a convergence.
      setWin(win, { rect: { left: t.left, top: t.top, width: t.width, height: t.height } });
      win.classList.add("tiling");
      win.style.left = t.left + "px";
      win.style.top = t.top + "px";
      win.style.width = t.width + "px";
      win.style.height = t.height + "px";
      // Relaxed to the cell for tiles below the floor, CLEARED otherwise.
      win.style.minWidth = t.width < WIN_MIN_W ? t.width + "px" : "";
      win.style.minHeight = t.height < WIN_MIN_H ? t.height + "px" : "";
      focusWin(win);
      setTimeout(() => win.classList.remove("tiling"), 260);
    });
    // A maximized console must not be BURIED by the tiles: `maxlock` leaves no
    // way to scroll away from a full bleed whose titlebar is covered.
    for (const win of wins) {
      if (win.classList.contains("maximized") || win.classList.contains("column")) focusWin(win);
    }
    // AFTER the 0.24s tiling transition: an immediate fold would measure the
    // pre-arrange boxes.
    setTimeout(() => {
      for (const win of members) {
        try {
          win._term?.fit.fit();
        } catch {}
      }
      refreshFenceChrome();
      applyExtent();
    }, 300);
  }

  return { buildFence, startFenceMove, startFenceResize, arrangeFence, detachFence };
}
