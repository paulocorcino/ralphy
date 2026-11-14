# Context menu

The menu that opens at the pointer on a right click. One element for the
whole page, `#ctxmenu.ctxmenu`; code writes its rows (`.ctx-item`).

Layer: component (molecule). Made of: rows with an [icon](./icon.md), separators.

## When to use it

- Use it for the acts on one thing under the pointer: a tab, a file.
- Use a [Dropdown menu](./dropdown.md) for acts behind a toolbar button.
- Never put an act only in a context menu: a phone has no right click.
  The same act is also on a button or in a dropdown.

## Reference markup

The container, from `index.html`:

```html
<div id="ctxmenu" class="ctxmenu"></div>
```

A row, built in `wb-tabs-store-menu.ts`:

```ts
b.className = "ctx-item" + (it.danger === true ? " danger" : "");
```

A separator is `<div class="ctx-sep">`.

## Spacing

Measured in Chromium on 2026-11-14. Menu: padding 4px, radius 6px. Row: padding 5.6px 8.8px, gap 8px, 28px high.

## Modes

Rows are 28px high in every mode.

Contrast, measured: a row passes (8.5:1). A danger row is 4.52:1 since `--danger` became `#ce8174` (audit, D6).

## States

| State                 | What the code does                                                                          |
| --------------------- | ------------------------------------------------------------------------------------------- |
| Default               | Fixed, `z-index: 200`, `--surface`, radius `6px`, shadow. Row `0.8rem`, icon `--text-muted` |
| Hover                 | Row ground `--surface-hi`                                                                   |
| Danger                | `.ctx-item.danger`: text and icon `--danger`                                                |
| Focus                 | The focus ring, drawn inside the row (offset -2px)                                          |
| Disabled              | `opacity: 0.45`, `cursor: default`                                                          |
| Empty, error, loading | Not used                                                                                    |

A click anywhere or a scroll hides it (`hideContextMenu` in `wb-wire.ts`).

The row is almost the same rule as the [Dropdown menu](./dropdown.md) row,
with other values: `0.8rem` against `0.82rem`, `0.45` against `0.4` when
disabled. See the [audit](./audit.md).

## Files

- `styles/04-canvas.css`: `.ctxmenu`, `.ctx-item`, `.ctx-sep`
- `wb-wire.ts`, `wb-tabs-store-menu.ts`

## Keyboard

See [Keyboard](./keyboard.md).
