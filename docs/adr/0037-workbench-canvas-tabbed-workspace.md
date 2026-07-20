# Workbench canvas: a tabbed workspace with a fixed Agents tab

Status: accepted (promotes a decision hardened in the shell during PRD #185 and
flagged as a candidate ADR by the workbench build guide; extracted from
`crates/ralphy-daemon/assets/ui/`).

The daemon workbench (ADR-0032, promoted to the daemon's `/` in #200) lays out
as four columns: **icon rail · sidebar · canvas · Runs panel**. This ADR records
what the **canvas** — the central pane — *is*, so future panes attach to the
established structure instead of reinventing it. It is a reference document, not
a new decision; the shape below is the one already shipped. Vocabulary
(**canvas**, **Agents tab**, **tab strip**, **workbench session**, **free
console**) lives in [CONTEXT.md](../../CONTEXT.md); the visual language lives in
[ADR-0035](0035-daemon-ui-visual-language.md).

## Decision

### 1. The canvas is a tabbed workspace, not a single view

A **tab strip** (`.tabbar`) runs across the top of the canvas. The pane below it
belongs to whichever tab is active. Tab state and lifecycle live in
`app.js` (`tabs`, `active`, `activate`, `openTab`, `closeTab`); the panes
themselves are owned by the viewer and console modules.

### 2. Tab 0 is the fixed Agents tab

The first tab is the **Agents** tab. It is **fixed**: it never closes and cannot
be reordered away from position 0. It hosts the floating agent consoles (the
**workbench sessions** and **free consoles**) over the dotted stage. The console
controls (**New console ▾**, **Arrange**) are pinned to the strip's right edge,
above the workspace, so a floating console can never cover them.

### 3. Files ride in as closable tabs after it

Every opened file becomes a **closable** tab inserted after the Agents tab. On
open, the pane is chosen by extension (`classify`): markdown renders, binaries
are refused (`open-refused`), everything else opens as source. Closing a file
tab never touches the Agents tab or its consoles.

## Rejected alternatives

- **A single-view canvas that swaps content (agents *or* a file, never both).**
  Rejected: a human running an agent console needs to open a file without
  tearing down the console. The fixed Agents tab keeps the live sessions present
  while files come and go beside them.
- **Making the Agents tab an ordinary, closable tab.** Rejected: the consoles
  are the workbench's reason to exist; a stray close (or reorder) that hides
  them is a footgun with no upside. Fixing tab 0 removes the failure mode.
- **A separate window/panel for file viewing** (outside the canvas). Rejected:
  it fragments focus and duplicates the tab machinery; one strip owning both
  agents and files is simpler and keeps the floating-console reflow math
  (`clampAll` on `#workspace`) in one place.

## Consequences

- New canvas content is a **tab**, not a new region — it joins the strip after
  the Agents tab and obeys `openTab`/`closeTab`.
- The Agents tab's fixedness is load-bearing: code that reorders or closes tabs
  must special-case index 0. This invariant is the decision, not an accident of
  the current `app.js`.
- Overlays that are explicitly *not* tabs (e.g. the Kanban board, which opens as
  an overlay over the canvas) stay overlays; they are a deliberate exception to
  "new canvas content is a tab," recorded here so the distinction is intentional.
