/* ---------------------------------------------------------------------------
   The workbench's secondary pane — "the slot" (ADR-0037 §3c) — as pure
   functions of their arguments.

   The canvas shows ONE tab, or one tab and a slot beside it. The slot holds
   either a pinned tab (`{kind:"pin", id}` — any tab, shown next to whichever
   tab is active) or a mirror (`{kind:"mirror"}` — the active code tab again, in
   a second editor over the same model). There is never more than one slot and
   never a tree of groups; the arrangement is per-client view state
   (ADR-0051 §8), never desk state.

   Nothing here reads the DOM, the store or a module-scope binding: `app.js`
   holds `slot`/`splitRatio`/`lastLeft`, feeds them through `resolve` on every
   `syncViewer`, and `wb-viewer.js` paints the answer. Same shape as
   `wb-geometry.js` → `wb-console.js`.

   Load order: BEFORE `app.js`; nothing else reads this namespace.
   --------------------------------------------------------------------------- */
window.WBSplit = (function () {
  // Below this canvas width the split is unavailable — single pane, the slot
  // kept but not painted. Two 280px editors plus the divider is the floor at
  // which either side still shows a line of code rather than a gutter; below
  // 900 the pair would be phones side by side, and the stylesheet's narrow
  // shape (`@container viewer (max-width: 560px)`) already answers a phone.
  const MIN_WIDTH = 900;
  const MIN_PANE = 280;
  const DEFAULT_RATIO = 0.5;

  function available(width) {
    return Number.isFinite(width) && width >= MIN_WIDTH;
  }

  const single = (left) => ({ left, right: null, mirror: false, focus: "left" });

  // What each column shows. `active` is the shell's active tab id, `paneless`
  // says it maps to no viewer pane (Consoles, Spend) — then nothing is painted
  // and the slot waits in state. A pin next to itself, a pin of a tab that is
  // gone, a mirror of a pane that is not code: single, slot kept, converging on
  // the next call once the world changes.
  function resolve({ active, slot, tabs, lastLeft, width, paneless }) {
    if (paneless || !active) return single(null);
    if (!slot || !available(width)) return single(active);
    const tab = (id) => (tabs || []).find((t) => t.id === id);
    if (slot.kind === "mirror") {
      return tab(active)?.kind === "code"
        ? { left: active, right: active, mirror: true, focus: "left" }
        : single(active);
    }
    if (slot.kind !== "pin" || !tab(slot.id)) return single(active);
    if (active !== slot.id) return { left: active, right: slot.id, mirror: false, focus: "left" };
    // The pinned tab was activated from the strip: the pane it sits beside is
    // the last tab the operator read on the left, else the nearest other tab
    // that has a pane; with no such tab there is nothing to split against.
    const left =
      lastLeft && lastLeft !== slot.id && tab(lastLeft)
        ? lastLeft
        : ((tabs || []).find((t) => t.closable && t.id !== slot.id) || {}).id;
    return left ? { left, right: slot.id, mirror: false, focus: "right" } : single(active);
  }

  // A closed tab takes its pin with it; a mirror follows the active tab and
  // survives any close.
  function afterClose(slot, closedId) {
    return slot?.kind === "pin" && slot.id === closedId ? null : slot;
  }

  // The left column's share of the canvas after a divider drag: the pointer's
  // x over the width, held so neither pane drops under `MIN_PANE` nor under a
  // fifth of the canvas — the range `wb-view.js` accepts back from the store.
  const MIN_RATIO = 0.2;
  function clampRatio(px, width, minPane = MIN_PANE) {
    if (!Number.isFinite(width) || width <= 0) return DEFAULT_RATIO;
    const lo = Math.max(MIN_RATIO, Math.min(minPane / width, 0.5));
    const hi = 1 - lo;
    return Math.min(hi, Math.max(lo, px / width));
  }

  // The stored shape (`wb.view.v1.split`): a pin names its file the way the
  // stored tabs do — project, path, checkout — so a reload finds the tab the
  // store itself reopened. A pin of a tab the store does not carry (a diff: its
  // sides are live git state) stores as no slot, the rule `persistView` applies
  // to the tab itself. `null` is a real value here: the store is
  // read-modify-write, and only an explicit null clears a stale pin.
  function toStored(slot, ratio, tabs) {
    const r = Number.isFinite(ratio) ? ratio : null;
    if (slot?.kind === "mirror") return { kind: "mirror", ratio: r };
    if (slot?.kind !== "pin") return null;
    const t = (tabs || []).find((x) => x.id === slot.id);
    if (!t || !t.id.startsWith("file:")) return null;
    return { kind: "pin", project: t.project, path: t.path, checkout: t.checkout ?? null, ratio: r };
  }

  function fromStored(stored, tabs) {
    if (!stored || typeof stored !== "object") return null;
    if (stored.kind === "mirror") return { kind: "mirror" };
    if (stored.kind !== "pin") return null;
    const t = (tabs || []).find(
      (x) =>
        x.project === stored.project &&
        x.path === stored.path &&
        (x.checkout ?? null) === (stored.checkout ?? null),
    );
    return t ? { kind: "pin", id: t.id } : null;
  }

  return { MIN_WIDTH, MIN_PANE, MIN_RATIO, DEFAULT_RATIO, available, resolve, afterClose, clampRatio, toStored, fromStored };
})();
