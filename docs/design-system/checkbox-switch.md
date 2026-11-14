# Checkbox and switch

Two controls for a yes or no value. A checkbox is a plain
`<input type="checkbox">`. A switch is `.switch` around a hidden checkbox.

Layer: element (atom). Made of: no smaller part.

## When to use it

- Use a checkbox in a list or a form where the value is saved with a
  button, for example a row to include.
- Use a switch when the change takes effect at once and turns a protection
  or a feature on or off: the cards of the Security dialog.
- Use a [Segmented control](./segmented-control.md) when there are more
  than two values, or when the two values need names.

## Reference markup

Switch, from `wb-security-dialog.part.html`:

```html
<label class="switch">
  <input type="checkbox" :checked="…" @change="…">
  <span class="switch-track"></span>
</label>
```

Checkbox: a plain `<input type="checkbox">`. `01-base.css` draws every
checkbox that is not inside `.switch`. Do not add a class to it.

## Spacing

Measured in Chromium on 2026-11-14. Checkbox 16 × 16px, radius 3px. Switch 38 × 20px, knob 14px.

## Modes

16px in every mode. The same size on touch, on purpose (audit, D7).

## States

| State                 | Checkbox                                      | Switch                                           |
| --------------------- | --------------------------------------------- | ------------------------------------------------ |
| Default               | 16px box, `--log-bg`, `--border-focus` border | 38 × 20px track, `--log-bg`, knob `--text-muted` |
| Checked               | Gold box and gold tick                        | Gold track, knob `--console-text`, moved 18px    |
| Focus                 | 1px `--focus-ring` outline, offset 2px        | The same ring on the track                       |
| Disabled              | `opacity: 0.5`                                | Track at `opacity: 0.5`, cursor `not-allowed`    |
| Empty, error, loading | Not used                                      | Not used                                         |

## Files

- `styles/01-base.css`: the checkbox
- `styles/08-modals.css`: `.switch`, `.switch-track`
- `wb-security-dialog.part.html`: the only part that uses `.switch`

## Keyboard

See [Keyboard](./keyboard.md). Space toggles both, from the browser.
