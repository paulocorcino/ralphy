const pascal = (name: string) =>
  name.replace(
    /(\w)(\w*)(_|-|\s*)/g,
    (_: string, first: string, rest: string) => first.toUpperCase() + rest.toLowerCase(),
  );

// `x-icon="'name'"` draws a lucide icon INTO its own `<svg>`. It never swaps the
// element (lucide's `createIcons` replaces it), so it is still the node Alpine
// bound: the icon renders wherever Alpine initializes an element — page load, a
// new `x-if` branch, a new `x-for` row — and a changed name redraws it. The
// output matches `createIcons` (lucide 0.460.0): the icon's default attributes
// where the markup set none, `data-lucide` (the stylesheets select on it), and
// the `lucide lucide-<name>` classes.
export function iconDirective(
  el: HTMLElement,
  { expression }: AlpineDirective,
  { evaluateLater, effect }: AlpineDirectiveUtilities,
) {
  const read = evaluateLater(expression);
  const authored = new Set(el.getAttributeNames());
  let drawn: string | null = null;
  effect(() =>
    read((name) => {
      if (name === drawn) return;
      if (drawn) el.classList.remove(`lucide-${drawn}`);
      drawn = name;
      const node = window.lucide?.icons[pascal(String(name))];
      if (!node) {
        console.warn(`x-icon: no lucide icon named "${name}"`);
        el.replaceChildren();
        return;
      }
      const [, attrs, children] = node;
      for (const [key, value] of Object.entries(attrs)) {
        // `class` is merged below: `:class` on the same element owns the rest.
        if (key !== "class" && !authored.has(key)) el.setAttribute(key, String(value));
      }
      el.setAttribute("data-lucide", name);
      el.classList.add("lucide", `lucide-${name}`);
      el.replaceChildren(...children.map((child) => window.lucide.createElement(child)));
    }),
  );
}
