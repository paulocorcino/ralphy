# Kbd hint

A small box that shows a keyboard shortcut at the right of a menu row.
Element `<kbd class="kbd">`.

Layer: element (atom). Made of: text.

## When to use it

- Use it at the end of a [Dropdown menu](./dropdown.md) row whose act has
  a shortcut.
- Use the `title` of a [Button](./button.md) to show the shortcut of a
  button.
- Write the shared part of a chord once, in the menu head, and only the
  last key on each row. The Consoles menu does this.
- Every shortcut shown here is listed in [Keyboard](./keyboard.md).

## Reference markup

From `wb-consoles-tab-menus.part.html`:

```html
<div class="dropdown-head">
  <span x-text="…"></span>
  <kbd class="kbd" x-text="consoleShortcutHint()"></kbd>
</div>
…
<button class="dropdown-item" …>
  <i class="bi bi-terminal"></i>
  <span x-text="it.label"></span>
  <kbd class="kbd" x-text="it.digit"></kbd>
</button>
```

## Spacing

Measured in Chromium on 2026-11-14. In a row: padding 0.32px 5.44px, 16px high, 10.56px text, radius 4px. The menu head shows it with the same box.

## Modes

Same in every mode.

## States

| State   | What the code does                                                            |
| ------- | ----------------------------------------------------------------------------- |
| Default | `0.66rem`, `--text-muted`, `--log-bg`, `--border`, radius `4px`, pushed right |
| Others  | Not used: the row carries hover and disabled                                  |

One rule for every `.kbd`, in a row or in a menu head.

## Files

- `styles/05-workspace.css`: `.dropdown-item .kbd` and the two others
- `wb-consoles-tab-menus.part.html`, `wb-palette.part.html`

## Keyboard

See [Keyboard](./keyboard.md).
