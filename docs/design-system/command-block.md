# Command block

A command that the operator copies and runs in their own terminal, with a
copy button. Classes `.rel-cmd-row`, `.rel-cmd`.

Layer: component (molecule). Made of: a `code`, a `.modal-x` copy button.

## When to use it

- Use it for a command the operator must run by hand: an update, the SSH
  setup on a host.
- Use inline `<code>` inside a help line for a short name or path that is
  not run.
- Use a console when Ralphy can run the command itself.

## Reference markup

From `wb-hosts-dialog.part.html`:

```html
<span class="rel-cmd" x-show="st.command">
  <code x-text="st.command"></code>
  <button class="modal-x" @click="copyHostCommand(…)" title="Copy the command"
          aria-label="Copy the command"><i class="bi bi-copy"></i></button>
</span>
```

## Spacing

Measured in Chromium on 2026-11-14. Padding 2px 2px 2px 10px, gap 6px, radius 4px, from the CSS. Not measured in the browser: it shows only when a host needs a manual step.

## Modes

Not measured.

## States

| State   | What the code does                                 |
| ------- | -------------------------------------------------- |
| Default | `--bg`, `--border`, radius `4px`, `12px` text      |
| Copy    | A `.modal-x` icon button (see [Modal](./modal.md)) |

This block uses `px` for its size and spacing; the rest of the chrome uses
`rem`.

## Files

- `styles/12-spend.css`: `.rel-cmd-row`, `.rel-cmd`
- `wb-release-dialogs.part.html`, `wb-hosts-dialog.part.html`

## Keyboard

See [Keyboard](./keyboard.md).
