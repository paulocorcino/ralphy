# Segmented control

A row of two to four pill buttons. One of them is on. Classes `.seg` and
`.seg-btn`.

Layer: component (molecule). Made of: buttons (`.seg-btn`).

## When to use it

- Use it to choose one value of a small set, when every value fits on
  screen: the search mode, the tabs of the Hosts dialog, the help tabs.
- Use a native `<select>` (see [Text field](./text-field.md)) when the set
  is long or does not fit.
- Use a [Dropdown menu](./dropdown.md) for actions, not values.
- Use [Button](./button.md) for an action that changes nothing on its own.

## Reference markup

From `wb-hosts-dialog.part.html`:

```html
<div class="seg host-tabs" role="tablist" x-show="sshHosts().length">
  <button class="seg-btn" role="tab" :class="{ on: hostTab() === 'hosts' }"
          :aria-selected="hostTab() === 'hosts'"
          @click="addHostTab('hosts')" x-text="…"></button>
  <button class="seg-btn" role="tab" :class="{ on: hostTab() === 'add' }"
          :aria-selected="hostTab() === 'add'"
          @click="addHostTab('add')" x-text="…"></button>
</div>
```

The on segment carries the class `on`. When the control switches tabs, it
has `role="tablist"` and each segment `role="tab"` with `aria-selected`.
When it chooses a value, it has `role="group"` and an `aria-label`.

## Spacing

Measured in Chromium on 2026-11-14. Track: padding 2px, radius 999px. Segment: padding 3.52px 12px, from the CSS (`0.22rem 0.75rem`). Not measured in the browser: no segmented control was on screen in the measured views.

## Modes

About 22px high, from the CSS: under the 44px touch size.

## Variants

| Variant | Markup                                              | Where        |
| ------- | --------------------------------------------------- | ------------ |
| Default | `.seg` with `.seg-btn` children                     | Hosts dialog |
| Sidebar | `.files-search .seg` with plain `<button>` children | Files search |

The sidebar variant has its own rules in `styles/02-rail-sidebar.css`. It
is a second copy of the same control, found in the audit.

## States

| State    | What the code does                                                                |
| -------- | --------------------------------------------------------------------------------- |
| Default  | Track: `--log-bg`, `--border`, radius `999px`. Segment: `0.78rem`, `--text-muted` |
| Hover    | Segment text `--text`                                                             |
| On       | Gold: `--console-text` on `--console-bg`, inset `--console-border` ring           |
| Focus    | No style. `all: unset` removes the browser outline. Not measured in the browser   |
| Disabled | `.seg-btn.unavailable`: `opacity: 0.45` and a line through the text               |
| Empty    | Hidden: the Hosts tabs show only when there is a host                             |
| Error    | Not used                                                                          |
| Loading  | Not used                                                                          |

## Files

- `styles/03-runs.css`: `.seg`, `.seg-btn`
- `styles/02-rail-sidebar.css`: `.files-search .seg`
- `wb-hosts-dialog.part.html`, `wb-files.part.html`, `wb-runs-panel.part.html`

## Keyboard

See [Keyboard](./keyboard.md).
