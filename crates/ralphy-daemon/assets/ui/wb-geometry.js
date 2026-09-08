/* ---------------------------------------------------------------------------
   The workbench's plane geometry — every rect fold the desk performs, as pure
   functions of their arguments.

   Lifted out of `wb-console.js` under ADR-0057. Nothing here reads the DOM, the
   socket, the desk store or a module-scope binding: given the same rects it
   returns the same rects, on any thread, in any order. That is the whole
   selection rule, and it is why this is the one seam `wb-console.js` had — the
   rest of that file is a web of reads and writes across twenty-one module-scope
   `let`s (ADR-0022 §5: a file with no existing seam is a design problem, not a
   split).

   `wb-console.js` re-exports every name below, so `WBConsole.tileIntoRect` and
   friends keep working for the callers and the tests that already use them
   (CLAUDE.md: the public surface is stable by default). New callers should
   prefer `WBGeometry`.

   Load order: BEFORE `wb-console.js`, in both `index.html` and
   `detached-fence.html`. The console destructures this namespace at module
   scope, so a later tag leaves it reading `undefined` on the first paint.
   --------------------------------------------------------------------------- */
window.WBGeometry = (function () {
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
  const STAGE_MARGIN = 200;

  function stageExtent(rects, viewport, margin) {
    const m = margin == null ? STAGE_MARGIN : margin;
    const mx = Math.max(m, viewport?.width || 0);
    const my = Math.max(m, viewport?.height || 0);
    let right = 0;
    let bottom = 0;
    for (const r of rects || []) {
      right = Math.max(right, (r.left || 0) + (r.width || 0));
      bottom = Math.max(bottom, (r.top || 0) + (r.height || 0));
    }
    return {
      width: Math.max(viewport?.width || 0, right + mx),
      height: Math.max(viewport?.height || 0, bottom + my),
    };
  }

  // ---- where a new fence lands (issue #340) ------------------------------------
  // Pure. A deterministic 2-column grid anchored at the viewport's CURRENT
  // offset, so a fence is born where the operator is looking rather than at the
  // pinned origin. DISJOINT BY CONSTRUCTION for every index: ADR-0051 §6's
  // non-overlap enforcement is the next slice's, and a spawn rule that stacked
  // fences would ship the overlap before the invariant exists to fix it.
  const FENCE_SIZE = { width: 720, height: 460 };
  const FENCE_MIN = { width: 240, height: 150 };
  const FENCE_INSET = 40;
  const FENCE_GAP = 24;
  const FENCE_COLS = 2;

  function fenceSpawnRect(offset, viewport, index) {
    const width = Math.max(
      FENCE_MIN.width,
      Math.min(FENCE_SIZE.width, (viewport?.width || 0) - 2 * FENCE_INSET),
    );
    const height = Math.max(
      FENCE_MIN.height,
      Math.min(FENCE_SIZE.height, (viewport?.height || 0) - 2 * FENCE_INSET),
    );
    const i = index || 0;
    const col = i % FENCE_COLS;
    const row = Math.floor(i / FENCE_COLS);
    return {
      left: Math.max(0, offset?.left || 0) + FENCE_INSET + col * (width + FENCE_GAP),
      top: Math.max(0, offset?.top || 0) + FENCE_INSET + row * (height + FENCE_GAP),
      width,
      height,
    };
  }

  function rectsOverlap(a, b) {
    return (
      (a?.left || 0) < (b?.left || 0) + (b?.width || 0) &&
      (a?.left || 0) + (a?.width || 0) > (b?.left || 0) &&
      (a?.top || 0) < (b?.top || 0) + (b?.height || 0) &&
      (a?.top || 0) + (a?.height || 0) > (b?.top || 0)
    );
  }

  // ---- a fence is a group (issue #341) -----------------------------------------
  // Membership is DERIVED, never stored: a window belongs to the fence that holds
  // its CENTRE point, so nothing has to be kept in step and a record can never
  // disagree with the geometry. Containment is HALF-OPEN (`left <= cx < left +
  // width`) — fences may abut, and a closed test would put a centre sitting on a
  // shared border inside both of them, breaking "exactly one fence".
  function rectCentre(rect) {
    return {
      x: (rect?.left || 0) + (rect?.width || 0) / 2,
      y: (rect?.top || 0) + (rect?.height || 0) / 2,
    };
  }

  // Does `rect` hold `point`? The half-open test above, extracted once (issue
  // #343) so membership and the floor's focus-clearing hit test can never drift
  // into two spellings of the same containment.
  function rectHolds(rect, point) {
    const r = rect || {};
    const left = r.left || 0;
    const top = r.top || 0;
    return (
      (point?.x || 0) >= left &&
      (point?.x || 0) < left + (r.width || 0) &&
      (point?.y || 0) >= top &&
      (point?.y || 0) < top + (r.height || 0)
    );
  }

  // `fences` is `[{ id, rect }]`, `windows` is `[{ id, rect }]`; the answer maps
  // EVERY fence id (an empty one to `[]`) and omits a window in no fence. The
  // `break` is the second half of "exactly one": half-open containment makes the
  // fences disjoint as point sets, and this makes the fold's own answer so even
  // if a stored rect pair ever overlaps.
  function fenceMembership(fences, windows) {
    const list = fences || [];
    const out = {};
    for (const f of list) out[f.id] = [];
    for (const w of windows || []) {
      const c = rectCentre(w?.rect);
      for (const f of list) {
        if (rectHolds(f.rect, c)) {
          out[f.id].push(w.id);
          break;
        }
      }
    }
    return out;
  }

  // May `candidate` (`{ id, rect }`) take the plane? Pure, and the SAME strict
  // `rectsOverlap` the spawn rule uses, so abutting fences stay buildable and
  // one predicate answers for create, move and resize alike. A candidate is
  // never compared with itself — a move must not refuse its own start rect.
  function fenceFits(fences, candidate) {
    const rect = candidate?.rect || {};
    return !(fences || []).some(
      (f) => f.id !== candidate?.id && rectsOverlap(rect, f.rect || {}),
    );
  }

  // The move delta a fence and the members it carries may actually take: the
  // request, clamped so no left/top lands below 0. The plane's origin is pinned
  // (issue #336) and grows right and down only, so a negative coordinate is not
  // a position — it is a lost window. The MEMBERS are in the fold too: one can
  // sit further left than the fence that carries it.
  function fenceMoveDelta(delta, fenceRect, memberRects) {
    const members = memberRects || [];
    const minLeft = Math.min(fenceRect?.left || 0, ...members.map((r) => r?.left || 0));
    const minTop = Math.min(fenceRect?.top || 0, ...members.map((r) => r?.top || 0));
    return {
      dx: Math.max(delta?.dx || 0, -minLeft),
      dy: Math.max(delta?.dy || 0, -minTop),
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
  const TILE_PAD = 12;
  const TILE_GAP = 10;
  const TILE_MIN = 24;

  function tileIntoRect(rect, members) {
    const n = (members || []).length;
    if (!n) return [];
    const cols = Math.ceil(Math.sqrt(n));
    const rows = Math.ceil(n / cols);
    const axis = (extent, k) => {
      const size = (extent - TILE_PAD * 2 - TILE_GAP * (k - 1)) / k;
      if (size >= TILE_MIN) return { pad: TILE_PAD, gap: TILE_GAP, size };
      return { pad: 0, gap: 0, size: extent / k };
    };
    const x = axis(rect?.width || 0, cols);
    const y = axis(rect?.height || 0, rows);
    return (members || []).map((_, i) => ({
      left: (rect?.left || 0) + x.pad + (i % cols) * (x.size + x.gap),
      top: (rect?.top || 0) + y.pad + Math.floor(i / cols) * (y.size + y.gap),
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
  const RESIZE_MIN = { width: 240, height: 150 }; // matches .session-window's CSS minimums

  function resizeRect(dir, rect, delta, min, bounds) {
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
    rectCentre,
    rectHolds,
    fenceMembership,
    fenceFits,
    fenceMoveDelta,
    TILE_PAD,
    TILE_GAP,
    TILE_MIN,
    tileIntoRect,
    RESIZE_MIN,
    resizeRect,
  };
})();
