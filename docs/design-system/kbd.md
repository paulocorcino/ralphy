# Kbd hint

A small box that shows a keyboard shortcut at the right of a menu row.
Element `<kbd class="kbd">`.

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

## States

| State   | What the code does                                                            |
| ------- | ----------------------------------------------------------------------------- |
| Default | `0.66rem`, `--text-muted`, `--log-bg`, `--border`, radius `4px`, pushed right |
| Others  | Not used: the row carries hover and disabled                                  |

The style applies only inside `.dropdown-item`, `.fence-item` and
`.console-choice .row-run`. A `.kbd` elsewhere is not styled.

## Files

- `styles/05-workspace.css`: `.dropdown-item .kbd` and the two others
- `wb-consoles-tab-menus.part.html`

## Keyboard

See [Keyboard](./keyboard.md).
