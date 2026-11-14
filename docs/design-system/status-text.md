# Status text: empty, error and help

Three kinds of small text that tell the operator about the state of a
place. Each kind has one shared class: `.empty-note`, `.error-line` and
`.help-text`, in `styles/01-base.css`. New markup uses the shared class.
The older classes join the shared rule through its selector list and keep
only their own padding and alignment.

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

## The shared rules

| Class         | Color          | Size                | Line height |
| ------------- | -------------- | ------------------- | ----------- |
| `.empty-note` | `--text-muted` | `--fs-sm` (12.48px) | inherited   |
| `.error-line` | `--danger`     | `--fs-sm` (12.48px) | inherited   |
| `.help-text`  | `--text-muted` | `--fs-sm` (12.48px) | 1.5         |

Use a domain class next to the shared one when code queries it, for
example `class="error-line move-error"`. Two texts stay outside on
purpose: the Spend ledger's notes (`.ledger-blank`, `.ledger-empty`,
`.ledger-note`, one rule of their own) and the empty or failed diagram in
a note (`.note-mermaid-*`, sized in `em` inside the note text).

## Classes in the shared lists

- Empty: `.attention-empty`, `.runs-empty`, `.side-empty`,
  `.outline-empty`, `.branch-empty`, `.kanban-empty`, `.kanban-col-empty`,
  `.move-empty`
- Error: `.side-error`, `.runs-error`, `.files-error`, `.chg-error`,
  `.prompt-error`, `.about-error`, `.desk-history-error`, `.devices-error`,
  `.kanban-error`, `.kd-error`, `.runs-verb-error`, `.move-error`,
  `.upload-error`, `.rel-error`
- Help: `.run-help`, `.set-help`, `.sec-note`

## Keyboard

See [Keyboard](./keyboard.md). This text takes no keys.
