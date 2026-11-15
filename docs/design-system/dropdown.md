# Dropdown menu

A list of actions that opens under a button. Classes `.menu-wrap`,
`.dropdown`, `.dropdown-head`, `.dropdown-item`.

Layer: component (molecule). Made of: menu rows, [icons](./icon.md), [kbd hints](./kbd.md), [status text](./status-text.md).

## When to use it

- Use it for a list of actions or targets behind one toolbar button: the
  Consoles menu, the Go to menu, the fence and note pickers.
- Use a [Segmented control](./segmented-control.md) when the choice is one
  value of two to four, and all of them fit on screen.
- Use a [Modal](./modal.md) when the choice needs a form or an explanation.
- Use a native `<select>` (see [Text field](./text-field.md)) for a value
  in a form.

## Reference markup

From `wb-consoles-tab-menus.part.html`:

```html
<div class="menu-wrap">
  <button class="btn" @click.stop="toggleWindowMenu()" title="Show a console">
    <i class="bi bi-crosshair"></i> Go to
  </button>
  <div class="dropdown window-menu" x-show="windowMenu"
       @click.outside="windowMenu = false" x-cloak>
    <div class="dropdown-head"
         x-text="windowList.length ? 'Consoles on the stage' : 'No consoles yet'"></div>
    <template x-for="w in windowList" :key="w.id">
      <button class="window-item" @click="revealWindow(w.id)">…</button>
    </template>
  </div>
</div>
```

A row with a shortcut puts it last, in `<kbd class="kbd">`.

## Spacing

Measured in Chromium on 2026-11-14. Menu: padding 4px, radius 6px. Head: padding 4.8px 8px, 25px high. Row: padding 5.6px 8.8px, gap 8px, 27px high.

## Modes

Rows are 27px high in every mode.

## Variants

The row grammar is one rule with six class names: `.dropdown-item`,
`.window-item`, `.fence-item`, `.note-item`, `.column-item`,
`.console-tab-item`. They share one CSS rule in `styles/05-workspace.css`.
A new menu uses `.menu-item`. The context menu row (`.ctx-item`) is in the
same rule.

## States

| State    | What the code does                                                                                    |
| -------- | ----------------------------------------------------------------------------------------------------- |
| Default  | `.dropdown`: `--surface`, `--border`, radius `6px`, shadow, `z-index: 1000`. Row: `0.82rem`, `--text` |
| Hover    | Row ground `--surface-hi`                                                                             |
| Focus    | The focus ring, drawn inside the row (offset -2px)                                                    |
| Disabled | `opacity: 0.4`, `cursor: not-allowed`, no hover ground                                                |
| Empty    | The `.dropdown-head` says so: "No consoles yet"                                                       |
| Error    | A `.side-error` line under the head (`--danger`, `0.78rem`)                                           |
| Loading  | Not used: the menu reads data the page already has                                                    |

## Files

- `styles/05-workspace.css`: `.menu-wrap`, `.dropdown`, `.dropdown-head`,
  the row rule
- `wb-consoles-tab-menus.part.html`, `wb-consoles-tab-menus.ts`
- Used in 7 `*.part.html` files and pages

## Keyboard

Up and Down move between the enabled rows, Home and End go to the ends,
and Escape closes the menu and gives the focus back. See
[Keyboard](./keyboard.md).
