# Tab

One entry of the tab bar above the canvas. Classes `.tabbar`, `.tabstrip`,
`.tab`. Index 0 is the fixed Consoles tab; the others are open files.

## When to use it

- Use it only in the canvas tab bar, for a file or a console opened in its
  own tab.
- Use a [Segmented control](./segmented-control.md) with `role="tablist"`
  for tabs inside a modal or a panel.

## Reference markup

From `index.html`:

```html
<div class="tab"
     :class="{ active: …, pinned: …, fixed: !t.closable }"
     @click="$store.tabs.activate(t.id)"
     @auxclick.prevent="$event.button === 1 && t.closable && $store.tabs.closeTab(t.id)"
     @contextmenu.prevent="$store.tabs.showTabMenu($event.clientX, $event.clientY, t)"
     :title="t.path || t.title">
  <i class="tab-ic" :class="t.icon"></i>
  <span class="tab-title" x-text="t.title"></span>
  <i class="tab-pin bi bi-pin-angle-fill" x-show="…"></i>
  <button class="tab-close" x-show="t.closable"
          @click.stop="$store.tabs.closeTab(t.id)" title="Close">
    <i class="bi bi-x"></i>
  </button>
</div>
```

## Variants and states

| State                           | What the code does                                                              |
| ------------------------------- | ------------------------------------------------------------------------------- |
| Default                         | `--chrome`, `--text-muted`, `0.8rem`, at most 220px, title cut with an ellipsis |
| Hover                           | `--surface`                                                                     |
| Active                          | `--bg` ground, `--text`, a 2px gold line at the top                             |
| Pinned                          | A gold pin icon and a 2px gold line at the bottom                               |
| Fixed                           | No close button                                                                 |
| Focus                           | None: the tab is a `div` with no `tabindex`                                     |
| Disabled, empty, error, loading | Not used                                                                        |

A middle click closes a closable tab. A right click opens the
[Context menu](./context-menu.md).

## Files

- `styles/05-workspace.css`: `.tab` and its parts
- `index.html`: the tab bar markup
- `wb-tabs-store.ts`, `wb-tabs-store-menu.ts`: open, close, the menu

## Keyboard

See [Keyboard](./keyboard.md).
