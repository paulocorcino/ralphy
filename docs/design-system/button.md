# Button

A text button, with an optional icon before the text. Class `.btn`.

## When to use it

- Use it for an action in a modal foot, a toolbar or a row: Cancel, Save,
  Restore, Go to.
- Use a [Dropdown menu](./dropdown.md) item for an action inside a menu.
- Use a [Segmented control](./segmented-control.md) to choose one value of
  a small set.
- Use `.modal-x` (see [Modal](./modal.md)) to close a modal.

## Reference markup

From `wb-confirm-dialog.part.html` and `wb-settings-dialog.part.html`:

```html
<button class="btn" @click="$store.modals.confirmRespond(false)"
        x-text="$store.modals.confirmModal.cancelLabel"></button>
<button class="btn" @click="downloadDeskVersion(row)"
        title="Save this layout as a file on this device">
  <i class="bi bi-download"></i> Download
</button>
```

## Variants

| Variant | Markup               | Look                              | Use                                |
| ------- | -------------------- | --------------------------------- | ---------------------------------- |
| Default | `class="btn"`        | `--surface`, `--border`, `--text` | Every other action                 |
| Primary | `class="btn accent"` | Gold: `--console-*` tokens        | The one main action of a modal     |
| Danger  | `class="btn danger"` | `--danger` text and border mix    | An action that deletes or discards |

A modal foot has at most one `accent` or `danger` button. The confirm
dialog picks one of the two with
`:class="$store.modals.confirmModal.danger ? 'danger' : 'accent'"`.

## States

| State    | What the code does                                                                                                                |
| -------- | --------------------------------------------------------------------------------------------------------------------------------- |
| Default  | `--surface` ground, `0.8rem` text, `0.3rem 0.7rem` padding                                                                        |
| Hover    | `--surface-hi` ground. `danger`: 14% `--danger` mix                                                                               |
| Focus    | No style of its own. The browser outline shows focus                                                                              |
| Disabled | `opacity: 0.5`, `cursor: default`. Bound with `:disabled="…"`                                                                     |
| Empty    | Not used: a button always has a label                                                                                             |
| Error    | Not used: the error shows in the form, not on the button                                                                          |
| Loading  | No spinner. The button is disabled while a request runs, for example `:disabled="removeHost.busy"` in `wb-hosts-dialog.part.html` |

## Files

- `styles/04-canvas.css`: `.btn`, `.btn:hover`, `.btn.accent`
- `styles/08-modals.css`: `.btn.danger`, `.btn:disabled`
- Used in 21 `*.part.html` files and pages; for example
  `wb-confirm-dialog.part.html`, `wb-move-dialog.part.html`,
  `wb-hosts-dialog.part.html`

## Keyboard

See [Keyboard](./keyboard.md).
