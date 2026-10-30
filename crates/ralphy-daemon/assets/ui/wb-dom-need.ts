// Look-ups for a value that must exist where the caller uses it: each returns
// the value or throws an Error that names it, so the type of the result needs
// no assertion.

/** A value the caller's wiring puts in place; `what` names it in the Error. */
export function required<T>(value: T | null | undefined, what: string): T {
  if (value === null || value === undefined) throw new Error(`${what} is missing`);
  return value;
}

/** An element the caller's own markup put in place. */
export function need<T extends Element = HTMLElement>(root: ParentNode, selector: string): T {
  const found = root.querySelector<T>(selector);
  if (!found) throw new Error(`the element ${selector} is missing`);
  return found;
}

/** An element inside the pane `rec` has mounted; throws where it has no element yet. */
export function inPane<T extends Element = HTMLElement>(
  rec: { el?: HTMLElement | undefined },
  selector: string,
): T {
  return need<T>(required(rec.el, "the pane element"), selector);
}
