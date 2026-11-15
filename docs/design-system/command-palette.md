# Command palette

A modal that lists the commands that can run now, each with its keys, and
runs the one picked. Classes `.palette-modal`, `.palette-input`,
`.palette-list`, `.palette-row`.

Layer: component (organism). Made of: [Modal](./modal.md),
[Text field](./text-field.md), menu rows of the
[Dropdown menu](./dropdown.md), [Kbd hints](./kbd.md),
[status text](./status-text.md) (`.empty-note`).

## When to use it

- It is the one place that reaches every page command by its name, also the
  commands with no key (Settings) and the ones whose key is hard to reach
  (Fn+⌥⇧F1 on a Mac).
- A new page command gets its row by being in the list of commands
  (`COMMANDS` in `wb-commands.ts`) with a binding. Do not add a row in the
  markup.
- Use a [Dropdown menu](./dropdown.md) for the actions of one toolbar
  button. Use `/` (the search) to find a file or a project, not the palette.

## Reference markup

From `wb-palette.part.html`:

```html
<div class="modal palette-modal" role="dialog" aria-modal="true" aria-label="Command palette">
  <input class="text-field palette-input" role="combobox" aria-controls="palette-list"
         x-ref="paletteInput" x-model="query" @keydown="paletteKey($event)" />
  <div id="palette-list" class="palette-list" role="listbox" aria-label="Commands">
    <template x-for="(row, i) in rows()" :key="row.id + row.which">
      <button class="dropdown-item palette-row" role="option" tabindex="-1"
              :class="{ active: i === active }" @click="pick(row)">
        <span class="palette-label" x-text="row.label"></span>
        <kbd class="kbd" x-show="row.keys" x-text="keysOf(row)"></kbd>
      </button>
    </template>
  </div>
</div>
```

The rows are menu rows but not inside a `.dropdown`: the text field keeps
the focus, and the active row is `aria-activedescendant`.

## Spacing

Width `min(560px, 96vw)`, 12vh from the top. Padding and gap `--space-5`
around the field and the list; `--space-1` between rows. The list scrolls
inside `max-height: 70vh`. At 390px wide the modal is 374px wide.

## Modes

Same in every mode. A screen with no keyboard has no way to open it yet;
its rows are clicked like menu rows.

## States

| State        | What the code does                                           |
| ------------ | ------------------------------------------------------------ |
| Open         | The field has the focus; the first row is active             |
| Active row   | `.active`: the hover ground of a menu row (`--surface-hi`)   |
| No match     | `.empty-note`: "No command matches."                         |
| Closed       | The focus goes back where it was, a terminal too             |
| Already open | Alt+Shift+P does nothing; the key goes on                    |

## Files

- `wb-palette.ts`, `wb-palette.part.html`
- `wb-commands.ts`: the list, `paletteRows`, `runRow`, `keysText`
- `styles/08-modals.css`: `.palette-*`

## Keyboard

Alt+Shift+P (⌥⇧P) opens it, also from a terminal. Typing filters, Up and
Down move the active row and wrap, Enter runs it, Escape closes. The
palette closes before the command runs, so the command moves the focus
where it goes. See [Keyboard](./keyboard.md).
