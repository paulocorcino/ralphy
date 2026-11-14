# Status text: empty, error and help

Three kinds of small text that tell the operator about the state of a
place. There is no shared class today: 12 classes draw an empty state, 18
an error line, and 4 a help line.

Layer: element (atom). Made of: text, sometimes an inline `code`.

## When to use each

- **Empty state**: a list or a panel has nothing to show. Say what is
  missing, and the act that fills it when there is one: "No consoles yet".
- **Error line**: an act or a read failed. Name the act and the cause, and
  the remedy when there is one (ADR-0065 §6). Show it in the place it
  concerns, not only in a toast.
- **Help line**: a short explanation under a field or a section.
- Use a [Toast](./toast.md) for a short message about an act that is done.
- Use a [Modal](./modal.md) confirm when the operator must answer.

## Reference markup

From `wb-move-dialog.part.html` and `wb-consoles-tab-menus.part.html`:

```html
<p class="move-empty" x-show="!movePick.busy && !movePick.error && movePick.entries.length === 0">…</p>
<p class="move-error" x-show="movePick.error" x-text="movePick.error"></p>
<p class="side-error" x-show="$store.projects.sessionsError()"
   x-text="$store.projects.sessionsError()"></p>
```

From `wb-settings-dialog.part.html`:

```html
<p class="set-help" x-text="…"></p>
```

## Spacing

Measured in Chromium on 2026-11-14. Set by each class: padding from none to 2rem. See the table below.

## Modes

Wraps on a phone; nothing else changes.

Contrast, measured: an empty state or help line passes (5.5:1 on `--chrome`). An error line is 5.01:1 on `--chrome` and 4.52:1 on `--surface` since `--danger` became `#ce8174` (audit, D6).

## What the classes share, and where they differ

| Kind  | Shared                          | Sizes in use                                                    |
| ----- | ------------------------------- | --------------------------------------------------------------- |
| Empty | `--text-muted`                  | `0.72`, `0.74`, `0.78`, `0.8rem`, `0.8em`; 3 centered, 1 italic |
| Error | `--danger`                      | `0.72`, `0.74`, `0.78`, `0.8rem`, `0.78em`; 1 with a border box |
| Help  | `--text-muted`, line height 1.5 | `0.74`, `0.76rem`                                               |

`.side-error` is used in 6 parts, outside the sidebar too. It is the
closest thing to a shared error line today. The [audit](./audit.md)
proposes one class for each kind.

## Classes

- Empty: `.attention-empty`, `.runs-empty`, `.projects-empty`,
  `.side-empty`, `.column-empty`, `.outline-empty`, `.branch-empty`,
  `.kanban-empty`, `.kanban-col-empty`, `.move-empty`, `.ledger-empty`,
  `.note-mermaid-empty`
- Error: `.side-error`, `.runs-error`, `.files-error`, `.chg-error`,
  `.prompt-error`, `.about-error`, `.desk-history-error`, `.devices-error`,
  `.kanban-error`, `.kd-error`, `.runs-verb-error`, `.move-error`,
  `.upload-error`, `.rel-error`, `.note-mermaid-error`, `.host-remove-error`,
  `.branch-error`, `.mermaid-error`
- Help: `.run-help`, `.set-help`, `.sec-note`, `.foot-note` (see
  [Modal](./modal.md))

## Keyboard

See [Keyboard](./keyboard.md). This text takes no keys.
