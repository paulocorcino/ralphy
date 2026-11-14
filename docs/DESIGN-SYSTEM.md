# Workbench design system

This file lists the tokens and the components of the workbench page
(`crates/ralphy-daemon/assets/ui/`). It documents what the code does today.
The source of the values is `styles/01-base.css`. The source of their meaning
is [ADR-0035](./adr/0035-daemon-ui-visual-language.md). The written voice is
[ADR-0065](./adr/0065-the-workbench-written-voice.md). How a page is built
from parts is [ADR-0080](./adr/0080-the-workbench-pages-are-built-from-parts.md).

## The rule

No new component, color, spacing value or keyboard shortcut goes into the
workbench without an entry here. Add the entry in the same change as the
code. A new color or accent is also an amendment to ADR-0035.

## Tokens

Every token is a CSS custom property on `:root` in `styles/01-base.css`.
Use the token, never its hex value. Monaco cannot read CSS variables, so
`wb-monaco.ts` repeats some hex values; change both together.

### Neutrals

| Token            | Value     | Use                                       |
| ---------------- | --------- | ----------------------------------------- |
| `--bg`           | `#24201c` | Page ground                               |
| `--chrome`       | `#2b2620` | Rail, sidebar, modal body                 |
| `--surface`      | `#342d27` | Buttons, modal head and foot, menus       |
| `--surface-hi`   | `#423a31` | Hover: one step up from `--surface`       |
| `--log-bg`       | `#2a2521` | Command log, text fields, segmented track |
| `--border`       | `#4c4239` | Default hairline                          |
| `--border-focus` | `#7a6d5f` | Focused window and focused field border   |
| `--text`         | `#d4ccc0` | Body text                                 |
| `--text-strong`  | `#e9e1d4` | Headings and `strong`                     |
| `--text-muted`   | `#a49c91` | Paths, status, labels, help text          |

### Accents

| Token              | Value     | Use                                         |
| ------------------ | --------- | ------------------------------------------- |
| `--console-text`   | `#e8d9a8` | Gold: the free console, the primary action  |
| `--console-bg`     | `#2b2410` | Gold ground                                 |
| `--console-border` | `#5a4a1e` | Gold border                                 |
| `--danger`         | `#c56b5c` | Offline, errors, destructive actions        |
| `--live`           | `#7fae7f` | A live daemon or console, a passed check    |
| `--info`           | `#6f9fc0` | Planning and sleep states in the Runs trail |

### Note tones

Six tones for note cards, and two inks for a solid card: `--note-ochre`,
`--note-sage`, `--note-rose`, `--note-slate`, `--note-plum`, `--note-sand`,
`--note-ink-light`, `--note-ink-dark`. Use them only on a note card.

### Type

| Token               | Value                             | Use                 |
| ------------------- | --------------------------------- | ------------------- |
| `--font-mono`       | `ui-monospace, SFMono-Regular, …` | All chrome (13px)   |
| `--font-ui`         | `ui-sans-serif, system-ui, …`     | Rendered prose only |
| `--reading-size`    | `14px`                            | Prose body size     |
| `--reading-measure` | `100ch`                           | Prose line length   |
| `--reading-leading` | `1.7`                             | Prose line height   |

The chrome sizes are `rem` values on a 13px body, not tokens. Common sizes:
`0.72rem` (menu heads, foot notes), `0.78rem` (segments, small text),
`0.8rem` (buttons), `0.82rem` (fields, menu rows), `0.85rem` (labels).

### Shape and layout

| Token           | Value     | Use                                            |
| --------------- | --------- | ---------------------------------------------- |
| `--radius`      | `4px`     | Buttons, fields, menu rows                     |
| `--rail-w`      | `48px`    | Rail width                                     |
| `--side-w`      | `300px`   | Sidebar width                                  |
| `--side-gutter` | `0.55rem` | One left edge for the whole sidebar column     |
| `--row-h`       | `33px`    | Project row and console title bar height       |
| `--runs-w`      | `400px`   | Runs panel width                               |
| `--kb-inset`    | `0px`     | Height of the on-screen keyboard (set by code) |

Radii that are literal values, not tokens: `6px` (menus, session windows),
`8px` (cards), `10px` (modals), `999px` (segmented control).

Shadows: `0 10px 30px rgba(0,0,0,0.55)` on menus and floating windows,
`0 24px 70px rgba(0,0,0,0.6)` on modals. A flat element has no shadow.

## Components

| Component                                                 | Class                       | Used in  |
| --------------------------------------------------------- | --------------------------- | -------- |
| [Button](./design-system/button.md)                       | `.btn`                      | 21 parts |
| [Modal](./design-system/modal.md)                         | `.modal`                    | 13 parts |
| [Dropdown menu](./design-system/dropdown.md)              | `.dropdown`                 | 7 parts  |
| [Text field](./design-system/text-field.md)               | `.set-input`, `.host-input` | 6 parts  |
| [Segmented control](./design-system/segmented-control.md) | `.seg`                      | 3 parts  |

The count is the number of `*.part.html` files and pages that use the
class, measured on 2026-11-14 with `grep -l`.

## Differences between ADR-0035 and the code

Found during this audit. They are listed, not fixed.

- ADR-0035 lists `--danger-border` and `--console-bg-hi`. `01-base.css` does
  not define them. `.btn.danger` mixes its border with `color-mix` instead.
- `--live`, `--info`, `--font-ui` and the note tones are in the code but not
  in ADR-0035.
- Two text field classes with different grounds: `.set-input` on `--log-bg`
  and `.host-input` on `--bg`. See [Text field](./design-system/text-field.md).
- `.btn` has no focus style of its own: the browser outline shows focus.
  `.seg-btn`, `.dropdown-item` and `.modal-x` start with `all: unset`, which
  also removes the browser outline, and they define no `:focus-visible`
  style. Read from the CSS; not checked in a browser.
