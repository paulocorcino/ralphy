# Icon

A small glyph before a label. Two icon libraries are in use today:
Bootstrap Icons (a font) and Lucide (inline SVG).

Layer: element (atom). Made of: no smaller part.

## When to use it

- Put an icon before the text of a [Button](./button.md), a menu row or a
  title. An icon alone needs a `title`, as `.modal-x` has.
- Use the file type icons of `devicon` only for files.
- Do not use an icon as the only sign of a state. Write the state too.

## Reference markup

Bootstrap Icons, from `wb-confirm-dialog.part.html`:

```html
<i class="bi bi-question-circle"></i>
```

Lucide, from `wb-consoles-tab-menus.part.html`, drawn by the `x-icon`
directive:

```html
<svg x-icon="'chevron-down'"></svg>
```

## Spacing

Measured in Chromium on 2026-11-14. No spacing of its own. The gap comes from the element around it.

## Modes

Measured sizes: 11.5 to 14.4px.

## The two libraries

| Library         | Markup                | Uses in parts | Since                 |
| --------------- | --------------------- | ------------- | --------------------- |
| Bootstrap Icons | `<i class="bi bi-…">` | 100           | The first page        |
| Lucide          | `<svg x-icon="'…'">`  | 55            | 2026-09-27 (`x-icon`) |

**A new icon is Lucide** (ADR-0035, amendment of 2026-11-14). A Bootstrap
icon moves to Lucide when its panel changes for other work, not in one
change.

## States

| State   | What the code does                                          |
| ------- | ----------------------------------------------------------- |
| Default | `1.05em`, stroke width 1.75 for Lucide, color from its text |
| Others  | From the element around it                                  |

## Files

- `styles/01-base.css`: `[data-lucide]`, `[x-icon]` size
- `vendor/bootstrap-icons/`, `vendor/lucide.min.js`, `vendor/devicon/`
- `wb-icon.ts`: the `x-icon` directive

## Keyboard

See [Keyboard](./keyboard.md). An icon takes no keys.
