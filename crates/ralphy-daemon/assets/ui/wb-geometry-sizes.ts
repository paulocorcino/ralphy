/* ---------------------------------------------------------------------------
   The size folds of the plane, as pure functions: how big the stage is, where
   a new fence lands, the cells of a tiled fence, and a resized window.
   `WBGeometry` (`wb-geometry.ts`) returns every name below.
   --------------------------------------------------------------------------- */
import type { Rect, Offset, Size, Delta } from "./wb-types.d.ts";

// ---- the stage extent --------------------------------------------------------
// How big the plane under the windows must be, as a pure function of the rects
// and the viewport: the bbox of the windows plus a margin of drag room past
// their own edges, unioned per axis with the viewport. The origin is pinned at
// 0,0 and the plane grows right and down only — a negative coordinate would
// mean re-anchoring the origin and rewriting every rect (issue #336).
// The viewport leg is what keeps an empty stage exactly viewport-sized, so a
// scrollbar only ever measures something real.
//
// The margin is a FLOOR, not the answer: past any content the plane carries a
// full viewport of room. That is what makes the top-left corner reachable —
// scrolling an item flush to the corner needs `scrollLeft = item.left`, and
// the ceiling is `extent - viewport`, so a 200 px margin left every item but
// the furthest one stuck mid-screen and ADR-0051 §7's "the list is the map"
// could not anchor anywhere. It is NOT §2's rejected fixed 8000×8000: the
// extent still derives from the content, and an empty stage is still exactly
// the viewport (right = 0 keeps the viewport leg on top).
export const STAGE_MARGIN = 200;

// The viewport's size; a missing viewport or side reads 0.
function viewSize(viewport: Partial<Size> | null | undefined) {
  return { width: viewport?.width ?? 0, height: viewport?.height ?? 0 };
}

// The right and bottom edge of the farthest rect, from the origin.
function farCorner(rects: readonly Rect[] | null | undefined) {
  let right = 0;
  let bottom = 0;
  for (const r of rects ?? []) {
    right = Math.max(right, (r.left || 0) + (r.width || 0));
    bottom = Math.max(bottom, (r.top || 0) + (r.height || 0));
  }
  return { right, bottom };
}

export function stageExtent(rects: readonly Rect[], viewport: Size, margin?: number | null) {
  const m = margin ?? STAGE_MARGIN;
  const view = viewSize(viewport);
  const far = farCorner(rects);
  return {
    width: Math.max(view.width, far.right + Math.max(m, view.width)),
    height: Math.max(view.height, far.bottom + Math.max(m, view.height)),
  };
}

// ---- where a new fence lands (issue #340) ------------------------------------
// Pure. A deterministic 2-column grid anchored at the viewport's CURRENT
// offset, so a fence is born where the operator is looking rather than at the
// pinned origin. DISJOINT BY CONSTRUCTION for every index: ADR-0051 §6's
// non-overlap enforcement is the next slice's, and a spawn rule that stacked
// fences would ship the overlap before the invariant exists to fix it.
export const FENCE_SIZE = { width: 720, height: 460 };
export const FENCE_MIN = { width: 240, height: 150 };
export const FENCE_INSET = 40;
export const FENCE_GAP = 24;
export const FENCE_COLS = 2;

export function fenceSpawnRect(
  offset: Offset | null | undefined,
  viewport: Size | null | undefined,
  index: number,
) {
  const width = Math.max(
    FENCE_MIN.width,
    Math.min(FENCE_SIZE.width, (viewport?.width ?? 0) - 2 * FENCE_INSET),
  );
  const height = Math.max(
    FENCE_MIN.height,
    Math.min(FENCE_SIZE.height, (viewport?.height ?? 0) - 2 * FENCE_INSET),
  );
  const i = index || 0;
  const col = i % FENCE_COLS;
  const row = Math.floor(i / FENCE_COLS);
  return {
    left: Math.max(0, offset?.left ?? 0) + FENCE_INSET + col * (width + FENCE_GAP),
    top: Math.max(0, offset?.top ?? 0) + FENCE_INSET + row * (height + FENCE_GAP),
    width,
    height,
  };
}

// Tiling, as a pure fold (issue #342): target rect plus member list in, one
// rect per member out, in order. The grid is the global Arrange's — `cols =
// ceil(sqrt(n))` — kept aspect-independent so this stays a MOVE of the act,
// not a redesign of it.
//
// Pad and gap degrade PER AXIS: a rect too small for its member count yields
// a cell below `TILE_MIN` (a NEGATIVE height, for a short fence), and that
// axis falls back to a bare `extent / k`. Collapsing both axes together would
// deform an axis that still fits. Containment holds by construction in either
// branch — the far edge lands at exactly `pad + k*size + (k-1)*gap = extent -
// pad` — which is what makes "no member escapes the fence" a property of the
// function rather than of its caller.
export const TILE_PAD = 12;
export const TILE_GAP = 10;
export const TILE_MIN = 24;

// One axis of the tile grid: `k` cells over `extent`.
function tileAxis(extent: number, k: number) {
  const size = (extent - TILE_PAD * 2 - TILE_GAP * (k - 1)) / k;
  if (size >= TILE_MIN) return { pad: TILE_PAD, gap: TILE_GAP, size };
  return { pad: 0, gap: 0, size: extent / k };
}

// `members` is only counted: no item of it is read.
export function tileIntoRect(
  rect: Rect | null | undefined,
  members: readonly unknown[] | null | undefined,
) {
  const n = (members ?? []).length;
  if (!n) return [];
  const cols = Math.ceil(Math.sqrt(n));
  const rows = Math.ceil(n / cols);
  const x = tileAxis(rect?.width ?? 0, cols);
  const y = tileAxis(rect?.height ?? 0, rows);
  return (members ?? []).map((_, i) => ({
    left: (rect?.left ?? 0) + x.pad + (i % cols) * (x.size + x.gap),
    top: (rect?.top ?? 0) + y.pad + Math.floor(i / cols) * (y.size + y.gap),
    width: x.size,
    height: y.size,
  }));
}

// ---- resize geometry ---------------------------------------------------------
// The eight directions differ only in which rectangle components move, so the
// whole resize is one pure function: `dir` (`n`/`s`/`e`/`w` and the four
// corners), the stage-relative start `rect`, the pointer `delta`, the
// minimum size and the stage `bounds` yield a new rect. East/south move the
// far edge; west/north move `left`/`top` and derive the size, so the OPPOSITE
// edge stays put and the window does not slide under the cursor.
/** The smallest size a resize leaves, and the stage size it stays inside. */
export type ResizeLimits = { min: Size; bounds: Size };
export const RESIZE_MIN = { width: 240, height: 150 }; // matches .session-window's CSS minimums

export function resizeRect(dir: string, rect: Rect, delta: Delta, limits: ResizeLimits) {
  const { min, bounds } = limits;
  let { left, top, width, height } = rect;
  const right = rect.left + rect.width;
  const bottom = rect.top + rect.height;
  if (dir.includes("e")) {
    width = Math.max(min.width, Math.min(rect.width + delta.dx, bounds.width - left));
  } else if (dir.includes("w")) {
    // Anchor the right edge: clamp the new left, then derive the width from it.
    left = Math.max(0, Math.min(rect.left + delta.dx, right - min.width));
    width = right - left;
  }
  if (dir.includes("s")) {
    height = Math.max(min.height, Math.min(rect.height + delta.dy, bounds.height - top));
  } else if (dir.includes("n")) {
    top = Math.max(0, Math.min(rect.top + delta.dy, bottom - min.height));
    height = bottom - top;
  }
  return { left, top, width, height };
}
