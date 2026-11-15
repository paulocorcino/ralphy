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

The gate checks part of it: stylelint refuses a hex color in any
stylesheet but `01-base.css` (`color-no-hex`). Two values carry an
exception comment with their reason: the white ground of the TOTP QR code
and the Rust file icon.

## Tokens

Every token is a CSS custom property on `:root` in `styles/01-base.css`.
Use the token, never its hex value. Monaco cannot read CSS variables, so
`wb-monaco.ts` repeats some hex values; change both together.

### Neutrals

| Token            | Value     | Use                                                  |
| ---------------- | --------- | ---------------------------------------------------- |
| `--bg`           | `#24201c` | Page ground                                          |
| `--chrome`       | `#2b2620` | Rail, sidebar, modal body                            |
| `--surface`      | `#342d27` | Buttons, modal head and foot, menus                  |
| `--surface-hi`   | `#423a31` | Hover: one step up from `--surface`                  |
| `--log-bg`       | `#2a2521` | Command log, text fields, segmented track            |
| `--window-bg`    | `#0c0a08` | Console window ground, behind the terminal           |
| `--border`       | `#4c4239` | Default hairline                                     |
| `--border-focus` | `#7a6d5f` | Focused window and focused field border              |
| `--border-field` | `#867565` | Edge of a text field: 3:1 on the field and the modal |
| `--text`         | `#d4ccc0` | Body text                                            |
| `--text-strong`  | `#e9e1d4` | Headings and `strong`                                |
| `--text-muted`   | `#a49c91` | Paths, status, labels, help text                     |

### Accents

| Token              | Value     | Use                                                        |
| ------------------ | --------- | ---------------------------------------------------------- |
| `--console-text`   | `#e8d9a8` | Gold: the free console, the primary action, the focus ring |
| `--console-bg`     | `#2b2410` | Gold ground                                                |
| `--console-border` | `#5a4a1e` | Gold border                                                |
| `--ink-on-gold`    | `#1a1613` | Dark ink on a gold fill (the current search hit)           |
| `--danger`         | `#ce8174` | Offline, errors, destructive actions                       |
| `--live`           | `#7fae7f` | A live daemon or console, a passed check                   |
| `--info`           | `#6f9fc0` | Planning and sleep states in the Runs trail                |
| `--warn`           | `#d29922` | Amber: needs attention (attention counter, urgent release) |

### Note tones

Six tones for note cards, and two inks for a solid card: `--note-ochre`,
`--note-sage`, `--note-rose`, `--note-slate`, `--note-plum`, `--note-sand`,
`--note-ink-light`, `--note-ink-dark`. Use them only on a note card.

### Type

| Token               | Value                             | Use                                     |
| ------------------- | --------------------------------- | --------------------------------------- |
| `--font-mono`       | `ui-monospace, SFMono-Regular, …` | All chrome                              |
| `--font-ui`         | `ui-sans-serif, system-ui, …`     | Rendered prose only                     |
| `--reading-size`    | `14px`                            | Prose body size                         |
| `--reading-measure` | `100ch`                           | Prose line length                       |
| `--reading-leading` | `1.7`                             | Prose line height                       |
| `--fs-2xs`          | `0.66rem`                         | Badge, kbd hint (10.56px)               |
| `--fs-xs`           | `0.72rem`                         | Chip, small segmented control (11.52px) |
| `--fs-sm`           | `0.78rem`                         | Empty, error and help text (12.48px)    |

The body is 13px, but `rem` is relative to the root, which is 16px. The
chrome uses six steps; every `font-size` in `rem` within 1px of a step
uses its token (decision D5; a value exactly between two steps went up).

| Token      | Value     | Pixels  | Used for                                   |
| ---------- | --------- | ------- | ------------------------------------------ |
| `--fs-2xs` | `0.66rem` | 10.56px | Badge, kbd hint, menu group label          |
| `--fs-xs`  | `0.72rem` | 11.52px | Chip, menu head, foot note, small segments |
| `--fs-sm`  | `0.78rem` | 12.48px | Empty, error and help text, segments       |
| `--fs-md`  | `0.8rem`  | 12.8px  | Button, tab, menu row, text field          |
| `--fs-lg`  | `0.9rem`  | 14.4px  | Settings label, card title, toast          |
| `--fs-xl`  | `1.15rem` | 18.4px  | Large titles                               |

The body (13px) and the modal title inherit their size. A few sizes stay
outside the scale on purpose: `1rem` and larger display sizes, `1.05rem`
(1.6px from a step, more than D5 allows), and sizes in `em` inside
rendered prose, which follow the reading tokens.

## Spacing

The spacing scale is the set of values the chrome already used most.

| Token       | Value    | Pixels |
| ----------- | -------- | ------ |
| `--space-1` | `0.1rem` | 1.6px  |
| `--space-2` | `0.2rem` | 3.2px  |
| `--space-3` | `0.3rem` | 4.8px  |
| `--space-4` | `0.4rem` | 6.4px  |
| `--space-5` | `0.5rem` | 8px    |
| `--space-6` | `0.6rem` | 9.6px  |
| `--space-7` | `0.8rem` | 12.8px |
| `--space-8` | `1rem`   | 16px   |
| `--space-9` | `1.5rem` | 24px   |

New spacing uses a token. 434 declarations use the scale; 276 still hold a
value between two steps (most often 0.35, 0.55, 0.25, 0.45 and 0.7rem).
Such a value moves to the nearest step when its panel changes for other
work. The measures below are the same either way.

Measured in Chromium:

| Place                 | Padding (top, sides)      | Gap    | Height |
| --------------------- | ------------------------- | ------ | ------ |
| Button in a modal     | 4.8px, 11.2px             | 5.6px  | 27px   |
| Button in a toolbar   | 0, 8.8px                  | 5.6px  | 24px   |
| Text field (settings) | 5.6px, 8px                | -      | 28px   |
| Text field (host)     | 5.6px, 8px                | -      | 28px   |
| Menu row              | 5.6px, 8.8px              | 8px    | 27px   |
| Context menu row      | 5.6px, 8.8px              | 8px    | 28px   |
| Menu, context menu    | 4px                       | -      | -      |
| Modal head            | 11.2px, 14.4px            | 8px    | 47px   |
| Modal foot            | 9.6px, 14.4px             | 12.8px | 47px   |
| Settings row          | 11.2px, 0                 | -      | -      |
| Card                  | 12.8px, 14.4px            | -      | -      |
| State badge           | 0.8px, 7.2px              | -      | 17px   |
| Toast                 | 7.2px, 9.6px              | 9.6px  | 42px   |
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

### Radius

| Token           | Value   | Use                                  |
| --------------- | ------- | ------------------------------------ |
| `--radius-sm`   | `3px`   | Checkbox, inline code, small buttons |
| `--radius`      | `4px`   | Buttons, fields, menu rows           |
| `--radius-md`   | `6px`   | Menus, session windows, toast        |
| `--radius-lg`   | `8px`   | Cards                                |
| `--radius-xl`   | `10px`  | Modals                               |
| `--radius-pill` | `999px` | Chips, badges, segmented control     |

A circle uses `50%`. A few radii are still literal (1px, 2px, 5px, 7px,
12px); the [audit](./design-system/audit.md) maps them to the scale.

### Depth

| Token            | Value                          | Use                     |
| ---------------- | ------------------------------ | ----------------------- |
| `--shadow-float` | `0 10px 30px rgba(0,0,0,0.55)` | Menus, floating windows |
| `--shadow-modal` | `0 24px 70px rgba(0,0,0,0.6)`  | Modals                  |
| `--shadow-toast` | `0 4px 18px rgba(0,0,0,0.4)`   | Toast                   |

A flat element has no shadow.

### Stacking order

The stacking order, low to high. The consoles use 0 to 120 (`Z_CEIL` in
`wb-console.ts`). A new layer above them gets a token here.

| Token           | Value  | Use                                           |
| --------------- | ------ | --------------------------------------------- |
| `--z-frame`     | `130`  | Frame chrome over the consoles                |
| `--z-panel`     | `150`  | Runs panel; the sidebar on a phone            |
| `--z-rail-menu` | `160`  | The rail while one of its menus is open       |
| `--z-ctxmenu`   | `200`  | Context menu                                  |
| `--z-modal`     | `500`  | Modal scrim                                   |
| `--z-zoom`      | `510`  | Image zoom overlay                            |
| `--z-modal-ask` | `520`  | Confirm and prompt, above the modal that asks |
| `--z-login`     | `900`  | Login gate                                    |
| `--z-dropdown`  | `1000` | Dropdown menu, build notice                   |
| `--z-toast`     | `3000` | Toast                                         |

### Focus

`--focus-ring` is `--console-text`. A control that removes the browser
outline shows focus with `outline: 1px solid var(--focus-ring)` and
`outline-offset: 2px`.

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
| [Command palette](./design-system/command-palette.md)     | `.palette-modal`                | 1 part   |

The count is the number of `*.part.html` files and pages that use the
class, measured on 2026-11-14 with `grep -l`. "Code" means the element is
built in TypeScript, not in a part.

Larger patterns have their own ADR and CSS file and are not listed as
components: the session window (ADR-0035 §4), the note card (ADR-0064),
the Runs, Spend and Board panels.

## Keyboard

Read from the code on 2026-11-14, at commit `55928b55`. The files are in
`crates/ralphy-daemon/assets/ui/`; the line is the line of the listener or
of the binding. A new shortcut gets a row here in the same change as its
code. How each component shows its keys: [Keyboard](./design-system/keyboard.md).

### The keyboard model

Decided on 2026-11-14 in [ADR-0087](./adr/0087-the-workbench-keyboard-is-one-command-list.md).
Built so far: the list (`COMMANDS` in `wb-commands.ts`) and its one
listener. Each page shortcut is a command of the list; a component binds
its action with `bindCommand`, and the Code column below names that line.
A terminal asks the list before it gives a chord to the page. The
[command palette](./design-system/command-palette.md) is built. The help
screen is not built yet; the tables below describe the code of today.

- **No prefix key.** A page shortcut is one chord: Alt+Shift (⌥⇧ on a Mac)
  and one more key. Only `/`, `?` and Ctrl+Shift+F (Cmd+Shift+F) differ;
  `/` and `?` work only with the focus outside a text field, an editor and
  a terminal.
- **One list of commands.** Each page shortcut is a command in one module:
  id, label, keys, "can it run now", action. One document listener runs it.
  The palette, the help screen and every key hint read the same list.
- **The terminal keeps its keys.** In a terminal every key goes to the
  child program, except a chord whose command can run now.
- **No key only to leave a terminal.** A command that opens or focuses
  something moves the focus. What it opens gives the focus back to the
  terminal when it closes.
- **Command palette:** Alt+Shift+P, also from a terminal. It lists the
  commands that can run now, with their keys; `/` stays the search.
- **Help screen:** `?`, or Alt+Shift+/ from anywhere, or a row of the
  palette. It shows the list, the keys inside components, and the line
  "In a terminal, every other key goes to the program."
- **Keys the list refuses:** a key without Alt+Shift (except the three
  above); a chord with Ctrl or Cmd (Ctrl belongs to the program, Cmd to the
  browser and the system); Escape, Tab, F1 to F12 alone; the keys a browser
  keeps (Ctrl or Cmd with W, T, N or Q; Ctrl+Shift+N, P, T, C, I or J;
  Cmd+Option+I; F11; F12; Alt+F4). Ctrl+Insert stays the copy key of a
  terminal on Windows and Linux; on a Mac it is Cmd+C.
- **Keys inside a component** stay in the component, and a component that
  takes Escape or Enter stops the event.
- **Fixed keys.** A person cannot change them.
- **Shown for the system:** "⌥⇧3" on a Mac, "Alt+Shift+3" elsewhere. On a
  Mac the fence keys need Fn; the palette reaches a fence by its name.

### The rules the shortcuts follow today

- A page shortcut uses **Alt+Shift** with no Ctrl and no Meta. Keys are
  matched on `e.code`, so the keyboard layout does not change them.
- A page shortcut does nothing while `$store.modals.consoleShortcutsBlocked()`
  is true: no login, a blocking dialog or "What's new" is open, or the focus
  is in an input, a textarea, an editable element or the code editor
  (`wb-modals-store.ts:229`). The helper input of a terminal is a textarea,
  so a terminal blocks them too, except where the next rule says otherwise.
- A terminal gives an Alt+Shift chord to the page only when a command of
  the list can run now and does something; it runs the command before xterm
  sees the key (`chordTaker`, `wb-console-press.ts:34`). The commands that
  pass a terminal are the digits, R, F1 to F12 and the arrows. Every other
  key in a terminal goes to xterm, and xterm sends it to the child program.
- The dialogs built in TypeScript (`wb-messages.ts`, `wb-console-worktree.ts`,
  `wb-notes-menus.ts`) listen on the document in the capture phase and stop
  Escape and Enter, so a terminal does not take them first.

### Shortcuts

| Action                                                                                 | Keys                                   | Component                   | Code                                                   |
| -------------------------------------------------------------------------------------- | -------------------------------------- | --------------------------- | ------------------------------------------------------ |
| Open the command palette                                                               | Alt+Shift+P                            | Command palette             | `wb-palette.ts:37`                                     |
| Open the search: files when a project is open, projects otherwise                      | `/`                                    | Page                        | `wb-wire.ts:448`                                       |
| Open the files search, also from the code editor and from a terminal                   | Ctrl+Shift+F (Cmd+Shift+F on macOS)    | Page                        | `wb-wire.ts:459`                                       |
| Run the row with that digit in the Consoles menu                                       | Alt+Shift+0 to 9                       | Consoles menu               | `wb-consoles-tab-menus.ts:311`                         |
| Open the Consoles menu on its command field                                            | Alt+Shift+R                            | Consoles menu               | `wb-consoles-tab-menus.ts:318`                         |
| Go to fence 1 to 12                                                                    | Alt+Shift+F1 to F12                    | Fences menu                 | `wb-consoles-tab-menus.ts:328`                         |
| Walk the columns (Left, Right) and the rows of a column (Up, Down), or else the fences | Alt+Shift+arrows                       | Stage                       | `wb-consoles-tab.ts:449`                               |
| Copy the selection of a terminal                                                       | Ctrl+Insert                            | Terminal                    | `wb-console-press.ts:53`                               |
| End the drag of a window where it is                                                   | Escape                                 | Session window              | `wb-stage-window-gestures.ts:199`                      |
| Scroll the stage sideways                                                              | Shift+wheel                            | Stage                       | `wb-stage-view.ts:428`                                 |
| Close the top modal                                                                    | Escape                                 | Modal                       | `wb-modals-store.ts:177`                               |
| Keep the focus inside the top modal                                                    | Tab, Shift+Tab                         | Modal                       | `wb-modals-store.ts:185`                               |
| Confirm, when no button has the focus                                                  | Enter                                  | Confirm dialog              | `wb-confirm-dialog.part.html:16`                       |
| Send the answer                                                                        | Enter                                  | Prompt dialog               | `wb-prompt-dialog.part.html:32`                        |
| Cancel; confirm when the confirm button has the focus                                  | Escape; Enter                          | Confirm built in TypeScript | `wb-messages.ts:120`                                   |
| Cancel; create the worktree (or cancel, when Cancel has the focus)                     | Escape; Enter                          | New worktree dialog         | `wb-console-worktree.ts:133`                           |
| Close the Markdown help                                                                | Escape, or Enter on Close              | Note help dialog            | `wb-notes-menus.ts:247`                                |
| Switch to the first branch in the list, or create the branch typed                     | Enter                                  | Branch dialog               | `wb-branch-dialog.part.html:30`                        |
| Move in the folder list; take the folder; add the project                              | Up, Down; Enter or Tab; Enter          | Add a project dialog        | `wb-add-project-dialog.ts:85`                          |
| Go to the next step; check the host again                                              | Enter                                  | Hosts dialog                | `wb-hosts-dialog.part.html:65`, `:155`                 |
| Confirm the update with the code                                                       | Enter                                  | Update dialog               | `wb-release-dialogs.part.html:61`                      |
| Send the password; cancel                                                              | Enter; Escape                          | Security step-up            | `wb-security-dialog.part.html:23`                      |
| Save the password                                                                      | Enter                                  | Security dialog             | `wb-security-dialog.part.html:71`, `:75`               |
| Confirm the authenticator code                                                         | Enter                                  | Security dialog             | `wb-security-dialog.part.html:115`                     |
| Save the Telegram token                                                                | Enter                                  | Settings dialog             | `wb-settings-dialog.part.html:185`                     |
| Go back one box and clear it; move between the boxes                                   | Backspace on an empty box; Left, Right | Login code                  | `wb-login.ts:123`                                      |
| Move between the enabled rows, wrapping at the ends; go to the first or last row       | Up, Down; Home, End                    | Dropdown menu, context menu | `wb-arrow-keys.ts:116`                                 |
| Close the open menus; the focus goes back to where it came from                        | Escape                                 | Dropdown menu, context menu | `wb-arrow-keys.ts:129`                                 |
| Move between the tabs; go to the first or last tab                                     | Left, Right; Home, End                 | Tab list                    | `wb-arrow-keys.ts:116`                                 |
| Open the tab that has the focus                                                        | Enter, Space                           | Canvas tab                  | `index.html:276`                                       |
| Close the menu                                                                         | Escape                                 | Checkout menu               | `wb-console-checkout.ts:261`                           |
| Close the menu                                                                         | Escape                                 | Encoding menu               | `wb-viewer-encoding.ts:129`                            |
| Swap in the first console of the list; close the menu                                  | Enter; Escape                          | Columns menu filter         | `wb-consoles-tab-columns.part.html:39`                 |
| Run the command; close the field                                                       | Enter; Escape                          | Consoles menu command field | `wb-consoles-tab-menus.part.html:60`                   |
| Clear the field                                                                        | Escape                                 | Projects search             | `index.html:168`                                       |
| Search now; close the search                                                           | Enter; Escape                          | Files search                | `wb-files.part.html:69`                                |
| Open the diff of the row                                                               | Enter                                  | Changes panel               | `wb-changes-panel.part.html:81`, `:104`                |
| Save the name; cancel                                                                  | Enter; Escape                          | Console title               | `wb-console-title.ts:321`                              |
| Save the name; cancel                                                                  | Enter; Escape                          | Fence name                  | `wb-stage-fences.ts:245`                               |
| Save the title; cancel                                                                 | Enter; Escape                          | Note title                  | `wb-notes-chrome.ts:107`                               |
| Save the file name; cancel                                                             | Enter; Escape                          | Note rename                 | `wb-notes-chrome.ts:336`                               |
| Save the note                                                                          | Ctrl+S (Cmd+S)                         | Note card                   | `wb-notes-build.ts:167`                                |
| Leave the editor; a second Escape leaves the card                                      | Escape                                 | Note card                   | `wb-notes-build.ts:167`                                |
| Apply the diagram; cancel                                                              | Ctrl+Enter (Cmd+Enter); Escape         | Diagram editor in a note    | `crates/ralphy-daemon/vendor-build/crepe/entry.js:666` |
| Save the file                                                                          | Ctrl+S (Cmd+S)                         | Code editor                 | `wb-file-viewer-code.ts:77`                            |
| Next match; the match before; close                                                    | Enter; Shift+Enter; Escape             | Markdown search             | `wb-viewer-markdown.ts:229`                            |
| Zoom in; zoom out; fit; close; keep the focus inside                                   | `+` or `=`; `-`; `0`; Escape; Tab      | Zoom overlay                | `wb-zoom-overlay.ts:341`                               |
| Rename                                                                                 | F2 (Enter on macOS)                    | Files tree                  | `wb-files-mount.ts:153`                                |
| Show the properties                                                                    | Alt+Enter                              | Files tree                  | `wb-files-mount.ts:178`                                |

Keys that come from a library and are not listed: the arrow keys of the
files tree (Wunderbaum), the keys of the code editor (Monaco) and of the
note editor (Milkdown), and every key that a terminal sends to its child
(xterm). The touch key strip is a row of buttons, not shortcuts:
[Touch key strip](./design-system/touch-keys.md).

### Conflicts

Found by reading the code and the vendored xterm (`vendor/xterm.js`).
Conflicts 1 to 4 were then tried in Chromium on 2026-11-14, on a daemon
with its own port and scratch store; each one happened as written. The
diagram editor keys were tried the same way and work.

1. **Fixed: an Alt+Shift chord that the page took from a terminal could be
   lost.** The terminal gave Alt+Shift+0 to 9 and Alt+Shift+R to the page
   always, also when the page then had nothing to do. Now the terminal asks
   the list, and a chord whose command cannot run goes to the child program.
   Code: `wb-console-press.ts:34`. Issue #953.
2. **Fixed: Alt+Shift+F1 to F12 did not work from a terminal.** The fence
   command now passes a terminal, as the digits, R and the arrows do. Code:
   `wb-commands.ts` (`fence.go`). Issue #954.
3. **Fixed: Escape during a window drag went to the child program.** The
   drag now listens in the capture phase and stops the Escape. Code:
   `wb-stage-window-gestures.ts:199`. Issue #955.
4. **Fixed: the Escape that closed the checkout menu was not stopped.** It is
   stopped now. The encoding menu (`wb-viewer-encoding.ts:129`) keeps the old
   shape; it opens in a file viewer, away from a terminal. Code:
   `wb-console-checkout.ts:261`. Issue #955.
5. **Alt+Shift on Windows.** With two or more keyboard layouts installed,
   Windows can use Left Alt+Shift to change the input language. Every page
   shortcut starts with Alt+Shift. Not measured with the workbench.

Checked, and no conflict found:

- `/` stays in a terminal: the terminal's textarea blocks the shortcut.
- Ctrl+Shift+F in a terminal: xterm sends nothing for Ctrl+Shift with a
  letter and lets the event go on, so the page takes it and the child
  program loses nothing.
- Ctrl+Insert copies only when the terminal has a selection; with no
  selection the key goes to the child program. Ctrl+C always goes to the
  child program. Ctrl+Shift+C is not used: in Chrome and Edge it opens the
  developer tools, and a page cannot take it.
- Ctrl+S is taken only in a note card and in the code editor. In a
  terminal, Ctrl+S goes to the child program.
- Alt+Shift+arrows and Shift+Alt+F of the code editor: the page shortcuts
  are blocked while the code editor has the focus.
- The keys of a modal: the modal keeps the focus inside, so a terminal does
  not have the focus while a modal is open.

### Gaps

- Fixed: every dropdown menu and the context menu close on Escape, and the
  focus goes back to the element it came from. A field inside a menu keeps
  its own Escape. Issue #956.

## Measuring a change

A change to the styles is measured before and after, in a real browser,
against a daemon on its own port and scratch store:

```sh
python tests/browser/workbench/wb_design_measure.py .ralphy/m-before
# change the styles, rebuild, then:
python tests/browser/workbench/wb_design_measure.py .ralphy/m-after
python tests/browser/workbench/wb_design_measure_diff.py \n  .ralphy/m-before/measure.json .ralphy/m-after/measure.json
```

A change that should not move anything gives `diffs: 0`. The focus of the
Files search field can differ between two runs of the same code: it is
timing, not a change.

## Audit and plan

What is not consistent today, the decisions to make, the proposed tokens
and shared classes, and the order of the work:
[audit](./design-system/audit.md). Nothing in it is applied yet.

Two colors are outside the palette on purpose: `#dea584`, the Rust file
icon in the file tree (file type icons keep their brand colors, as the
devicon icons do), and `#fff`, the ground a QR code needs to scan.
