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

## The shared rules

| Shape    | Padding     | Size                 | Older classes in the list                                             |
| -------- | ----------- | -------------------- | --------------------------------------------------------------------- |
| `.chip`  | 1.6px 7.2px | `--fs-xs` (11.52px)  | `.branch-chip`, `.checkout-chip`, `.ledger-chip`, `.kanban-plan-chip` |
| `.badge` | 0.8px 7.2px | `--fs-2xs` (10.56px) | `.scope-chip`, `.sec-badge`, `.chg-badge`, `.plan-verdict-chip`       |

The older classes keep their own colors and states. New markup writes
`class="chip tone-gold"` or `class="badge tone-live"`.

The attention counter on the rail (`.attention-badge`) is not a badge:
it is a 16px count bubble on an icon, with a fixed height. It uses
`--warn`.

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
