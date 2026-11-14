# Chip and badge

A small rounded label. A **chip** names a thing and may be clicked. A
**badge** gives a count or a state and is never clicked. There is no
shared class today: ten classes draw this shape.

## When to use it

- Use a chip for a branch, a checkout, a scope or a filter on a list.
- Use a badge for a count (changes, attention) or an on/off state.
- Use a [Button](./button.md) for an act.
- Use a [kbd hint](./kbd.md) for a shortcut.

## Reference markup

A state badge, from `wb-security-dialog.part.html`:

```html
<div class="sec-card-head">
  <span class="sec-card-title"><i class="bi bi-shield-lock"></i> …</span>
  <span class="sec-badge" :class="… ? 'on' : 'off'" x-text="…"></span>
</div>
```

## The classes that exist

All are `999px` round except `.attention-badge` (`8px`).

| Class                | Text                            | Size      | Ground and border                 |
| -------------------- | ------------------------------- | --------- | --------------------------------- |
| `.branch-chip`       | `--text-muted`                  | `0.72rem` | None, border on hover             |
| `.checkout-chip`     | `--text`                        | `0.72rem` | None, `--border`                  |
| `.scope-chip`        | `--text-muted`, `.project` gold | `0.66rem` | `--log-bg`, `--border`, uppercase |
| `.ledger-chip`       | `--console-text`                | `0.72rem` | Gold mix, with a clear button     |
| `.kanban-plan-chip`  | `--text`                        | `0.7rem`  | `--info` mix; `.warn` variant     |
| `.plan-verdict-chip` | `--danger`                      | `0.68rem` | `--danger` mix border             |
| `.sec-badge`         | `--text-muted`, `.on` `--live`  | `0.68rem` | `--log-bg`, `--border`, uppercase |
| `.chg-badge`         | `--text-muted`                  | `0.68rem` | `--surface`, `--border`           |
| `.attention-badge`   | `--bg`                          | `0.64rem` | `#d29922`, a color with no token  |
| `.viewer-disk-badge` | `--console-text`                | inherited | None                              |

Five sizes and four grounds for one shape. The [audit](./audit.md) proposes
one `.chip` with tones.

## States

| State                        | What the code does                                                 |
| ---------------------------- | ------------------------------------------------------------------ |
| Hover                        | `.branch-chip`, `.checkout-chip`, `.kanban-plan-chip`: one step up |
| Disabled                     | `.branch-chip.disabled`: dimmed, no hover                          |
| Focus, empty, error, loading | Not used                                                           |

## Files

- `styles/09-branch.css`, `styles/08-modals.css`, `styles/10-kanban.css`,
  `styles/12-spend.css`, `styles/04-canvas.css`, `styles/02-rail-sidebar.css`

## Keyboard

See [Keyboard](./keyboard.md).
