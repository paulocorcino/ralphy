/* ---------------------------------------------------------------------------
   wb-events.ts — send a `workbench:*` event (#633).
   The names and their `detail` types are listed once, in `globals.d.ts`
   (`WindowEventMap`, `DocumentEventMap`). A name that is not listed, or a
   detail of the wrong shape, is a type error here. The target is a parameter:
   a module sends on the `window` or `document` it was given, so a test passes
   its own stubs. A listener needs no helper: `addEventListener` reads the same
   lists.
   --------------------------------------------------------------------------- */

/** The `workbench:*` names sent on `window`. */
export type WindowEventName = Extract<keyof WindowEventMap, `workbench:${string}`>;
/** The `workbench:*` names sent on `document`. */
export type DocumentEventName = Extract<keyof DocumentEventMap, `workbench:${string}`>;

type DetailOf<E> = E extends CustomEvent<infer D> ? D : never;
// An event whose detail is `null` takes no detail argument.
type DetailArgs<D> = [D] extends [null] ? [] : [detail: D];

/** Send `name` on `target`, a window. */
export function sendWindow<K extends WindowEventName>(
  target: Window,
  name: K,
  ...detail: DetailArgs<DetailOf<WindowEventMap[K]>>
): void {
  target.dispatchEvent(new CustomEvent(name, { detail: detail[0] }));
}

/** Send `name` on `target`, a document. */
export function sendDocument<K extends DocumentEventName>(
  target: Document,
  name: K,
  ...detail: DetailArgs<DetailOf<DocumentEventMap[K]>>
): void {
  target.dispatchEvent(new CustomEvent(name, { detail: detail[0] }));
}
