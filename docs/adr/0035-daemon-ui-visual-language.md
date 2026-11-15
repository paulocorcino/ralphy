# Daemon web UI: the visual language

Status: accepted (documents the design shipped in the daemon UI; extracted
from `crates/ralphy-daemon/assets/ui/index.html`).
Amended 2026-11-14 (a third accent `--warn`, a lighter `--danger`, one focus
ring, Lucide for new icons).

The daemon serves a single embedded page — the workbench launcher and its
floating session windows (ADR-0032). This ADR records its **visual language**
(a "design system" / "style guide"): the palette, type, spacing, and the
component tokens, so the look stays coherent as the page grows and nobody
reinvents a colour by eye. It is a reference document, not a new decision — the
values below are the ones already in the stylesheet. Vocabulary (**Workbench
session**, **free console**, **launcher**) lives in
[CONTEXT.md](../../CONTEXT.md).

## Design intent

A **terminal-native, warm-dark** aesthetic. The page is a thin chrome around
xterm.js, so it dresses like a terminal: an all-monospace face, a dark
warm-brown ground (not the usual cold blue-grey), and low-chroma earth tones
that let the live terminal output — the only saturated thing on screen — carry
the colour. Restraint is the rule: one accent for the escape hatch, one for
danger, everything else a grey ramp.

## Decision

### 1. Palette

A single warm, desaturated ramp anchored on a brown ground, plus two purposeful
accents. All values are the literal hex tokens in the stylesheet.

**Amendment — the ramp was lifted one bracket, and the top of it lowered.** The
original ground was `#14110f`, a near-black; headings were `#f2ede4`, all but
white. Against a warm ground that top end glared, and the near-black made long
reading sessions harsh. Every neutral moved up together — ground, chrome,
surface, log, border — because lifting `bg` alone would have collapsed the
depth between the canvas floor and the panels framing it. `text` came *down*
(`#e8e2d9` → `#d4ccc0`) and the new `text-strong` replaces the near-white, so
hierarchy is a step in the ramp rather than maximum luminance. Body-on-ground
contrast is 10.2:1 and strong-on-ground 12.5:1 — both well past WCAG AAA (7:1),
so the change costs no legibility. `window-bg` stays true black: xterm paints
its own ground and must not drift with the chrome.

Monaco's theme API takes no CSS variables, so `wb-monaco.js` mirrors these hexes
literally and must be changed in lockstep; `wb_monaco_308.py` asserts the editor
ground, and is the backstop against that pair drifting apart.

#### Neutrals (the ramp)

| Token          | Hex       | Role                                              |
| -------------- | --------- | ------------------------------------------------- |
| `bg`           | `#24201c` | Page ground (warm dark brown)                     |
| `chrome`       | `#2b2620` | Topbar / rail / sidebar, a step above the ground  |
| `surface`      | `#342d27` | Tiles, title bars, buttons                        |
| `surface-hi`   | `#423a31` | Tile / button hover                               |
| `log-bg`       | `#2a2521` | Command-log pane ground                           |
| `window-bg`    | `#000000` | True black: session-window body (behind xterm)    |
|                |           | and the Monaco code surface                       |
| `border`       | `#4c4239` | Default hairline (tiles, windows, divider)        |
| `border-focus` | `#7a6d5f` | Focused window border + resize grip               |
| `text`         | `#d4ccc0` | Primary text (warm off-white)                     |
| `text-strong`  | `#e9e1d4` | Headings and `strong` — a step above `text`       |
| `text-log`     | `#cfc8bd` | Command-log body text                             |
| `text-muted`   | `#a49c91` | Secondary text: paths, status, timestamps, labels |

#### Accents

| Token             | Hex       | Role                                       |
| ----------------- | --------- | ------------------------------------------ |
| `danger`          | `#c56b5c` | Offline status, `close` action (terracotta)|
| `danger-border`   | `#5a2e28` | Border of the `close` button               |
| `console-text`    | `#e8d9a8` | Free-console tile text (muted gold)        |
| `console-bg`      | `#2b2410` | Free-console tile ground                   |
| `console-bg-hi`   | `#3a3016` | Free-console tile hover                    |
| `console-border`  | `#5a4a1e` | Free-console tile border                   |

The **gold accent is reserved exclusively for the free console** (the escape
hatch, #167). It signals "off the curated path" — visually apart from the
neutral repo × agent tiles so it never reads as the default click target. The
**terracotta accent is reserved for danger/offline** — a dead daemon and the
`close` action. No other accents exist; adding one is a design decision, not a
convenience.

### 2. Typography

- **One family, everywhere:** `ui-monospace, SFMono-Regular, Menlo, Consolas,
  monospace`. The chrome matches the terminal it wraps.
- **Scale:** `h1` (identity) `1.6rem` / weight 600 / letter-spacing `0.04em`;
  section `h2` `0.85rem`, uppercase, letter-spacing `0.04em`, muted — a quiet
  eyebrow, not a headline; body `1rem`; tiles & status `0.8rem`; command-log
  `0.78rem` at line-height `1.35`.
- **Emphasis by weight and colour, not size:** repo slugs are weight 600; paths
  and metadata drop to `text-muted`. Unreachable/offline elements use
  `opacity` (0.4–0.45), never a separate grey — they read as dimmed, not
  restyled.

### 2b. Reading is not chrome (amendment)

The scale above is the **chrome's** — monospace at 13px, because the chrome
dresses like the terminal it wraps. **Rendered markdown is not chrome** and does
not inherit it. Running prose (a file in the viewer, an issue body in the
drawer) has its own three tokens, and everything else derives from them:

| Token              | Value  | Role                                            |
| ------------------ | ------ | ----------------------------------------------- |
| `reading-size`     | `14px` | Body size for rendered prose                    |
| `reading-measure`  | `100ch`| Line length of the text column                  |
| `reading-leading`  | `1.7`  | Line height                                     |

Three rules follow, and they are the decision:

- **The article is a little wider than the text.** `max-width: 108ch` on the
  article, `100ch` on `p`/`ul`/`ol`/`blockquote`/headings. A code block, a table
  or a screenshot spans the article — clamping them to the measure turns a wide
  ASCII diagram into a horizontal scroll. **The gap is deliberately small.** An
  early cut used `96ch`/`68ch`; the table then stood ~210px past the paragraph
  above it, the page read as two competing columns, and the prose looked
  squeezed against a block that was not. Keep the two within about one line of
  slack — measured on `docs/daemon-bench.md`, the gap is now ~2px.
- **The measure is set against GitHub, not against a typographic ideal.** These
  documents are read on GitHub too, and a column much narrower than that render
  reads as cramped when the operator switches between them: GitHub's
  `.markdown-body` is 1012px at 16px, about 118 characters. `100ch` at `14px`
  is ~755px. Deliberately past the classic 45–90 character comfort band —
  matching the other window this content lives in won here, and the earlier
  problem was never line length alone but 13px type on a 95-character line.
- **Hierarchy by weight and space, never by rules.** Headings are weight 600 at
  `1.9 / 1.45 / 1.15 / 1em`, separated by a `2.2em` top margin, in
  `text-strong`. The full-width hairline previously under every `h1`/`h2` sliced
  the page into bands and moved the visual weight onto horizontal lines.
- **Inline code carries no box.** Gold ink (`console-text`) on a 22% wash of
  `console-border`. A bordered chip on every identifier stippled the paragraph.

**One system, two surfaces.** The file viewer (`.md-body`) and the issue drawer
(`.kd-body` / `.kd-comment-body`) share these rules. The drawer is a 400px
column, so it overrides `--reading-size` to `13.5px` and `--reading-measure` to
`none` — **and nothing else**. Any further divergence is the bug this replaced:
the two surfaces had grown separate rule sets, and the drawer's headings ended
up 4% larger than its own body, which is no hierarchy at all.

### 3. Spacing & shape

- **Radius:** `4px` on tiles, buttons, and the log pane; `6px` on session
  windows (the larger surface gets the softer corner).
- **Rhythm:** section gaps at `1.5rem`; tile padding `0.1rem 0.5rem`; inline
  gaps `0.3–0.6rem`. Layout is centred (`place-items: center`) with a
  left-aligned repo list inside.
- **Dividers:** the escape hatch is fenced off by a `1px dashed border` in the
  neutral `border` tone — a soft separation, reinforcing "this is apart."
- **Depth:** only floating session windows cast a shadow
  (`0 10px 30px rgba(0,0,0,0.55)`); the launcher is flat. Elevation means
  "detached, draggable window," nothing else.

### 4. Component tokens

- **Agent tile** (`.tile`) — the curated launch button. `surface` ground,
  `border` hairline, `surface-hi` on hover. The neutral default; the whole page
  optimises for clicking these.
- **Console tile** (`.console-tile`) — the gold escape hatch. Same geometry as a
  tile, gold palette, deliberately distinct.
- **Session window** (`.session-window`) — black body, `border` hairline going
  `border-focus` when focused, shadowed, `6px` radius. Title bar is `surface`
  and doubles as the drag handle; a diagonal-hatch grip (in `border-focus`)
  marks the resize corner.
- **`close` button** — the only destructive control; terracotta text on a
  `danger-border` outline. Danger is always this colour, never a plain tile.
- **Status text** (`#status`, `.cmd-status`) — `text-muted`, flipping to
  `danger` when the daemon goes offline.
- **Command-log pane** (`.cmd-log`) — `log-bg` ground, `text-log` body,
  monospace, scrollable, hidden until output streams in (#180).

### 5. State conventions

- **Offline / unreachable:** dim via `opacity`, and (for live status only) shift
  to the `danger` colour. Never remove or restyle — a moved repo stays listed
  and greyed.
- **Hover:** one step up the neutral ramp (`surface` → `surface-hi`; console
  gold → its `-hi`). No transitions, borders-on-hover, or transforms.
- **Focus (windows):** border goes `border-focus` and the window raises in the
  stacking order. Colour, not shadow, marks focus.

## Rejected alternatives

- **A CSS framework / design-token file.** The surface is one embedded page with
  a few dozen rules; a framework or extracted token layer would outweigh it.
  This ADR *is* the token registry until the CSS demands more.
- **A cold (blue-grey) dark theme.** The default for dev tools, and rejected on
  purpose — the warm brown-black ground is the daemon's signature and keeps the
  terminal's own colours as the only saturated element on the page.
- **A second decorative accent.** Two accents, each load-bearing (gold = escape
  hatch, terracotta = danger). A third with no semantic job would dilute both.

## Consequences

- New UI reuses the tokens above; a genuinely new colour or accent is a
  deliberate amendment to this ADR, not an ad-hoc hex in the stylesheet.
- The palette is documented once; the stylesheet stays the single source of the
  literal values, and this ADR the source of their *meaning*.

## Amendment (2026-11-14): measured gaps, a third accent, one focus ring

An audit of the stylesheet and a measure of the page in Chromium (desktop,
150% zoom, tablet, phone) found gaps between this ADR and the page. The
audit is [docs/design-system/audit.md](../design-system/audit.md). The
tokens, their pixel values and every component are listed in
[docs/DESIGN-SYSTEM.md](../DESIGN-SYSTEM.md). That file is now the
registry of values; this ADR stays the source of their meaning. It changes
this ADR in these places.

- **§1 Palette: the token table follows the stylesheet.** `danger-border`
  and `console-bg-hi` were never defined in `01-base.css`; the danger
  button mixes its border from `--danger` with `color-mix()`. Five tokens
  exist that the table did not list: `--live` (a live daemon or console, a
  passed check), `--info` (planning and sleep states), `--font-ui` (rendered
  prose), and the note tones of ADR-0064.
- **§1 Accents: a third accent, `--warn`, amber `#d29922`.** Its one job is
  "needs attention": the attention badge and an urgent release note. Gold
  means "off the curated path" and terracotta means "danger"; neither says
  "look at this soon". It was already in the stylesheet three times as a
  literal hex value. The rule against a decorative accent stands: `--warn`
  has a job.
- **§1 Accents: `--danger` is `#ce8174`, not `#c56b5c`.** Measured, the old
  value gave 3.63:1 as text on `--surface` and 4.02:1 on `--chrome`, under
  the 4.5:1 of WCAG AA. The new value has the same hue and gives 4.52:1 on
  `--surface` and 5.01:1 on `--chrome`. Danger text, the danger button and
  the offline state follow the one token.
- **§5 Focus: one focus ring.** Every control that removes the browser
  outline shows focus with `outline: 1px solid var(--focus-ring)` and a 2px
  offset, where `--focus-ring` is `--console-text`. The checkbox already
  drew focus this way. A window still marks focus with `--border-focus`.
  A modal keeps Tab inside itself while it is open.
- **Icons: Lucide for new icons.** The page used two libraries: Bootstrap
  Icons (a font, 100 uses in the parts) and Lucide (SVG through `x-icon`,
  55 uses). A new icon is Lucide. A Bootstrap icon moves to Lucide when
  its panel changes for other work, not in one change.
- **Text field: one ground, `--log-bg`.** The same as the checkbox, the
  switch track and the segmented track.
- **Text field: an edge at 3:1, `--border-field` (`#867565`).** `--border`
  gave 1.4 to 1.65:1 against the field and the modal, under the 3:1 that
  WCAG 1.4.11 asks for the edge of a control. A focused field keeps this
  edge and shows the focus ring.
- **Sizes on touch screens stay as they are.** A control does not grow on
  a phone or a tablet; larger controls would distort the page. A size may
  go down, or be made the same as its siblings. The touch key strip of a
  console stays the one element sized for fingers.
