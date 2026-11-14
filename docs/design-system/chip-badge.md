# Chip and badge

A small rounded label. A **chip** names a thing and may be clicked. A
**badge** gives a count or a state and is never clicked. There is no
shared class today: ten classes draw this shape.

Layer: element (atom). Made of: text, sometimes an [icon](./icon.md).

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

## Spacing

Measured in Chromium on 2026-11-14. State badge: padding 0.8px 8px, 17px high, 10.88px text. Branch chip: padding 1.28px 6.4px, 19px high.

## Modes

Same in every mode. The scope chip in Settings stays on one line on a phone (18px high).

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

| State                 | What the code does                                                 |
| --------------------- | ------------------------------------------------------------------ |
| Hover                 | `.branch-chip`, `.checkout-chip`, `.kanban-plan-chip`: one step up |
| Focus                 | A chip that is a button (`.branch-chip`) shows the focus ring      |
| Disabled              | `.branch-chip.disabled`: dimmed, no hover                          |
| Empty, error, loading | Not used                                                           |

## Files

- `styles/09-branch.css`, `styles/08-modals.css`, `styles/10-kanban.css`,
  `styles/12-spend.css`, `styles/04-canvas.css`, `styles/02-rail-sidebar.css`

## Keyboard

See [Keyboard](./keyboard.md).
