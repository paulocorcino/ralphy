/* ---------------------------------------------------------------------------
   wb-events.ts — send a `workbench:*` event (#633), and send a gesture as
   `workbench:action` (#651; the gestures are `WorkbenchActions`).
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

// Every `WorkbenchActions` name, at run time. `satisfies` makes a missing or
// an extra name a type error.
const ACTION_NAMES = {
  "kanban-toggle": true,
  "branch-switch": true,
  "branch-create": true,
  "run-issue-focus": true,
  "run-start": true,
  command: true,
  "issue-label-change": true,
  login: true,
  logoff: true,
  "open-refused": true,
  "open-diff": true,
  "detach-blocked": true,
  detach: true,
  "console-open": true,
  "console-close": true,
  "console-restart": true,
  "console-switch-checkout": true,
  "worktree-created": true,
  "fence-reattach": true,
  "fence-focus": true,
  "fence-detach": true,
  "fence-detach-blocked": true,
  "fence-detach-refused": true,
  "fence-remove-refused": true,
  rename: true,
  open: true,
  delete: true,
  "copy-path": true,
  create: true,
  "setting-change": true,
  save: true,
  reload: true,
} satisfies Record<WorkbenchActionName, true>;

/** The `WB` of the shell page: a gesture becomes a `workbench:action` event
 * on `document`, with the time it was sent. */
export function createEmitter(document: Document): Window["WB"] {
  return {
    emit(action, ...detail) {
      // The signature ties the detail to the action; TypeScript cannot follow
      // that tie through a spread of a generic detail.
      const full = { action, ...detail[0], at: new Date().toISOString() } as WorkbenchAction;
      sendDocument(document, "workbench:action", full);
      // eslint-disable-next-line no-console
      console.log("[workbench:action]", full);
    },
  };
}

/** Whether `v` names a `workbench:action` gesture. */
export function isWorkbenchActionName(v: unknown): v is WorkbenchActionName {
  return typeof v === "string" && Object.hasOwn(ACTION_NAMES, v);
}

/** Send, on `wb`, a gesture a popup posted. An action that is not a gesture
 * name is dropped: the answer is `false`. `fromWindow` is the popup, for a
 * gesture whose answer goes back to it. */
export function forwardAction(wb: Window["WB"], action: unknown, detail: unknown, fromWindow?: Window): boolean {
  if (!isWorkbenchActionName(action)) return false;
  // The popup is a page of this build, and its own `WB.emit` checked the
  // detail against the action before it posted it.
  const known = (typeof detail === "object" && detail ? detail : {}) as NonNullable<WorkbenchActions[WorkbenchActionName]>;
  wb.emit(action, fromWindow ? { ...known, fromWindow } : known);
  return true;
}
