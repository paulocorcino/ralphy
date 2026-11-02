/* ---------------------------------------------------------------------------
   The messages a detached fence window posts to its opener (`wb-detached-fence.ts`),
   and the check the opener makes before it acts on one.
   --------------------------------------------------------------------------- */
import { isOptionalString, isRecord } from "./wb-api.ts";
import type { DetachMessage } from "./wb-detach-link.ts";

// What a popup says about a card's file: the card's id and the name it gave.
export type NoteReport = Pick<DetachMessage, "noteId" | "path" | "claim">;

/** A message a detached fence window sends its opener (`wb-detached-fence.ts`). */
export type FencePopupMessage =
  | { type: "wb-fence-ready" }
  | { type: "wb-emit"; action: unknown; detail?: unknown }
  | ({ type: "wb-note-named" } & NoteReport)
  | ({ type: "wb-note-claimed" } & NoteReport)
  | { type: "wb-fence-reattach" };

/** Whether `v` is a message a detached fence window sends. The daemon serves
 * the popup's page when it opens, so it can be a newer build than this tab. */
export function isFencePopupMessage(v: unknown): v is FencePopupMessage {
  if (!isRecord(v)) return false;
  switch (v.type) {
    case "wb-fence-ready":
    case "wb-emit":
    case "wb-fence-reattach":
      return true;
    case "wb-note-named":
    case "wb-note-claimed":
      return isOptionalString(v.noteId) && isOptionalString(v.path) && isOptionalString(v.claim);
    default:
      return false;
  }
}
