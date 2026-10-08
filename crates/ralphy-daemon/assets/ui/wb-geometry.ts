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

   `wb-console.ts` and `wb-notes.ts` import this module.
   --------------------------------------------------------------------------- */
export const WBGeometry = (function () {
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

  function stageExtent(rects: any, viewport: any, margin: any) {
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

  function fenceSpawnRect(offset: any, viewport: any, index: any) {
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

  function rectsOverlap(a: any, b: any) {
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
  function rectCentre(rect: any) {
    return {
      x: (rect?.left || 0) + (rect?.width || 0) / 2,
      y: (rect?.top || 0) + (rect?.height || 0) / 2,
    };
  }

  // Does `rect` hold `point`? The half-open test above, extracted once (issue
  // #343) so membership and the floor's focus-clearing hit test can never drift
  // into two spellings of the same containment.
  function rectHolds(rect: any, point: any) {
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
  function fenceMembership(fences: any, windows: any) {
    const list = fences || [];
    const out: any = {};
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

  // The fence that holds `rect` — the SAME half-open test and the SAME
  // first-match rule as `fenceMembership`, so the two can never disagree about
  // whose a window is. `null` when no fence holds its centre. This is what a
  // gesture consults to ask "is the fence under this window locked?".
  function fenceOf(fences: any, rect: any) {
    const c = rectCentre(rect);
    for (const f of fences || []) {
      if (rectHolds(f.rect, c)) return f;
    }
    return null;
  }

  // May `candidate` (`{ id, rect }`) take the plane? Pure, and the SAME strict
  // `rectsOverlap` the spawn rule uses, so abutting fences stay buildable and
  // one predicate answers for create, move and resize alike. A candidate is
  // never compared with itself — a move must not refuse its own start rect.
  function fenceFits(fences: any, candidate: any) {
    const rect = candidate?.rect || {};
    return !(fences || []).some(
      (f: any) => f.id !== candidate?.id && rectsOverlap(rect, f.rect || {}),
    );
  }

  // The move delta a fence and the members it carries may actually take: the
  // request, clamped so no left/top lands below 0. The plane's origin is pinned
  // (issue #336) and grows right and down only, so a negative coordinate is not
  // a position — it is a lost window. The MEMBERS are in the fold too: one can
  // sit further left than the fence that carries it.
  function fenceMoveDelta(delta: any, fenceRect: any, memberRects: any) {
    const members = memberRects || [];
    const minLeft = Math.min(fenceRect?.left || 0, ...members.map((r: any) => r?.left || 0));
    const minTop = Math.min(fenceRect?.top || 0, ...members.map((r: any) => r?.top || 0));
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

  function tileIntoRect(rect: any, members: any) {
    const n = (members || []).length;
    if (!n) return [];
    const cols = Math.ceil(Math.sqrt(n));
    const rows = Math.ceil(n / cols);
    const axis = (extent: any, k: any) => {
      const size = (extent - TILE_PAD * 2 - TILE_GAP * (k - 1)) / k;
      if (size >= TILE_MIN) return { pad: TILE_PAD, gap: TILE_GAP, size };
      return { pad: 0, gap: 0, size: extent / k };
    };
    const x = axis(rect?.width || 0, cols);
    const y = axis(rect?.height || 0, rows);
    return (members || []).map((_: any, i: any) => ({
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

  function resizeRect(dir: any, rect: any, delta: any, min: any, bounds: any) {
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

  // ---- navigating the plane ----------------------------------------------------
  // The scroll offsets that bring `target` (a STAGE-relative rect) into the
  // viewport, pure: centre it, then clamp to `[0, extent - viewport]`. ALWAYS
  // centres. Callers: `reveal` (the Go-to picker, #337) and ADR-0051 §7's fence
  // jump.
  // ONE clamp per axis: the final `Math.max(0, …)` stops a viewport bigger than
  // the extent asking for a negative offset. Flooring the ceiling too would
  // make that floor unfalsifiable by the table's negative control.
  function clampOffset(offset: any, viewport: any, extent: any) {
    const maxLeft = (extent?.width || 0) - (viewport?.width || 0);
    const maxTop = (extent?.height || 0) - (viewport?.height || 0);
    return {
      left: Math.max(0, Math.min(offset?.left || 0, maxLeft)),
      top: Math.max(0, Math.min(offset?.top || 0, maxTop)),
    };
  }

  function bringIntoView(target: any, viewport: any, extent: any) {
    const vw = viewport?.width || 0;
    const vh = viewport?.height || 0;
    const left = (target?.left || 0) + (target?.width || 0) / 2 - vw / 2;
    const top = (target?.top || 0) + (target?.height || 0) / 2 - vh / 2;
    return clampOffset({ left, top }, viewport, extent);
  }

  // The other anchoring: the target's TOP-LEFT corner, one inset in from the
  // viewport's. A fence is a region the operator works inside, not a point of
  // interest — centring it wastes the screen above and left. `bringIntoView`
  // keeps CENTRING for the Go-to picker (#337 pins it).
  const VIEW_INSET = 24;

  function anchorIntoView(target: any, viewport: any, extent: any, inset: any) {
    const pad = inset == null ? VIEW_INSET : inset;
    return clampOffset(
      { left: (target?.left || 0) - pad, top: (target?.top || 0) - pad },
      viewport,
      extent,
    );
  }


  // Ease-out cubic on a 0..1 clock: fast off the mark, settling into the target.
  function slideEase(t: any) {
    const x = Math.min(1, Math.max(0, t));
    return 1 - Math.pow(1 - x, 3);
  }

  // The bounding box of a set of stage-relative rects; all zeros for none, so an
  // empty desk centres on the pinned origin rather than on nothing.
  function bboxOf(rects: any) {
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
  function viewLanding(stored: any, rects: any, viewport: any, extent: any) {
    const num = (v: any) => (typeof v === "number" && Number.isFinite(v) ? v : null);
    const left = num(stored?.left);
    const top = num(stored?.top);
    if (left !== null && top !== null) {
      const at = clampOffset({ left, top }, viewport, extent);
      const vw = viewport?.width || 0;
      const vh = viewport?.height || 0;
      const shows = (rects || []).some(
        (r: any) =>
          (r.left || 0) < at.left + vw &&
          (r.left || 0) + (r.width || 0) > at.left &&
          (r.top || 0) < at.top + vh &&
          (r.top || 0) + (r.height || 0) > at.top,
      );
      if (shows) return at;
    }
    return bringIntoView(bboxOf(rects), viewport, extent);
  }

  // How far the plane scrolls per frame while a window is dragged against the
  // viewport edge, and how wide the pressure band at each edge is.
  const PAN_BAND = 48;
  const PAN_STEP = 24;
  // The auto-pan rule, pure. `viewport` is a CLIENT rect; `pointer` is a client
  // point. Each edge contributes a pressure in `[0, band]` and the axis takes
  // their DIFFERENCE — deliberately, so a viewport narrower than two bands
  // cancels instead of oscillating between its own two edges.
  function panNudge(pointer: any, viewport: any, band: any, step: any) {
    const b = band == null ? PAN_BAND : band;
    const s = step == null ? PAN_STEP : step;
    const press = (v: any) => Math.max(0, Math.min(v, b));
    const axis = (near: any, far: any) => Math.round((s * (far - near)) / b);
    return {
      dx: axis(
        press(b - ((pointer?.x || 0) - (viewport?.left || 0))),
        press(b - ((viewport?.right || 0) - (pointer?.x || 0))),
      ),
      dy: axis(
        press(b - ((pointer?.y || 0) - (viewport?.top || 0))),
        press(b - ((viewport?.bottom || 0) - (pointer?.y || 0))),
      ),
    };
  }

  // Is this console held by a LOCKED fence? Never in the popup (`autoBoot:
  // false`): `mountDetached` re-origins the members' rects into that window
  // while `fences` keeps the shell's stage coordinates, so the fold would
  // match a console to whatever fence covers the translated point. A detached
  // fence's members move freely there; the fence's lock holds again on the
  // stage when they come home. Note cards follow the same rule.
  function fenceHolds(records: any, rect: any, popup: any) {
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

  function spawnRectIn(fence: any, index: any, headH: any) {
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
    // centre-based fold would report the newborn console in NO fence.
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
  // centre a locked fence holds (the `fenceHolds` fold); when every slot is
  // held, the box moves right of the fence that holds it until one is free.
  // `moved` says the box left the viewport's cascade, so the caller reveals it.
  function freeSpawnRect(view: any, index: any, fences: any) {
    const v = view || {};
    // An unmeasurable viewport is a tab still `display:none`: plain caps.
    const width = v.width ? Math.max(WIN_MIN_W, Math.min(560, Math.round(v.width * 0.62))) : 560;
    const height = v.height ? Math.max(WIN_MIN_H, Math.min(340, Math.round(v.height * 0.6))) : 340;
    const at = (k: any) => ({
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
    const rect = at(start);
    // Each step leaves one fence behind for good, so the walk ends within one
    // step per fence.
    for (let i = 0; i <= (fences || []).length; i++) {
      const held = fenceOf(fences, rect);
      if (!held?.locked) break;
      rect.left = (held.rect?.left || 0) + (held.rect?.width || 0) + SPAWN_PAD;
    }
    return { rect, moved: true };
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
