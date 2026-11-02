/* ---------------------------------------------------------------------------
   The workbench's plane geometry — every rect fold the desk performs, as pure
   functions of their arguments.

   Lifted out of `wb-console.ts` under ADR-0057. Nothing here reads the DOM, the
   socket, the desk store or a module-scope binding: given the same rects it
   returns the same rects, on any thread, in any order. That is the whole
   selection rule, and it is why this is the one seam `wb-console.ts` had — the
   rest of that file is a web of reads and writes across twenty-one module-scope
   `let`s (ADR-0022 §5: a file with no existing seam is a design problem, not a
   split).

   `wb-console.ts` re-exports every FUNCTION below — not the constants, which had
   no outside callers — so `WBConsole.tileIntoRect` and friends keep working for
   the callers and the tests that already use them
   (CLAUDE.md: the public surface is stable by default). New callers should
   prefer `WBGeometry`.

   The size folds (stage extent, new fence, tiling, resize) are in
   `wb-geometry-sizes.ts`; `WBGeometry` returns them with the rest.

   `wb-console.ts` and `wb-notes.ts` import this module.
   --------------------------------------------------------------------------- */
import type { Rect, Point, Offset, Size, Delta, DeskFence } from "./wb-types.d.ts";
import {
  STAGE_MARGIN,
  stageExtent,
  FENCE_SIZE,
  FENCE_MIN,
  FENCE_INSET,
  FENCE_GAP,
  FENCE_COLS,
  fenceSpawnRect,
  TILE_PAD,
  TILE_GAP,
  TILE_MIN,
  tileIntoRect,
  RESIZE_MIN,
  resizeRect,
} from "./wb-geometry-sizes.ts";

// A rect with an id: a fence or a window, as the membership folds read it.
type Placed = { id: string; rect: Rect };
// A client rect: `getBoundingClientRect()`.
type ClientBox = { left: number; top: number; right: number; bottom: number };

export const WBGeometry = (function () {
  // A point; a missing point or coordinate reads 0.
  function pointOf(point: Partial<Point> | null | undefined) {
    return { x: point?.x || 0, y: point?.y || 0 };
  }

  // The four edges of a rect; a missing rect or field reads 0.
  function edgesOf(r: Partial<Rect> | null | undefined) {
    const left = r?.left || 0;
    const top = r?.top || 0;
    return { left, top, right: left + (r?.width || 0), bottom: top + (r?.height || 0) };
  }

  function rectsOverlap(a: Partial<Rect>, b: Partial<Rect>) {
    const p = edgesOf(a);
    const q = edgesOf(b);
    return p.left < q.right && p.right > q.left && p.top < q.bottom && p.bottom > q.top;
  }

  // ---- a fence is a group (issue #341) -----------------------------------------
  // Membership is DERIVED, never stored: a window belongs to the fence that holds
  // the middle of its TITLE BAR (ADR-0051 amendment, #743), so nothing has to be
  // kept in step and a record can never disagree with the geometry. The operator
  // grabs a window by its title bar: one whose title bar is outside a fence is
  // free, whatever its body covers. Containment is HALF-OPEN (`left <= x < left +
  // width`) — fences may abut, and a closed test would put a point sitting on a
  // shared border inside both of them, breaking "exactly one fence".
  // Half of `--row-h` (33px, styles/01-base.css), the console title bar's
  // height; a note card's 22px head is inside it too.
  const TITLE_MID = 16;

  function titleBarPoint(rect: Rect | undefined) {
    return {
      x: (rect?.left || 0) + (rect?.width || 0) / 2,
      y: (rect?.top || 0) + Math.min(TITLE_MID, (rect?.height || 0) / 2),
    };
  }

  // Does `rect` hold `point`? The half-open test above, extracted once (issue
  // #343) so membership and the floor's focus-clearing hit test can never drift
  // into two spellings of the same containment.
  function rectHolds(rect: Partial<Rect> | null | undefined, point: Point) {
    const edge = edgesOf(rect);
    const at = pointOf(point);
    return at.x >= edge.left && at.x < edge.right && at.y >= edge.top && at.y < edge.bottom;
  }

  // `fences` is `[{ id, rect }]`, `windows` is `[{ id, rect }]`; the answer maps
  // EVERY fence id (an empty one to `[]`) and omits a window in no fence. The
  // `break` is the second half of "exactly one": half-open containment makes the
  // fences disjoint as point sets, and this makes the fold's own answer so even
  // if a stored rect pair ever overlaps.
  function fenceMembership(fences: readonly Placed[], windows: readonly Placed[]) {
    const list = fences || [];
    const out: Record<string, string[]> = {};
    for (const f of list) out[f.id] = [];
    for (const w of windows || []) {
      const c = titleBarPoint(w?.rect);
      for (const f of list) {
        if (rectHolds(f.rect, c)) {
          out[f.id]?.push(w.id);
          break;
        }
      }
    }
    return out;
  }

  // The fence that holds `rect` — the SAME half-open test and the SAME
  // first-match rule as `fenceMembership`, so the two can never disagree about
  // whose a window is. `null` when no fence holds its title bar point. This is what a
  // gesture consults to ask "is the fence under this window locked?". A missing
  // rect is read at the origin, as `titleBarPoint` reads it.
  function fenceOf(fences: readonly DeskFence[], rect: Rect | undefined) {
    const c = titleBarPoint(rect);
    for (const f of fences || []) {
      if (rectHolds(f.rect, c)) return f;
    }
    return null;
  }

  // May `candidate` (`{ id, rect }`) take the plane? Pure, and the SAME strict
  // `rectsOverlap` the spawn rule uses, so abutting fences stay buildable and
  // one predicate answers for create, move and resize alike. A candidate is
  // never compared with itself — a move must not refuse its own start rect.
  function fenceFits(fences: readonly Placed[], candidate: Placed) {
    const rect = candidate?.rect || {};
    return !(fences || []).some((f) => f.id !== candidate?.id && rectsOverlap(rect, f.rect || {}));
  }

  // The move delta a fence and the members it carries may actually take: the
  // request, clamped so no left/top lands below 0. The plane's origin is pinned
  // (issue #336) and grows right and down only, so a negative coordinate is not
  // a position — it is a lost window. The MEMBERS are in the fold too: one can
  // sit further left than the fence that carries it.
  function fenceMoveDelta(delta: Delta, fenceRect: Rect, memberRects: readonly Rect[]) {
    const members = memberRects || [];
    const minLeft = Math.min(fenceRect?.left || 0, ...members.map((r) => r?.left || 0));
    const minTop = Math.min(fenceRect?.top || 0, ...members.map((r) => r?.top || 0));
    return {
      dx: Math.max(delta?.dx || 0, -minLeft),
      dy: Math.max(delta?.dy || 0, -minTop),
    };
  }

  // ---- navigating the plane ----------------------------------------------------
  // The scroll offsets that bring `target` (a STAGE-relative rect) into the
  // viewport, pure: centre it, then clamp to `[0, extent - viewport]`. ALWAYS
  // centres. Callers: `reveal` (the Go-to picker, #337) and ADR-0051 §7's fence
  // jump.
  // ONE clamp per axis: the final `Math.max(0, …)` stops a viewport bigger than
  // the extent asking for a negative offset. Flooring the ceiling too would
  // make that floor unfalsifiable by the table's negative control.
  function clampOffset(offset: Offset, viewport: Size, extent: Size) {
    const v = sizeOrZero(viewport);
    const e = sizeOrZero(extent);
    return {
      left: Math.max(0, Math.min(offset?.left || 0, e.width - v.width)),
      top: Math.max(0, Math.min(offset?.top || 0, e.height - v.height)),
    };
  }

  // A size whose missing side reads 0.
  function sizeOrZero(size: Partial<Size> | null | undefined) {
    return { width: size?.width || 0, height: size?.height || 0 };
  }

  // A rect whose missing edge or side reads 0.
  function rectOrZero(rect: Partial<Rect> | null | undefined) {
    return { left: rect?.left || 0, top: rect?.top || 0, ...sizeOrZero(rect) };
  }

  function bringIntoView(target: Rect, viewport: Size, extent: Size) {
    const v = sizeOrZero(viewport);
    const t = rectOrZero(target);
    const left = t.left + t.width / 2 - v.width / 2;
    const top = t.top + t.height / 2 - v.height / 2;
    return clampOffset({ left, top }, viewport, extent);
  }

  // The other anchoring: the target's TOP-LEFT corner, one inset in from the
  // viewport's. A fence is a region the operator works inside, not a point of
  // interest — centring it wastes the screen above and left. `bringIntoView`
  // keeps CENTRING for the Go-to picker (#337 pins it).
  const VIEW_INSET = 24;

  function anchorIntoView(target: Offset, viewport: Size, extent: Size, inset?: number | null) {
    const pad = inset == null ? VIEW_INSET : inset;
    return clampOffset(
      { left: (target?.left || 0) - pad, top: (target?.top || 0) - pad },
      viewport,
      extent,
    );
  }

  // Ease-out cubic on a 0..1 clock: fast off the mark, settling into the target.
  function slideEase(t: number) {
    const x = Math.min(1, Math.max(0, t));
    return 1 - Math.pow(1 - x, 3);
  }

  // The bounding box of a set of stage-relative rects; all zeros for none, so an
  // empty desk centres on the pinned origin rather than on nothing.
  function bboxOf(rects: readonly Rect[]) {
    const list = rects || [];
    if (!list.length) return { left: 0, top: 0, width: 0, height: 0 };
    let left = Infinity;
    let top = Infinity;
    let right = -Infinity;
    let bottom = -Infinity;
    for (const r of list) {
      left = Math.min(left, r.left || 0);
      top = Math.min(top, r.top || 0);
      right = Math.max(right, (r.left || 0) + (r.width || 0));
      bottom = Math.max(bottom, (r.top || 0) + (r.height || 0));
    }
    return { left, top, width: right - left, height: bottom - top };
  }

  // Where the viewport lands on load (#339), pure. The stored per-client offset
  // wins only while it still SHOWS work (some window intersects the viewport
  // placed there); otherwise the bbox landing. The clamp comes BEFORE the test:
  // an offset saved on a bigger screen is a legitimate view pulled into this
  // extent.
  // `stored` is the offset this client kept in its own storage: each value is
  // checked before it is used.
  function viewLanding(
    stored: { left?: unknown; top?: unknown } | null | undefined,
    rects: readonly Rect[],
    viewport: Size,
    extent: Size,
  ) {
    const at = storedOffset(stored);
    if (at) {
      const clamped = clampOffset(at, viewport, extent);
      if (showsWork(rects, clamped, viewport)) return clamped;
    }
    return bringIntoView(bboxOf(rects), viewport, extent);
  }

  // The offset a client kept, or null when either value is not a finite number.
  function storedOffset(
    stored: { left?: unknown; top?: unknown } | null | undefined,
  ): Offset | null {
    const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : null);
    const left = num(stored?.left);
    const top = num(stored?.top);
    return left !== null && top !== null ? { left, top } : null;
  }

  // Whether some rect intersects the viewport placed at `at`.
  function showsWork(rects: readonly Rect[], at: Offset, viewport: Size) {
    const v = sizeOrZero(viewport);
    return (rects || []).some((rect) => {
      const r = rectOrZero(rect);
      return (
        r.left < at.left + v.width &&
        r.left + r.width > at.left &&
        r.top < at.top + v.height &&
        r.top + r.height > at.top
      );
    });
  }

  // How far the plane scrolls per frame while a window is dragged against the
  // viewport edge, and how wide the pressure band at each edge is.
  const PAN_BAND = 48;
  const PAN_STEP = 24;
  // A client rect's edges; a missing viewport or edge reads 0.
  function clientBox(viewport: Partial<ClientBox> | null | undefined) {
    return {
      left: viewport?.left || 0,
      top: viewport?.top || 0,
      right: viewport?.right || 0,
      bottom: viewport?.bottom || 0,
    };
  }
  // The auto-pan rule, pure. `viewport` is a CLIENT rect; `pointer` is a client
  // point. Each edge contributes a pressure in `[0, band]` and the axis takes
  // their DIFFERENCE — deliberately, so a viewport narrower than two bands
  // cancels instead of oscillating between its own two edges.
  function panNudge(
    pointer: Point,
    viewport: ClientBox,
    band?: number | null,
    step?: number | null,
  ) {
    const b = band == null ? PAN_BAND : band;
    const s = step == null ? PAN_STEP : step;
    const press = (v: number) => Math.max(0, Math.min(v, b));
    const axis = (near: number, far: number) => Math.round((s * (far - near)) / b);
    const at = pointOf(pointer);
    const box = clientBox(viewport);
    return {
      dx: axis(press(b - (at.x - box.left)), press(b - (box.right - at.x))),
      dy: axis(press(b - (at.y - box.top)), press(b - (box.bottom - at.y))),
    };
  }

  // Is this console held by a LOCKED fence? Never in the popup (`autoBoot:
  // false`): `mountDetached` re-origins the members' rects into that window
  // while `fences` keeps the shell's stage coordinates, so the fold would
  // match a console to whatever fence covers the translated point. A detached
  // fence's members move freely there; the fence's lock holds again on the
  // stage when they come home. Note cards follow the same rule.
  function fenceHolds(records: readonly DeskFence[], rect: Rect, popup: boolean) {
    return !popup && !!fenceOf(records, rect)?.locked;
  }
  // `.session-window`'s CSS floor (`styles.css`, pinned by
  // `shell_arranges_into_the_fence`). It OUTRANKS an inline width (MEASURED: a
  // 176x116 cell rendered 240x150, 52 px past its fence).
  const WIN_MIN_W = 240;
  const WIN_MIN_H = 150;

  // Where a console born into a focused fence lands (#343), pure: fence rect,
  // cascade index and head-band height in, one box out. The box may shrink
  // BELOW the CSS floor for a small fence; the caller relaxes
  // `minWidth`/`minHeight` for exactly those axes. `roomX`/`roomY` cap the
  // cascade offset: a bare `k * step` walks out of a small fence.
  const SPAWN_PAD = 12;
  const SPAWN_STEP = 24;

  function spawnRectIn(fence: Partial<Rect> | null | undefined, index: number, headH: number) {
    const f = fence || {};
    const fl = f.left || 0;
    const ft = f.top || 0;
    const fw = f.width || 0;
    const fh = f.height || 0;
    const head = headH || 0;
    const width = Math.max(1, Math.min(560, fw - SPAWN_PAD * 2));
    const height = Math.max(1, Math.min(340, fh - head - SPAWN_PAD * 2));
    const k = (index || 0) % 8;
    const roomX = Math.max(0, fw - SPAWN_PAD * 2 - width);
    const roomY = Math.max(0, fh - head - SPAWN_PAD * 2 - height);
    // The outer `Math.min` is for the DEGENERATE fence narrower than the pad
    // pair: the pad alone would push the box past the far edge, and the
    // title-bar fold would report the newborn console in NO fence.
    const offX = Math.min(SPAWN_PAD + Math.min(k * SPAWN_STEP, roomX), Math.max(0, fw - width));
    const offY = Math.min(
      head + SPAWN_PAD + Math.min(k * SPAWN_STEP, roomY),
      Math.max(0, fh - height),
    );
    return { left: fl + offX, top: ft + offY, width, height };
  }

  // Where a console born OUTSIDE a fence lands, pure: viewport (offset and
  // size), cascade index and the fence records in, one box out. A console is
  // never born held by a LOCKED fence: it would wear that lock at once, and
  // the operator could not drag it out. The cascade steps past a slot whose
  // title bar a locked fence holds (the `fenceHolds` fold); when every slot is
  // held, the box moves right of the fence that holds it until one is free.
  // `moved` says the box left the viewport's cascade, so the caller reveals it.
  function freeSpawnRect(
    view: Partial<Rect> | null | undefined,
    index: number,
    fences: readonly DeskFence[],
  ) {
    const v = view || {};
    const { width, height } = spawnSize(v);
    const at = (k: number) => ({
      left: Math.max(0, v.left || 0) + 30 + (k % 8) * SPAWN_STEP,
      top: Math.max(0, v.top || 0) + 20 + (k % 8) * SPAWN_STEP,
      width,
      height,
    });
    const start = index || 0;
    for (let i = 0; i < 8; i++) {
      const rect = at(start + i);
      if (!fenceHolds(fences, rect, false)) return { rect, moved: false };
    }
    return { rect: pastLockedFences(fences, at(start)), moved: true };
  }

  // The size of a console born outside a fence. An unmeasurable viewport is a
  // tab still `display:none`: plain caps.
  function spawnSize(v: Partial<Rect>) {
    return {
      width: v.width ? Math.max(WIN_MIN_W, Math.min(560, Math.round(v.width * 0.62))) : 560,
      height: v.height ? Math.max(WIN_MIN_H, Math.min(340, Math.round(v.height * 0.6))) : 340,
    };
  }

  // Move `rect` right of each locked fence that holds it. Each step leaves one
  // fence behind for good, so the walk ends within one step per fence.
  function pastLockedFences(fences: readonly DeskFence[], rect: Rect) {
    for (let i = 0; i <= (fences || []).length; i++) {
      const held = fenceOf(fences, rect);
      if (!held?.locked) break;
      rect.left = (held.rect?.left || 0) + (held.rect?.width || 0) + SPAWN_PAD;
    }
    return rect;
  }

  return {
    STAGE_MARGIN,
    stageExtent,
    FENCE_SIZE,
    FENCE_MIN,
    FENCE_INSET,
    FENCE_GAP,
    FENCE_COLS,
    fenceSpawnRect,
    rectsOverlap,
    titleBarPoint,
    rectHolds,
    fenceMembership,
    fenceOf,
    fenceFits,
    fenceMoveDelta,
    TILE_PAD,
    TILE_GAP,
    TILE_MIN,
    tileIntoRect,
    RESIZE_MIN,
    resizeRect,
    clampOffset,
    bringIntoView,
    anchorIntoView,
    slideEase,
    bboxOf,
    viewLanding,
    panNudge,
    spawnRectIn,
    freeSpawnRect,
    fenceHolds,
    VIEW_INSET,
    PAN_BAND,
    PAN_STEP,
    SPAWN_PAD,
    SPAWN_STEP,
    WIN_MIN_W,
    WIN_MIN_H,
  };
})();
