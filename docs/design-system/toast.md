# Toast and flash line

Two short messages that go away by themselves. The **toast** floats at the
bottom of the page and may carry one act, such as Undo. The **flash line**
is one line of text in the Runs panel.

Layer: component (molecule). Made of: text, an optional action button, a close button.

## When to use it

- Use a toast after an act that can be undone: one line, one verb.
- Use the flash line for a short fact about an act that is done.
- Use an error line (see [Status text](./status-text.md)) for a failure
  that the operator must read: a toast or a flash goes away in seconds.
- Use a [Modal](./modal.md) confirm when the operator must answer.

## How to show one

Code outside `shell()` uses the door for operator messages in
`wb-messages.ts`: `toast()`, `dismissToast()`, `flash()`. Do not build the
element yourself. `wb-messages.ts` builds the toast:

```ts
el.className = "wb-toast";
line.className = "wb-toast-text";
btn.className = "wb-toast-action";
```

## Spacing

Measured in Chromium on 2026-11-14. Padding 7.2px 9.6px, gap 9.6px, 40px high, radius 6px.

## Modes

Same in every mode.

## States

| State   | What the code does                                                                                                 |
| ------- | ------------------------------------------------------------------------------------------------------------------ |
| Default | Fixed at the bottom center, `z-index: 3000`, `--surface-hi`, `--border-focus`, radius `6px`, `--font-ui` `0.85rem` |
| Action  | `.wb-toast-action`: outlined, `--text-strong`; hover `--surface`                                                   |
| Close   | `.wb-toast-close`: `--text-muted`; hover `--text`                                                                  |
| Focus   | The browser outline: the buttons do not use `all: unset`                                                           |
| Others  | Not used                                                                                                           |

The toast is the only chrome element in `--font-ui`; the rest of the
chrome is monospace.

## Files

- `styles/13-notes.css`: `.wb-toast` and its parts
- `wb-messages.ts`: `toast()`, `flash()`
- `wb-modals-store.ts`: the flash line state

## Keyboard

See [Keyboard](./keyboard.md).
