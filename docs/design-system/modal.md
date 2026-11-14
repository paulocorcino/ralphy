# Modal

A dialog over a dimmed page. It has a head, a body and a foot. Classes
`.modal-scrim`, `.modal`, `.modal-head`, `.modal-foot`.

## When to use it

- Use it for a task that needs the whole attention of the operator:
  settings, security, adding a host, confirming a delete.
- Use the confirm dialog (`askConfirm()`) for a yes or no question. Do not
  use the browser's `window.confirm`.
- Use the prompt dialog (`askPrompt()`) to ask for one line of text.
- Use a [Dropdown menu](./dropdown.md) for a short list of choices that does
  not need a form.

## Reference markup

From `wb-confirm-dialog.part.html` (bindings shortened):

```html
<div class="modal-scrim" x-cloak
     x-bind="$store.modals.scrim('$store.modals.confirmModal.open',
             () => $store.modals.confirmRespond(false))">
  <div class="modal confirm-modal" role="alertdialog" aria-modal="true"
       :aria-label="$store.modals.confirmModal.title">
    <div class="modal-head">
      <i class="bi bi-question-circle"></i>
      <span class="modal-title" x-text="$store.modals.confirmModal.title"></span>
      <span class="spacer"></span>
      <button class="modal-x" @click="$store.modals.confirmRespond(false)"
              title="Close"><i class="bi bi-x-lg"></i></button>
    </div>
    <p class="confirm-body" x-text="$store.modals.confirmModal.message"></p>
    <div class="modal-foot">
      <button class="btn" …>Cancel</button>
      <button class="btn accent" …>Confirm</button>
    </div>
  </div>
</div>
```

Every scrim takes its behavior from `$store.modals.scrim(openFlag, close)`
in `wb-modals-store.ts`. A click on the scrim closes nothing, so a stray
click does not lose what the modal holds. Only its own buttons and Escape
close it.

## Parts

| Part           | Look                                                                                                      |
| -------------- | --------------------------------------------------------------------------------------------------------- |
| `.modal-scrim` | Fixed, `z-index: 500`, `rgba(8,6,5,0.62)`, 2px blur                                                       |
| `.modal`       | `--chrome`, `--border`, radius `10px`, width `min(880px, 96vw)`                                           |
| `.modal-head`  | `--surface`, icon in `--console-text`, `.modal-title` weight 600, optional `.modal-sub` in `--text-muted` |
| `.modal-x`     | 24px square, `--text-muted`; hover `--surface-hi`                                                         |
| `.modal-foot`  | `--surface`; optional `.foot-note` in `--text-muted`                                                      |

## Variants

The width is set by a second class: `confirm-modal`, `prompt-modal`,
`security-modal` (`560px`), `run-modal`, `host-modal`, `plan-modal`. The
confirm and prompt scrims use `z-index: 520`, so they show above a modal
that opened them. A danger confirm shows `bi-exclamation-triangle` in
`--danger` in its head and a `.btn.danger` in its foot.

## States

| State    | What the code does                                                                          |
| -------- | ------------------------------------------------------------------------------------------- |
| Default  | Shown when the open flag named in `scrim()` is true                                         |
| Focus    | `scrim()` moves focus into the modal and back to the opener on close (`wb-modals-store.ts`) |
| Disabled | Not used on the modal. Its foot buttons are disabled instead                                |
| Empty    | Per modal: for example `.move-empty` in `wb-move-dialog.part.html`                          |
| Error    | Per modal: a `.side-error` or `.move-error` line in the body                                |
| Loading  | Per modal: a busy flag disables the primary button                                          |

## Files

- `styles/08-modals.css`: scaffold, `confirm-modal`, `prompt-modal`
- `styles/03-runs.css`: `run-modal`, `host-modal`
- `wb-modals-store.ts`: `scrim()`, `askConfirm()`, `askPrompt()`
- `wb-confirm-dialog.part.html`, `wb-prompt-dialog.part.html`, and 11 more
  `*-dialog.part.html` and panel parts

## Keyboard

See [Keyboard](./keyboard.md).
