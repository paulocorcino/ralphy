# Help screen

A modal that shows the keyboard shortcuts: the page commands, the keys
inside the components, and one line that says every other key in a
terminal goes to the program. Classes `.help-modal`, `.help-body`,
`.help-section`, `.help-line`.

Layer: component (organism). Made of: [Modal](./modal.md),
[Kbd hints](./kbd.md), [status text](./status-text.md) (`.help-text`).

## When to use it

- It is the one screen that lists the shortcuts for a person. Its page
  lines come from the list of commands (`COMMANDS` in `wb-commands.ts`):
  a new command with keys gets its line with no change here.
- A new key inside a component gets its line in `componentKeys`
  (`wb-help.ts`), in the same change as its code.
- It only shows text. To run a command, use the
  [Command palette](./command-palette.md).

## Reference markup

From `wb-help.part.html`:

```html
<div class="modal help-modal" role="dialog" aria-modal="true" aria-label="Keyboard shortcuts">
  <div class="modal-head">…</div>
  <div class="help-body">
    <template x-for="section in sections" :key="section.title">
      <section class="help-section">
        <h3 class="help-title" x-text="section.title"></h3>
        <template x-for="line in section.lines" :key="line.label">
          <div class="help-line">
            <span class="help-label" x-text="line.label"></span>
            <kbd class="kbd" x-text="line.keys"></kbd>
          </div>
        </template>
      </section>
    </template>
    <p class="help-text">In a terminal, every other key goes to the program.</p>
  </div>
</div>
```

## Spacing

Width `min(640px, 96vw)`. Body padding and the gap between sections
`--space-8`; `--space-3` between lines; `--space-5` between a label and its
keys. At 390px wide the modal is 374px wide, and a long label wraps.

## Modes

Same in every mode. On a Mac the keys read `⌥⇧`, `⌘`, and a last line says
to hold the fn key for F1 to F12. With no keyboard it opens from the account menu
("Keyboard shortcuts").

## States

| State        | What the code does                                   |
| ------------ | ---------------------------------------------------- |
| Open         | The close button has the focus; the body scrolls     |
| Closed       | The focus goes back where it was, a terminal too     |
| Already open | `?` and Alt+Shift+/ do nothing more; the key goes on |

## Files

- `wb-help.ts`, `wb-help.part.html`
- `wb-commands.ts`: the list and `commandKeysText`
- `wb-account.part.html`: the "Keyboard shortcuts" row
- `styles/08-modals.css`: `.help-*`

## Keyboard

`?` opens it while the focus is outside a field, an editor and a terminal.
Alt+Shift+/ (⌥⇧/) opens it from anywhere, a terminal too. Escape closes it.
See [Keyboard](./keyboard.md).
