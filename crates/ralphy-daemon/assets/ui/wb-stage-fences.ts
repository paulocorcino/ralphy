/* ---------------------------------------------------------------------------
   The console's fences: the fence element and its tools, tiling a fence's
   consoles, and detaching a fence into its own window (ADR-0075 phase 5). The
   move and resize gestures are `wb-stage-fence-gestures.ts`.

   `createFences(deps)` returns `buildFence`, `startFenceMove`,
   `startFenceResize`, `arrangeFence` and `detachFence` for one console
   (ADR-0075 D7). They read the console only through `deps`, and `FenceDeps`
   lists every read, so `tsc` refuses a read outside it. `wb-console.ts`
   creates one per console and keeps the desk and the lifecycle channel; the
   fence list is `wb-stage-fence-list.ts`. A gesture begins and ends through the gestures owner,
   and a detach changes the popup registry only through its calls.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import * as WBDeskFolds from "./wb-desk-folds.ts";
import { DIRS } from "./wb-stage-chrome.ts";
import { sendDocument } from "./wb-events.ts";
import { createFenceGestures } from "./wb-stage-fence-gestures.ts";
import type { Gestures } from "./wb-stage-chrome.ts";
import type { PopupEntry, PopupMember, PopupRegistry } from "./wb-desk-popups.ts";
import type { FenceList } from "./wb-stage-fence-list.ts";
import type { OpenerLink } from "./wb-desk-detach.ts";
import type { ConfirmOptions } from "./wb-console-title.ts";
import type { DetachReason } from "./wb-console-session.ts";
import type { ConsoleWin, DeskFence, DeskWindowFields, ExtentOpts, Point, Rect } from "./wb-types.d.ts";

const { fenceMembership, tileIntoRect, WIN_MIN_W, WIN_MIN_H } = WBGeometry;
const { detachFold, DETACH_MAX } = WBDeskFolds;

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
    fenceWindows,
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

  const { startFenceMove, startFenceResize } = createFenceGestures({
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
  });

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
      // A console in its own tab stays there: it is in one place at a time.
      const items = all.filter((w) => ids.has(w.id) && !w.inTab).map((w) => ({ id: w.id, rect: w.rect }));
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

  function detachFence(id: string) {
    const out = detachFold(popups.detachedIds(), { type: "detach", fenceId: id });
    if (detachEnded(id, out.effects)) return;
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
    const leaving = fenceMemberIds(st, id);
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
    // BEFORE the teardown: a throw there would leave the entry with no watcher.
    armPopupWatch(id, entry);
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

  // A detach the fold ends here: the fence is already in a popup (raise it),
  // or the cap refuses one more. Neither commits the registry.
  function detachEnded(id: string, effects: ReturnType<typeof detachFold>["effects"]) {
    for (const effect of effects) {
      if (effect.type === "focus") {
        focusDetached(id);
        return true;
      }
      if (effect.type === "refuse") {
        fenceNotice(id, `Maximum of ${DETACH_MAX} detached fences`);
        WB.emit("fence-detach-refused", { fence: id, reason: effect.reason });
        return true;
      }
    }
    return false;
  }

  // Raising a popup that already holds this fence — the ONLY way to raise
  // one. The handle is the direct route; after a reload it died with the
  // document and the channel is the only one left.
  function focusDetached(id: string) {
    const live = popups.entry(id);
    if (live?.handle && !live.handle.closed) live.handle.focus();
    else link.post({ type: "origin-focus", tab: link.tab, fenceId: id, pid: live?.pid ?? undefined });
    WB.emit("fence-focus", { fence: id });
  }

  // The ids of the windows the stage holds in fence `id`.
  function fenceMemberIds(st: HTMLElement | null, id: string) {
    return st ? fenceMembership(readFenceRects(st), readWindowRects(st))[id] || [] : [];
  }

  // A force-closed popup fires no `beforeunload`, so the opener watches for it.
  // The fold is idempotent on a re-attach, so the doubled signal costs nothing.
  function armPopupWatch(id: string, entry: PopupEntry & { handle: Window }) {
    entry.poll = setInterval(() => {
      if (entry.handle.closed) reattachFence(id);
    }, 500);
    // A page that never completes the handshake (load failure, navigation, an
    // auth interstitial) is NOT closed, so the poll never fires. Bring the
    // consoles home instead.
    entry.rescue = setTimeout(() => {
      if (!entry.greeted && popups.entry(id) === entry) reattachFence(id);
    }, 5000);
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
    const headH = fenceHeadHeight(el);
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
    raiseFullBleeds();
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

  // A maximized console must not be BURIED by the tiles: `maxlock` leaves no
  // way to scroll away from a full bleed whose titlebar is covered.
  function raiseFullBleeds() {
    for (const win of wins) {
      if (win.classList.contains("maximized") || win.classList.contains("column")) focusWin(win);
    }
  }

  function fenceHeadHeight(el: HTMLElement) {
    return el.querySelector<HTMLElement>(".fence-head")?.offsetHeight || 28;
  }

  return { buildFence, startFenceMove, startFenceResize, arrangeFence, detachFence };
}
