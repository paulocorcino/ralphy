// The file viewer's two panes (ADR-0037 §3c): which panes a slot puts on
// screen, in which grid column, and which editor takes the focus. Pure rules
// over the records `wb-file-viewer.ts` holds; the viewer does the painting.

/** The second pane the shell resolved (`WBSplit.resolve`). */
export type ViewerSlot = { id: string; mirror?: boolean; focus?: boolean; ratio: number | null };

/** What these rules read of an open pane: its kind, and its editor once mounted. */
export type PaneShape = { kind: string; ed?: unknown };

/** Where one pane goes on a paint: shown or not, its `display`, and its grid column. */
export type PaneLayout = { on: boolean; display: "flex" | "none"; gridColumn: "1" | "3" | "" };

/** Whether the slot shows a second pane beside the active one. */
export function isSplit(slot: ViewerSlot | null, slotRec: PaneShape | null | undefined): boolean {
  // A mirror needs an editor to mirror; a pin needs its pane open.
  return !!slot && !!slotRec && (!slot.mirror || (slotRec.kind === "code" && !!slotRec.ed));
}

/** The place of the pane `recId` when `id` is active: the active pane takes
 * column 1 and the slot's pane column 3, and one pane alone has no column. */
export function paneLayout(
  recId: string,
  id: string | null,
  split: boolean,
  slotId: string | undefined,
): PaneLayout {
  const on = recId === id || (split && recId === slotId);
  if (!on) return { on, display: "none", gridColumn: "" };
  if (!split) return { on, display: "flex", gridColumn: "" };
  return { on, display: "flex", gridColumn: recId === id ? "1" : "3" };
}

/** The editor a focused slot hands the keyboard to: the mirror's own editor
 * when the slot is a mirror, else the pinned pane's. */
export function focusTarget<E>(
  slot: ViewerSlot,
  mirror: { ed: E | null } | null,
  slotEd: E | undefined,
): E | null | undefined {
  return slot.mirror ? mirror?.ed : slotEd;
}
