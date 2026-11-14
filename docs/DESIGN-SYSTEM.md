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
| `--font-mono`       | `ui-monospace, SFMono-Regular, …` | All chrome          |
| `--font-ui`         | `ui-sans-serif, system-ui, …`     | Rendered prose only |
| `--reading-size`    | `14px`                            | Prose body size     |
| `--reading-measure` | `100ch`                           | Prose line length   |
| `--reading-leading` | `1.7`                             | Prose line height   |

The body is 13px, but `rem` is relative to the root, which is 16px. So
`0.8rem` is 12.8px, a little smaller than the body. The chrome sizes are
`rem` values, not tokens. Common sizes, measured in Chromium:

| Value     | Pixels  | Used for                                  |
| --------- | ------- | ----------------------------------------- |
| `0.66rem` | 10.56px | Kbd hint, scope chip                      |
| `0.68rem` | 10.88px | State badge, change count                 |
| `0.72rem` | 11.52px | Menu head, branch chip, foot note         |
| `0.76rem` | 12.16px | Help line under a setting                 |
| `0.78rem` | 12.48px | Segments, empty state, error line         |
| `0.8rem`  | 12.8px  | Button, tab, context menu row, host field |
| `0.82rem` | 13.12px | Menu row, settings field                  |
| `13px`    | 13px    | Body, modal title                         |
| `0.85rem` | 13.6px  | Settings label, toast                     |
| `0.9rem`  | 14.4px  | Card title                                |

## Spacing

Spacing values are `rem` literals, not tokens. Measured in Chromium:

| Place                 | Padding (top, sides)      | Gap    | Height |
| --------------------- | ------------------------- | ------ | ------ |
| Button in a modal     | 4.8px, 11.2px             | 5.6px  | 27px   |
| Button in a toolbar   | 0, 8.8px                  | 5.6px  | 24px   |
| Text field (settings) | 5.6px, 8px                | -      | 28px   |
| Text field (host)     | 4.8px, 7.2px              | -      | 27px   |
| Menu row              | 5.6px, 8.8px              | 8px    | 27px   |
| Context menu row      | 5.6px, 8.8px              | 8px    | 28px   |
| Menu, context menu    | 4px                       | -      | -      |
| Modal head            | 11.2px, 14.4px            | 8px    | 47px   |
| Modal foot            | 9.6px, 14.4px             | 12.8px | 47px   |
| Settings row          | 11.2px, 0                 | -      | -      |
| Card                  | 12.8px, 14.4px            | -      | -      |
| State badge           | 0.8px, 8px                | -      | 17px   |
| Toast                 | 7.2px, 9.6px              | 9.6px  | 40px   |
| Tab                   | 0, 8px right, 11.2px left | 6.4px  | 39px   |

The rhythm: a control is 24 to 28px high. Inside a row, the gap between an
icon and its text is 5.6 to 8px. A container (menu, modal part, card) pads
4px, about 10px or about 14px. The [audit](./design-system/audit.md)
proposes one scale for these values.

## Layers

The components are sorted by layer, as in atomic design. The layer says
what a component may be made of: a component uses only the layers below it.

| Layer                  | Members                                                                                          |
| ---------------------- | ------------------------------------------------------------------------------------------------ |
| Tokens                 | The custom properties above                                                                      |
| Elements (atoms)       | Button, text field, checkbox and switch, icon, kbd hint, chip and badge, status text             |
| Components (molecules) | Segmented control, tab, card, toast, command block, touch key strip, dropdown menu, context menu |
| Components (organisms) | Modal                                                                                            |
| Patterns               | Settings row, modal form, session window, the Runs, Spend and Board panels, note card            |

## Modes

Each component was measured in Chromium in four modes on 2026-11-14:
desktop (1440 × 900), 150% zoom (960 × 600), tablet with touch
(1024 × 768) and phone (390 × 844, touch). Sizes and spacing are the same
in every mode; only text wrapping changes. The phone moves the sidebar over
the canvas (`max-width: 560px`) and shows the [touch key
strip](./design-system/touch-keys.md) on a console. The detached console
and fence pages were not measured.

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

| Component                                                 | Class                           | Used in  |
| --------------------------------------------------------- | ------------------------------- | -------- |
| [Button](./design-system/button.md)                       | `.btn`                          | 21 parts |
| [Modal](./design-system/modal.md)                         | `.modal`                        | 13 parts |
| [Dropdown menu](./design-system/dropdown.md)              | `.dropdown`                     | 7 parts  |
| [Text field](./design-system/text-field.md)               | `.set-input`, `.host-input`     | 6 parts  |
| [Segmented control](./design-system/segmented-control.md) | `.seg`                          | 3 parts  |
| [Icon](./design-system/icon.md)                           | `.bi`, `x-icon`                 | 23 parts |
| [Status text](./design-system/status-text.md)             | `*-empty`, `*-error`, `*-help`  | 18 parts |
| [Chip and badge](./design-system/chip-badge.md)           | 10 classes                      | 10 parts |
| [Tab](./design-system/tab.md)                             | `.tab`                          | 1 page   |
| [Context menu](./design-system/context-menu.md)           | `.ctxmenu`, `.ctx-item`         | 1 page   |
| [Checkbox and switch](./design-system/checkbox-switch.md) | `input`, `.switch`              | 1 part   |
| [Card](./design-system/card.md)                           | `.sec-card`                     | 1 part   |
| [Kbd hint](./design-system/kbd.md)                        | `.kbd`                          | 1 part   |
| [Toast and flash line](./design-system/toast.md)          | `.wb-toast`                     | Code     |
| [Command block](./design-system/command-block.md)         | `.rel-cmd`                      | 2 parts  |
| [Touch key strip](./design-system/touch-keys.md)          | `.session-keys`, `.session-key` | Code     |

The count is the number of `*.part.html` files and pages that use the
class, measured on 2026-11-14 with `grep -l`. "Code" means the element is
built in TypeScript, not in a part.

Larger patterns have their own ADR and CSS file and are not listed as
components: the session window (ADR-0035 §4), the note card (ADR-0064),
the Runs, Spend and Board panels.

## Keyboard

All shortcuts and the keys of each component: [Keyboard](./design-system/keyboard.md).

## Audit and plan

What is not consistent today, the decisions to make, the proposed tokens
and shared classes, and the order of the work:
[audit](./design-system/audit.md). Nothing in it is applied yet.
