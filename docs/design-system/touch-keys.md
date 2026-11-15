# Touch key strip

A row of large keys under a console, for keys a phone or tablet keyboard
does not have: the keyboard toggle, Esc, Shift, Tab, Enter, `/` and more.
Classes `.session-keys`, `.session-key`.

Layer: component (molecule). Made of: buttons with an [icon](./icon.md) or
a short text.

## When to use it

- It is shown on a console window that has the class `keys`. Code sets it
  from the key bar setting and from touch input (`keyBarVisible` in
  `wb-console-keys.ts`).
- Use it only for keys that the terminal needs and the touch keyboard
  lacks. An act on the window goes in the title bar, not here.
- Use a [Button](./button.md) for an act in the rest of the page. A
  button is not sized for a finger; this key is.

## Reference markup

Built in `wb-console-keys.ts`:

```ts
bar.className = "session-keys";
b.className = "session-key";
i.className = "bi " + icon; // a key drawn as a glyph
```

## Spacing

At least 44px wide and 40px high: the touch target size in Apple's
guidelines. Padding 0 6.4px, radius 5px, `0.78rem` (12.48px) text. Up to
430px wide, a key is at least 40px wide with padding 0 4.8px, so the first
eight keys fit a 393px screen.

## States

| State                 | What the code does                                                          |
| --------------------- | --------------------------------------------------------------------------- |
| Default               | `--surface-hi`, `--border`, text `--text-muted`                             |
| Hover                 | Text `--text`, border `--border-focus`                                      |
| Pressed               | `:active`: ground `--surface`                                               |
| Latched               | `aria-pressed="true"`: text and border `--console-text`, until the next key |
| Disabled              | `opacity: 0.4`                                                              |
| Focus                 | Not in the tab order, on purpose: a keyboard user has these keys            |
| Empty, error, loading | Not used                                                                    |

`touch-action: manipulation` removes the delay before a tap counts.

## Files

- `styles/06-consoles.css`: `.session-keys`, `.session-key`
- `wb-console-keys.ts`

## Keyboard

See [Keyboard](./keyboard.md). This strip is for touch; it takes no keys.
