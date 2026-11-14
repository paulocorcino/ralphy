# Design system audit, 2026-11-14

An audit of the workbench styles (`crates/ralphy-daemon/assets/ui/styles/`,
13 files, 11,827 lines) and parts. It measures where the page has no single
rule, and it proposes a plan. Nothing here is applied yet. Each step of the
plan is its own change, with a browser check before and after.

Two kinds of numbers are used. Sections 2 to 4 come from a script that
reads every CSS declaration outside comments; they count declarations, not
elements on screen. Section 5 comes from Chromium (Playwright) against a
daemon with a scratch store, in four modes: desktop 1440 × 900, 150% zoom
(960 × 600), tablet with touch (1024 × 768) and phone (390 × 844, touch).
It reads computed styles, walks the page with Tab, and computes contrast
from the colors on screen. The detached console and fence pages were not
measured.

## 1. Inventory

| Layer      | What exists                                                                                                              |
| ---------- | ------------------------------------------------------------------------------------------------------------------------ |
| Tokens     | 36 custom properties in `01-base.css`: 10 neutrals, 6 accents, 8 note tones, 5 type, 7 layout. All 36 are used           |
| Elements   | Button, text field, checkbox, switch, icon, kbd, chip and badge, status text                                             |
| Components | Modal, dropdown menu, context menu, segmented control, tab, card, toast, command block, touch key strip                  |
| Patterns   | Settings row, modal form, panel (Runs, Spend, Board), session window, note card. Each has its own ADR or CSS file        |
| Governance | ADR-0035 for the palette, ADR-0065 for the words. No rule for sizes, spacing, z-index or new components before this file |

## 2. What is consistent

- **Color.** 32 distinct hex values, and 24 of them are token values. Only
  8 raw hex values are left (section 4). 61 `color-mix()` calls build tints
  from tokens instead of new hex values.
- **Font family.** 46 of 52 `font-family` declarations use a token; 5 spell the stack by hand.
- **Radius.** `var(--radius)` is the most used value (55).
- **Motion.** 8 durations, mostly `0.12s` and `0.15s`. Small and calm, as
  ADR-0035 asks.

## 3. Fragmentation: one shape, many rules

| Area           | Measured                                                                                           | Should be      |
| -------------- | -------------------------------------------------------------------------------------------------- | -------------- |
| Font size      | Step 7 done: six tokens; 7 `rem` values stay outside (1rem and larger, 1.05rem)                    | 6 steps        |
| Spacing        | Step 7: nine tokens; 434 exact uses converted, 276 between two steps move when their panel changes | 9 steps        |
| Radius         | Step 1 done: 6 tokens. Still literal: 2px, 1px, 5px, 12px, 7px                                     | 6 names        |
| z-index        | Step 1 done: every layer from 130 up is a token                                                    | Named layers   |
| Shadow         | 28 distinct `box-shadow` values                                                                    | 3 levels       |
| Empty state    | Step 4 done: one rule, `.empty-note`                                                               | 1 class        |
| Error line     | Step 4 done: one rule, `.error-line`                                                               | 1 class        |
| Help line      | Step 4 done: one rule, `.help-text`                                                                | 1 class        |
| Chip and badge | Step 6 done: `.chip` and `.badge`, five tones                                                      | 1 class, tones |
| Menu row       | Step 5 done: one rule, `.menu-item`                                                                | 1 rule         |
| Text field     | Step 5 done: one rule, `.text-field`, on `--log-bg`                                                | 1 rule         |
| Segmented      | Step 5 done: one rule, with a small size `.seg-sm`                                                 | 1 rule         |
| Icons          | Step 8: new icons are Lucide (D1); Bootstrap Icons move when their panel changes                   | 1 library      |

## 4. Drift: the code and ADR-0035

| Finding                                                                                                    | Where                                                                                       |
| ---------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| ADR-0035 lists `--danger-border` and `--console-bg-hi`; the CSS has neither                                | `01-base.css`                                                                               |
| `--live`, `--info`, `--font-ui` and the note tones are not in ADR-0035                                     | ADR-0035 §1                                                                                 |
| Fixed in step 6: amber is the token `--warn`                                                               |                                                                                             |
| Fixed in step 6: the release dot uses `--live`                                                             |                                                                                             |
| `#dea584` is a color with no token                                                                         | `02-rail-sidebar.css:653`                                                                   |
| `var(--danger, #e5484d)`: a fallback that never applies, 8 times                                           | `03-runs.css`, `04-canvas.css`, `10-kanban.css`                                             |
| ADR-0035 says the console window ground is `#000000`; the code uses `#0c0a08`, now the token `--window-bg` | `06-consoles.css`                                                                           |
| `var(--surface-2, …)` and `var(--mono, …)` read tokens that do not exist; the fallback always wins         | `04-canvas.css:418`, `11-appended.css:149`, `:172`                                          |
| 5 declarations spell the monospace stack instead of `var(--font-mono)`, one of them a shorter stack        | `01-base.css:134`, `06-consoles.css:970`, `07-viewers.css:166`, `:795`, `08-modals.css:610` |
| The toast is the only chrome element in `--font-ui`                                                        | `13-notes.css:1196`                                                                         |
| The command block sizes in `px`; the chrome sizes in `rem`                                                 | `12-spend.css:980`                                                                          |
| Fixed in step 5: the kbd hint in a menu head had no box; one `.kbd` rule now                               | `05-workspace.css:644`                                                                      |

## 5. Accessibility and modes, measured in a browser

Items 1 to 5 are fixed by step 2 of the plan, and item 8 by step 3
(2026-11-14). Measured after
the fix: Tab stays inside each of the five modals measured, every control
that gets focus shows it, the canvas tab is in the Tab order, and the
danger button text is 4.52:1.

No page error in any mode. No horizontal scroll in any mode.

1. **Focus leaves a modal.** In Settings, Security, Hosts, Add a project
   and the confirm dialog, Tab goes on after the last control to the rail,
   the sidebar and the consoles behind the scrim. The modal has
   `aria-modal="true"` but does not keep focus. Only the zoom overlay does
   (`trapTab` in `wb-zoom-overlay.ts`). The first focus is right: `scrim()`
   focuses the first control (Cancel in a confirm).
2. **Focus is not visible on many controls.** Measured, these get focus
   and show no change: every rail button, the sidebar header buttons
   (refresh, add a project, add a host), `.head-act`, `.tab-close`,
   `.tab-add`, `.branch-chip`, `.dropdown-item` (7 of 7), `.settings-navitem`
   (13 of 13) and the avatar button. In the CSS, 28 rules start with
   `all: unset` and 20 set `outline: none`; there are 15 `:focus-visible`
   rules. Where the browser outline is left, it is visible: Chromium draws
   two rings. `.set-input` shows focus with its border.
3. **A tab cannot be reached with the keyboard.** Measured: Tab skips
   `.tab`, a `div` with a click handler and no `tabindex`.
4. **A disabled switch looks enabled.** No `.switch input:disabled` rule.
   "Require login" is disabled until TOTP is set up.
5. **Danger text is under 4.5:1.** WCAG AA asks 4.5:1 for text this size.
   `--danger` gives 3.63 on `--surface` (danger button, danger menu row),
   4.02 on `--chrome` (error lines in the sidebar and modals), 4.07 on
   `--log-bg`. `--text-muted` gives 4.12 on `--surface-hi` (a muted label on
   a hovered row). Every other token pair passes: `--text` 7.0 to 10.2,
   `--text-muted` 5.0 to 6.0 on the other grounds. Disabled text is dimmed
   on purpose and WCAG does not count it.
6. **Field edges were faint.** Fixed after the plan: the token `--border-field`
   (`#867565`) gives 3.43:1 on the field and 3.06:1 on the modal. `--border` against the ground around it is
   1.38:1 on `--surface` and 1.65:1 on `--bg`. WCAG 1.4.11 asks 3:1 for the
   edge of a control. The field ground differs from the modal ground too,
   which helps, but only a little.
7. **Touch targets are small in every mode.** No control except the touch
   key strip reaches 44px, on the phone too. Under the 24px minimum of WCAG
   2.2: `.tab-close` 16 × 16, `.project-remove` 18 × 18, the checkbox
   16 × 16, `.branch-chip` 19px high, and the console title bar buttons
   (full screen, maximize, restart, lock, close) 22 × 22. Sizes and spacing
   do not change between desktop, tablet and phone. This is kept on
   purpose: see D7.
8. **The Settings modal does not fit a phone.** At 390px it keeps two
   columns: the Bind address field is cut at the right edge, and the scope
   chip wraps onto three lines.
9. **No arrow keys** in menus or in tab lists. Only Tab moved. Fixed
   after the plan (`wb-arrow-keys.ts`), checked in Chromium.

The detached pages were tried after the plan. The detached fence window
throws on `main` too while it builds its consoles (`windowShown` in
`wb-console.ts` reads the Alpine store, which that page does not have), so
it could not be measured; this is a defect outside the design system. The
detached file page stayed empty in the scratch setup, on `main` too, so it
was not measured either.

## 6. Decisions

All eight were decided on 2026-11-14: D1 to D6 and D8 as recommended
below, D7 as written. The palette, focus, icon and field decisions are
recorded in ADR-0035 (amendment of 2026-11-14).

| #   | Question                                       | Recommendation                                                                                                                                                                                                                       |
| --- | ---------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| D1  | Which icon library do new icons use?           | Lucide. It is the newer path (`x-icon`, 2026-09-27), it draws SVG that takes the text color and stroke. Move the 100 Bootstrap uses over time, not in one change                                                                     |
| D2  | Is amber a third accent, `--warn`?             | Yes. It already does a job, "needs attention", that neither gold nor terracotta does. It needs an ADR-0035 amendment                                                                                                                 |
| D3  | One text field ground: `--log-bg` or `--bg`?   | `--log-bg`. It is the ground of `.set-input`, the checkbox, the switch track and the segmented track                                                                                                                                 |
| D4  | One focus ring for all controls?               | Yes: `1px solid var(--console-text)`, offset 2px. The checkbox already uses it                                                                                                                                                       |
| D5  | Merge font sizes that differ by less than 1px? | Yes, with a screenshot of each panel before and after                                                                                                                                                                                |
| D6  | How does danger text reach 4.5:1?              | Lighten `--danger` to `#ce8174`: same hue, 4.52 on `--surface`, 5.01 on `--chrome`. One token, every danger text follows. It needs an ADR-0035 amendment                                                                             |
| D7  | Do controls grow on a touch screen?            | **Decided 2026-11-14: no.** The current sizes stay on phone and tablet; larger controls would distort the page. A size may go down, or be made the same as its siblings. The touch key strip stays the one element sized for fingers |
| D8  | Settings on a phone: one column?               | Yes: below 560px, the section list above the section, as the sidebar does                                                                                                                                                            |

## 7. Proposed tokens

Not in `01-base.css` yet. The values are the most used ones today, so most
declarations keep their value.

**Type** (`rem` is 16px, so `0.8rem` is 12.8px):

| Token      | Value     | Takes today's          |
| ---------- | --------- | ---------------------- |
| `--fs-2xs` | `0.66rem` | 0.64, 0.65, 0.66, 0.68 |
| `--fs-xs`  | `0.72rem` | 0.7, 0.72, 0.74, 0.75  |
| `--fs-sm`  | `0.78rem` | 0.76, 0.78             |
| `--fs-md`  | `0.8rem`  | 0.8, 0.82              |
| `--fs-lg`  | `0.9rem`  | 0.85, 0.9              |
| `--fs-xl`  | `1.15rem` | 1.05, 1.15             |

The largest moves are 1.05 → 1.15rem (1.6px, 2 places) and 0.85 → 0.9rem
(0.8px, 11 places). Every other move is 0.48px or less. Sizes in `em`
inside rendered prose follow the reading tokens and do not change.

**Spacing (corrected 2026-11-14, before step 7):** the scale first proposed
here (0.15, 0.25, 0.35 … rem) fell between the values the chrome uses most:
0.2, 0.3, 0.4 and 0.55rem were each exactly between two steps, so most
declarations had no nearest step. The scale that step 7 adopted is the set
of values already most used: `--space-1` to `--space-9` = 0.1, 0.2, 0.3,
0.4, 0.5, 0.6, 0.8, 1 and 1.5rem. Only exact matches were converted, with
no visible change; the rest moves when its panel changes.

**Radius:** `--radius-sm` 3px, `--radius` 4px, `--radius-md` 6px,
`--radius-lg` 8px, `--radius-xl` 10px, `--radius-pill` 999px. 50% stays
for a circle.

**Layers:** `--z-window` (the consoles, up to 120), `--z-panel` 150,
`--z-ctxmenu` 200, `--z-modal` 500, `--z-modal-ask` 520, `--z-dropdown`
1000, `--z-toast` 3000. The values stay; only the names are new. Note that
a dropdown (1000) is above a modal (500) today.

**Shadows:** `--shadow-float` `0 10px 30px rgba(0,0,0,0.55)` for menus and
windows, `--shadow-modal` `0 24px 70px rgba(0,0,0,0.6)`, and
`--shadow-toast` `0 4px 18px rgba(0,0,0,0.4)`.

**Color:** `--warn` (D2), and `--focus-ring` (D4).

## 8. Proposed shared classes

| Class         | Replaces                              | Look                                                                         |
| ------------- | ------------------------------------- | ---------------------------------------------------------------------------- |
| `.empty-note` | 12 `*-empty` classes                  | `--text-muted`, `--fs-sm`; the place sets padding and alignment              |
| `.error-line` | 18 `*-error` classes                  | `--danger`, `--fs-sm`                                                        |
| `.help-text`  | `.run-help`, `.set-help`, `.sec-note` | `--text-muted`, `--fs-xs`, line height 1.5                                   |
| `.chip`       | 10 chip and badge classes             | Pill, `--fs-2xs`, tones: `neutral`, `gold`, `live`, `warn`, `danger`, `info` |
| `.menu-item`  | `.dropdown-item` rule and `.ctx-item` | One rule; the old names stay as aliases while old queries need them          |
| `.text-field` | `.set-input`, `.host-input`           | D3                                                                           |

A domain class can stay next to the shared one when code queries it, for
example `class="error-line move-error"`.

## 9. Plan, in order

Each step is one change, with a browser check of the panels it touches.

1. **Tokens with no visible change.** Add the layer, radius, shadow and
   focus tokens with today's values and use them. Remove the dead
   fallbacks (`#e5484d`), replace `--surface-2` and `--mono` with real
   tokens, and use `var(--font-mono)` in the 5 places. Nothing moves on
   screen.
2. **Accessibility.** Keep Tab inside an open modal, in `scrim()`, so
   every modal gets it at once. One focus ring for every control that
   removes the outline (D4). A disabled style for the switch. Make `.tab`
   reachable with the keyboard. Danger text at 4.5:1 (D6). These change
   what a keyboard user sees, so they come early.
3. **Phone.** Settings in one column on a phone (D8). Control sizes stay
   as they are (D7); the size of a control changes only to match its
   siblings, for example the five title bar buttons and `.tab-close`.
4. **Status text.** `.empty-note`, `.error-line`, `.help-text`. Small and
   low risk; many places.
5. **Controls.** `.menu-item`, `.text-field` (D3), one segmented rule, one
   kbd hint rule.
6. **Chip and badge** with tones, and `--warn` (D2, ADR-0035 amendment).
7. **Type and spacing scales** (D5). Done: type in one change (every size
   within 1px of a step), spacing by exact match only.
8. **Icons** (D1), as panels are touched for other work. Done as a rule:
   the icon page and ADR-0035 say a new icon is Lucide.

## 10. Keeping it

- [DESIGN-SYSTEM.md](../DESIGN-SYSTEM.md) holds the rule: no new
  component, color, spacing value or shortcut without an entry.
- Done in step 8: stylelint refuses a hex color outside `01-base.css`
  (`color-no-hex`, off for that file). Two sites carry an exception
  comment with the reason.
  After steps 4 and 7, it can refuse a font size or spacing that is not a
  token (`declaration-strict-value`, a plugin to add). A count that may
  only go down, like the other ratchets, lets the old sites stay until
  their step.
- Run the measure again after each step and update section 3.
