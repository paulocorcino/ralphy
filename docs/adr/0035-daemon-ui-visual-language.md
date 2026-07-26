# Daemon web UI: the visual language

Status: accepted (documents the design shipped in the daemon UI; extracted
from `crates/ralphy-daemon/assets/ui/index.html`).

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
xterm.js, so it dresses like a terminal: an all-monospace face, a near-black
warm-brown ground (not the usual cold blue-grey), and low-chroma earth tones
that let the live terminal output — the only saturated thing on screen — carry
the colour. Restraint is the rule: one accent for the escape hatch, one for
danger, everything else a grey ramp.

## Decision

### 1. Palette

A single warm, desaturated ramp anchored on a brown-black ground, plus two
purposeful accents. All values are the literal hex tokens in the stylesheet.

**Neutrals (the ramp)**

| Token          | Hex       | Role                                              |
| -------------- | --------- | ------------------------------------------------- |
| `bg`           | `#14110f` | Page ground (warm near-black)                     |
| `surface`      | `#241f1b` | Tiles, title bars, buttons                        |
| `surface-hi`   | `#322b25` | Tile / button hover                               |
| `log-bg`       | `#1a1613` | Command-log pane ground                            |
| `window-bg`    | `#000000` | Session-window body (true black behind xterm)     |
| `border`       | `#3a332d` | Default hairline (tiles, windows, divider)        |
| `border-focus` | `#6b5f52` | Focused window border + resize grip               |
| `text`         | `#e8e2d9` | Primary text (warm off-white)                     |
| `text-log`     | `#cfc8bd` | Command-log body text                             |
| `text-muted`   | `#9b948a` | Secondary text: paths, status, timestamps, labels |

**Accents**

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
</content>
</invoke>

## Amendment (2026-07-26, ADR-0048): the whiteboard pane is exempt

[ADR-0048](./0048-whiteboard-vendored-excalidraw.md) embeds Excalidraw, a
vendored React application that ships its own complete UI and its own dark
theme. That pane **does not wear this palette**, and the exemption is recorded
here so a future reader does not read it as drift and "fix" it.

The reason is the one ADR-0048 §3 is built on: the vendored library is pinned
and meant to be left alone, and our code is meant to touch the smallest possible
slice of its API. Retinting a third-party React app means reaching into its
internals — the exact coupling that makes a version bump expensive, in service
of a pane the operator entered deliberately and will read as a distinct tool
anyway. Monaco set the precedent from the other side: it got the `wb` theme
(`wb-monaco.js`) because Monaco *exposes* a theme API taking our literal tokens.
Excalidraw offers no equivalent seam, so the honest answer is an exemption
rather than a hack that looks like compliance.

The exemption is **the pane's interior only**. Everything framing it — the tab,
the tab strip, the toolbar around the canvas, any dialog the workbench itself
raises over it — stays in this language. The boundary is visible on screen, and
that is acceptable: a vendored tool that looks like a vendored tool is more
honest than one dressed to pass as native and failing at the edges.

No token below changes. Nothing else on the page is exempt.
