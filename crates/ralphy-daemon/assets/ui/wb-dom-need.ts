// Look-ups for a value that must exist where the caller uses it: each returns
// the value or throws an Error that names it, so the type of the result needs
// no assertion.

/** A value the caller's wiring puts in place; `what` names it in the Error. */
export function required<T>(value: T | null | undefined, what: string): T {
  if (value === null || value === undefined) throw new Error(`${what} is missing`);
  return value;
}

/** An element the caller's own markup put in place. */
export function need(root: ParentNode, selector: string): HTMLElement {
  const found = root.querySelector<HTMLElement>(selector);
  if (!found) throw new Error(`the element ${selector} is missing`);
  return found;
}

/** An element inside the pane `rec` has mounted; throws where it has no element yet. */
export function inPane(rec: { el?: HTMLElement | undefined }, selector: string): HTMLElement {
  return need(required(rec.el, "the pane element"), selector);
}

/** An `<input>` the caller's own markup put in place. */
export function needInput(root: ParentNode, selector: string): HTMLInputElement {
  return required(root.querySelector<HTMLInputElement>(selector), `the element ${selector}`);
}

// An event's target, narrowed where a handler reads it. A target that is not
// an element (the window, a document) has no `closest`.

/** Whether `t` is an element: it has `closest`. */
export function isElement(t: EventTarget | null | undefined): t is Element {
  return t != null && "closest" in t;
}

/** Whether `el` is an element that takes focus: it has `focus`. */
export function canFocus(el: Element | null): el is HTMLElement {
  return el != null && "focus" in el;
}

/** Whether `el` is a text field: it has `setSelectionRange`. */
export function isTextField(el: Element | null): el is HTMLInputElement {
  return el != null && "setSelectionRange" in el;
}

/** Whether `t` is a DOM node: it has `nodeType`. */
export function isNode(t: EventTarget | null | undefined): t is Node {
  return t != null && "nodeType" in t;
}

/** The nearest ancestor of the target of `e`, itself included, that matches
 * `selector`; null when the target is not an element. */
export function targetClosest(e: Event, selector: string): HTMLElement | null {
  return isElement(e.target) ? e.target.closest<HTMLElement>(selector) : null;
}
