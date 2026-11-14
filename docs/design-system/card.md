# Card

A bordered box that groups one setting or one fact with its title, a
state badge and its controls. Classes `.sec-card`, `.sec-card-head`,
`.sec-card-title`.

## When to use it

- Use it inside a modal body, for one item among a few that each have a
  title, a state and an act: the protections of the Security dialog.
- Use a settings row (`.set-row`, see [Text field](./text-field.md)) for a
  long list of plain values.
- A note card on the stage is not this component. It has its own tones
  (`--note-*`) and its own rules (ADR-0064).

## Reference markup

From `wb-security-dialog.part.html`:

```html
<div class="sec-card">
  <div class="sec-card-head">
    <span class="sec-card-title"><i class="bi bi-shield-lock"></i> …</span>
    <span class="sec-badge" :class="… ? 'on' : 'off'" x-text="…"></span>
    <label class="switch">…</label>
  </div>
  <p class="sec-note">…</p>
  <div class="sec-actions">…</div>
</div>
```

## States

| State                                         | What the code does                                                                            |
| --------------------------------------------- | --------------------------------------------------------------------------------------------- |
| Default                                       | `--surface`, `--border`, radius `8px`, padding `0.8rem 0.9rem`                                |
| Title                                         | weight 600, `0.9rem`, icon in `--console-text`                                                |
| On, off                                       | Shown by the [badge](./chip-badge.md) and the [switch](./checkbox-switch.md), not by the card |
| Hover, focus, disabled, empty, error, loading | Not used                                                                                      |

## Files

- `styles/08-modals.css`: `.sec-card` and its parts
- `wb-security-dialog.part.html`

## Keyboard

See [Keyboard](./keyboard.md).
