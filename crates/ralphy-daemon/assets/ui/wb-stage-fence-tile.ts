/* ---------------------------------------------------------------------------
   Tiling a fence's consoles into the fence's rect (#342). `createFences` in
   `wb-stage-fences.ts` creates it and hands out `arrangeFence`.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import type { FenceDeps } from "./wb-stage-fences.ts";
import type { FenceList } from "./wb-stage-fence-list.ts";
import type { ConsoleWin } from "./wb-types.d.ts";

const { fenceMembership, tileIntoRect, WIN_MIN_W, WIN_MIN_H } = WBGeometry;

// What tiling reads from the console, and nothing else.
export type FenceTileDeps = Pick<
  FenceDeps,
  "popups" | "wins" | "fences" | "stage" | "applyExtent" | "focusWin" | "restoreRect" | "setWin"
> &
  Pick<FenceList, "fenceEl" | "fenceLocked" | "refreshFenceChrome">;

function fenceHeadHeight(el: HTMLElement) {
  return (el.querySelector<HTMLElement>(".fence-head")?.offsetHeight ?? 0) || 28;
}

export function createFenceTiling(deps: FenceTileDeps) {
  const { popups, wins, fences, stage, fenceEl, fenceLocked, refreshFenceChrome } = deps;
  const { applyExtent, focusWin, restoreRect, setWin } = deps;
  // Tile ONE fence's members into its rect (#342), inset by the fence's OWN chrome:
  // a member on the head band or the SE `.fence-grip` buries the fence's controls.
  const FENCE_GRIP = 14;
  function arrangeFence(id: string) {
    // Tiling the empty box would rewrite the rects the popup restores from.
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
    // The FULL fence list with this fence's LIVE rect: the fold's `break` decides
    // an overlapping pair.
    const live = fences().map((x) => (x.id === id ? { id: x.id, rect } : x));
    const ids = new Set(fenceMembership(live, all)[id] ?? []);
    // A maximized console is NOT tiled: a tile rect written onto it is invisible
    // while it REPLACES the pre-maximize rect (#338). A LOCKED one is skipped too.
    const members = all
      .filter(
        (m) =>
          ids.has(m.id) &&
          !m.el.classList.contains("maximized") &&
          !m.el.classList.contains("column") &&
          !m.el._deskLocked,
      )
      .map((m) => m.el);
    if (members.length === 0) return;
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
      if (!t) return;
      // Written now: a read after the transition would race a hidden tab.
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
      setTimeout(() => {
        win.classList.remove("tiling");
      }, 260);
    });
    raiseFullBleeds();
    // AFTER the 0.24s tiling transition: else the fold measures the old boxes.
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

  // `maxlock` leaves no way to scroll away from a buried full bleed.
  function raiseFullBleeds() {
    for (const win of wins) {
      if (win.classList.contains("maximized") || win.classList.contains("column")) focusWin(win);
    }
  }

  return { arrangeFence };
}
